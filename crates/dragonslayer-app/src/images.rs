//! Decodes and downscales frame JPEGs off the UI thread and keeps them as GPU textures.
//!
//! Built for scrubbing and playing hundreds of camera-sized JPEGs:
//! - JPEGs are decoded straight at 1/2, 1/4 or 1/8 size (DCT scaling), so a filmstrip
//!   thumbnail of a 24 MP photo costs a fraction of a full decode.
//! - What's on screen now is decoded first, then prefetches; requests nobody has asked for
//!   recently are cancelled, so scrubbing never waits behind frames already scrolled past.
//! - Textures stay cached up to a memory budget (least recently used go first), so a
//!   looping playback plays from memory.
//! - Finished images are uploaded to the GPU a few per frame, so a burst can't cause a hitch.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

/// GPU memory kept for decoded frames before the least recently used are dropped.
const BUDGET_BYTES: usize = 600 << 20;
/// Pixels uploaded to the GPU per UI frame; the rest wait for the next frame.
const UPLOAD_PIXELS_PER_FRAME: usize = 6_000_000;
/// A request not repeated for this many UI frames is no longer wanted.
const CANCEL_AFTER_FRAMES: u64 = 90;
/// Failed decodes are retried after this long unused (the file may have been fixed).
const FORGET_FAILED_AFTER_FRAMES: u64 = 600;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    /// Full RGB thumbnail, for scene rows and the last-frame view.
    Full,
    /// Sobel-edge outline of the frame, mostly-transparent with white edges.
    /// Painted tinted in the viewer so the ghost outlines the previous frame
    /// without obscuring live view.
    Edges,
}

type Key = (PathBuf, u32, Kind);

enum Slot {
    Loading,
    Ready(TextureHandle, usize),
    Failed,
}

struct Entry {
    slot: Slot,
    last_used: u64,
}

/// Jobs waiting for a worker, by priority. Higher runs first.
#[derive(Default)]
struct Queue {
    jobs: HashMap<Key, u64>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

pub struct Images {
    shared: Arc<Shared>,
    results: Receiver<(Key, Option<ColorImage>)>,
    cache: HashMap<Key, Entry>,
    frame: u64,
    bytes: usize,
}

impl Images {
    pub fn new(ctx: &egui::Context) -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, results) = mpsc::channel();
        let workers = thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1)).clamp(2, 6);
        for i in 0..workers {
            let shared = shared.clone();
            let tx = tx.clone();
            let ctx = ctx.clone();
            thread::Builder::new()
                .name(format!("image loader {i}"))
                .spawn(move || {
                    while let Some(key) = next_job(&shared) {
                        let img = load(&key.0, key.1, key.2);
                        if tx.send((key, img)).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                })
                .expect("spawn loader");
        }
        Self { shared, results, cache: HashMap::new(), frame: 0, bytes: 0 }
    }

    /// Texture for `path` at most `max_width` wide. Starts loading it if needed; `None` until ready.
    pub fn get(&mut self, path: &Path, max_width: u32) -> Option<TextureHandle> {
        self.get_kind(path, max_width, Kind::Full)
    }

    pub fn get_kind(&mut self, path: &Path, max_width: u32, kind: Kind) -> Option<TextureHandle> {
        // On screen now: ahead of everything requested earlier, and of prefetches.
        self.want((path.to_path_buf(), max_width, kind), self.frame * 2 + 1)
    }

    /// True once `path` has been tried and couldn't be decoded (damaged or unreadable).
    pub fn failed(&self, path: &Path, max_width: u32) -> bool {
        self.cache
            .get(&(path.to_path_buf(), max_width, Kind::Full))
            .is_some_and(|e| matches!(e.slot, Slot::Failed))
    }

    /// Load in the background without drawing (the next frames in Preview, a new capture).
    /// Runs after whatever is on screen.
    pub fn prefetch(&mut self, path: &Path, max_width: u32) {
        let _ = self.want((path.to_path_buf(), max_width, Kind::Full), self.frame * 2);
    }

    fn want(&mut self, key: Key, priority: u64) -> Option<TextureHandle> {
        let frame = self.frame;
        match self.cache.get_mut(&key) {
            Some(entry) => {
                entry.last_used = frame;
                match &entry.slot {
                    Slot::Ready(t, _) => return Some(t.clone()),
                    Slot::Failed => return None,
                    Slot::Loading => {}
                }
                // Still queued: raise its priority. Already being decoded: nothing to do.
                if let Some(p) = self.shared.queue.lock().unwrap().jobs.get_mut(&key) {
                    *p = (*p).max(priority);
                }
                None
            }
            None => {
                self.cache.insert(key.clone(), Entry { slot: Slot::Loading, last_used: frame });
                let mut q = self.shared.queue.lock().unwrap();
                q.jobs.insert(key, priority);
                drop(q);
                self.shared.wake.notify_one();
                None
            }
        }
    }

    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        let frame = self.frame;

        let mut uploaded = 0;
        while uploaded < UPLOAD_PIXELS_PER_FRAME {
            let Ok((key, img)) = self.results.try_recv() else { break };
            // Cancelled while decoding: drop it.
            let Some(entry) = self.cache.get_mut(&key) else { continue };
            entry.slot = match img {
                Some(img) => {
                    let pixels = img.pixels.len();
                    uploaded += pixels;
                    self.bytes += pixels * 4;
                    let name = format!("{}@{}:{:?}", key.0.display(), key.1, key.2);
                    Slot::Ready(ctx.load_texture(name, img, TextureOptions::LINEAR), pixels * 4)
                }
                None => Slot::Failed,
            };
        }
        if uploaded >= UPLOAD_PIXELS_PER_FRAME {
            ctx.request_repaint();
        }

        // Cancel what nobody has asked for lately (scrolled or scrubbed past).
        let stale: Vec<Key> = self
            .cache
            .iter()
            .filter(|(_, e)| match e.slot {
                Slot::Loading => frame - e.last_used > CANCEL_AFTER_FRAMES,
                Slot::Failed => frame - e.last_used > FORGET_FAILED_AFTER_FRAMES,
                Slot::Ready(..) => false,
            })
            .map(|(k, _)| k.clone())
            .collect();
        if !stale.is_empty() {
            let mut q = self.shared.queue.lock().unwrap();
            for k in &stale {
                q.jobs.remove(k);
                self.cache.remove(k);
            }
        }

        // Over budget: drop the least recently used textures, never ones drawn this frame
        // or the last (they'd flicker).
        if self.bytes > BUDGET_BYTES {
            let mut ready: Vec<(u64, Key, usize)> = self
                .cache
                .iter()
                .filter_map(|(k, e)| match e.slot {
                    Slot::Ready(_, b) if frame - e.last_used > 1 => Some((e.last_used, k.clone(), b)),
                    _ => None,
                })
                .collect();
            ready.sort_by_key(|(used, ..)| *used);
            let target = BUDGET_BYTES / 10 * 9;
            for (_, k, b) in ready {
                if self.bytes <= target {
                    break;
                }
                self.cache.remove(&k);
                self.bytes -= b;
            }
        }
    }

    #[cfg(test)]
    pub fn debug_state(&self) -> String {
        let loading = self.cache.values().filter(|e| matches!(e.slot, Slot::Loading)).count();
        let ready = self.cache.values().filter(|e| matches!(e.slot, Slot::Ready(..))).count();
        let failed = self.cache.values().filter(|e| matches!(e.slot, Slot::Failed)).count();
        format!("{ready} ready, {loading} loading, {failed} failed, {} queued, {} MB, frame {}", self.queued(), self.bytes >> 20, self.frame)
    }

    #[cfg(test)]
    fn queued(&self) -> usize {
        self.shared.queue.lock().unwrap().jobs.len()
    }
}

impl Drop for Images {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().closed = true;
        self.shared.wake.notify_all();
    }
}

/// Blocks until there's a job; returns the highest-priority one, or `None` when closed.
fn next_job(shared: &Shared) -> Option<Key> {
    let mut q = shared.queue.lock().unwrap();
    loop {
        if q.closed {
            return None;
        }
        let best = q.jobs.iter().max_by_key(|(_, p)| **p).map(|(k, _)| k.clone());
        if let Some(key) = best {
            q.jobs.remove(&key);
            return Some(key);
        }
        q = shared.wake.wait(q).unwrap();
    }
}

fn load(path: &Path, max_width: u32, kind: Kind) -> Option<ColorImage> {
    let img = decode(path, max_width)?;
    // Within a quarter of the target (a 1/4-scale decode of a 5184-wide photo is 1296 for a
    // 1280 viewer), resampling costs more than it's worth: the GPU scales it when drawing.
    let img = if img.width() > max_width.saturating_mul(5) / 4 {
        // Rounded, and resized exactly: `thumbnail` fits inside the box, and a floored
        // height made the height the tighter limit, giving images a pixel narrower than asked.
        let h = ((f64::from(img.height()) * f64::from(max_width) / f64::from(img.width())).round() as u32).max(1);
        img.thumbnail_exact(max_width, h)
    } else {
        img
    };
    match kind {
        Kind::Full => {
            let rgba = img.to_rgba8();
            Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
        }
        Kind::Edges => Some(sobel_edges(&img.to_luma8())),
    }
}

fn decode(path: &Path, max_width: u32) -> Option<image::DynamicImage> {
    if dragonslayer_core::scene::is_jpeg(path)
        && let Some(img) = decode_jpeg_scaled(path, max_width)
    {
        return Some(img);
    }
    image::open(path).ok()
}

/// Decodes at the smallest of 1/1, 1/2, 1/4, 1/8 size that is still at least `max_width`
/// wide. `None` for anything unusual (CMYK, 16-bit), which then takes the full decoder.
fn decode_jpeg_scaled(path: &Path, max_width: u32) -> Option<image::DynamicImage> {
    let mut dec = jpeg_decoder::Decoder::new(BufReader::new(File::open(path).ok()?));
    dec.read_info().ok()?;
    let info = dec.info()?;
    let (full_w, full_h) = (u32::from(info.width), u32::from(info.height));
    if full_w == 0 || full_h == 0 {
        return None;
    }
    // Scaling only pays off (and is only exact enough) when there's plenty to throw away.
    let (w, h) = if full_w >= max_width.saturating_mul(2) {
        let want_h = (u64::from(full_h) * u64::from(max_width) / u64::from(full_w)).max(1) as u32;
        dec.scale(max_width.max(1) as u16, want_h as u16).ok()?
    } else {
        (info.width, info.height)
    };
    let pixels = dec.decode().ok()?;
    let (w, h) = (u32::from(w), u32::from(h));
    if w < max_width.min(full_w) {
        // The decoder went smaller than asked: let the full decoder do it properly.
        return None;
    }
    match dec.info()?.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            image::RgbImage::from_raw(w, h, pixels).map(image::DynamicImage::ImageRgb8)
        }
        jpeg_decoder::PixelFormat::L8 => {
            image::GrayImage::from_raw(w, h, pixels).map(image::DynamicImage::ImageLuma8)
        }
        _ => None,
    }
}

/// Sobel magnitude on a grayscale image, then a soft threshold so faint edges
/// stay faint. Output is white-on-transparent RGBA so it can be tinted freely
/// by the caller and painted over live view without covering it.
fn sobel_edges(gray: &image::GrayImage) -> ColorImage {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let mut out = vec![0u8; w * h * 4];
    if w < 3 || h < 3 {
        return ColorImage::from_rgba_premultiplied([w, h], &out);
    }
    let g = gray.as_raw();
    let idx = |x: usize, y: usize| g[y * w + x] as i32;

    // Border pixels stay transparent; we skip them so we don't have to bounds-check.
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            // Sobel kernels.
            let gx = -idx(x - 1, y - 1) - 2 * idx(x - 1, y) - idx(x - 1, y + 1)
                + idx(x + 1, y - 1) + 2 * idx(x + 1, y) + idx(x + 1, y + 1);
            let gy = -idx(x - 1, y - 1) - 2 * idx(x, y - 1) - idx(x + 1, y - 1)
                + idx(x - 1, y + 1) + 2 * idx(x, y + 1) + idx(x + 1, y + 1);
            // Approximate magnitude; keeps things fast.
            let mag = (gx.abs() + gy.abs()).min(255) as u8;
            // Soft floor: pixels quieter than 40 disappear so noise doesn't clutter.
            let a = if mag < 40 { 0 } else { mag };
            let i = (y * w + x) * 4;
            out[i] = a;
            out[i + 1] = a;
            out[i + 2] = a;
            out[i + 3] = a;
        }
    }
    ColorImage::from_rgba_premultiplied([w, h], &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn jpeg(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        img.save(path).unwrap();
    }

    fn wait_ready(images: &mut Images, ctx: &egui::Context, path: &Path, w: u32) -> TextureHandle {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            images.begin_frame(ctx);
            if let Some(t) = images.get(path, w) {
                return t;
            }
            assert!(Instant::now() < deadline, "never loaded {}", path.display());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn scaled_jpeg_decode_is_at_least_as_wide_as_asked_then_trimmed_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("big.jpg");
        jpeg(&p, 2400, 1600);
        let img = decode_jpeg_scaled(&p, 160).unwrap();
        assert!(img.width() >= 160 && img.width() <= 400, "1/8 scale of 2400 is 300: got {}", img.width());
        let loaded = load(&p, 160, Kind::Full).unwrap();
        assert_eq!(loaded.size, [160, 107], "exactly as wide as asked, height rounded");
        // Asking for more than the image has doesn't upscale.
        assert_eq!(load(&p, 5000, Kind::Full).unwrap().size, [2400, 1600]);
    }

    #[test]
    fn broken_and_missing_files_fail_cleanly_and_edges_work() {
        let tmp = tempfile::tempdir().unwrap();
        let ok = tmp.path().join("a.jpg");
        jpeg(&ok, 50, 40);
        // Up to a quarter wider than asked is kept as decoded (the GPU scales it).
        let [w, h] = load(&ok, 20, Kind::Edges).unwrap().size;
        assert!((20..=25).contains(&w) && h * 5 == w * 4, "{w}x{h}");
        // Upper-case extension, as cameras write it.
        let upper = tmp.path().join("B.JPG");
        jpeg(&upper, 50, 40);
        let [w, _] = load(&upper, 20, Kind::Full).unwrap().size;
        assert!((20..=25).contains(&w), "{w}");
        let broken = tmp.path().join("broken.jpg");
        std::fs::write(&broken, [0xFF, 0xD8, 0xFF, 0xE0, 1, 2]).unwrap();
        assert!(load(&broken, 100, Kind::Full).is_none());
        assert!(load(&tmp.path().join("missing.jpg"), 100, Kind::Full).is_none());
    }

    #[test]
    fn tiny_images_do_not_break_edge_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jpg");
        jpeg(&p, 2, 1);
        assert_eq!(load(&p, 100, Kind::Edges).unwrap().size, [2, 1]);
    }

    #[test]
    fn loads_come_back_as_textures_and_stale_requests_are_cancelled() {
        let ctx = egui::Context::default();
        let mut images = Images::new(&ctx);
        let tmp = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = (0..40).map(|i| tmp.path().join(format!("{i}.jpg"))).collect();
        for p in &paths {
            jpeg(p, 800, 600);
        }
        // "Scroll past" 39 frames in one go, then stop on the last.
        for p in &paths {
            images.get(p, 400);
        }
        let last = paths.last().unwrap();
        let t = wait_ready(&mut images, &ctx, last, 400);
        assert_eq!(t.size(), [400, 300]);
        // Keep looking only at the last frame: everything else is cancelled in time.
        for _ in 0..(CANCEL_AFTER_FRAMES + 5) {
            images.begin_frame(&ctx);
            images.get(last, 400);
        }
        assert_eq!(images.queued(), 0);
        let loading = images.cache.values().filter(|e| matches!(e.slot, Slot::Loading)).count();
        assert_eq!(loading, 0, "no stale loads left");
    }

    #[test]
    fn a_failed_file_is_retried_after_a_while() {
        let ctx = egui::Context::default();
        let mut images = Images::new(&ctx);
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("later.jpg");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !matches!(images.cache.get(&(p.clone(), 100, Kind::Full)).map(|e| &e.slot), Some(Slot::Failed)) {
            images.begin_frame(&ctx);
            images.get(&p, 100);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        jpeg(&p, 200, 100);
        for _ in 0..(FORGET_FAILED_AFTER_FRAMES + 2) {
            images.begin_frame(&ctx);
        }
        let _ = wait_ready(&mut images, &ctx, &p, 100);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    fn time<T>(f: impl FnOnce() -> T) -> std::time::Duration {
        let t = Instant::now();
        let _ = f();
        t.elapsed()
    }

    /// `cargo test -p dragonslayer-app --release -- --ignored --nocapture decode_speed`
    #[test]
    #[ignore = "timing, not a check"]
    fn decode_speed() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("100d.jpg");
        // Canon 100D size; smooth gradients with a little texture, like a real scene.
        let img = image::RgbImage::from_fn(5184, 3456, |x, y| {
            let n = ((x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) >> 28) as u8;
            image::Rgb([(x / 21) as u8 ^ n, (y / 14) as u8, ((x + y) / 34) as u8])
        });
        img.save(&p).unwrap();
        println!("file {:.1} MB", std::fs::metadata(&p).unwrap().len() as f64 / 1e6);
        let jd = |scale: Option<(u16, u16)>| {
            let mut d = jpeg_decoder::Decoder::new(BufReader::new(File::open(&p).unwrap()));
            d.read_info().unwrap();
            if let Some((w, h)) = scale {
                d.scale(w, h).unwrap();
            }
            d.decode().unwrap()
        };
        println!("image::open (zune) full  {:>9.1?}", time(|| image::open(&p).unwrap()));
        println!("jpeg-decoder full        {:>9.1?}", time(|| jd(None)));
        println!("jpeg-decoder 1/4         {:>9.1?}", time(|| jd(Some((1296, 864)))));
        println!("jpeg-decoder 1/8         {:>9.1?}", time(|| jd(Some((648, 432)))));
        for w in [160, 1280] {
            let old = time(|| {
                let img = image::open(&p).unwrap().thumbnail(w, w);
                img.to_rgba8()
            });
            let new = time(|| load(&p, w, Kind::Full));
            println!("width {w:>4}: old {old:>8.1?}  new {new:>8.1?}  {:.1}x", old.as_secs_f64() / new.as_secs_f64());
        }
    }
}

#[cfg(test)]
mod real_files {
    use super::*;

    /// DS_FRAMES=<folder> cargo test -p dragonslayer-app --release -- --ignored --nocapture real_frames
    #[test]
    #[ignore = "needs DS_FRAMES pointing at real camera JPEGs"]
    fn real_frames() {
        let Ok(dir) = std::env::var("DS_FRAMES") else { return };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| dragonslayer_core::scene::is_jpeg(p)).collect();
        files.sort();
        for w in [160u32, 640, 960, 1280] {
            let (mut ok, mut fail, mut slow) = (0, Vec::new(), std::time::Duration::ZERO);
            let t0 = std::time::Instant::now();
            for f in &files {
                let t = std::time::Instant::now();
                let scaled = std::panic::catch_unwind(|| decode_jpeg_scaled(f, w));
                let r = std::panic::catch_unwind(|| load(f, w, Kind::Full));
                slow = slow.max(t.elapsed());
                match (&scaled, &r) {
                    (Ok(Some(_)), Ok(Some(_))) => ok += 1,
                    _ => fail.push(format!("{} scaled={:?} load={:?}", f.file_name().unwrap().to_string_lossy(),
                        scaled.as_ref().map(|o| o.as_ref().map(|i| (i.width(), i.height()))).map_err(|_| "PANIC"),
                        r.as_ref().map(|o| o.as_ref().map(|i| i.size)).map_err(|_| "PANIC"))),
                }
            }
            println!("width {w}: {ok}/{} ok, total {:?}, slowest {:?}", files.len(), t0.elapsed(), slow);
            for f in fail.iter().take(5) { println!("   {f}"); }
        }
    }
}

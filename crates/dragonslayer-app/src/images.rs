//! Decodes and downscales frame JPEGs off the UI thread and keeps them as GPU textures.
//! Textures not drawn for a few seconds are dropped, so memory tracks what's on screen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

const WORKERS: usize = 2;
const EVICT_AFTER_FRAMES: u64 = 300;

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
    Ready(TextureHandle),
    Failed,
}

struct Entry {
    slot: Slot,
    last_used: u64,
}

pub struct Images {
    requests: Sender<Key>,
    results: Receiver<(Key, Option<ColorImage>)>,
    cache: HashMap<Key, Entry>,
    frame: u64,
}

impl Images {
    pub fn new(ctx: &egui::Context) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Key>();
        let (res_tx, res_rx) = mpsc::channel();
        let req_rx = Arc::new(Mutex::new(req_rx));
        for i in 0..WORKERS {
            let rx = req_rx.clone();
            let tx = res_tx.clone();
            let ctx = ctx.clone();
            thread::Builder::new()
                .name(format!("image loader {i}"))
                .spawn(move || loop {
                    let Ok(key) = rx.lock().unwrap().recv() else { return };
                    let img = load(&key.0, key.1, key.2);
                    if tx.send((key, img)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                })
                .expect("spawn loader");
        }
        Self { requests: req_tx, results: res_rx, cache: HashMap::new(), frame: 0 }
    }

    /// Texture for `path` at most `max_width` wide. Starts loading it if needed; `None` until ready.
    pub fn get(&mut self, path: &Path, max_width: u32) -> Option<TextureHandle> {
        self.get_kind(path, max_width, Kind::Full)
    }

    pub fn get_kind(&mut self, path: &Path, max_width: u32, kind: Kind) -> Option<TextureHandle> {
        let key = (path.to_path_buf(), max_width, kind);
        let frame = self.frame;
        let entry = self.cache.entry(key.clone()).or_insert_with(|| {
            let _ = self.requests.send(key);
            Entry { slot: Slot::Loading, last_used: frame }
        });
        entry.last_used = frame;
        match &entry.slot {
            Slot::Ready(t) => Some(t.clone()),
            _ => None,
        }
    }

    /// Warm the cache without drawing (e.g. right after a capture).
    pub fn prefetch(&mut self, path: &Path, max_width: u32) {
        let _ = self.get(path, max_width);
    }

    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok((key, img)) = self.results.try_recv() {
            let Some(entry) = self.cache.get_mut(&key) else { continue };
            entry.slot = match img {
                Some(img) => {
                    let name = format!("{}@{}:{:?}", key.0.display(), key.1, key.2);
                    Slot::Ready(ctx.load_texture(name, img, TextureOptions::LINEAR))
                }
                None => Slot::Failed,
            };
        }
        let frame = self.frame;
        self.cache
            .retain(|_, e| matches!(e.slot, Slot::Loading) || frame - e.last_used < EVICT_AFTER_FRAMES);
    }
}

fn load(path: &Path, max_width: u32, kind: Kind) -> Option<ColorImage> {
    let img = image::open(path).ok()?;
    let img = if img.width() > max_width {
        let h = (u64::from(img.height()) * u64::from(max_width) / u64::from(img.width())).max(1) as u32;
        img.thumbnail(max_width, h)
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

/// Sobel magnitude on a grayscale image, then a soft threshold so faint edges
/// stay faint. Output is white-on-transparent RGBA so it can be tinted freely
/// by the caller and painted over live view without covering it.
fn sobel_edges(gray: &image::GrayImage) -> ColorImage {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let g = gray.as_raw();
    let idx = |x: usize, y: usize| g[y * w + x] as i32;

    let mut out = vec![0u8; w * h * 4];
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

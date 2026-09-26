//! UVC webcam backend.
//!
//! Covers laptop cameras, USB webcams, HDMI capture cards, and — the reason
//! this exists — Android phones running in "USB webcam" mode (Android 14+
//! ships this natively; some vendor apps add it to older versions).
//!
//! `nokhwa` picks the OS's native stack — MediaFoundation on Windows,
//! AVFoundation on macOS, V4L2 on Linux — so the same code path works
//! everywhere.
//!
//! Limitations vs. the libgphoto2 backend:
//! - No RAW capture (UVC is JPEG-only in practice).
//! - "Capture" is really "grab the current live-view frame at full
//!   resolution", so shutter timing and depth of field are whatever the OS
//!   camera pipeline gives us.
//!
//! Threading: `nokhwa::Camera` isn't `Send`, so it lives on its own worker
//! thread — same pattern as the libgphoto2 backend. Public `Camera` methods
//! talk to that thread via mpsc channels.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
use nokhwa::{query, Camera as NokhwaCam};

use crate::{
    live_view_channel, Camera, CameraBackend, CameraError, Capabilities, CaptureHandle,
    CapturedFile, DeviceInfo, FileKind, LiveFrame, LiveViewSender, LiveViewStream, Result,
};

const PORT_PREFIX: &str = "webcam:";

pub struct WebcamBackend;

fn map(err: nokhwa::NokhwaError) -> CameraError {
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();
    if lower.contains("access") || lower.contains("permission") || lower.contains("in use") {
        CameraError::Busy("close any app using the camera (Zoom, Photo Booth, Camera, etc.)".into())
    } else if lower.contains("not found") || lower.contains("no device") {
        CameraError::NotFound
    } else {
        CameraError::Backend(msg)
    }
}

impl CameraBackend for WebcamBackend {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>> {
        let devices = query(ApiBackend::Auto).map_err(map)?;
        Ok(devices
            .into_iter()
            .map(|d| {
                let idx = match d.index() {
                    CameraIndex::Index(n) => n.to_string(),
                    CameraIndex::String(s) => s.clone(),
                };
                DeviceInfo {
                    make: String::new(),
                    model: d.human_name().to_string(),
                    serial: None,
                    port: format!("{PORT_PREFIX}{idx}"),
                }
            })
            .collect())
    }

    fn open(&self, device: &DeviceInfo) -> Result<Box<dyn Camera>> {
        let raw = device
            .port
            .strip_prefix(PORT_PREFIX)
            .ok_or_else(|| CameraError::Backend(format!("not a webcam port: {}", device.port)))?;
        let index = if let Ok(n) = raw.parse::<u32>() {
            CameraIndex::Index(n)
        } else {
            CameraIndex::String(raw.to_string())
        };

        // Spin up the worker thread and wait for it to confirm the device
        // opened. That way `open` returns a real error (busy, not found, etc.)
        // synchronously to the caller.
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let wanted = device.clone();
        let handle = thread::Builder::new()
            .name(format!("webcam {}", device.port))
            .spawn(move || worker(index, cmd_rx, ready_tx))
            .map_err(|e| CameraError::Backend(e.to_string()))?;
        ready_rx.recv().map_err(|_| CameraError::Disconnected)??;
        Ok(Box::new(WebcamCamera { info: wanted, cmd: cmd_tx, worker: Some(handle) }))
    }
}

pub struct WebcamCamera {
    info: DeviceInfo,
    cmd: Sender<Cmd>,
    worker: Option<JoinHandle<()>>,
}

enum Cmd {
    StartLive(LiveViewSender),
    StopLive,
    Capture(Sender<Result<Vec<u8>>>),
    Close,
}

impl WebcamCamera {
    fn call<T>(&self, make: impl FnOnce(Sender<Result<T>>) -> Cmd) -> Result<T> {
        let (tx, rx) = mpsc::channel();
        self.cmd.send(make(tx)).map_err(|_| CameraError::Disconnected)?;
        rx.recv().map_err(|_| CameraError::Disconnected)?
    }
}

impl Camera for WebcamCamera {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_view: true, capture: true, download: true, raw_plus_jpeg: false }
    }

    fn start_live_view(&mut self) -> Result<LiveViewStream> {
        let (tx, rx) = live_view_channel();
        self.cmd.send(Cmd::StartLive(tx)).map_err(|_| CameraError::Disconnected)?;
        Ok(rx)
    }

    fn stop_live_view(&mut self) -> Result<()> {
        self.cmd.send(Cmd::StopLive).map_err(|_| CameraError::Disconnected)
    }

    fn capture(&mut self) -> Result<CaptureHandle> {
        let jpeg = self.call(Cmd::Capture)?;
        // Send the raw bytes back on the wire via the CaptureHandle's string:
        // we hex-encode a fixed marker + stash the bytes in a queue keyed by
        // the marker. Simpler: since Camera's download() is stateful and
        // called next, we just store the frame in the worker until the
        // matching Download command arrives. But our trait doesn't have a
        // Download command back into the worker — so we tunnel the bytes
        // through the CaptureHandle as a base64-ish blob would be huge.
        //
        // Instead: return the JPEG bytes encoded in a temporary file on
        // disk. Simpler still: hold the bytes on the WebcamCamera itself.
        // That requires interior mutability because `download` takes `&mut
        // self`, which it already does, and we can stash there. But
        // `capture` also takes `&mut self`, so we can just set a field.
        // Refactor: store on self.
        Ok(CaptureHandle(vec![encode_jpeg_blob(&jpeg)]))
    }

    fn download(&mut self, handle: CaptureHandle, dest: &Path) -> Result<Vec<CapturedFile>> {
        let blob = handle.0.into_iter().next().ok_or_else(|| CameraError::Backend("empty capture handle".into()))?;
        let jpeg = decode_jpeg_blob(&blob)
            .ok_or_else(|| CameraError::Backend("invalid capture handle payload".into()))?;
        let path: PathBuf = dest.join(format!("webcam_{}.jpg", jpeg_seq()));
        let mut f = File::create(&path).map_err(|source| CameraError::Io { path: path.clone(), source })?;
        f.write_all(&jpeg).map_err(|source| CameraError::Io { path: path.clone(), source })?;
        Ok(vec![CapturedFile { path, kind: FileKind::Jpeg }])
    }

    fn close(self: Box<Self>) -> Result<()> {
        let _ = self.cmd.send(Cmd::Close);
        Ok(())
    }
}

impl Drop for WebcamCamera {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Close);
        if let Some(handle) = self.worker.take() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !handle.is_finished() {
                if Instant::now() >= deadline {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let _ = handle.join();
        }
    }
}

/// Encode raw JPEG bytes into a printable ASCII blob so we can round-trip
/// them through `CaptureHandle(Vec<String>)` without adding a new trait
/// surface. Uses hex — cheap and dependency-free; a 5 MB frame becomes 10 MB
/// of ASCII, which is fine since the strings live only until download().
fn encode_jpeg_blob(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2 + 4);
    s.push_str("hex:");
    for &b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn decode_jpeg_blob(s: &str) -> Option<Vec<u8>> {
    let hex = s.strip_prefix("hex:")?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let hi = from_hex(pair[0])?;
        let lo = from_hex(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn jpeg_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

// -------------------------------------------------------------------- worker

fn worker(index: CameraIndex, cmds: Receiver<Cmd>, ready: Sender<Result<()>>) {
    // Ask for the highest-resolution MJPEG stream on offer, falling back to
    // any format if the device doesn't advertise MJPEG. MJPEG is what Android
    // USB webcams and most laptop cameras deliver by default, and its buffer
    // bytes are already a valid JPEG for us to write to disk unmodified.
    let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestResolution);
    let mut cam = match NokhwaCam::new(index, format) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(map(e)));
            return;
        }
    };
    // Prefer MJPEG when the device offers it — cheap conversion since
    // buffer.buffer() is already JPEG bytes.
    let _ = cam.set_frame_format(FrameFormat::MJPEG);
    let _ = ready.send(Ok(()));

    let mut live: Option<LiveViewSender> = None;
    let mut seq: u64 = 0;
    loop {
        // While live view is running, poll for both the next frame and any
        // new commands. When idle, block on commands with a small timeout so
        // we don't burn CPU.
        let cmd = if live.is_some() {
            match cmds.try_recv() {
                Ok(c) => Some(c),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        } else {
            match cmds.recv_timeout(Duration::from_secs(1)) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };

        match cmd {
            Some(Cmd::StartLive(tx)) => {
                let _ = cam.open_stream();
                live = Some(tx);
            }
            Some(Cmd::StopLive) => {
                live = None;
                let _ = cam.stop_stream();
            }
            Some(Cmd::Capture(reply)) => {
                // Capture wants a fresh frame even if live view was off.
                let opened_here = live.is_none();
                if opened_here {
                    let _ = cam.open_stream();
                }
                let result = cam.frame().map(|b| b.buffer().to_vec()).map_err(map);
                if opened_here {
                    let _ = cam.stop_stream();
                }
                let _ = reply.send(result);
            }
            Some(Cmd::Close) => break,
            None => {}
        }

        if let Some(tx) = &live {
            if tx.is_stopped() {
                live = None;
                let _ = cam.stop_stream();
                continue;
            }
            match cam.frame() {
                Ok(buf) => {
                    seq += 1;
                    // buffer.buffer() is the raw source frame — MJPEG bytes
                    // when the stream is MJPEG. Send directly.
                    if !tx.send(LiveFrame { seq, jpeg: buf.buffer().to_vec() }) {
                        live = None;
                        let _ = cam.stop_stream();
                    }
                }
                Err(_) => {
                    // Transient — pause briefly and continue.
                    thread::sleep(Duration::from_millis(30));
                }
            }
        }
    }
    let _ = cam.stop_stream();
}

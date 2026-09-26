//! Camera interface (spec §7) and backends.
//!
//! Backends: [`mock::MockBackend`] (always available, used by tests and demos),
//! `gphoto::GphotoBackend` behind the `gphoto2` feature (DSLR / mirrorless
//! bodies via libgphoto2), and `webcam::WebcamBackend` behind the `webcam`
//! feature (UVC cameras including laptop webcams, HDMI capture cards, and
//! Android phones running in USB webcam mode).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

pub mod mock;
#[cfg(feature = "gphoto2")]
pub mod gphoto;
#[cfg(feature = "webcam")]
pub mod webcam;

mod multi;
pub use multi::MultiBackend;

pub type Result<T> = std::result::Result<T, CameraError>;

#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("no camera found")]
    NotFound,
    #[error("camera disconnected")]
    Disconnected,
    #[error("this camera does not support {0}")]
    Unsupported(&'static str),
    /// Another program holds the camera (macOS `ptpcamerad`, Image Capture, Photos, ...).
    #[error("camera is in use by another program: {0}")]
    Busy(String),
    /// Windows: the camera is still on the Windows driver instead of WinUSB (see README, Zadig).
    #[error("camera is using the Windows driver; switch it to WinUSB with Zadig (see README)")]
    WrongDriver,
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Backend(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
    /// Backend-specific address, e.g. `usb:001,004`.
    pub port: String,
}

impl DeviceInfo {
    pub fn display_name(&self) -> String {
        if self.make.is_empty() || self.model.starts_with(&self.make) {
            self.model.clone()
        } else {
            format!("{} {}", self.make, self.model)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub live_view: bool,
    pub capture: bool,
    pub download: bool,
    pub raw_plus_jpeg: bool,
}

impl Capabilities {
    /// Capture and download are both required (spec §4).
    pub fn usable(&self) -> bool {
        self.capture && self.download
    }
}

/// Opaque reference to files the camera produced for one shot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureHandle(pub Vec<String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Jpeg,
    Raw,
    Other,
}

impl FileKind {
    pub fn of(path: &Path) -> Self {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" => FileKind::Jpeg,
            "cr2" | "cr3" | "crw" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "rw2" | "raf" | "orf"
            | "ori" | "pef" | "dng" | "raw" => FileKind::Raw,
            _ => FileKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFile {
    pub path: PathBuf,
    pub kind: FileKind,
}

impl CapturedFile {
    pub fn new(path: PathBuf) -> Self {
        let kind = FileKind::of(&path);
        Self { path, kind }
    }
}

pub trait Camera: Send {
    fn info(&self) -> &DeviceInfo;
    fn capabilities(&self) -> Capabilities;
    fn start_live_view(&mut self) -> Result<LiveViewStream>;
    fn stop_live_view(&mut self) -> Result<()>;
    fn capture(&mut self) -> Result<CaptureHandle>;
    fn download(&mut self, handle: CaptureHandle, dest: &Path) -> Result<Vec<CapturedFile>>;
    fn close(self: Box<Self>) -> Result<()>;
}

pub trait CameraBackend: Send + Sync {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>>;
    fn open(&self, device: &DeviceInfo) -> Result<Box<dyn Camera>>;
}

/// Capture and download in one step.
pub fn shoot(camera: &mut dyn Camera, dest: &Path) -> Result<Vec<CapturedFile>> {
    let handle = camera.capture()?;
    camera.download(handle, dest)
}

/// One live view frame, JPEG-encoded as the camera sends it.
#[derive(Debug, Clone)]
pub struct LiveFrame {
    pub seq: u64,
    pub jpeg: Vec<u8>,
}

/// Receiving end of live view. Holds at most a couple of frames; the producer
/// drops frames the consumer hasn't caught up with, and [`latest`](Self::latest)
/// skips to the newest one.
pub struct LiveViewStream {
    rx: Receiver<LiveFrame>,
    stop: Arc<AtomicBool>,
}

impl LiveViewStream {
    /// Newest frame available right now, if any.
    pub fn latest(&self) -> Option<LiveFrame> {
        self.rx.try_iter().last()
    }

    /// Waits for the next frame, then skips to the newest. `Err(Disconnected)` once the producer ends.
    pub fn next_timeout(&self, timeout: Duration) -> Result<Option<LiveFrame>> {
        match self.rx.recv_timeout(timeout) {
            Ok(f) => Ok(Some(self.latest().unwrap_or(f))),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CameraError::Disconnected),
        }
    }
}

impl Drop for LiveViewStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Producing end of live view, held by a backend.
#[derive(Clone)]
pub struct LiveViewSender {
    tx: SyncSender<LiveFrame>,
    stop: Arc<AtomicBool>,
}

impl LiveViewSender {
    /// Offers a frame. Returns `false` once the stream is gone or stopped.
    pub fn send(&self, frame: LiveFrame) -> bool {
        if self.is_stopped() {
            return false;
        }
        match self.tx.try_send(frame) {
            Ok(()) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn live_view_channel() -> (LiveViewSender, LiveViewStream) {
    let (tx, rx) = mpsc::sync_channel(2);
    let stop = Arc::new(AtomicBool::new(false));
    (LiveViewSender { tx, stop: stop.clone() }, LiveViewStream { rx, stop })
}

/// The default real-camera backend for this build, if one was compiled in.
/// The real-camera backend for this build. Composes every compile-time-enabled
/// backend into a single [`MultiBackend`], so DSLRs (libgphoto2) and UVC
/// webcams (nokhwa) both show up in `stopgap cameras`. Returns `None` in a
/// mock-only build.
pub fn default_backend() -> Option<Box<dyn CameraBackend>> {
    let mut backends: Vec<Box<dyn CameraBackend>> = Vec::new();
    #[cfg(feature = "gphoto2")]
    backends.push(Box::new(gphoto::GphotoBackend));
    #[cfg(feature = "webcam")]
    backends.push(Box::new(webcam::WebcamBackend));
    if backends.is_empty() {
        None
    } else {
        Some(Box::new(MultiBackend::new(backends)))
    }
}

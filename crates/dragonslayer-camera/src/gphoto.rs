//! libgphoto2 backend.
//!
//! libgphoto2 isn't thread-safe, so each open camera lives on its own worker
//! thread and the [`Camera`] handle talks to it over channels. Live view frames
//! are pulled between commands, so a capture waits at most one preview frame.
//!
//! UNVERIFIED: written against the `gphoto2` crate v3 API but not yet built
//! against libgphoto2 or run on hardware (GH5 / EOS 100D).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gphoto2::camera::CameraEvent;
use gphoto2::Context;

use crate::{
    live_view_channel, Camera, CameraBackend, CameraError, Capabilities, CaptureHandle, CapturedFile, DeviceInfo,
    LiveFrame, LiveViewSender, LiveViewStream, Result,
};

/// After the first file, how long to wait between events before giving up on more.
/// A second file (RAW of a RAW+JPEG pair) normally arrives within a few tens of ms of the first.
const EXTRA_FILE_POLL: Duration = Duration::from_millis(400);

pub struct GphotoBackend;

impl CameraBackend for GphotoBackend {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>> {
        let ctx = Context::new().map_err(backend)?;
        let list = ctx.list_cameras().wait().map_err(backend)?;
        Ok(list.map(|d| device_info(&d.model, &d.port)).collect())
    }

    fn open(&self, device: &DeviceInfo) -> Result<Box<dyn Camera>> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let wanted = device.clone();
        let worker = thread::Builder::new()
            .name(format!("gphoto2 {}", device.port))
            .spawn(move || worker(wanted, cmd_rx, ready_tx))
            .map_err(|e| CameraError::Backend(e.to_string()))?;
        let caps = ready_rx.recv().map_err(|_| CameraError::Disconnected)??;
        Ok(Box::new(GphotoCamera { info: device.clone(), caps, cmd: cmd_tx, worker: Some(worker) }))
    }
}

pub struct GphotoCamera {
    info: DeviceInfo,
    caps: Capabilities,
    cmd: Sender<Cmd>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for GphotoCamera {
    /// If the caller didn't `close`, close now: send Close, then give the worker up to
    /// 3 s to run `gp_camera_exit` (releases the PTP session so the camera doesn't wedge).
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Close);
        let Some(worker) = self.worker.take() else { return };
        let deadline = Instant::now() + Duration::from_secs(3);
        while !worker.is_finished() {
            if Instant::now() >= deadline {
                // Leak the thread rather than block forever; libgphoto2 is stuck on IO.
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = worker.join();
    }
}

enum Cmd {
    StartLive(LiveViewSender),
    StopLive,
    Capture(Sender<Result<CaptureHandle>>),
    Download(CaptureHandle, PathBuf, Sender<Result<Vec<CapturedFile>>>),
    Close,
}

impl GphotoCamera {
    fn call<T>(&self, make: impl FnOnce(Sender<Result<T>>) -> Cmd) -> Result<T> {
        let (tx, rx) = mpsc::channel();
        self.cmd.send(make(tx)).map_err(|_| CameraError::Disconnected)?;
        rx.recv().map_err(|_| CameraError::Disconnected)?
    }
}

impl Camera for GphotoCamera {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    fn start_live_view(&mut self) -> Result<LiveViewStream> {
        if !self.caps.live_view {
            return Err(CameraError::Unsupported("live view"));
        }
        let (tx, rx) = live_view_channel();
        self.cmd.send(Cmd::StartLive(tx)).map_err(|_| CameraError::Disconnected)?;
        Ok(rx)
    }

    fn stop_live_view(&mut self) -> Result<()> {
        self.cmd.send(Cmd::StopLive).map_err(|_| CameraError::Disconnected)
    }

    fn capture(&mut self) -> Result<CaptureHandle> {
        let handle = self.call(Cmd::Capture)?;
        if handle.0.len() > 1 {
            self.caps.raw_plus_jpeg = true;
        }
        Ok(handle)
    }

    fn download(&mut self, handle: CaptureHandle, dest: &Path) -> Result<Vec<CapturedFile>> {
        let dest = dest.to_path_buf();
        self.call(|tx| Cmd::Download(handle, dest, tx))
    }

    fn close(self: Box<Self>) -> Result<()> {
        // Drop does the join-with-timeout so the PTP session is released.
        Ok(())
    }
}

fn worker(wanted: DeviceInfo, cmds: Receiver<Cmd>, ready: Sender<Result<Capabilities>>) {
    let opened = (|| {
        let ctx = Context::new().map_err(backend)?;
        let desc = ctx
            .list_cameras()
            .wait()
            .map_err(backend)?
            .find(|d| d.port == wanted.port)
            .ok_or(CameraError::NotFound)?;
        let camera = ctx.get_camera(&desc).wait().map_err(open_error)?;
        let ops = camera.abilities().camera_operations();
        // RAW+JPEG can't be known until a capture returns two files; `capture` updates it.
        let caps = Capabilities {
            live_view: ops.capture_preview(),
            capture: ops.capture_image(),
            download: true,
            raw_plus_jpeg: false,
        };
        Ok((ctx, camera, caps))
    })();

    let (ctx, camera, caps) = match opened {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(caps));

    let mut live: Option<LiveViewSender> = None;
    let mut seq = 0u64;
    let outcome = 'outer: loop {
        // Block while idle; poll while live view is running.
        let cmd = if live.is_some() {
            match cmds.try_recv() {
                Ok(c) => Some(c),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => break 'outer,
            }
        } else {
            match cmds.recv_timeout(Duration::from_secs(1)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break 'outer,
            }
        };

        match cmd {
            Some(Cmd::StartLive(tx)) => live = Some(tx),
            Some(Cmd::StopLive) => live = None,
            Some(Cmd::Capture(reply)) => {
                let _ = reply.send(capture(&camera));
            }
            Some(Cmd::Download(handle, dest, reply)) => {
                let _ = reply.send(download(&camera, &handle, &dest));
            }
            Some(Cmd::Close) => break 'outer,
            None => {}
        }

        if let Some(tx) = &live {
            if tx.is_stopped() {
                live = None;
                continue;
            }
            match camera.capture_preview().wait().and_then(|f| f.get_data(&ctx).wait()) {
                Ok(data) => {
                    seq += 1;
                    if !tx.send(LiveFrame { seq, jpeg: data.into_vec() }) {
                        live = None;
                    }
                }
                // Dropping the sender ends the stream; the UI sees Disconnected.
                Err(_) => live = None,
            }
        }
    };
    // Release the PTP session cleanly so the camera doesn't wedge (gphoto2's Drop
    // only unrefs the struct; without gp_camera_exit the camera stays in the
    // half-open state we hit earlier).
    exit_camera(&camera, &ctx);
    let _ = outcome;
}

/// Call `gp_camera_exit` via the sys crate. Ignored on failure — the camera may
/// have been unplugged already; there's nothing sensible to do about it here.
fn exit_camera(camera: &gphoto2::Camera, ctx: &Context) {
    let cam_ptr = camera.as_ref() as *const _ as *mut libgphoto2_sys::Camera;
    let ctx_ptr = ctx.as_ref() as *const _ as *mut libgphoto2_sys::GPContext;
    unsafe {
        libgphoto2_sys::gp_camera_exit(cam_ptr, ctx_ptr);
    }
}

fn capture(camera: &gphoto2::Camera) -> Result<CaptureHandle> {
    let first = camera.capture_image().wait().map_err(backend)?;
    let mut files = vec![join(&first.folder(), &first.name())];

    // With RAW+JPEG the second file arrives as a NewFile event, usually within tens of ms.
    // We break on the first quiet interval, so JPEG-only shooters don't pay a fixed wait.
    loop {
        match camera.wait_event(EXTRA_FILE_POLL).wait() {
            Ok(CameraEvent::NewFile(p)) => files.push(join(&p.folder(), &p.name())),
            Ok(CameraEvent::CaptureComplete) => break,
            Ok(CameraEvent::Timeout) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Ok(CaptureHandle(files))
}

fn download(camera: &gphoto2::Camera, handle: &CaptureHandle, dest: &Path) -> Result<Vec<CapturedFile>> {
    let fs = camera.fs();
    let mut out = Vec::new();
    for remote in &handle.0 {
        let (folder, name) = remote.rsplit_once('/').unwrap_or(("/", remote));
        let folder = if folder.is_empty() { "/" } else { folder };
        let path = dest.join(name);
        fs.download_to(folder, name, &path).wait().map_err(backend)?;
        out.push(CapturedFile::new(path));
    }
    Ok(out)
}

fn join(folder: &str, name: &str) -> String {
    format!("{}/{}", folder.trim_end_matches('/'), name)
}

fn device_info(model: &str, port: &str) -> DeviceInfo {
    let (make, rest) = model.split_once(' ').unwrap_or(("", model));
    DeviceInfo {
        make: make.to_owned(),
        model: if make.is_empty() { rest.to_owned() } else { model.to_owned() },
        serial: None,
        port: port.to_owned(),
    }
}

fn backend(e: gphoto2::Error) -> CameraError {
    let msg = e.to_string();
    let lower = msg.to_ascii_lowercase();
    if cfg!(windows) && (lower.contains("claim") || lower.contains("access denied")) {
        return CameraError::WrongDriver;
    }
    if lower.contains("claim") || lower.contains("busy") || lower.contains("in use") {
        return CameraError::Busy(
            if cfg!(target_os = "macos") {
                "quit Image Capture/Photos, then run `killall ptpcamerad` and reconnect".into()
            } else {
                "close any camera app using the camera, then reconnect".into()
            },
        );
    }
    if lower.contains("not connected") || lower.contains("no camera") {
        return CameraError::NotFound;
    }
    CameraError::Backend(msg)
}

/// Turns "can't claim the device" into guidance (spec §5).
fn open_error(e: gphoto2::Error) -> CameraError {
    let msg = e.to_string();
    let lower = msg.to_ascii_lowercase();
    if lower.contains("claim") || lower.contains("busy") || lower.contains("in use") {
        if cfg!(windows) {
            CameraError::WrongDriver
        } else {
            CameraError::Busy(
                "quit Image Capture/Photos, then run `killall ptpcamerad` and reconnect the camera".into(),
            )
        }
    } else {
        CameraError::Backend(msg)
    }
}

//! Camera session thread. Owns the camera, forwards live view, runs captures,
//! and notices disconnects (polling every second when there's no live view).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage};
use dragonslayer_camera::{
    Camera, CameraBackend, CameraError, Capabilities, DeviceInfo, LiveViewStream, Setting, SettingKind,
};
use dragonslayer_core::capture::{self, Captured};
use dragonslayer_core::Project;

const POLL_EVERY: Duration = Duration::from_millis(1000);

pub enum Cmd {
    Capture(Project),
    /// Request that the camera worker stops (false) or resumes (true) the live-view feed.
    /// Panasonic PTP wedges under a sustained stream of preview requests, so we pause
    /// live view when the UI enters Preview mode where it's not needed.
    SetLiveActive(bool),
    /// Change one camera setting; the worker re-reads all settings afterwards.
    SetSetting(SettingKind, String),
}

#[derive(Clone, Debug)]
pub enum Status {
    NoBackend,
    Searching,
    Connected { name: String, caps: Capabilities },
    /// Windows: camera is detected but bound to the stock driver, not WinUSB. Actionable.
    WrongDriver { name: Option<String> },
    Problem { name: Option<String>, message: String },
}

pub enum Event {
    Status(Status),
    Live(ColorImage),
    Captured(Result<Captured, String>),
    /// Current camera settings. Sent on connect and after every change attempt.
    Settings(Vec<Setting>),
    SettingFailed(String),
}

pub struct Session {
    pub cmd: Sender<Cmd>,
    pub events: Receiver<Event>,
}

impl Session {
    pub fn start(backend: Option<Box<dyn CameraBackend>>, ctx: egui::Context) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        thread::Builder::new()
            .name("camera session".into())
            .spawn(move || Worker::new(backend, ev_tx, ctx).run(cmd_rx))
            .expect("spawn camera thread");
        Self { cmd: cmd_tx, events: ev_rx }
    }
}

struct Worker {
    backend: Option<Box<dyn CameraBackend>>,
    /// True while the UI wants a live feed. Set false when the app enters Preview mode
    /// so the camera worker stops issuing PTP preview requests (Panasonic PTP wedges
    /// under sustained preview traffic; give the camera a rest while browsing frames).
    want_live: bool,
    events: Sender<Event>,
    ctx: egui::Context,
    camera: Option<(DeviceInfo, Box<dyn Camera>)>,
    live: Option<LiveViewStream>,
    last_poll: Option<Instant>,
    last_status: Option<String>,
}

impl Worker {
    fn new(backend: Option<Box<dyn CameraBackend>>, events: Sender<Event>, ctx: egui::Context) -> Self {
        Self { backend, want_live: true, events, ctx, camera: None, live: None, last_poll: None, last_status: None }
    }

    fn send(&self, ev: Event) {
        // Live-view events don't need a repaint request: the app schedules its own repaint
        // every frame while `live.is_some()`. Calling request_repaint 15 times a second from
        // this thread on top of that hammered egui's viewport lock (visible on macOS CI
        // as import worker starvation).
        let repaint = !matches!(ev, Event::Live(_));
        let _ = self.events.send(ev);
        if repaint {
            self.ctx.request_repaint();
        }
    }

    fn status(&mut self, s: Status) {
        let key = format!("{s:?}");
        if self.last_status.as_deref() != Some(&key) {
            self.last_status = Some(key);
            self.send(Event::Status(s));
        }
    }

    fn run(mut self, cmds: Receiver<Cmd>) {
        if self.backend.is_none() {
            self.status(Status::NoBackend);
        }
        loop {
            // Wait for a command, or for a live frame while live view is running.
            let cmd = if let Some(live) = &self.live {
                match cmds.try_recv() {
                    Ok(c) => Some(c),
                    Err(mpsc::TryRecvError::Empty) => {
                        match live.next_timeout(Duration::from_millis(40)) {
                            Ok(Some(frame)) => {
                                if let Some(img) = decode(&frame.jpeg) {
                                    self.send(Event::Live(img));
                                }
                            }
                            Ok(None) => {}
                            Err(_) => self.lost("Live view stopped: the camera stopped responding."),
                        }
                        None
                    }
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            } else {
                match cmds.recv_timeout(Duration::from_millis(200)) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };

            match cmd {
                Some(Cmd::Capture(project)) => self.capture(&project),
                Some(Cmd::SetLiveActive(active)) => self.set_live_active(active),
                Some(Cmd::SetSetting(kind, value)) => self.set_setting(kind, &value),
                None => {}
            }

            if self.last_poll.is_none_or(|t| t.elapsed() >= POLL_EVERY) {
                self.last_poll = Some(Instant::now());
                self.poll();
            }
        }
    }

    /// Connect if we have no camera; otherwise check it is still plugged in.
    fn poll(&mut self) {
        let Some(backend) = &self.backend else { return };
        let devices = match backend.enumerate() {
            Ok(d) => d,
            Err(e) => {
                let msg = explain(&e);
                self.status(Status::Problem { name: None, message: msg });
                return;
            }
        };
        if let Some((info, _)) = &self.camera {
            // Live view errors already catch disconnects quickly; this covers idle time.
            if self.live.is_none() && !devices.iter().any(|d| d.port == info.port) {
                self.lost("Camera disconnected. Reconnect it to keep shooting.");
            }
            return;
        }
        let Some(device) = devices.into_iter().next() else {
            self.status(Status::Searching);
            return;
        };
        match backend.open(&device) {
            Ok(cam) => {
                let caps = cam.capabilities();
                if !caps.usable() {
                    let _ = cam.close();
                    self.status(Status::Problem {
                        name: Some(device.display_name()),
                        message: "This camera can't capture and download over USB, so DragonSlayer can't use it. See CAMERAS.md.".into(),
                    });
                    return;
                }
                self.status(Status::Connected { name: device.display_name(), caps });
                self.camera = Some((device, cam));
                self.save_to_card();
                self.send_settings();
                self.start_live();
            }
            Err(CameraError::WrongDriver) => {
                self.status(Status::WrongDriver { name: Some(device.display_name()) });
            }
            Err(e) => {
                let message = explain(&e);
                self.status(Status::Problem { name: Some(device.display_name()), message });
            }
        }
    }

    fn start_live(&mut self) {
        if !self.want_live {
            return;
        }
        let Some((_, cam)) = &mut self.camera else { return };
        if !cam.capabilities().live_view || self.live.is_some() {
            return;
        }
        match cam.start_live_view() {
            Ok(stream) => self.live = Some(stream),
            Err(e) => {
                let msg = explain(&e);
                self.lost(&msg);
            }
        }
    }

    fn set_live_active(&mut self, active: bool) {
        if self.want_live == active {
            return;
        }
        self.want_live = active;
        if active {
            self.start_live();
        } else {
            self.live = None;
            if let Some((_, cam)) = &mut self.camera {
                let _ = cam.stop_live_view();
            }
        }
    }

    fn lost(&mut self, message: &str) {
        let name = self.camera.as_ref().map(|(d, _)| d.display_name());
        self.live = None;
        if let Some((_, cam)) = self.camera.take() {
            let _ = cam.close();
        }
        self.status(Status::Problem { name, message: message.into() });
    }

    /// Read once on connect and after changes only: polling would add PTP traffic,
    /// which is what wedges Panasonic bodies.
    /// Tethered cameras (Canon especially) default to keeping shots only in their own
    /// memory, so nothing lands on the card. Switch to the card when the camera offers it:
    /// every frame then has a second copy, which is what the card-rescue import relies on.
    fn save_to_card(&mut self) {
        let Some((_, cam)) = &mut self.camera else { return };
        let Ok(settings) = cam.settings() else { return };
        let Some(target) = settings.iter().find(|s| s.kind == SettingKind::CaptureTarget) else { return };
        if target.readonly || target.value.to_ascii_lowercase().contains("card") {
            return;
        }
        if let Some(card) = dragonslayer_camera::card_choice(&target.choices).map(str::to_owned)
            && let Err(e) = cam.set_setting(SettingKind::CaptureTarget, &card)
        {
            let msg = format!("Couldn't switch the camera to save on its memory card: {}", explain(&e));
            self.send(Event::SettingFailed(msg));
        }
    }

    fn send_settings(&mut self) {
        let Some((_, cam)) = &mut self.camera else { return };
        match cam.settings() {
            Ok(settings) => self.send(Event::Settings(settings)),
            Err(e) => self.send(Event::SettingFailed(explain(&e))),
        }
    }

    fn set_setting(&mut self, kind: SettingKind, value: &str) {
        let Some((_, cam)) = &mut self.camera else { return };
        if let Err(e) = cam.set_setting(kind, value) {
            let msg = format!("Couldn't set {}: {}", kind.label(), explain(&e));
            self.send(Event::SettingFailed(msg));
        }
        // Re-read either way: the camera may have snapped to a nearby value, or refused.
        self.send_settings();
    }

    fn capture(&mut self, project: &Project) {
        let Some((info, cam)) = &mut self.camera else {
            self.send(Event::Captured(Err("No camera connected.".into())));
            return;
        };
        let name = info.display_name();
        let mut disconnected = false;
        let result = capture::capture(project, Some(&name), |dir| {
            dragonslayer_camera::shoot(cam.as_mut(), dir)
                .map(|files| files.into_iter().map(|f| f.path).collect())
                .map_err(|e| {
                    disconnected = matches!(e, CameraError::Disconnected);
                    explain(&e)
                })
        });
        self.send(Event::Captured(result.map_err(|e| e.to_string())));
        if disconnected {
            self.lost("Camera disconnected during capture. Reconnect it; the frame may still be on the card.");
        }
    }
}

/// Plain-English fixes for the common failures (spec §12).
fn explain(e: &CameraError) -> String {
    match e {
        CameraError::WrongDriver => "Windows is still using its own driver for this camera. Run Zadig once to switch it to WinUSB (README → \"Windows: Zadig\").".into(),
        CameraError::Busy(fix) => format!("Another program is using the camera. {fix}"),
        CameraError::Disconnected => "Camera disconnected. Reconnect it to keep shooting.".into(),
        CameraError::NotFound => "Camera not found. Check it is on, in PC/PTP mode, and plugged in.".into(),
        other => other.to_string(),
    }
}

pub fn decode(jpeg: &[u8]) -> Option<ColorImage> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

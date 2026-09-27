//! A fake camera that renders a moving square. Lets the whole pipeline run without hardware.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::{Rgb, RgbImage};

use crate::{
    live_view_channel, Camera, CameraBackend, CameraError, Capabilities, CaptureHandle, CapturedFile, DeviceInfo,
    LiveFrame, LiveViewSender, LiveViewStream, Result, Setting, SettingKind,
};

pub struct MockBackend {
    pub capabilities: Capabilities,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self { capabilities: Capabilities { live_view: true, capture: true, download: true, raw_plus_jpeg: true } }
    }
}

impl MockBackend {
    fn device() -> DeviceInfo {
        DeviceInfo { make: "DragonSlayer".into(), model: "Mock Camera".into(), serial: Some("MOCK0001".into()), port: "mock:0".into() }
    }
}

impl CameraBackend for MockBackend {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>> {
        Ok(vec![Self::device()])
    }

    fn open(&self, device: &DeviceInfo) -> Result<Box<dyn Camera>> {
        if device.port != "mock:0" {
            return Err(CameraError::NotFound);
        }
        Ok(Box::new(MockCamera {
            info: Self::device(),
            caps: self.capabilities,
            shots: 0,
            pending: None,
            live: None,
            settings: default_settings(),
        }))
    }
}

pub struct MockCamera {
    info: DeviceInfo,
    caps: Capabilities,
    shots: u64,
    pending: Option<u64>,
    live: Option<LiveViewSender>,
    settings: Vec<Setting>,
}

/// A plausible manual-mode camera. White balance is read-only to exercise that path in the UI.
fn default_settings() -> Vec<Setting> {
    let s = |kind, value: &str, choices: &[&str], readonly| Setting {
        kind,
        value: value.into(),
        choices: choices.iter().map(|c| c.to_string()).collect(),
        readonly,
    };
    vec![
        s(SettingKind::Aperture, "f/5.6", &["f/2.8", "f/4", "f/5.6", "f/8", "f/11", "f/16"], false),
        s(SettingKind::Shutter, "1/60", &["1/250", "1/125", "1/60", "1/30", "1/15", "1/8", "1/4", "1/2", "1"], false),
        s(SettingKind::Iso, "200", &["100", "200", "400", "800", "1600"], false),
        s(SettingKind::WhiteBalance, "Daylight", &["Auto", "Daylight", "Tungsten", "Fluorescent"], true),
        s(SettingKind::ImageFormat, "RAW + Large Fine JPEG", &["Large Fine JPEG", "RAW", "RAW + Large Fine JPEG"], false),
    ]
}

impl Camera for MockCamera {
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
        self.stop_live_view()?;
        let (tx, rx) = live_view_channel();
        let producer = tx.clone();
        let start = self.shots;
        thread::spawn(move || {
            for seq in 0.. {
                let t = start as f32 + seq as f32 / 15.0;
                if !producer.send(LiveFrame { seq, jpeg: render(640, 360, t, 70) }) {
                    break;
                }
                thread::sleep(Duration::from_millis(66));
            }
        });
        self.live = Some(tx);
        Ok(rx)
    }

    fn stop_live_view(&mut self) -> Result<()> {
        if let Some(tx) = self.live.take() {
            tx.stop();
        }
        Ok(())
    }

    fn capture(&mut self) -> Result<CaptureHandle> {
        if !self.caps.capture {
            return Err(CameraError::Unsupported("capture"));
        }
        self.shots += 1;
        self.pending = Some(self.shots);
        let n = self.shots;
        let mut files = vec![format!("/store_00010001/DCIM/100MOCK/MOCK{n:04}.JPG")];
        if self.caps.raw_plus_jpeg {
            files.push(format!("/store_00010001/DCIM/100MOCK/MOCK{n:04}.RAW"));
        }
        Ok(CaptureHandle(files))
    }

    fn download(&mut self, handle: CaptureHandle, dest: &Path) -> Result<Vec<CapturedFile>> {
        let n = self.pending.take().ok_or_else(|| CameraError::Backend("nothing to download".into()))?;
        let mut out = Vec::new();
        for remote in handle.0 {
            let name = remote.rsplit('/').next().unwrap_or(&remote);
            let path = dest.join(name);
            let bytes = if name.ends_with(".JPG") {
                render(1920, 1080, n as f32, 90)
            } else {
                format!("MOCKRAW frame {n}\n").into_bytes()
            };
            fs::write(&path, bytes).map_err(|source| CameraError::Io { path: path.clone(), source })?;
            out.push(CapturedFile::new(path));
        }
        Ok(out)
    }

    fn close(mut self: Box<Self>) -> Result<()> {
        self.stop_live_view()
    }

    fn settings(&mut self) -> Result<Vec<Setting>> {
        Ok(self.settings.clone())
    }

    fn set_setting(&mut self, kind: SettingKind, value: &str) -> Result<()> {
        let s = self
            .settings
            .iter_mut()
            .find(|s| s.kind == kind)
            .ok_or(CameraError::Unsupported("that setting"))?;
        if s.readonly {
            return Err(CameraError::Backend(format!("{} is read-only in the current camera mode", kind.label())));
        }
        if !s.choices.iter().any(|c| c == value) {
            return Err(CameraError::Backend(format!("{value:?} is not a valid {}", kind.label())));
        }
        s.value = value.into();
        Ok(())
    }
}

/// A grey backdrop with an orange square whose position depends on `t`.
fn render(w: u32, h: u32, t: f32, quality: u8) -> Vec<u8> {
    let size = h / 5;
    let travel = (w - size) as f32;
    let x0 = ((t * 0.05).fract() * travel) as u32;
    let y0 = h / 2 - size / 2;
    let img = RgbImage::from_fn(w, h, |x, y| {
        if (x0..x0 + size).contains(&x) && (y0..y0 + size).contains(&y) {
            Rgb([235, 120, 40])
        } else {
            let g = 60 + (y * 60 / h) as u8;
            Rgb([g, g, g + 8])
        }
    });
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality).encode_image(&img).expect("in-memory encode");
    buf
}

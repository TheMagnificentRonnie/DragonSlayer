//! Composes several [`CameraBackend`]s into one. Enumeration unions their
//! device lists; open dispatches to whichever backend enumerated the device
//! (matched by port prefix, e.g. `usb:` → libgphoto2, `webcam:` → nokhwa).

use crate::{Camera, CameraBackend, CameraError, DeviceInfo, Result};

pub struct MultiBackend {
    inner: Vec<Box<dyn CameraBackend>>,
}

impl MultiBackend {
    pub fn new(inner: Vec<Box<dyn CameraBackend>>) -> Self {
        Self { inner }
    }
}

impl CameraBackend for MultiBackend {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>> {
        let mut out = Vec::new();
        for b in &self.inner {
            // Best-effort: one backend failing (e.g. libgphoto2 sees no
            // devices and returns an error on some platforms) shouldn't hide
            // devices from the others.
            if let Ok(mut list) = b.enumerate() {
                out.append(&mut list);
            }
        }
        Ok(out)
    }

    fn open(&self, device: &DeviceInfo) -> Result<Box<dyn Camera>> {
        // Route by re-enumerating each backend and taking the one that owns
        // this port. Cheap because each enumerate is a quick USB scan.
        for b in &self.inner {
            if let Ok(list) = b.enumerate()
                && list.iter().any(|d| d.port == device.port)
            {
                return b.open(device);
            }
        }
        Err(CameraError::NotFound)
    }
}

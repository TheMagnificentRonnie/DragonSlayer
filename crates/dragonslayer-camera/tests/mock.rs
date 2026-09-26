//! Tests for the mock camera backend. The mock is what the CLI runs under
//! `--mock` and what CI uses when no hardware is attached, so it's worth
//! keeping honest.

use std::time::Duration;

use dragonslayer_camera::mock::MockBackend;
use dragonslayer_camera::{CameraBackend, CameraError, Capabilities, FileKind};

#[test]
fn enumerate_reports_exactly_one_device_with_a_stable_port() {
    let backend = MockBackend::default();
    let devices = backend.enumerate().unwrap();
    assert_eq!(devices.len(), 1);
    let d = &devices[0];
    assert_eq!(d.port, "mock:0");
    assert!(!d.display_name().is_empty());
}

#[test]
fn open_by_wrong_port_returns_not_found() {
    let backend = MockBackend::default();
    let mut fake = backend.enumerate().unwrap().pop().unwrap();
    fake.port = "usb:001,002".into();
    assert!(matches!(backend.open(&fake), Err(CameraError::NotFound)));
}

#[test]
fn capabilities_default_is_fully_usable() {
    let backend = MockBackend::default();
    let device = backend.enumerate().unwrap().pop().unwrap();
    let cam = backend.open(&device).unwrap();
    let c = cam.capabilities();
    assert!(c.usable());
    assert!(c.live_view);
    assert!(c.raw_plus_jpeg);
    cam.close().ok();
}

#[test]
fn shoot_writes_a_jpeg_and_a_raw_that_both_decode_as_expected() {
    // The mock returns .JPG and .RAW pairs. Verifying both files reach disk
    // and the JPEG is a real JPEG (magic bytes) — not just an empty stub —
    // catches regressions where the mock stops producing usable output.
    let tmp = tempfile::tempdir().unwrap();
    let backend = MockBackend::default();
    let device = backend.enumerate().unwrap().pop().unwrap();
    let mut cam = backend.open(&device).unwrap();
    let files = dragonslayer_camera::shoot(cam.as_mut(), tmp.path()).unwrap();
    assert_eq!(files.len(), 2);
    let jpeg = files.iter().find(|f| f.kind == FileKind::Jpeg).unwrap();
    let raw = files.iter().find(|f| f.kind == FileKind::Raw).unwrap();
    assert!(jpeg.path.is_file());
    assert!(raw.path.is_file());
    let jpeg_bytes = std::fs::read(&jpeg.path).unwrap();
    assert!(jpeg_bytes.starts_with(&[0xff, 0xd8, 0xff]), "not a JPEG: {:?}", &jpeg_bytes[..8.min(jpeg_bytes.len())]);
    cam.close().ok();
}

#[test]
fn download_without_a_prior_capture_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = MockBackend::default();
    let device = backend.enumerate().unwrap().pop().unwrap();
    let mut cam = backend.open(&device).unwrap();
    // Fabricate a handle without going through capture().
    let handle = dragonslayer_camera::CaptureHandle(vec!["/store/DCIM/nope.JPG".into()]);
    let err = cam.download(handle, tmp.path()).unwrap_err();
    assert!(matches!(err, CameraError::Backend(_)), "got {err:?}");
    cam.close().ok();
}

#[test]
fn download_after_capture_consumes_the_pending_shot() {
    // After download, a second download without another capture must fail —
    // that's the invariant that keeps recovery honest.
    let tmp = tempfile::tempdir().unwrap();
    let backend = MockBackend::default();
    let device = backend.enumerate().unwrap().pop().unwrap();
    let mut cam = backend.open(&device).unwrap();
    let handle = cam.capture().unwrap();
    let files = cam.download(handle.clone(), tmp.path()).unwrap();
    assert!(!files.is_empty());
    // Same handle again → nothing pending.
    let err = cam.download(handle, tmp.path()).unwrap_err();
    assert!(matches!(err, CameraError::Backend(_)), "got {err:?}");
    cam.close().ok();
}

#[test]
fn live_view_delivers_frames_and_stops_when_stream_dropped() {
    let backend = MockBackend::default();
    let device = backend.enumerate().unwrap().pop().unwrap();
    let mut cam = backend.open(&device).unwrap();
    let stream = cam.start_live_view().unwrap();
    // The producer runs on a thread, so first frame may take a moment.
    let frame = stream.next_timeout(Duration::from_secs(2)).unwrap();
    assert!(frame.is_some(), "no live view frame within 2s");
    let jpeg = frame.unwrap().jpeg;
    assert!(jpeg.starts_with(&[0xff, 0xd8, 0xff]), "live frame not a JPEG");
    // Dropping the stream signals the producer to stop.
    drop(stream);
    // Second start should succeed (no leftover state stopping us).
    let stream2 = cam.start_live_view().unwrap();
    let frame2 = stream2.next_timeout(Duration::from_secs(2)).unwrap();
    assert!(frame2.is_some(), "no live view frame on restart");
    cam.close().ok();
}

#[test]
fn live_view_errors_when_backend_disables_the_capability() {
    let backend = MockBackend { capabilities: Capabilities { live_view: false, ..MockBackend::default().capabilities } };
    let device = backend.enumerate().unwrap().pop().unwrap();
    let mut cam = backend.open(&device).unwrap();
    // LiveViewStream isn't Debug, so unwrap_err doesn't compile — pattern match instead.
    match cam.start_live_view() {
        Err(CameraError::Unsupported("live view")) => {}
        Err(e) => panic!("wrong error: {e:?}"),
        Ok(_) => panic!("expected an error"),
    }
    cam.close().ok();
}

#[test]
fn capture_errors_when_backend_disables_the_capability() {
    let backend = MockBackend { capabilities: Capabilities { capture: false, ..MockBackend::default().capabilities } };
    let device = backend.enumerate().unwrap().pop().unwrap();
    let mut cam = backend.open(&device).unwrap();
    let err = cam.capture().unwrap_err();
    assert!(matches!(err, CameraError::Unsupported("capture")), "got {err:?}");
    // With no capture, the camera reports itself as not usable.
    assert!(!cam.capabilities().usable());
    cam.close().ok();
}

#[test]
fn jpeg_only_configuration_returns_no_raw_file() {
    let backend = MockBackend {
        capabilities: Capabilities { raw_plus_jpeg: false, ..MockBackend::default().capabilities },
    };
    let device = backend.enumerate().unwrap().pop().unwrap();
    let mut cam = backend.open(&device).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let files = dragonslayer_camera::shoot(cam.as_mut(), tmp.path()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, FileKind::Jpeg);
    cam.close().ok();
}

#[test]
fn file_kind_recognises_common_raw_extensions_case_insensitively() {
    use std::path::Path;
    let jpegs = ["a.jpg", "a.JPG", "a.jpeg", "a.JPEG"];
    for p in jpegs {
        assert_eq!(FileKind::of(Path::new(p)), FileKind::Jpeg, "{p}");
    }
    let raws = ["a.cr2", "a.CR3", "a.nef", "a.arw", "a.rw2", "a.dng", "a.raw"];
    for p in raws {
        assert_eq!(FileKind::of(Path::new(p)), FileKind::Raw, "{p}");
    }
    assert_eq!(FileKind::of(Path::new("a.png")), FileKind::Other);
    assert_eq!(FileKind::of(Path::new("no-extension")), FileKind::Other);
}

#[test]
fn capabilities_usable_requires_both_capture_and_download() {
    let cases = [
        (Capabilities { live_view: true, capture: true, download: true, raw_plus_jpeg: true }, true),
        (Capabilities { live_view: true, capture: false, download: true, raw_plus_jpeg: true }, false),
        (Capabilities { live_view: true, capture: true, download: false, raw_plus_jpeg: true }, false),
        (Capabilities { live_view: false, capture: true, download: true, raw_plus_jpeg: false }, true),
    ];
    for (caps, want) in cases {
        assert_eq!(caps.usable(), want, "{caps:?}");
    }
}

#[test]
fn display_name_does_not_duplicate_the_make_when_the_model_starts_with_it() {
    use dragonslayer_camera::DeviceInfo;
    let d = DeviceInfo { make: "Panasonic".into(), model: "Panasonic DC-GH5".into(), serial: None, port: "usb:1".into() };
    assert_eq!(d.display_name(), "Panasonic DC-GH5");
    let d = DeviceInfo { make: "Canon".into(), model: "EOS 100D".into(), serial: None, port: "usb:2".into() };
    assert_eq!(d.display_name(), "Canon EOS 100D");
    let d = DeviceInfo { make: String::new(), model: "Some Model".into(), serial: None, port: "usb:3".into() };
    assert_eq!(d.display_name(), "Some Model");
}

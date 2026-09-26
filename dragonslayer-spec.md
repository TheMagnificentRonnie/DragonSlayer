# DragonSlayer: Free Stop-Motion Capture for Students

**Working title.** Spec v0.7, 26 September 2026.

## 1. Purpose

Students who own a real camera (DSLR or mirrorless) can't afford tethered stop-motion software. Phones are already covered by free apps; real cameras aren't.

DragonSlayer is a free, open source app that tethers the camera a student already owns, lets them capture a film scene by scene with onion skinning, and compiles it into a video. Nothing else.

## 2. Principles

- **Free and open source.** Hosted on GitLab. Source only; no compiled binaries.
- **Small.** Capture and compile. Anything not needed to make a film stays out.
- **Broad camera support for little effort.** One camera library (libgphoto2) covers most cameras students own. The project tests on one reference camera and relies on user reports for the rest.
- **Never lose a frame.**

## 3. Scope

### In

- Projects containing ordered scenes; create, rename, reorder and delete scenes.
- Connect to a camera over USB via libgphoto2.
- Live view.
- Onion skin over live view: previous 1 to 5 frames of the active scene, adjustable opacity, on/off.
- Toggle between live view and the last captured frame.
- Capture a frame into the active scene (full-res; RAW+JPEG where the camera supports it).
- Delete the last frame of the active scene.
- Compile one scene, or the whole project (scenes in order), into a video.
- Windows and macOS. Linux works through the same code but isn't a release target.

### Out

Timeline editing, takes, exposure or focus control from the app, audio, x-sheet, drawing tools, DMX, motion control, multi-camera, webcams, phones, vendor SDKs. Exposure, focus and white balance are set on the camera.

## 4. Camera support

### Library

**libgphoto2** (LGPL) on all platforms. It supports hundreds of cameras, including the bodies students most commonly own: Canon EOS (entry-level DSLRs and mirrorless), Nikon, Sony Alpha, Panasonic Lumix, Fujifilm, OM System/Olympus.

### Reference camera

Panasonic Lumix GH5. Every release is tested end to end on it, on Windows and macOS.

### Other cameras

- The README links to libgphoto2's supported camera list.
- `CAMERAS.md` in the repo records community reports per model: connects, live view, capture, download, notes.
- A GitLab issue template for camera reports.
- If a camera connects but has no live view, DragonSlayer still captures, and onion skin runs against the last frame instead of live.

### Capabilities check

On connect, DragonSlayer probes what the camera supports and adjusts the UI:

| Capability | If missing |
|---|---|
| Live view | Show last frame only; capture still works |
| Capture | Camera unusable; clear message and link to CAMERAS.md |
| Download to computer | Camera unusable; same |
| RAW+JPEG | Use JPEG; warn that no RAW is kept |

## 5. Platforms

### macOS

- libgphoto2 via Homebrew.
- macOS processes (`ptpcamerad`, Image Capture, Photos) often grab the camera. DragonSlayer detects this and tells the user how to release it.

### Windows

- **v1: libgphoto2 with libusb.** Windows binds cameras to its own driver by default, which libusb can't use. The user swaps the camera's driver to WinUSB once using **Zadig** (https://zadig.akeo.ie).
- The README has a step-by-step Zadig guide with screenshots, including how to switch back in Device Manager.
- Side effect: while swapped, the Photos app and vendor tools won't see the camera.
- DragonSlayer detects "camera plugged in but still on the Windows driver" and shows the Zadig instructions instead of a generic error.
- **Later (contributions welcome):** port libgphoto2's PTP transport to Windows Portable Devices (WPD), which can send raw PTP commands through Windows' own driver. That removes the Zadig step. Not v1.

## 6. Architecture

```
+------------------------------------------------------------+
|  UI: scene list | live view + onion skin | Capture | Compile |
+-----------------------------+------------------------------+
                              |
+-----------------------------v------------------------------+
|  Core (Rust)                                                |
|  project + scenes | camera session | frame store | compile  |
+-----------------------------+------------------------------+
                              |
                        libgphoto2 (FFI)
```

- **Core.** Rust library: project model, camera session, frame store, compile.
- **Camera.** Thin Rust wrapper over libgphoto2, implementing the interface in section 7, so a WPD backend can be added later without touching anything else.
- **Compositor.** GPU-backed (wgpu). Blends live view with onion skin frames, which are the scene's JPEGs downscaled to live view size and cached in memory on capture and on scene switch.
- **Compile.** Calls ffmpeg.
- **UI.** Single window. Toolkit open (section 15).
- **CLI.** Same operations as the UI, for testing and scripting:
  - `dragonslayer new <project>`
  - `dragonslayer scene add|rename|move|delete ...`
  - `dragonslayer cameras` (list detected cameras and capabilities)
  - `dragonslayer capture <project> <scene>`
  - `dragonslayer compile <project> [<scene>]`

## 7. Camera interface

```rust
pub struct Capabilities {
    pub live_view: bool,
    pub capture: bool,
    pub download: bool,
    pub raw_plus_jpeg: bool,
}

pub trait Camera: Send {
    fn info(&self) -> &DeviceInfo;             // make, model, serial, port
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
```

- Live view frames arrive on a bounded channel; stale frames are dropped.
- The camera session belongs to the project, not the scene, so switching scenes doesn't reconnect.
- Disconnect is detected within 2 seconds; capture is disabled until the camera is back.

## 8. Projects and scenes

- A **project** has a name, a frame rate and an ordered list of scenes.
- A **scene** has a stable ID, a display name and its frames.
- Exactly one scene is **active**; captures go there.
- New scenes get IDs `sc010`, `sc020`, ... so scenes can be inserted between without renumbering (`sc015`).
- Scene order lives in `project.json`, independent of IDs.
- **Rename** changes the display name only.
- **Delete scene** moves its folder to the project `trash/`.
- Frame rate is project-wide; a scene may override it.

## 9. Capture

A capture is a transaction. The UI only confirms after step 4.

1. **Pre-flight.** Camera connected, active scene set, disk space available.
2. **Trigger** capture.
3. **Download** files to `scenes/<id>/incoming/`.
4. **Commit.** Flush to disk (fsync / `FlushFileBuffers`), move into `frames/`, append to the scene journal, flush the journal.

Failures between 2 and 4 leave a `pending` entry, recovered on next launch.

**Delete last frame** moves it to the scene's `trash/`. Nothing is destroyed.

## 10. Project format

A project is a plain folder, so students can copy it to a USB stick or hand it in.

```
MyFilm/
  project.json
  scenes/
    sc010/
      scene.json
      journal.ndjson
      frames/
        000001.CR2
        000001.jpg
        000002.CR2
        000002.jpg
      trash/
    sc020/
      ...
  trash/
  exports/
```

### project.json

```json
{
  "format": "dragonslayer/1",
  "name": "My Film",
  "fps": 12,
  "scenes": ["sc010", "sc020", "sc015"],
  "active_scene": "sc020"
}
```

### scene.json

```json
{
  "id": "sc010",
  "name": "Opening",
  "fps": null
}
```

### journal.ndjson (per scene)

```json
{"t":"2026-09-26T14:02:11Z","op":"capture","frame":"000001","camera":"Canon EOS 2000D"}
{"t":"2026-09-26T14:04:02Z","op":"delete","frame":"000001"}
```

- Frame order within a scene is capture order, rebuilt from the journal.
- The camera is recorded per frame, so a scene can be shot on different cameras across sessions.
- `project.json` is written atomically (temp file, flush, rename).

## 11. Compile

- **Scope:** one scene, or the whole project.
- **Project compile** joins scenes in order with straight cuts. Empty scenes are skipped with a warning.
- **Input:** each scene's JPEGs in frame order. RAW files stay in the folder.
- **Frame rate:** project fps, or the scene override. Mixed rates are conformed to the project fps by frame duration.
- **Settings:** format, resolution (source, 4K UHD, 1080p; crop or fit).
- **Outputs:** H.264 MP4 (default, plays everywhere) and ProRes 422 MOV.
- **Engine:** ffmpeg, a build prerequisite. Concat demuxer for project compiles.
- **Naming:** `exports/<project>_<scene|all>_<timestamp>.<ext>`. Never overwrites.

## 12. UI

One window, designed so a first-time student gets going without instructions:

- **Scene list** on the left: name, frame count, last-frame thumbnail. Click to make active, drag to reorder, add, rename, delete.
- **Live view** filling the rest, with onion skin overlaid.
- **Capture:** button and spacebar.
- **Delete last:** button and Backspace.
- **Onion skin:** on/off (O), frame count 1 to 5, opacity slider.
- **Live / last frame toggle:** Tab.
- Active scene name and frame counter always visible.
- **Compile:** "This scene" or "Whole project", then format and resolution.
- **Camera status:** make/model, connected or not, and a plain-English fix when something's wrong (camera grabbed by another app, Windows driver not swapped, no live view on this model).

## 13. Reliability

- No frame is confirmed until it's flushed to disk.
- A crash loses at most the in-flight frame, recoverable from the camera card.
- Prevent system sleep during a session (`SetThreadExecutionState` on Windows, `IOPMAssertion` on macOS).
- Soak test: 1,000 automated captures across 5 scenes on the GH5, on each platform, before tagging a release.

## 14. Distribution

- Hosted on GitLab. Source only: no binaries, installers or release artefacts.
- README covers prerequisites and build steps for Windows and macOS: Rust toolchain, libgphoto2, ffmpeg, and the Zadig step on Windows.
- GitLab CI builds and runs core tests on Windows and macOS runners. CI artefacts are not published.
- Licensing: app under Apache-2.0 or GPL-3.0 (open question); libgphoto2 is LGPL and dynamically linked.

## 15. Open questions

1. UI toolkit: Qt6 or Tauri.
2. Licence: Apache-2.0 or GPL-3.0.
3. Name.

## 16. Spikes

| Spike | Pass criteria |
|---|---|
| libgphoto2 + GH5, macOS | Live view, capture and download work over 500 frames |
| libgphoto2 + GH5, Windows (Zadig/WinUSB) | Same |
| Onion skin | 3 frames overlaid on GH5 live view with no visible lag versus raw live view |
| Compile | 3 scenes, 1,000 JPEGs total, to 4K H.264 and ProRes in correct scene and frame order |
| Second make | Borrow one common student camera (e.g. entry-level Canon EOS) and repeat the capture spike |

## 17. Plan

| Phase | Scope | Done when |
|---|---|---|
| 0 | Spikes | All pass |
| 1 | Build | A 3-scene, 30-second film captured and compiled on a GH5 on Windows and macOS, built from a clean GitLab checkout by following the README |
| 2 | Students | Used by a class or group of students on their own cameras; CAMERAS.md has 10+ models reported |

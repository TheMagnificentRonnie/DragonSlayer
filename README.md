<h1 align="center">DragonSlayer</h1>

<p align="center">
  <b>Free stop-motion capture for the DSLR or mirrorless camera you already own.</b><br>
  Tether it over USB. Shoot scene by scene with onion skinning. Compile it into a video.<br>
  <i>Nothing else.</i>
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#features">Features</a> ·
  <a href="#supported-cameras">Cameras</a> ·
  <a href="#build-from-source">Build</a> ·
  <a href="#project-format">Project format</a> ·
  <a href="#contributing">Contributing</a>
</p>

---

## Why

Real cameras are already sitting in classrooms, in kids' hands and in student kit bags — but the software to *tether* them for stop-motion (Dragonframe, Stop Motion Studio Pro, iStopMotion) costs money students don't have. Phones already have free apps. Real cameras do not.

DragonSlayer is that missing piece. One camera library ([libgphoto2](http://gphoto.org)) covers hundreds of DSLRs and mirrorless bodies. One window, one keyboard, one film.

## Features

- **Live view over USB** on any camera libgphoto2 supports live view for
- **Onion skin** — full-frame ghost or edge-detected outlines of the previous frame, so you can see exactly how far you've moved between shots
- **Scenes** with independent frame rates, drag-to-reorder, rename, per-scene trash
- **Never lose a frame** — every capture is a filesystem transaction, replayed on the next launch if the app crashes mid-shot
- **RAW + JPEG** kept side by side when the camera supports it
- **Compile** to H.264 MP4 or ProRes 422 MOV, source resolution / 4K / 1080p, crop or fit
- **Prevents system sleep** during a shooting session
- **Windows driver setup built in** — bundles Zadig for the one-time WinUSB swap
- **Extensive in-app help** with Windows/macOS-aware troubleshooting for wedged cameras, driver issues and vendor-specific quirks
- **egui + wgpu** — pure Rust, cross-platform, no Electron

## Quick start

### Windows

1. **Build** (see [Build from source](#build-from-source)), or download the pre-built folder once releases are published.
2. **Plug in your camera** on USB, turn it on, set it to *PC* / *PC(Tether)* / *PTP* mode.
3. Run `bin\dragonslayer-app.cmd`.
4. If Windows is still using its own driver, click **Set up USB driver…** in the top right. The bundled Zadig walks through a one-time swap to WinUSB (nothing on the camera changes — only which Windows driver claims the USB port).
5. Click **New project…**, capture with **Space**, delete with **Backspace**, hit **H** any time for help.

### macOS

1. `brew install libgphoto2 ffmpeg pkg-config`
2. Build (see below).
3. Quit Photos and Image Capture; in Terminal: `killall ptpcamerad`.
4. Plug in your camera in PC/PTP mode, open the app.

## Keyboard

| Key | Action |
|---|---|
| **Space** | Capture the next frame into the active scene |
| **Backspace** | Move the last frame to the scene's trash folder |
| **O** | Toggle onion skin on/off |
| **Tab** | Switch between live view and the last captured frame |
| **H** | Open the in-app help |
| **Esc** | Close a modal |

## Supported cameras

DragonSlayer uses libgphoto2's PTP driver, which supports **hundreds** of cameras — most Canon EOS, Nikon, Sony Alpha, Panasonic Lumix, Fujifilm and OM System/Olympus bodies made in the last 15 years.

- **Reference camera:** Panasonic Lumix GH5 — tested end to end on Windows.
- **Second test camera:** Canon EOS 100D.
- **Everything else:** should work; please open an issue if it does or doesn't.

Full list: <http://www.gphoto.org/proj/libgphoto2/support.php>

If your camera connects but doesn't support live view over USB, DragonSlayer falls back to showing the last captured frame — onion skin still works, capture still works.

## Build from source

You need Rust 1.95 or newer ([rustup.rs](https://rustup.rs)) and ffmpeg on your PATH.

### With mock camera only (no libgphoto2 needed)

```sh
cargo build --release
cargo run --release -p dragonslayer-app -- --mock
```

The mock camera renders a moving square with live view and simulates RAW+JPEG so you can try the whole pipeline without hardware.

### With real cameras — Windows (MSYS2 UCRT64)

Install MSYS2 (msys2.org), then in its **UCRT64** shell:

```sh
pacman -S mingw-w64-ucrt-x86_64-{toolchain,pkgconf,libgphoto2,clang}
rustup target add x86_64-pc-windows-gnu

export PATH="/c/msys64/ucrt64/bin:$PATH"
export PKG_CONFIG_PATH="/c/msys64/ucrt64/lib/pkgconfig"
export PKG_CONFIG_ALLOW_CROSS=1
export LIBCLANG_PATH="/c/msys64/ucrt64/bin"

cargo build --release --features gphoto2 \
  -p dragonslayer-cli -p dragonslayer-app \
  --target x86_64-pc-windows-gnu
```

Then run `bin\dragonslayer-app.cmd` from a regular Command Prompt.

### With real cameras — macOS

```sh
brew install libgphoto2 ffmpeg pkg-config
cargo build --release --features gphoto2 -p dragonslayer-cli -p dragonslayer-app
```

## Command line

The CLI has the same operations as the app — useful for testing, scripting or headless captures.

```sh
dragonslayer new MyFilm --name "My Film"       # create a project
dragonslayer scene add MyFilm "Chase"          # add scene
dragonslayer scene list MyFilm                 # list scenes
dragonslayer cameras                           # what libgphoto2 sees
dragonslayer capture MyFilm sc010 --count 12   # shoot 12 frames
dragonslayer delete-last MyFilm sc010          # undo last (to trash)
dragonslayer compile MyFilm --resolution 1080p # render project
dragonslayer compile MyFilm sc010 --format prores --resolution 4k --crop
```

Scenes are addressed by ID (`sc010`) or display name. Videos land in `<project>/exports/` and are never overwritten.

## Project format

A project is a plain folder. Copy it to a USB stick or hand it in.

```
MyFilm/
  project.json                  # name, fps, scene order, active scene
  scenes/
    sc010/
      scene.json                # display name, optional fps override
      journal.ndjson            # capture/delete history (frame order rebuilt from this)
      frames/
        000001.jpg
        000001.CR2
      incoming/                 # in-flight downloads; recovered next launch after a crash
      trash/                    # deleted frames (never destroyed)
  trash/                        # deleted scenes
  exports/                      # compiled videos
```

- **Frame order** is capture order, rebuilt from `journal.ndjson`.
- **Frame numbers are never reused.** Deleting frame 5 doesn't free up "5" for the next shot — the next capture becomes 6.
- **New scenes** get IDs `sc010`, `sc020`, `sc030`... so you can insert between two scenes with `sc015` without renumbering.
- **project.json** is written atomically (temp file → flush → rename), so a crash never leaves a half-written project file.

## Reliability

- **Every capture is a transaction.** File flushed to disk (`fsync` / `FlushFileBuffers`), moved from `incoming/` into `frames/`, then appended to the scene journal. Failures leave a `pending` entry, recovered on next launch.
- **A crash loses at most the in-flight frame,** and that one is recoverable from the camera card.
- **Camera cleanup** on shutdown calls `gp_camera_exit` so the PTP session doesn't wedge on the camera side. If you *do* end up with a wedged camera (usually from a crash of some earlier session), turn the camera off and back on — nothing is broken, just PTP session state.

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  UI (egui + wgpu)                                            │
│  scene list │ live view + onion skin │ capture │ compile     │
└─────────────────────────────┬────────────────────────────────┘
                              │
┌─────────────────────────────▼────────────────────────────────┐
│  dragonslayer-core (Rust)                                    │
│  project + scenes │ camera session │ frame store │ compile   │
└─────────────────────────────┬────────────────────────────────┘
                              │
                       libgphoto2 (FFI)
```

- **dragonslayer-core** — project model, scene/journal, capture transaction, compile orchestration, no camera or UI code.
- **dragonslayer-camera** — a thin `Camera` trait with two backends: a mock camera (always available, used for testing) and libgphoto2 (behind the `gphoto2` feature).
- **dragonslayer-cli** — every operation the app has, from the terminal.
- **dragonslayer-app** — egui/wgpu window: scene list, live view compositor with onion skinning, capture/delete/compile UI, in-app help modal, camera status.

The camera trait is designed so a future WPD (Windows Portable Devices) backend can slot in without touching the app — see `dragonslayer-spec.md` for the shape of that work.

## Contributing

- **Camera reports** — see [`CAMERAS.md`](CAMERAS.md) for what's tested. Open an issue with your camera model, OS, and what worked / didn't. This is the most useful contribution.
- **Bug reports** — a screenshot and the last few lines of `%TEMP%\dragonslayer.log` (Windows) or `~/Library/Logs/dragonslayer.log` (macOS) usually pinpoint an issue quickly.
- **Code** — the [`dragonslayer-spec.md`](dragonslayer-spec.md) is the design document. Small PRs welcome. Pass `cargo test --workspace` and `cargo clippy --workspace --all-targets`.

## Licence

To be finalised — either Apache-2.0 or GPL-3.0. libgphoto2 is LGPL-2.1+ and dynamically linked.

## Credits

- [libgphoto2](http://gphoto.org) — the camera library that makes any of this possible.
- [egui](https://github.com/emilk/egui) and [wgpu](https://github.com/gfx-rs/wgpu) — the UI stack.
- [Zadig](https://zadig.akeo.ie) / [libwdi](https://github.com/pbatard/libwdi) — bundled for the Windows WinUSB one-time driver swap.
- [ffmpeg](https://ffmpeg.org) — the video compile engine.
- Dragonframe, whose approach to onion skinning and scene management set the bar this project aims at.

# Manual test plan

For running through DragonSlayer end to end, with or without a real camera.

**On Windows, the primary transport is WPD** — the camera stays on the stock Windows
driver, no Zadig, no driver swap. Zadig + WinUSB is only a fallback for cameras that
WPD can't handle (§7).

Do §0–§5 with a clean checkout. §2, §6 and §7 need a real camera. §2 also needs the
`libgphoto2-wpd` fork built (see below); until it is, §6 falls back to §7 on Windows.

Each test says what to do, what to expect, and where to look if it fails. Tick the
box when it passes; if it fails, note what happened before moving on — later tests
usually depend on earlier ones.

## 0. Prerequisites

- [ ] **0.1  Rust.** `rustc --version` ≥ 1.95.0. If not: `rustup update stable`.
- [ ] **0.2  ffmpeg.** `ffmpeg -version` shows a version, line mentions `libx264` and `prores`.
      Windows: `winget install Gyan.FFmpeg`. macOS: `brew install ffmpeg`.
- [ ] **0.3  Real camera on Windows (primary path).** MSYS2 (msys2.org). In the
      **UCRT64** shell:
      `pacman -S mingw-w64-ucrt-x86_64-{toolchain,pkgconf,libtool,gettext,libxml2,libexif}`
      Rust GNU target: `rustup target add x86_64-pc-windows-gnu`.
      Then build the fork (see 1.4). No Zadig on this path.
- [ ] **0.4  Real camera on Windows (fallback path, Zadig).** Nothing to install now;
      the swap happens per camera in §7.
- [ ] **0.5  Real camera on macOS.** `brew install libgphoto2 pkg-config`.
- [ ] **0.6  Visual Studio Build Tools with the C++ x64 workload.** Only needed for the
      WPD spike (§2). Check with:
      `"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`
      → prints an install path.

## 1. Build

- [ ] **1.1  DragonSlayer builds clean.** `cargo build --release -p dragonslayer-cli -p dragonslayer-app`
      → `Finished \`release\``, no `warning:` or `error:`.
- [ ] **1.2  Tests pass.** `cargo test --workspace` → `test result: ok. 10 passed`.
- [ ] **1.3  Clippy clean.** `cargo clippy --workspace --all-targets` → no warnings.
- [ ] **1.4  libgphoto2-wpd fork builds (Windows, UCRT64 shell).**
      In `G:\libgphoto2-wpd`, follow its README to run `autoreconf && ./configure
      --enable-wpd --disable-ptpip --disable-serial --without-libexif --without-libxml-2.0`,
      then `make`. Expected: `libgphoto2.dll` and the `wpd` iolib plus `ptp2` camlib
      end up in the install prefix. `gphoto2.exe` also builds.
- [ ] **1.5  gphoto2 CLI works.** In the same shell:
      `gphoto2 --version` prints the fork's version. `gphoto2 --list-ports` lists the
      built-in ports including `wpd`.
- [ ] **1.6  DragonSlayer with real-camera support builds.**
      macOS: `cargo build --release --features gphoto2 -p dragonslayer-cli -p dragonslayer-app`.
      Windows (UCRT64 shell, after 1.4):
      `PKG_CONFIG_PATH=<fork prefix>/lib/pkgconfig cargo build --release --features gphoto2 -p dragonslayer-cli -p dragonslayer-app --target x86_64-pc-windows-gnu`
      → `Finished`.

## 2. WPD transport check (Windows, per camera model)

Runs the WPD low-level spike so we know the Windows driver passes what ptp2 needs
*before* going through DragonSlayer. Do this once per new model, then again after any
Windows update that touches the camera driver.

Camera must be on the stock Windows driver. If Zadig was ever applied, undo it first:
Device Manager → the camera → **Uninstall device**, tick "delete driver", unplug,
replug.

- [ ] **2.1  Spike builds.** In `cmd.exe`, `cd G:\libgphoto2-wpd\spikes\wpd-raw-ptp` and
      run `build.bat`. Expected: exit 0, `wpdptp.exe` is present.
- [ ] **2.2  List devices.** `wpdptp --list`.
      No camera: `No WPD devices found…`.
      With the camera on and connected in PC/PTP mode: one entry with make, model, and
      a `usb#vid_xxxx&pid_xxxx#…` device ID.
- [ ] **2.3  GetDeviceInfo (PTP 0x1001).** `wpdptp --device 0`.
      Prints response code `0x2001` (RC_OK) and the DeviceInfo dataset: standard
      version, vendor extension id and version, manufacturer, model, serial. The
      manufacturer/model must match the camera.
- [ ] **2.4  Vendor opcodes.** `wpdptp --device 0 --opcodes`.
      Prints the list the Windows driver will let through. Cross-check against
      `libgphoto2/camlibs/ptp2/ptp.h`:
      - Canon EOS: at minimum `0x9114 SetRemoteMode`, `0x9115 SetEventMode`,
        `0x9116 GetEvent`, `0x910F RemoteReleaseOn`/`0x9110 RemoteReleaseOff`,
        `0x9153 GetViewFinderData`.
      - Panasonic: `0x9401`–`0x9410` range (SetProperty, GetProperty, Liveview*,
        InitiateCapture).
      Any missing opcode is a red flag — record it and try anyway in 2.5.
- [ ] **2.5  Canon EOS only, PC-remote round trip.**
      `wpdptp --device 0 --no-data 0x9114 1` (SetRemoteMode PC)
      `wpdptp --device 0 --no-data 0x9115 1` (SetEventMode)
      `wpdptp --device 0 --read 0x9116` (GetEvent)
      Each returns `0x2001`. The GetEvent hex dump is a valid event dataset.
- [ ] **2.6  Record the result.** Append the outcome to `CAMERAS-WINDOWS.md`: model,
      driver version (Device Manager → Details → Driver Version), which of 2.2–2.5
      passed, any missing opcode.
- [ ] **2.7  Unplug during a transaction.** In one shell, run `wpdptp --device 0` on
      loop (`for /L %i in (1,1,1000) do wpdptp --device 0 >nul`). Unplug the cable.
      The current call returns a device-not-connected error within ~2 s; the loop
      does not hang. Plug back in.

## 3. CLI with the mock camera

Substitute your own path for `~/DragonSlayerTest` below.

- [ ] **3.1  New project.** `dragonslayer new ~/DragonSlayerTest/Film1 --name "Test 1"`.
      Expected: `Created "Test 1"`. Folder has `project.json`, `scenes/sc010/`,
      `exports/`, `trash/`. `project.json`: `format: "dragonslayer/1"`, `fps: 12`,
      `scenes: ["sc010"]`, `active_scene: "sc010"`.
- [ ] **3.2  Scene commands.** In order:
      `dragonslayer scene add ~/DragonSlayerTest/Film1 "Chase"` → sc020.
      `dragonslayer scene add ~/DragonSlayerTest/Film1 "Middle" --after sc010` → sc015.
      `dragonslayer scene move ~/DragonSlayerTest/Film1 Chase 1`
      `dragonslayer scene rename ~/DragonSlayerTest/Film1 sc010 "Opening"`
      `dragonslayer scene fps ~/DragonSlayerTest/Film1 Chase 6`
      `dragonslayer scene list ~/DragonSlayerTest/Film1`
      Expected list, `*` on Middle:
      `  1. sc020  Chase   (0 frames, 6 fps)`
      `  2. sc010  Opening (0 frames)`
      `* 3. sc015  Middle  (0 frames)`
- [ ] **3.3  Detect mock camera.** `dragonslayer --mock cameras`.
      Expected: `DragonSlayer Mock Camera  [mock:0]`, all three of live view / capture /
      download `yes`.
- [ ] **3.4  Capture by scene name.** `dragonslayer --mock capture ~/DragonSlayerTest/Film1 Opening --count 5`.
      Expected: `sc010: frame 000001 (2 files)` through `000005`. On disk:
      `scenes/sc010/frames/000001..000005.jpg` and `.RAW`. `journal.ndjson` has 5
      alternating `pending`/`capture` lines; no `abandon`.
- [ ] **3.5  Delete last.** `dragonslayer delete-last ~/DragonSlayerTest/Film1 Opening`.
      Expected: `Moved frame 000005 ... (4 left)`. Files in `scenes/sc010/trash/`. Next
      capture becomes `000006`, not `000005`.
- [ ] **3.6  Compile whole project.**
      `dragonslayer --mock capture ~/DragonSlayerTest/Film1 Chase --count 3`, then
      `dragonslayer compile ~/DragonSlayerTest/Film1`.
      Expected: `Wrote ...Test_1_all_YYYYMMDD-HHMMSS.mp4 (7 frames)` and a warning that
      Middle is empty. ffprobe: `h264`, `12/1`, `nb_frames=7`, `duration≈0.833`.
- [ ] **3.7  Compile one scene, ProRes 4K.**
      `dragonslayer compile ~/DragonSlayerTest/Film1 Chase --format prores --resolution 4k --crop`.
      ffprobe: `prores`, `3840x2160`, `nb_frames=3`, `duration≈0.5`.
- [ ] **3.8  No overwrite.** Run 3.6 again straight away. A new `.mp4` appears next to
      the first (never overwrites; the file name has `-2` or the next second's
      timestamp).

## 4. App with the mock camera

Launch: `dragonslayer-app --mock ~/DragonSlayerTest/Film1`.

- [ ] **4.1  Opens.** Window ≥ 1280×800, title "DragonSlayer". Top-right shows
      `● DragonSlayer Mock Camera · connected` (green dot). Scene list on the left has
      Chase / Opening / Middle with thumbnails and frame counts. Viewer shows the mock
      live view (orange square drifting) with `LIVE` top-right.
- [ ] **4.2  Onion skin.** Click **Opening**. Live view has one earlier frame ghosted
      over it. Slide "frames" to 3, then 5: more ghosts appear, older ones fainter.
      Slide "opacity" down: they fade further. Press **O**: onion skin off; **O** again:
      back on.
- [ ] **4.3  Tab toggles live / last frame.** Press **Tab**: viewer freezes on
      `LAST FRAME 000004`; **Tab** again: back to live.
- [ ] **4.4  Space captures.** Press **Space** five times. Each time bottom-left says
      `Frame 00000N saved`, Opening's thumbnail updates, count rises. Files land in
      `scenes/sc010/frames/000006..000010`.
- [ ] **4.5  Backspace deletes.** Press **Backspace** twice. Confirmations bottom-left;
      count drops by two; files in `scenes/sc010/trash/`.
- [ ] **4.6  Scene edits.** Click **+ Add scene**: "Scene 4" appears, is active.
      Double-click it, type "Outro", Enter. Drag its row up: drop marker follows the
      mouse, release moves it. Right-click → **Delete scene**: it leaves the list and
      `trash/sc040/` exists in the project folder.
- [ ] **4.7  Per-scene fps.** Right-click Chase → **Frame rate → 8 fps**. Row shows
      `... 8 fps`. Right-click → **Same as project**: fps disappears.
- [ ] **4.8  Compile dialog.** Click **Compile…**. Whole project, MP4, 1080p, Fit,
      **Compile**. Spinner runs, then the dialog shows the output path and frame
      count. **Show in folder** opens a file window with the new .mp4 selected.
- [ ] **4.9  Camera-lost message.** Quit. Restart without `--mock`:
      `dragonslayer-app ~/DragonSlayerTest/Film1`. Dot is grey or amber; the message explains
      ("No camera support in this build" if built without `gphoto2`, otherwise "No
      camera" plus a plug-in hint). Capture button disabled. Quit.

## 5. Crash recovery

- [ ] **5.1  Interrupted download.** `dragonslayer-app --mock ~/DragonSlayerTest/Film1`, make
      Opening active, press Space, close the window abruptly (taskkill / Alt-F4 in the
      middle of the capture). Restart with the same command. Either a one-line
      `Recovered N…` message appears (if timing caught one mid-flight) or nothing.
      No orphan folders under `scenes/*/incoming/`; no `pending` entries left in any
      `journal.ndjson`.
- [ ] **5.2  Torn journal line.** Quit. In `scenes/sc010/journal.ndjson`, append an
      incomplete line (no `}`, no trailing newline) with a text editor. Restart the
      app on the same project. Scene opens fine, frame count unchanged, the next
      capture appends cleanly and is readable.

## 6. Real camera on Windows via WPD (primary)

Needs §1.4–1.6 built and §2 passed for this camera. Camera must be on the stock
Windows driver.

- [ ] **6.1  Free the camera.** Disable AutoPlay for the camera (Settings → Bluetooth &
      devices → AutoPlay). Close the Photos app. Dismiss any camera-app notifications.
- [ ] **6.2  DragonSlayer sees it.** `dragonslayer cameras` in the UCRT64 shell.
      Expected: one line with make/model, port starts with `wpd:` (not `usb:`),
      capture / download both `yes`. If port is `usb:` instead, libgphoto2 fell back to
      libusb; the fork isn't in use — check `PKG_CONFIG_PATH` and rebuild.
- [ ] **6.3  Capture from the CLI.** Create a project with `dragonslayer new`, then
      `dragonslayer capture ~/DragonSlayerTest/RealFilm Opening --count 3`.
      Expected: three lines `sc010: frame 00000N (2 files)`. The JPEG opens and looks
      right. With RAW+JPEG on, both files present; without, one stderr warning and
      the JPEG only.
- [ ] **6.4  App launches with live view.** `dragonslayer-app ~/DragonSlayerTest/RealFilm`. Top-
      right dot green with the camera name. Live-view feed updates smoothly at the
      camera's own rate.
- [ ] **6.5  Camera without live view.** If the model reports `live view: no`, the
      status shows the short explanation and the viewer shows the last captured frame.
      Onion skin still works.
- [ ] **6.6  Onion skin over live view.** With one frame captured, press **O** off and
      on while moving the camera: the ghost stays put while the live feed moves.
- [ ] **6.7  Space captures the real camera.** Press **Space** ~30 times, making small
      changes between shots. Frame counter, thumbnails and files on disk keep up.
      **Backspace**: last frame → `trash/`, count drops by one.
- [ ] **6.8  Disconnect handling.** Unplug during idle. Within ~2 s the status turns
      red with "Camera disconnected. Reconnect it…" and Capture is disabled. Plug back
      in; within ~2 s status returns to connected; Capture works again; live view
      recovers if it was on.
- [ ] **6.9  Vendor-tool conflict.** With DragonSlayer connected, open the Windows Photos
      app pointed at the camera. Expected: DragonSlayer either notices and shows the
      "camera is in use by another program" message, or Photos fails to open. Close
      Photos; DragonSlayer recovers.
- [ ] **6.10 Sleep prevention.** Set Windows sleep to 2 min. Launch the app with a
      project open, don't touch keyboard or mouse for 3 min. Screen and machine stay
      awake. Quit / close the project: normal sleep behaviour returns.
- [ ] **6.10a Camera settings.** Mode dial on **M**. `dragonslayer settings` lists
      aperture, shutter, ISO, white balance and image format with the values shown on
      the camera. `dragonslayer settings iso 400` changes the camera's ISO (check the
      camera's own screen). In the app, the **Exposure** tab shows the same values; changing
      shutter visibly brightens/darkens live view, and the next capture uses it. Switch
      the dial to **P**/**A**: some settings become read-only (greyed out) rather than erroring.
      Change settings ~20 times with live view running: no wedge. Record any setting that
      is missing for this model and the name `gphoto2 --list-config` uses for it.
- [ ] **6.11 Transaction record.** With the debug build option on
      (`--features gphoto2,ptp-record`, once implemented), run 6.3 with the record file
      set. A `<camera>.ptprec` file is written; replay it in CI without hardware and
      the recorded response codes match.

## 7. Fallback: Zadig + WinUSB

For a camera where §2 or §6 fails (WPD or the Windows driver won't pass a required
opcode). Use only when needed; each camera swapped is invisible to Photos and vendor
tools until unswapped.

- [ ] **7.1  Zadig.** Follow the README "Windows: Zadig" section: List All Devices →
      the camera → WinUSB → Replace Driver. Verify in Device Manager the camera shows
      under **Universal Serial Bus devices**.
- [ ] **7.2** Repeat 6.2–6.10; expect the port in 6.2 to start with `usb:` instead of
      `wpd:`. §6.9 no longer applies (Photos can't see the camera while WinUSB owns it).
- [ ] **7.3  Undo.** Device Manager → the camera under USB devices → **Uninstall
      device**, tick "delete driver software", unplug, replug. It comes back as the
      normal camera.

## 8. Real camera on macOS

- [ ] **8.1  Free the camera.** Quit Image Capture and Photos. `killall ptpcamerad`. Plug in.
- [ ] **8.2** Repeat 6.2–6.10 (no Zadig; port starts with `usb:`). For 6.9, test that
      opening Image Capture triggers our "in use" message; then close it and recover.
- [ ] **8.3  Packaged app is self-contained.** `scripts/package-macos.sh`. Then, with
      Homebrew hidden from it:
      ```
      R=$PWD/target/dist/DragonSlayer.app/Contents
      env -i HOME=$HOME PATH=$R/Resources/bin:/usr/bin:/bin \
        CAMLIBS=$R/Resources/libgphoto2/camlibs IOLIBS=$R/Resources/libgphoto2/iolibs \
        DYLD_PRINT_LIBRARIES=1 $R/MacOS/dragonslayer-cli cameras 2>&1 | grep -c /opt/homebrew
      ```
      Expected: `0`, and the camera is listed. Repeat with `capture` and `compile` into a
      scratch project: frames land, MP4 is written.
- [ ] **8.4  Packaged app from Finder.** Double-click `target/dist/DragonSlayer.app`: the
      camera connects, and **Export** writes an MP4 (proves the bundled ffmpeg is found
      without Terminal's PATH). `~/Library/Logs/dragonslayer.log` has a "starting" line.

## 9. Soak test (before tagging a release)

- [ ] **9.1  1,000 captures across 5 scenes.** With a project of 5 scenes:
      ```
      for i in $(seq 1 200); do
        for s in sc010 sc020 sc030 sc040 sc050; do
          dragonslayer capture ~/DragonSlayerTest/Soak $s --count 1 --interval 1 || exit 1
        done
      done
      ```
      Expected: exit 0. Total frame count 1,000, 200 per scene. No `abandon` lines in
      any journal. No files left in any `incoming/`.
      `dragonslayer compile ~/DragonSlayerTest/Soak` produces a valid ~83 s MP4 at 12 fps.
- [ ] **9.2  Do 9.1 on both Windows (via WPD) and macOS.**

---

## What "pass" means before we tag a release

- §0–§5 all ticked on both Windows and macOS.
- §2 and §6 ticked on Windows with the GH5 (reference camera). At least §2.2–§2.4 and
  §6.2–§6.3 also ticked with the Canon EOS 100D (second-make spike, spec §16).
- §7 (Zadig fallback) not required for release, but any camera that needs it is
  recorded in `CAMERAS-WINDOWS.md`.
- §8 ticked on macOS with the GH5.
- §9 ticked on both platforms.

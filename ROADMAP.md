# DragonSlayer — Roadmap to Dragonframe parity (and beyond)

A living document of what DragonSlayer has today, what it needs to reach professional stop-motion territory (Dragonframe, Stop Motion Studio Pro, iStopMotion) and where we could go further.

Items are grouped by area and marked with:

- ✅ Done
- 🚧 Partial / in progress
- 🎯 Blocker for pro parity — targeted for the next couple of releases
- ⭐ Competitive polish — makes it feel professional
- 🌱 Stretch — nice future work

Each item has a rough complexity rating: **S** (a day), **M** (a week), **L** (a month), **XL** (multi-month or ecosystem-blocked).

---

## Where we are today (0.3.0-beta)

- ✅ Live view over USB via libgphoto2 (hundreds of DSLR / mirrorless bodies)
- ✅ Project + scene model, drag-to-reorder, rename, per-scene fps, trash
- ✅ Capture as a filesystem transaction, crash-recovery on next launch
- ✅ Onion skin: full-frame ghost or Sobel edge outlines
- ✅ Compile to H.264 MP4 or ProRes 422 MOV, source/4K/1080p, crop/fit, per-compile fps override
- ✅ Windows: bundled Zadig for one-time WinUSB driver setup
- ✅ Camera cleanup on shutdown (calls `gp_camera_exit` — no more wedged cameras)
- ✅ Extensive in-app help with Windows/macOS troubleshooting
- ✅ Sleep prevention during session
- ✅ Mock camera for demos and CI
- ✅ MIT-licensed, open source, portable Windows bundle
- ✅ Two modes: Capture (live view + capture) and Preview (review only), Tab to switch *(0.2.0)*
- ✅ Filmstrip timeline with clickable thumbnails; arrow / Home / End / Shift-arrow navigation; play at scene fps *(0.2.0)*
- ✅ Picture-in-picture of the other view *(0.2.0)*
- ✅ Dockable panels (egui_dock), menu bar, Phosphor icons, three themes *(0.2.0)*
- ✅ Self-contained macOS app (Apple Silicon): libgphoto2 + camera drivers + ffmpeg bundled, `scripts/package-macos.sh` *(0.2.1)*
- ✅ Interval capture: N frames every S seconds from the Camera panel, stoppable, stops cleanly on camera loss *(0.3.0)*
- ✅ Camera settings from the app: aperture, shutter, ISO, white balance, image format (Exposure panel, `dragonslayer settings`) *(0.3.0)*
- ✅ Compile progress bar with time remaining *(0.3.0)*
- ✅ Camera diagnosis: USB driver per port, hubs, camera answers, live view; spots a port that needs Zadig automatically; `dragonslayer diagnose` *(0.3.0)*
- ✅ Advanced camera troubleshooting tab in help *(0.3.0)*
- ✅ Minimal view (F) and fill-to-crop viewer *(0.3.0)*
- ✅ Canon EOS 100D thoroughly tested on Windows *(0.3.0)*
- ✅ GitHub Actions builds Windows + macOS on every push; releases on demand *(0.3.0)*
- ✅ Import images / rescue a film from the camera card into a scene, safe on damaged cards, re-runnable (`dragonslayer import`) *(unreleased)*

---

## 🎯 Phase 1 — Core professional stop-motion parity

The essentials that every serious stop-motion tool has. Getting these done takes us from "usable prototype" to "would a real animator use this for a real short film?".

### Playback and timeline (M, blocker)

- ✅ **Timeline strip** along the bottom of the viewer: thumbnails of every frame in the active scene, clickable to scrub. This is *the* most-used feature in Dragonframe and its absence is the single biggest UX gap.
- 🚧 **Playback with variable speed** — play the scene at project fps, half speed, quarter speed, in reverse, loop. Space to play/pause, arrow keys to step. (Done: P plays at scene fps. Still to do: half/quarter speed, reverse, loop.)
- 🎯 **Loop range** — select a start/end frame on the timeline and loop just that segment.
- ✅ **Frame-by-frame scrub** with left/right arrows, holding for autoscrub.
- 🎯 **Playback while capturing** — one thread captures, another loops last N frames for review between shots.

### Takes and versions (S)

- 🎯 **Multiple takes per shot** — capture into a "take" that lives alongside the main sequence. When a movement doesn't work, start a new take instead of deleting. Switch which take is active for compile.

### Onion skin polish (S-M)

- 🎯 **Difference / motion mask** mode — subtract live view from previous frame, highlight where things moved. This is the killer onion-skin variant for tricky animation.
- 🎯 **Per-frame onion offsets** — separate opacity for previous frames vs. next frames (for scenes shot out of order).
- 🎯 **Onion of a specific frame** ("keyframe onion") — pin an earlier frame as reference, always overlaid regardless of position.
- 🎯 **Independent tint per layer** — traditional Dragonframe look is red-tinted past frames, green-tinted next frames.

### Reference (M)

- 🎯 **Import reference video** — drop a video file, scrub it alongside the current frame to plan action.
- 🎯 **Reference image** — pin a still (concept art, character turn-around) as an overlay with adjustable opacity.
- 🎯 **Compare shot** — flip between two captured frames (e.g. beginning and end of a movement) to check pose consistency.

### Camera settings (M)

- ✅ **Aperture, shutter, ISO, WB from the app** — Exposure panel and `dragonslayer settings` CLI, plus image format (to switch on RAW+JPEG). Read on connect and after each change, never polled. Verified on the Canon 100D; the GH5 still needs checking (config names differ per driver).
- 🎯 **Focus assist** — magnify a region of live view; edge peaking overlay.
- 🎯 **Focus stacking** — capture N frames at stepped focus positions, compile with Helicon Focus / focus-stack.
- 🎯 **Exposure bracketing** — three-shot bracket per frame for later HDR merge.

### Compile and export (S-M)

- 🎯 **Preview compile** — quick low-res proxy MP4 without leaving the app.
- 🎯 **Export EDL / XML** for round-tripping to Premiere / Resolve / Final Cut.
- 🎯 **Contact sheet PDF** — print all frames of a scene for physical annotation.
- 🎯 **Frame range export** — compile just frames 40–120, not the whole scene.

---

## ⭐ Phase 2 — Competitive polish

What makes DragonSlayer feel like an intentional product rather than a hobby prototype.

### UI overhaul (M-L)

0.2.0 replaced the egui defaults with a themed, dockable layout. What's left:

- ✅ **Custom theme** — proper dark mode designed for a shooting environment (mostly-black, warm accents, no eye-strain whites). Egui supports theming; we need a cohesive palette.
- ⭐ **Typography** — bundle a good UI font (Inter, JetBrains Mono for code, or the SF Pro system font on macOS). Set consistent sizes.
- ✅ **Icons** — replace text-only buttons with icons + labels. Use a bundled icon font (Lucide, Material Symbols) or SVG sprites.
- ⭐ **Proper spacing and elevation** — panel dividers with drag handles, cards with subtle shadows, consistent padding scale.
- ⭐ **Viewer treatment** — larger, framed, with corner overlays for frame count / scene name / mode indicator (LIVE / LAST / take name).
- 🚧 **Timeline design** — Dragonframe-style horizontal filmstrip with playhead, in/out markers, loop region.
- ⭐ **Empty states** — the welcome screen is currently just two buttons on a blank canvas. Illustrated "start here" flow: create project, connect camera, capture first frame.
- ⭐ **Status states** — the current amber/red dot is fine but the info hierarchy needs work. Persistent camera info panel with battery, storage, exposure readout.
- ✅ **Focus mode** *(as "Minimal view")* — F key hides all panels, viewer goes full-screen with just a minimal control bar. For actual shooting.
- ⭐ **Animated transitions** — panel show/hide, modal open/close, capture flash. Nothing gratuitous, just enough to feel responsive.
- ⭐ **Hover / focus / active states** — currently a bit flat. Every clickable thing needs three visual states.

### Audio (M)

- ⭐ **Reference audio** — import a WAV / MP3 (dialogue track, music), see its waveform on the timeline, scrub in sync with frames. Essential for lip-sync animation.
- ⭐ **Per-frame audio notes** — record a voice memo per frame with the Mac/PC mic. Useful for "come back and fix this" annotations.
- ⭐ **Sound-timed captures** — space bar down triggers next capture on the next audio waveform peak. Niche but Dragonframe has it.

### Motion control (L)

- ⭐ **DMX output** — for controlled lighting between shots (e.g. cycle through a set of light positions each frame for later 3D relighting).
- ⭐ **Arduino / serial motion control** — plug in a stepper controller, program a move (dolly, pan, rack focus) and have DragonSlayer step the motion between each capture. This is the Dragonframe DMC feature and is huge for studios doing puppet work.
- ⭐ **Motion control preview** — run the whole move without capture to verify it before commit.

### Multi-angle capture (L) — backlog

One scene, two (or more) cameras: one Space press = one frame from every angle. Stop-motion doesn't need true sync (nothing moves between shots), so cameras can fire sequentially; capture time is the slowest camera's.

- ⭐ **Data model** — a frame is a *moment* with files per angle: `frames/A/000001.jpg`, `frames/B/000001.jpg`. One journal and frame numbering per scene, so angles stay in lockstep. Each angle folder imports directly into an NLE as an image sequence. Delete-last removes the moment from all angles. Format bump to `dragonslayer/2`; existing projects migrate as angle A.
- ⭐ **Partial frames** — if angle A succeeds and B fails, commit A, mark B missing, offer "reshoot angle B for frame N". Never discard a good frame. Recovery gets per-angle pending folders.
- ⭐ **Camera identity by serial number** — USB ports change on replug. Bind angles to serials; fall back to port with a warning when a camera doesn't report one (matters most with two identical bodies).
- ⭐ **One session worker per camera**, each with its own libgphoto2 context. Windows needs Zadig per camera.
- ⭐ **Live view from the viewed angle only** — two streams doubles USB traffic and wedge risk (Panasonic) plus sensor heat. The other angle idles until capture.
- ⭐ **UI** — 1/2 (or Tab) switches angle in the viewer; PiP shows the other angle; onion skin per angle; Preview keeps the frame index across angle switches; per-camera status in the status bar (`A ● GH5  B ● 100D`). Interval capture waits for all angles.
- ⭐ **Compile one video per angle** (`sc010_A.mp4`, `sc010_B.mp4`), identical frame count and timing so they line up in an editor. No auto-cutting between angles — that's editing.
- **Build order:** multi-device mock + core tests → format v2 + migration → multi-worker session with serial assignment → UI → real GH5 + 100D test.
- **Open questions:** 2 angles or N? Partial frames kept-and-marked vs all-or-nothing? One live feed at a time acceptable?

### Guides and overlays (S)

- ⭐ **Rule of thirds / centred grid** overlay on live view.
- ⭐ **Custom safe-area / crop-marker masks** — arbitrary shapes (letterbox, phone portrait, Instagram square).
- ⭐ **Line-up mode** — semi-transparent overlay of a specific frame in a specific colour to align live view against.
- ⭐ **Chroma key preview** — real-time green-screen matte on live view, with adjustable threshold, so you can pose against a background before the render.

### Metadata and organisation (S-M)

- ⭐ **Per-frame notes and flags** — mark a frame as "reshoot", "hero", "test". Comment on it. Filter the timeline by flag.
- ⭐ **Shot / scene naming conventions** — enforce or suggest a naming scheme (SC010_SH020_TK03).
- ⭐ **Session summary** — end-of-day report: frames captured, scenes touched, total shooting time, storage used.

### Preview quality (S)

- ⭐ **Live view resolution selection** — some cameras offer larger preview streams for a small performance hit; expose it.
- ⭐ **Live view crop / zoom** — for detailed inspection without moving the camera.

---

## 🌱 Phase 3 — Beyond Dragonframe

Where we could genuinely differentiate.

### Phone / tablet as capture device (L)

**The user asked for this specifically.** Three architecturally distinct options:

- 🌱 **iOS + macOS: Continuity Camera** — Apple's built-in iPhone-as-webcam feature exposes the phone as a standard AVFoundation camera. On macOS we could add an AVFoundation backend alongside libgphoto2. Zero setup for users on modern iOS/macOS. **Complexity: M.**
- 🌱 **Android + USB (ADB + Camera2)** — Android phones can expose their camera over USB via the Camera2 API through an on-device companion app + ADB port-forward, or via Android's UVC webcam mode (Android 14+). **Complexity: L** (needs a companion app on the phone).
- 🌱 **WiFi/BLE companion app** — a small phone app (Flutter or React Native) that runs a WebSocket server; DragonSlayer connects and treats the phone as a camera. Works on any OS. Captures are sent over WiFi as JPEG. **Complexity: XL** (whole separate app to build and maintain).

Recommendation: start with **Continuity Camera on macOS** — free, uses existing OS integration, no separate app to ship. Then Android USB Camera2 next.

### Cloud collaboration (XL)

- 🌱 **Cloud sync of projects** — projects live in a folder synced to iCloud/OneDrive/Dropbox — this actually works today with the plain-folder format. But we could offer explicit "publish to review" with a web viewer.
- 🌱 **Web review viewer** — client / director watches your latest scene in a browser without installing anything. Frame-by-frame comments.
- 🌱 **Multi-user editing lock** — if two people open the same scene, second gets read-only + notification.

### Windows: no-Zadig capture (L)

- 🌱 **libgphoto2-wpd** — fork libgphoto2, add a Windows Portable Devices transport so cameras work through the stock Windows driver. Spike started in `../libgphoto2-wpd` repo. Would remove the biggest Windows onboarding friction.

### Advanced compile (M)

- 🌱 **Speed ramp** — variable playback speed across a scene (start slow, speed up).
- 🌱 **Cross-dissolve between scenes** — currently we do straight cuts; offer optional dissolves at scene boundaries.
- 🌱 **Colour grading** — a simple curve/LUT pass on export.
- 🌱 **Audio waveform on compile** — burn in an audio waveform strip on preview compiles for review.

### Accessibility (M)

- 🌱 **Full keyboard-only operation** — every action addressable by keyboard.
- 🌱 **Screen-reader compatible** — proper labels on all controls.
- 🌱 **High-contrast / colour-blind theme options** — the current cyan onion works for most; offer red/blue and yellow/magenta variants.
- 🌱 **Localised UI** — currently English-only. Extract strings, add i18n framework.

### Hardware integrations (M each)

- 🌱 **Foot pedal** — configure a USB foot pedal (or MIDI pedal) to trigger capture. Common in claymation studios where hands are busy.
- 🌱 **Loupedeck / Stream Deck** — map keys to capture / delete / play / onion toggle for shooting-day muscle memory.
- 🌱 **Bluetooth camera remote** — support standard BLE camera remotes as a capture trigger.
- 🌱 **DMX-controlled turntable** — sync a rotary stage with capture for 360° turnaround shots.

---

## Suggested next milestones

**0.2.0 — Playback** *(shipped: timeline strip, playback at scene fps, plus most of the 0.3.0 UI refresh; loop range and focus mode carried forward)*
Timeline strip. Playback at project fps. Loop range. Focus mode.
_Delivers: makes the app usable for actual shooting sessions instead of just capturing._

**0.3.0 — Camera control + reliability** *(shipped: camera settings in-app, interval capture, compile progress, camera diagnosis, minimal view, Canon 100D tested, CI builds for both platforms)*
_Delivers: usable without touching the camera during a shoot, and camera problems explain themselves._

**0.4.0 — Focus assist + UI polish**
Magnify + peaking. Grid overlays. Typography, empty states, loop range.
_Delivers: nail focus from the app; doesn't look like a Rust prototype anymore._

**0.5.0 — Phone as camera (macOS Continuity)**
AVFoundation backend, iPhone via Continuity Camera. Matches Stop Motion Studio's core value prop.
_Delivers: opens DragonSlayer to iOS-camera users on Macs._

**0.6.0 — Audio + takes**
Reference audio timeline, per-frame notes, multiple takes.
_Delivers: dialogue lip-sync workflow, professional shot management._

**0.7.0+** — motion control, DMX, chroma key, multi-angle capture, cloud review.

**Distribution (any release)** — signed + notarised Mac build (needs a paid Apple Developer account, US$99/year; removes the first-launch "Open Anyway" step) and an Intel Mac build. Mac + Windows zips on the same release is done: GitHub Actions builds both.

---

## What we deliberately *aren't* doing (yet)

Kept out of scope on purpose so the app stays focused:

- **Any form of AI / generative features.** No auto-onion, no in-betweening, no smart cleanup, no ML-based frame prediction, no "helpful" model calls. Ever. Stop-motion is a craft — every frame is a person's choice. This is a hard line, not a scoping decision.
- Full video editor / NLE — export to Premiere/Resolve/FCP instead.
- Vector animation, tweening, drawing tools — that's Toon Boom / TVPaint territory.
- 3D rigging / puppeting — that's Blender / Cascadeur.
- Voiceover recording sessions as a standalone tool.
- Social sharing / built-in exporters to TikTok/YouTube — a directory of MP4s is enough.
- Payment / subscription features — this stays free and open source.

The rest could change with a compelling case. The AI line cannot.

---

## Contributing

Pick anything with 🎯 or ⭐. Open an issue first for anything beyond an S — happy to design it together before code is written. The [`dragonslayer-spec.md`](dragonslayer-spec.md) is the reference for the architecture the new features have to fit into.

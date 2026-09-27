//! The help window: Guide and Advanced camera troubleshooting.

use super::*;

impl DragonSlayerApp {
    pub(super) fn help_window(&mut self, ctx: &egui::Context) {
        if !self.help_open {
            return;
        }
        // egui::Modal centres itself on the screen and doesn't persist window
        // position, so it can't drift off-screen the way our old Window did.
        let response = egui::Modal::new(egui::Id::new("help modal")).show(ctx, |ui| {
            ui.set_max_width(760.0);
            ui.set_max_height(680.0);
            ui.horizontal(|ui| {
                ui.heading("Help");
                ui.add_space(8.0);
                ui.selectable_value(&mut self.help_tab, HelpTab::Guide, "Guide");
                ui.selectable_value(&mut self.help_tab, HelpTab::Advanced, "Advanced camera troubleshooting");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        self.help_open = false;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Instructions for:").small());
                ui.selectable_value(&mut self.help_os, HelpOs::Windows, "Windows");
                ui.selectable_value(&mut self.help_os, HelpOs::Mac, "macOS");
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt(self.help_tab).auto_shrink([false, false]).show(ui, |ui| {
                match self.help_tab {
                    HelpTab::Guide => help_content(ui, self.help_os),
                    HelpTab::Advanced => help_advanced(ui, self.help_os),
                }
            });
        });
        // Click outside the modal or press Escape → close.
        if response.should_close() {
            self.help_open = false;
        }
    }
}

/// Deeper diagnosis for when the Guide's troubleshooting hasn't fixed the camera.
pub(super) fn help_advanced(ui: &mut egui::Ui, os: HelpOs) {
    let h = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(8.0);
        ui.heading(s);
    };
    let sub = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(6.0);
        ui.label(RichText::new(s).strong());
    };
    let p = |ui: &mut egui::Ui, s: &str| {
        ui.label(s);
        ui.add_space(2.0);
    };
    let code = |ui: &mut egui::Ui, s: &str| {
        ui.add(egui::Label::new(RichText::new(s).monospace().background_color(crate::theme::palette().bg_elevated)).wrap());
        ui.add_space(2.0);
    };
    let windows = os == HelpOs::Windows;

    p(ui, "Start with Help → Diagnose camera… (or the stethoscope button next to the camera \
        status). It runs these checks for you and says which one fails. The rest of this page \
        is for when that isn't enough; most camera problems are one of the first three sections.");

    h(ui, "1. Read the message");
    p(ui, "The status bar (top right) says what the camera last reported. What it usually means:");
    sub(ui, "\"Windows is still using its own driver\" / \"Could not claim the USB device\"");
    p(ui, if windows {
        "Windows has the camera on its own photo driver, not WinUSB. Almost always because \
        the camera is on a different USB port than the one Zadig was run on, or a Windows \
        update reset it. Run Zadig again for this port (section 2 shows how to check)."
    } else {
        "Another program has the camera: macOS's ptpcamerad, Image Capture, Photos, or a \
        vendor app. See section 6."
    });
    sub(ui, "\"I/O error\", \"PTP Timeout\", or live view dies straight away");
    p(ui, "The camera stopped answering. Usually a stuck USB session left over from a crash \
        or a disconnect mid-transfer: do the full reset in section 3. If it comes straight \
        back after a reset, it's the USB connection itself: see section 4.");
    sub(ui, "\"Camera disconnected\"");
    p(ui, "The USB link dropped: cable knocked, camera slept or battery died. Check section 5, \
        then reconnect. Frames already captured are safe; one caught mid-download is \
        recovered on the next launch or is still on the camera card.");
    sub(ui, "Connected, but some settings are missing from the Exposure panel");
    p(ui, "Camera brands name settings differently and DragonSlayer may not know your \
        camera's name for one yet. Please report it (section 8).");

    if windows {
        h(ui, "2. Check which driver Windows is using");
        p(ui, "Open Device Manager (right-click Start → Device Manager) with the camera on and \
            plugged in:");
        p(ui, "· Under \"Universal Serial Bus devices\" → WinUSB. This is what DragonSlayer needs.");
        p(ui, "· Under \"Portable Devices\" (or \"Cameras\") → Windows' own driver. Run Zadig: \
            Options → List All Devices, pick the camera, WinUSB, Replace Driver.");
        p(ui, "Zadig's change applies to one USB port. Pick one port for the camera, run \
            Zadig with it there, and always use that port.");
    } else {
        h(ui, "2. Check macOS isn't holding the camera");
        p(ui, "DragonSlayer quits macOS's camera service (ptpcamerad) when it starts, but it \
            restarts itself and grabs cameras plugged in later. If the camera was plugged in \
            after DragonSlayer opened, quit DragonSlayer and open it again. From Terminal:");
        code(ui, "killall ptpcamerad");
        p(ui, "In Image Capture, select the camera and set \"Connecting this camera opens\" to \
            \"No application\" so Photos stops launching.");
    }

    h(ui, "3. The full reset, in this order");
    p(ui, "Order matters. Resetting the camera while DragonSlayer still has it open just \
        wedges it again.");
    p(ui, "1. Quit DragonSlayer (and any other camera software).");
    p(ui, "2. Turn the camera off. If it doesn't respond, take the battery out for 10 seconds.");
    p(ui, "3. Turn it back on and wait until its screen is up.");
    p(ui, "4. Open DragonSlayer.");
    p(ui, "A stuck session never damages the camera. It's software state, and the reset \
        clears it.");

    h(ui, "4. USB: hubs, cables and ports");
    p(ui, "If the camera times out again right after a clean reset, the connection is the \
        likely culprit:");
    p(ui, "· Hubs: plug straight into the computer. Unpowered hubs, monitor USB ports and \
        dongles with many ports are the most common cause of timeouts.");
    p(ui, "· Cables: use a short data cable. Some cables only charge; long ones drop out.");
    p(ui, "· Ports: prefer a port on the back of a desktop (on the motherboard).");
    if windows {
        p(ui, "Changing port on Windows means running Zadig again for the new port.");
    }

    h(ui, "5. Power");
    p(ui, "· Turn auto power off / sleep off in the camera's menu while shooting.");
    p(ui, "· A low battery kills live view before it stops the camera taking photos. For \
        long shoots, a mains adapter (dummy battery) for your camera is worth having.");
    p(ui, "· Live view keeps the sensor on and warms the camera. If it overheats it shuts \
        down to protect itself: let it cool, then carry on.");

    h(ui, "6. Camera-specific");
    sub(ui, "Canon EOS (e.g. 100D)");
    p(ui, "· Mode dial on M. Scene and green auto modes can refuse remote live view.");
    p(ui, "· Red camera menu → Live View shooting: Enable. Without it, live view never starts.");
    p(ui, "· Movie mode blocks stills capture. Use the stills position.");
    p(ui, "· The camera shows a computer icon while connected. That's normal: the picture \
        appears in DragonSlayer, not on the camera's screen.");
    sub(ui, "Panasonic Lumix (e.g. GH5)");
    p(ui, "· Menu → Setup → USB Mode → PC(Tether).");
    p(ui, "· Panasonic bodies can lock up under long live-view sessions. DragonSlayer pauses \
        live view in Preview mode to give the camera a rest. If it keeps happening, switch \
        to Preview between shots.");
    if !windows {
        sub(ui, "macOS");
        p(ui, "· Quit Photos, Image Capture and vendor apps before opening DragonSlayer.");
    }

    h(ui, "7. Test the camera without the app");
    p(ui, "The command-line tool talks to the camera directly, which tells you whether the \
        problem is the camera or the app. Quit DragonSlayer first.");
    if windows {
        p(ui, "In a Command Prompt in the DragonSlayer folder:");
        code(ui, "DragonSlayer-CLI.cmd diagnose");
    } else {
        p(ui, "In Terminal:");
        code(ui, "R=/Applications/DragonSlayer.app/Contents; CAMLIBS=$R/Resources/libgphoto2/camlibs IOLIBS=$R/Resources/libgphoto2/iolibs $R/MacOS/dragonslayer-cli diagnose");
    }
    p(ui, "\"diagnose\" checks the USB driver and hub, opens the camera, reads its settings and \
        grabs a live view frame, printing ok or x for each step. Swap it for \"cameras\" or \
        \"settings\" to see just those.");

    h(ui, "8. Reporting a problem");
    p(ui, "Open an issue on GitHub with your camera model, operating system, what you did and \
        what happened, and the last lines of the log:");
    code(ui, if windows { "%TEMP%\\dragonslayer.log" } else { "~/Library/Logs/dragonslayer.log" });
    p(ui, "Include the output of \"diagnose\" from section 7, and of \"settings\" for missing \
        Exposure settings.");
}

pub(super) fn help_content(ui: &mut egui::Ui, os: HelpOs) {
    let h = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(6.0);
        ui.heading(s);
    };
    let p = |ui: &mut egui::Ui, s: &str| {
        ui.label(s);
        ui.add_space(2.0);
    };
    let k = |ui: &mut egui::Ui, key: &str, what: &str| {
        ui.horizontal(|ui| {
            // Explicit light text on dark chip: the previous version had no colour set
            // and inherited whatever the ambient style decided, which came out black on
            // pale grey inside the modal frame.
            ui.label(
                RichText::new(format!(" {key} "))
                    .monospace()
                    .color(crate::theme::palette().text_primary)
                    .background_color(crate::theme::palette().bg_elevated),
            );
            ui.label(RichText::new(what).color(crate::theme::palette().text_primary));
        });
    };

    h(ui, "What DragonSlayer does");
    p(ui, "DragonSlayer tethers your DSLR or mirrorless camera over USB, shoots your film scene \
        by scene with onion skinning, and compiles it into a video. Nothing else. It's meant to \
        be simple enough that a first-time student can pick it up without a manual — this Help \
        panel exists for the setup bits that are one-time.");

    h(ui, "Get started");
    p(ui, "1. Click New project… and pick a folder. DragonSlayer creates it with one empty scene.");
    p(ui, "2. Plug in your camera on USB, in PC / PC(Tether) / PTP mode. The dot top-right \
        should go green with the camera's name.");
    p(ui, "3. Press Space to capture. The frame lands in the active scene, the counter goes up, \
        and the thumbnail on the left updates.");
    p(ui, "4. Move your subject a tiny bit, press Space again. Onion skin shows where the \
        previous frame was, so you know how far to move.");
    p(ui, "5. When the scene is done, click Add scene (top of the Scenes panel) and keep going.");
    p(ui, "6. Click Compile… to render an MP4 or ProRes MOV.");

    h(ui, "Keyboard shortcuts");
    k(ui, "Space", "Capture the next frame into the active scene (Capture mode only)");
    k(ui, "Backspace", "Move the last frame to that scene's trash folder (undo delete by \
        moving it back from disk — nothing is destroyed)");
    k(ui, "O", "Toggle onion skin on/off");
    k(ui, "Tab", "Switch between Capture and Preview");
    k(ui, "← / →", "Previous / next frame (Shift jumps 10); Home / End for first / last");
    k(ui, "P", "Play / pause at the scene's frame rate");
    k(ui, "F", "Minimal view: full-screen capture with a small floating control bar");
    k(ui, "Esc", "Leave minimal view, or go back to Capture");

    h(ui, "Scene list");
    p(ui, "· Click a scene to make it active. Captures go into the active scene.");
    p(ui, "· Double-click to rename. Enter saves, Esc cancels.");
    p(ui, "· Drag a scene up or down to reorder it in the film.");
    p(ui, "· Right-click for frame rate override and delete. Deleted scenes go to the project \
        trash folder — recover them by moving them back.");
    p(ui, "· Add scene (top of the Scenes panel) inserts a new scene after the active one.");

    h(ui, "Onion skin");
    p(ui, "Ghosts of previous frames overlaid on the live view, so you know where your subject \
        was on the last shot. Frames slider: 1 to 5 previous frames. Opacity: how prominent the \
        ghosts are. Outlines: show only edges of the previous frame instead of the whole ghost, \
        which keeps the live view clear.");

    h(ui, "Exposure (camera settings)");
    p(ui, "The Exposure panel changes aperture, shutter, ISO, white balance and image format \
        on the camera, so you don't have to touch it (and knock the shot) between frames. \
        Put the mode dial on M: in auto modes the camera locks some settings (they show \
        greyed out) and changes exposure between frames, which makes the film flicker. \
        Set Image format to RAW + JPEG there to keep both files.");

    h(ui, "Interval capture");
    p(ui, "In the Camera panel: capture N frames automatically, a set number of seconds \
        apart, for time-lapses. Each shot waits for the previous one to finish, so a slow \
        camera just stretches the schedule. Stop ends it at any time; it also stops by \
        itself if the camera disconnects or you switch to Preview.");

    h(ui, "Import images (rescuing a film from the card)");
    p(ui, "Tools → Import images… pulls photos from a camera \
        card or any folder into a scene. Use it to rebuild a film when a project was lost or \
        damaged but the photos are still on the card, or to add shots taken without \
        DragonSlayer. Pick the card's DCIM folder; each shot's JPEG and RAW become one frame, \
        in the order they were taken. The card is only read, never changed.");
    p(ui, "On a damaged card, files that can't be read are skipped and listed, and JPEGs that \
        look cut short are flagged so you can check them. Running the import again skips \
        frames it already brought in, so you can retry after copying stubborn files off the \
        card another way.");

    h(ui, "Compile");
    p(ui, "Compile… asks whether to render this scene or the whole project, what format \
        (H.264 MP4 or ProRes 422 MOV), what resolution (source, 4K, 1080p) and how to frame \
        (fit with black bars, or crop to fill). A progress bar shows how far along it is and \
        roughly how long is left. Videos land in your project's exports/ folder and are never \
        overwritten.");

    match os {
        HelpOs::Windows => {
            h(ui, "Windows: one-time setup");
            p(ui, "Windows binds cameras to its own driver by default. DragonSlayer needs libusb, \
                which needs the WinUSB driver, which needs a one-time swap with Zadig.");
            p(ui, "1. Download Zadig from https://zadig.akeo.ie (single .exe, no install).");
            p(ui, "2. Plug in the camera, turn it on, set it to PC / PC(Tether) / PTP mode.");
            p(ui, "3. In Zadig: Options → List All Devices.");
            p(ui, "4. Pick your camera from the dropdown (for example DC-GH5 or EOS 100D).");
            p(ui, "5. Target driver: WinUSB. Click Replace Driver. Wait ~30 s.");
            p(ui, "Nothing on the camera changes — only the Windows setting for which driver \
                claims that USB port. To undo: Device Manager → the camera under Universal \
                Serial Bus devices → Uninstall device → tick \"delete the driver software\" → \
                unplug/replug.");
            p(ui, "While swapped, the Windows Photos app and vendor tools won't see the \
                camera. That's expected.");
            p(ui, "Zadig's change belongs to one USB port. Plug the camera into the same port \
                every time: on a different port Windows sees a new device, puts its own driver \
                back, and you'd need to run Zadig again. Use a port on the computer itself \
                rather than a hub, which can make the camera time out.");
        }
        HelpOs::Mac => {
            h(ui, "macOS: releasing the camera");
            p(ui, "macOS grabs cameras for Image Capture and Photos. Before DragonSlayer can \
                talk to yours:");
            p(ui, "1. Quit Image Capture and Photos.");
            p(ui, "2. In Terminal, run:  killall ptpcamerad");
            p(ui, "3. Reconnect the camera and open DragonSlayer.");
            p(ui, "If DragonSlayer says the camera is busy, run killall ptpcamerad again — the \
                process restarts on its own.");
        }
    }

    h(ui, "Camera USB mode");
    p(ui, "Cameras have several USB modes. DragonSlayer needs PC control, not mass-storage.");
    p(ui, "· Panasonic (e.g. GH5): Menu → SETUP → USB Mode → PC(Tether).");
    p(ui, "· Canon EOS (e.g. 100D): works as soon as it's plugged in. For live view, set the \
        mode dial to M and check Live View shooting is enabled in the red camera menu. \
        The camera shows a small computer icon while connected; that's normal, the \
        picture appears in DragonSlayer.");
    p(ui, "· Set RAW+JPEG on the camera if you want both files kept. If off, DragonSlayer \
        keeps the JPEG and prints a one-off warning.");

    h(ui, "Troubleshooting");

    let sub = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(6.0);
        ui.label(RichText::new(s).strong());
    };

    sub(ui, "\"No camera found\" but the camera is plugged in");
    p(ui, "The most common cause is a wedged PTP session — a previous session (DragonSlayer or \
        anything else) crashed while the camera was open, and the camera thinks it's still \
        talking to a host that isn't there. Nothing on the camera is broken; it just needs a \
        reset.");
    p(ui, "Fix in order:");
    p(ui, "1. Turn the camera off, wait 3 seconds, turn it back on. This is the fix for \
        a wedged camera 90% of the time.");
    p(ui, "2. If still nothing: unplug the USB, wait 3 seconds, plug it back into the same \
        port. (On Windows, a different port needs Zadig again; see the Windows setup section.)");
    p(ui, "3. Check the camera's USB mode (see the \"Camera USB mode\" section above).");
    p(ui, "4. Close any other program that might have grabbed the camera: Photos, Image \
        Capture, Canon EOS Utility, Lumix Tether, etc.");
    match os {
        HelpOs::Windows => {
            p(ui, "5. On Windows: check that Zadig was applied to THIS USB port (Device \
                Manager should show the camera under \"Universal Serial Bus devices\", not \
                under \"Portable Devices\" or \"Imaging devices\").");
        }
        HelpOs::Mac => {
            p(ui, "5. On macOS: open Terminal and run:  killall ptpcamerad  — this stops \
                the macOS camera daemon from grabbing the connection. The daemon restarts on \
                its own if needed.");
        }
    }

    sub(ui, "\"Camera in use by another program\"");
    p(ui, "Something else already has the USB interface open. Close: the vendor's own \
        tether app (Canon EOS Utility, Lumix Tether, Nikon Camera Control), Windows Photos, \
        macOS Photos and Image Capture, any Adobe Lightroom import wizard, browser tabs \
        showing camera previews. Unplug and replug after closing.");
    if matches!(os, HelpOs::Mac) {
        p(ui, "On macOS the most common culprit is ptpcamerad, a background process. Run:  \
            killall ptpcamerad  — it restarts itself but releases the camera in between.");
    }

    sub(ui, "Capture fails with a timeout, works after camera reboot");
    p(ui, "Same as \"wedged camera\" above: the PTP session got out of sync. Camera off, \
        3 s, camera on. If it keeps happening on one specific model, please report the \
        camera and the exact error text.");

    sub(ui, "Capture takes 3–20 seconds per frame");
    p(ui, "Not a bug for some bodies. Panasonic in particular is slow through libgphoto2's \
        PTP path — the camera itself takes several seconds per shot. Canon is usually 1–3 s, \
        Nikon 2–5 s. What you can improve:");
    p(ui, "· Turn off RAW+JPEG in the camera if you don't need RAW. RAW downloads take \
        significantly longer.");
    p(ui, "· Reduce the JPEG size on the camera (Fine → Normal, or Large → Medium).");
    p(ui, "· Use a fast USB-A port directly on the computer, not through a hub.");

    sub(ui, "Live view stops but capture still works");
    p(ui, "Panasonic bodies sometimes exit live view after a capture and don't recover on \
        their own. Toggle Live/Last frame (Tab) once — this restarts the request. If it \
        happens every capture, check the camera doesn't have an auto power-off setting \
        that's cutting live view.");

    sub(ui, "Camera keeps going to sleep");
    p(ui, "Every DSLR has an auto power-off. During long stop-motion sessions this will \
        interrupt tethering. In the camera menu, set auto power-off to \"Off\" or the \
        longest available setting. DragonSlayer already prevents the computer from sleeping \
        while the app is open with a project.");

    sub(ui, "Live view runs but no live view is shown");
    p(ui, "Not every camera supports USB live view (older DSLRs often don't). If the \
        model reports no live view at connect, DragonSlayer shows the last captured frame \
        instead — press Space to capture, and the viewer will follow along. Onion skin \
        still works, layered on the last frame.");

    sub(ui, "Onion skin looks muddy or doesn't help");
    p(ui, "Turn Outlines on — that shows only the edges of the previous frame, which \
        makes movement much clearer than a full ghost. If you shoot on a bright \
        background and outlines are noisy, drop the opacity slider a notch.");

    sub(ui, "Frames aren't appearing in scenes/");
    p(ui, "Check the message bar at the bottom of the app for an error. If capture is \
        succeeding (the counter goes up, thumbnails update) but you can't find the files, \
        make sure you're looking at the correct project — the top bar shows its name — \
        and the correct scene folder inside scenes/. Deleted frames go to \
        scenes/scXXX/trash/, not out of existence.");

    sub(ui, "The app won't start");
    match os {
        HelpOs::Windows => {
            p(ui, "If Windows shows \"the program can't start because <name>.dll is missing\", \
                the DLLs weren't next to the .exe. Reinstall by copying the whole target/ \
                folder or use the .cmd wrapper in bin/.");
            p(ui, "If clicking the .cmd flashes a window then closes, an old dragonslayer-app.exe \
                is still running. Open Task Manager, kill any \"dragonslayer-app.exe\" entries, \
                and try again. The wrapper now kills any stale one automatically.");
        }
        HelpOs::Mac => {
            p(ui, "If macOS says the app is damaged or from an unidentified developer, run:  \
                xattr -dr com.apple.quarantine /path/to/dragonslayer-app  — this removes the \
                quarantine attribute Safari adds to downloaded binaries.");
        }
    }

    sub(ui, "Recovering from a crash");
    p(ui, "Nothing on disk is destroyed. Every capture is a transaction — the file is \
        flushed to disk before it's recorded in the journal. On the next launch DragonSlayer \
        replays the journal, moves any file left in the incoming/ folder into frames/, \
        and marks anything with no file as abandoned (usually still on the camera card). \
        You should see a brief \"Recovered N captures\" message at the bottom.");

    sub(ui, "\"No camera support in this build\"");
    p(ui, "The app was compiled without the gphoto2 feature. Rebuild with:  \
        cargo build --release --features gphoto2 -p dragonslayer-app  — or launch with \
        --mock to try the built-in fake camera.");

    h(ui, "The project folder");
    p(ui, "A project is a plain folder — you can copy it to a USB stick or hand it in. \
        Frames live under scenes/scXXX/frames/. Deleted frames and scenes go to trash \
        folders next to them, never destroyed. journal.ndjson records every capture and \
        delete so the frame order can be rebuilt from disk.");

    h(ui, "About this build");
    p(ui, &format!(
        "DragonSlayer v{}. Free and open source. Report bugs, ask for cameras to be added, \
        or contribute at https://github.com/TheMagnificentRonnie/DragonSlayer",
        env!("CARGO_PKG_VERSION")
    ));

    h(ui, "Disclaimer — please read");
    ui.label(
        RichText::new(
            "DragonSlayer is free, open-source hobby software. It is provided \"as is\", \
             with no warranty of any kind — express or implied — including but not limited \
             to fitness for purpose, merchantability, or non-infringement. Use it entirely \
             at your own risk.",
        )
        .strong(),
    );
    ui.add_space(4.0);
    p(ui, "By using this software you accept that:");
    p(ui, "· The authors and contributors are not liable for any lost footage, damaged \
        files, missed shots, corrupt SD cards, wedged cameras, missed deadlines, delayed \
        productions, or any other direct, indirect, incidental, special, exemplary or \
        consequential damages arising from use or misuse of this software.");
    p(ui, "· Nothing about DragonSlayer is professionally supported. There is no help \
        desk. There is no SLA. Bug fixes happen when someone in the community writes them.");
    p(ui, "· This is a BETA. Features may change, break, or disappear. The project format \
        should stay compatible, but that is not a guarantee.");
    p(ui, "· For anything mission-critical — a paid gig, an assessed project, an \
        irreplaceable shot — back up frequently, keep the camera card, and consider \
        using established commercial software as well or instead.");
    p(ui, "· Camera firmware, USB drivers and OS updates can break tethering in ways \
        outside this project's control. If your camera stops responding, the fix is \
        usually to power-cycle it (see Troubleshooting → wedged camera above).");
    p(ui, "· The bundled Zadig helper installs a WinUSB driver on Windows. This is a \
        Windows setting change, not a camera modification, and can be undone in Device \
        Manager. While active, the Windows Photos app and vendor tools will not see the \
        camera.");
    p(ui, "· If you don't agree to any of this, don't use the software. Delete it, \
        keep your camera card, and have a nice day.");
    ui.add_space(12.0);
}

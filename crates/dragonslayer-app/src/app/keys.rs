//! Keyboard shortcuts.

use super::*;

impl DragonSlayerApp {
    pub(super) fn shortcuts_active(&self) -> bool {
        self.renaming.is_none() && !self.compile.open && !self.help_open && !self.diagnose_open && !self.import.open
    }

    /// Takes the shortcut keys out of the input before egui sees them, so a
    /// focused button can't swallow Space and Tab doesn't move keyboard focus.
    pub(super) fn take_shortcuts(&mut self, raw: &mut egui::RawInput, typing: bool) {
        // While typing in a text or number field, Backspace and Space belong to the field,
        // not to delete-last-frame and capture.
        if !self.shortcuts_active() || typing {
            return;
        }
        // Keys we handle ourselves. Arrow keys and Home/End are navigation.
        const KEYS: [Key; 12] = [
            Key::Space, Key::Backspace, Key::O, Key::Tab, Key::H, Key::P, Key::F,
            Key::ArrowLeft, Key::ArrowRight, Key::Home, Key::End, Key::Escape,
        ];
        let mut pressed = Vec::new();
        // With Shift for coarser navigation.
        let mut pressed_shift = Vec::new();
        raw.events.retain(|e| match e {
            egui::Event::Key { key, pressed: down, repeat, modifiers, .. } if KEYS.contains(key) => {
                let plain = modifiers.is_none();
                let shift_only = modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.command;
                if (plain || shift_only) && *down {
                    // Let arrow keys auto-repeat while held so scrubbing feels natural;
                    // everything else fires once per press.
                    let allow_repeat = matches!(key, Key::ArrowLeft | Key::ArrowRight);
                    if !*repeat || allow_repeat {
                        if shift_only {
                            pressed_shift.push(*key);
                        } else {
                            pressed.push(*key);
                        }
                    }
                    false
                } else {
                    true
                }
            }
            egui::Event::Text(t) => !matches!(t.as_str(), " " | "o" | "O" | "h" | "H" | "p" | "P" | "f" | "F"),
            _ => true,
        });
        self.keys.extend(pressed);
        // Shift+arrow → coarser step; we tag by pushing a sentinel None-encoding via a separate field
        // would need more scaffolding; simplest is to push the arrow multiple times.
        for k in pressed_shift {
            match k {
                Key::ArrowLeft | Key::ArrowRight => {
                    for _ in 0..10 { self.keys.push(k); }
                }
                _ => self.keys.push(k),
            }
        }
    }

    pub(super) fn handle_keys(&mut self) {
        for key in std::mem::take(&mut self.keys) {
            // Minimal view is capture-only: no Preview navigation.
            if self.minimal
                && matches!(key, Key::Tab | Key::P | Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End)
            {
                continue;
            }
            match key {
                Key::Space => self.capture(),
                Key::Backspace => self.delete_last(),
                Key::O => self.onion_on = !self.onion_on,
                Key::Tab => self.toggle_live_last(),
                Key::H => self.help_open = !self.help_open,
                Key::P => self.toggle_play(),
                Key::ArrowLeft => self.step(-1),
                Key::ArrowRight => self.step(1),
                Key::Home => self.jump_to(Some(0)),
                Key::End => self.jump_to_end(),
                Key::F => self.toggle_minimal(),
                Key::Escape if self.minimal => self.minimal = false,
                Key::Escape => self.mode = Mode::Capture,
                _ => {}
            }
        }
    }
}

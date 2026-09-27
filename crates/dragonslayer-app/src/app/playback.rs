//! Moving through captured frames in Preview, and playing them.

use super::*;

impl DragonSlayerApp {
    /// Ctrl-Tab-style toggle between live view and the last captured frame.
    pub(super) fn toggle_live_last(&mut self) {
        self.mode = match self.mode {
            Mode::Capture => match self.frames.len().checked_sub(1) {
                Some(i) => Mode::Preview { index: i, playing: false, last_advance: Instant::now() },
                None => Mode::Capture,
            },
            _ => Mode::Capture,
        };
    }

    pub(super) fn step(&mut self, delta: i32) {
        if self.frames.is_empty() {
            return;
        }
        let last = self.frames.len() - 1;
        let current = self.mode.preview_index().unwrap_or(last);
        let next = (current as i64 + delta as i64).clamp(0, last as i64) as usize;
        self.mode = Mode::Preview { index: next, playing: false, last_advance: Instant::now() };
    }

    pub(super) fn jump_to(&mut self, idx: Option<usize>) {
        if self.frames.is_empty() {
            return;
        }
        let last = self.frames.len() - 1;
        let target = idx.unwrap_or(last).min(last);
        self.mode = Mode::Preview { index: target, playing: false, last_advance: Instant::now() };
    }

    pub(super) fn jump_to_end(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.mode = Mode::Preview { index: self.frames.len() - 1, playing: false, last_advance: Instant::now() };
    }

    pub(super) fn toggle_play(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.mode = match self.mode {
            Mode::Preview { index, playing: true, .. } => {
                // Pause on the current frame.
                Mode::Preview { index, playing: false, last_advance: Instant::now() }
            }
            Mode::Capture => Mode::Preview { index: 0, playing: true, last_advance: Instant::now() },
            Mode::Preview { index, playing: false, .. } => {
                let start = if index == self.frames.len() - 1 { 0 } else { index };
                Mode::Preview { index: start, playing: true, last_advance: Instant::now() }
            }
        };
        self.playback_tick = Instant::now();
    }

    /// Advance the playback cursor by one frame if the fps interval has passed.
    pub(super) fn tick_playback(&mut self) {
        // Only advance while actually playing. Bug from an earlier refactor was matching
        // any Preview state here, which auto-advanced frames after every keypress.
        let Mode::Preview { index, last_advance, playing: true } = self.mode else { return };
        if self.frames.is_empty() {
            self.mode = Mode::Capture;
            return;
        }
        let fps = self
            .project
            .as_ref()
            .and_then(|p| p.active_scene().ok().map(|s| p.fps_for(&s)))
            .unwrap_or(12)
            .max(1);
        let step = Duration::from_secs_f64(1.0 / f64::from(fps));
        let elapsed = last_advance.elapsed();
        if elapsed < step {
            return;
        }
        // Handle "we fell behind by multiple frames" gracefully.
        let steps = (elapsed.as_secs_f64() / step.as_secs_f64()).floor() as usize;
        let next = index + steps;
        if next >= self.frames.len() {
            // End of scene → pause on last frame.
            self.mode = Mode::Preview { index: self.frames.len() - 1, playing: false, last_advance: Instant::now() };
        } else {
            self.mode = Mode::Preview { index: next, playing: true, last_advance: last_advance + step * (steps as u32),
             };
        }
        self.playback_tick = Instant::now();
    }

    // ---- ui -------------------------------------------------------------
}

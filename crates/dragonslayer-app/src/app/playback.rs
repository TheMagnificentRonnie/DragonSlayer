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
        let (lo, hi) = self.play_bounds();
        // Give the starting frame a moment before advancing, so slow first decodes on macOS
        // and Windows don't blank the viewer at the very first step.
        let head_start = Instant::now() + Duration::from_millis(150);
        self.mode = match self.mode {
            Mode::Preview { index, playing: true, .. } => {
                // Pause on the current frame.
                Mode::Preview { index, playing: false, last_advance: Instant::now() }
            }
            Mode::Capture => Mode::Preview { index: lo, playing: true, last_advance: head_start },
            Mode::Preview { index, playing: false, .. } => {
                // From the end of the range, or outside it: start again at the in point.
                let start = if index >= hi || index < lo { lo } else { index };
                Mode::Preview { index: start, playing: true, last_advance: head_start }
            }
        };
        self.playback_tick = Instant::now();
    }

    /// Advance the playback cursor by one frame if the fps interval has passed. Plays the
    /// marked range if there is one (else the whole scene); wraps when looping, else stops.
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
        let (lo, hi) = self.play_bounds();
        let next = index + steps;
        let advanced = last_advance + step * (steps as u32);
        self.mode = if next <= hi {
            Mode::Preview { index: next, playing: true, last_advance: advanced }
        } else if self.loop_on {
            let len = hi - lo + 1;
            Mode::Preview { index: lo + (next - lo) % len, playing: true, last_advance: advanced }
        } else {
            // End of the range → pause on its last frame.
            Mode::Preview { index: hi, playing: false, last_advance: Instant::now() }
        };
        self.playback_tick = Instant::now();
    }

    /// Frame shown now: Preview's, or the last one while capturing.
    pub(super) fn current_index(&self) -> Option<usize> {
        self.mode.preview_index().or(self.frames.len().checked_sub(1))
    }

    /// The marked range, 0-based and inclusive, clamped to the frames there are. One mark
    /// alone runs to the end (or from the start).
    pub(super) fn marked_range(&self) -> Option<(usize, usize)> {
        let last = self.frames.len().checked_sub(1)?;
        let (a, b) = match (self.mark_in, self.mark_out) {
            (Some(a), Some(b)) => (a.min(b), a.max(b)),
            (Some(a), None) => (a, last),
            (None, Some(b)) => (0, b),
            (None, None) => return None,
        };
        Some((a.min(last), b.min(last)))
    }

    /// What playback covers: the marked range, or the whole scene.
    pub(super) fn play_bounds(&self) -> (usize, usize) {
        self.marked_range().unwrap_or((0, self.frames.len().saturating_sub(1)))
    }

    /// `[`: in point at the current frame; again on the same frame clears it.
    pub(super) fn toggle_mark_in(&mut self) {
        let Some(i) = self.current_index() else { return };
        self.mark_in = if self.mark_in == Some(i) { None } else { Some(i) };
    }

    /// `]`: out point at the current frame; again on the same frame clears it.
    pub(super) fn toggle_mark_out(&mut self) {
        let Some(i) = self.current_index() else { return };
        self.mark_out = if self.mark_out == Some(i) { None } else { Some(i) };
    }

    pub(super) fn clear_marks(&mut self) {
        self.mark_in = None;
        self.mark_out = None;
    }

    // ---- ui -------------------------------------------------------------
}

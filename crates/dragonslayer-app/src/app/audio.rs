//! Reference audio: decoding a scene's sound for the waveform under the filmstrip, and
//! playing it in step with playback and scrubbing.

use std::num::NonZero;
use std::sync::mpsc::TryRecvError;

use super::*;

/// Waveform resolution: loudest sample per 1/200 s.
pub(super) const PEAKS_PER_SEC: f64 = 200.0;

/// A decoded sound, mixed down to mono.
pub(super) struct Track {
    pub path: PathBuf,
    pub rate: u32,
    pub samples: Arc<Vec<f32>>,
    pub peaks: Vec<f32>,
}

impl Track {
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / f64::from(self.rate)
    }

    /// Loudest peak between two times in the sound (0.0 outside it).
    pub fn peak(&self, from: f64, to: f64) -> f32 {
        let a = (from * PEAKS_PER_SEC).floor().max(0.0) as usize;
        let b = ((to * PEAKS_PER_SEC).ceil().max(0.0) as usize).max(a + 1).min(self.peaks.len());
        self.peaks.get(a..b).map_or(0.0, |p| p.iter().copied().fold(0.0, f32::max))
    }
}

pub(super) fn decode(path: &Path) -> Result<Track, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let decoder = rodio::Decoder::try_from(file).map_err(|e| format!("can't read this sound: {e}"))?;
    use rodio::Source as _;
    let channels = usize::from(decoder.channels().get());
    let rate = decoder.sample_rate().get();
    let interleaved: Vec<f32> = decoder.collect();
    let samples: Vec<f32> = interleaved.chunks(channels).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    let bucket = ((f64::from(rate) / PEAKS_PER_SEC).round() as usize).max(1);
    let peaks = samples.chunks(bucket).map(|c| c.iter().fold(0.0f32, |m, s| m.max(s.abs())).min(1.0)).collect();
    Ok(Track { path: path.to_path_buf(), rate, samples: Arc::new(samples), peaks })
}

/// Part of a track, played from memory.
struct Clip {
    samples: Arc<Vec<f32>>,
    pos: usize,
    end: usize,
    rate: u32,
}

impl Iterator for Clip {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        (self.pos < self.end).then(|| {
            self.pos += 1;
            self.samples[self.pos - 1]
        })
    }
}

impl rodio::Source for Clip {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::<u16>::MIN
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(self.rate).unwrap_or(NonZero::<u32>::MIN)
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64((self.end - self.pos) as f64 / f64::from(self.rate)))
    }
}

/// What was last sent to the speakers (kept for the tests, which have no sound device).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Cue {
    /// Seconds into the sound.
    pub from: f64,
    /// A short blip while stepping or scrubbing, rather than playing on.
    pub snippet: bool,
}

#[derive(Default)]
pub(super) struct Audio {
    /// The active scene's sound, once decoded.
    pub track: Option<Arc<Track>>,
    loading: Option<(PathBuf, Receiver<Result<Track, String>>)>,
    /// A file that wouldn't decode; not retried until the scene's sound changes.
    failed: Option<PathBuf>,
    sink: Option<rodio::MixerDeviceSink>,
    no_device: bool,
    player: Option<rodio::Player>,
    /// Playing and frame as of the last sync, to notice starts, stops, jumps and steps.
    last: Option<(bool, usize)>,
    pub cue: Option<Cue>,
    pub muted: bool,
    /// Start (seconds) while the user drags it; saved on release.
    pub dragging_start: Option<f64>,
}

impl Audio {
    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Makes `path` the sound to show and play: decodes it in the background if it's new.
    fn want(&mut self, path: Option<&Path>, ctx: &egui::Context) -> Option<String> {
        let mut problem = None;
        if let Some((p, rx)) = &self.loading {
            match rx.try_recv() {
                Ok(Ok(track)) => {
                    self.track = Some(Arc::new(track));
                    self.loading = None;
                }
                Ok(Err(e)) => {
                    problem = Some(format!("Reference audio {}: {e}", p.display()));
                    self.failed = Some(p.clone());
                    self.loading = None;
                }
                Err(TryRecvError::Disconnected) => self.loading = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let current = self.loading.as_ref().map(|(p, _)| p.as_path()).or(self.track.as_ref().map(|t| t.path.as_path()));
        if current == path {
            return problem;
        }
        self.stop();
        self.track = None;
        self.loading = None;
        let Some(path) = path else {
            self.failed = None;
            return problem;
        };
        if self.failed.as_deref() == Some(path) {
            return problem;
        }
        self.failed = None;
        let (tx, rx) = mpsc::channel();
        let (p, ctx) = (path.to_path_buf(), ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(decode(&p));
            ctx.request_repaint();
        });
        self.loading = Some((path.to_path_buf(), rx));
        problem
    }

    pub fn stop(&mut self) {
        self.player = None;
    }

    /// Plays the track from `from` seconds, after `delay`, for `length` (or to the end).
    fn play(&mut self, from: f64, delay: Duration, length: Option<f64>) {
        self.stop();
        self.cue = Some(Cue { from, snippet: length.is_some() });
        let Some(track) = self.track.clone() else { return };
        if self.muted || cfg!(test) || self.no_device {
            return;
        }
        if self.sink.is_none() {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut sink) => {
                    sink.log_on_drop(false);
                    self.sink = Some(sink);
                }
                Err(e) => {
                    log_line(&format!("no sound output: {e}"));
                    self.no_device = true;
                    return;
                }
            }
        }
        let Some(sink) = &self.sink else { return };
        let rate = f64::from(track.rate);
        let pos = ((from.max(0.0) * rate) as usize).min(track.samples.len());
        let end = length.map_or(track.samples.len(), |l| (pos + (l * rate) as usize).min(track.samples.len()));
        if pos >= end {
            return;
        }
        let clip = Clip { samples: track.samples.clone(), pos, end, rate: track.rate };
        let player = rodio::Player::connect_new(sink.mixer());
        use rodio::Source as _;
        player.append(clip.delay(delay));
        self.player = Some(player);
    }
}

impl DragonSlayerApp {
    /// The active scene's sound and the second in it at frame 1 (a drag in progress wins).
    pub(super) fn audio_start(&self) -> Option<f64> {
        self.scene_audio.as_ref().map(|(_, s)| self.audio.dragging_start.unwrap_or(*s))
    }

    /// Seconds into the sound where frame `index` of the active scene falls.
    pub(super) fn audio_time(&self, index: usize) -> Option<f64> {
        Some(self.audio_start()? + index as f64 / f64::from(self.active_fps.max(1)))
    }

    /// Keeps the sound in step with the viewer: plays along with playback (from the frame
    /// it starts on, again after a loop or jump), stops on pause, and blips each frame
    /// stepped or scrubbed to.
    pub(super) fn sync_audio(&mut self, ctx: &egui::Context) {
        let path = self.scene_audio.as_ref().map(|(p, _)| p.clone());
        if let Some(problem) = self.audio.want(path.as_deref(), ctx) {
            self.error(problem);
        }
        let now = match self.mode {
            Mode::Preview { index, playing, .. } => Some((playing, index)),
            Mode::Capture => None,
        };
        let before = std::mem::replace(&mut self.audio.last, now);
        if now == before || self.audio.track.is_none() {
            if now.is_none() && before.is_some() {
                self.audio.stop();
            }
            return;
        }
        let Some((playing, index)) = now else {
            self.audio.stop();
            return;
        };
        let Some(at) = self.audio_time(index) else { return };
        if playing {
            // Carry on unless playback has jumped (a loop back to the in point, or a click).
            let carried_on = matches!(before, Some((true, i)) if index > i);
            if !carried_on {
                let delay = match self.mode {
                    Mode::Preview { last_advance, .. } => last_advance.saturating_duration_since(Instant::now()),
                    Mode::Capture => Duration::ZERO,
                };
                self.audio.play(at, delay, None);
            }
        } else if matches!(before, Some((true, _))) {
            self.audio.stop();
        } else {
            // Stepped or scrubbed: a frame's worth of sound, a little more at slow rates.
            let length = (1.0 / f64::from(self.active_fps.max(1))).max(0.08);
            self.audio.play(at, Duration::ZERO, Some(length));
        }
    }

    /// Asks for a sound file and makes it the active scene's reference audio.
    pub(super) fn pick_scene_audio(&mut self) {
        let Some(id) = self.project.as_ref().and_then(|p| p.file.active_scene.clone()) else { return };
        let Some(file) = rfd::FileDialog::new()
            .set_title("Reference audio for this scene")
            .add_filter("Sound", &["wav", "mp3", "flac", "ogg", "m4a", "aac"])
            .pick_file()
        else {
            return;
        };
        self.set_scene_audio(&id, Some(&file));
    }

    pub(super) fn set_scene_audio(&mut self, id: &str, file: Option<&Path>) {
        let id = id.to_owned();
        self.edit(|p| p.set_scene_audio(&id, file));
        if file.is_some() && self.scene_audio.is_some() {
            self.info("Reference audio added: it plays along in Preview, and goes in the compiled film");
        }
    }

    /// Saves where frame 1 falls in the sound, in seconds.
    pub(super) fn set_audio_start(&mut self, secs: f64) {
        let Some(id) = self.project.as_ref().and_then(|p| p.file.active_scene.clone()) else { return };
        let ms = (secs.max(0.0) * 1000.0).round() as u64;
        self.edit(|p| p.set_audio_start(&id, ms));
    }
}

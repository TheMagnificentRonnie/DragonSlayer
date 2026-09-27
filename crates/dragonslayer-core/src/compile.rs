//! Compile a scene or the whole project to video with ffmpeg (spec §11).

use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use time::macros::format_description;
use time::OffsetDateTime;

use crate::error::{Error, IoContext, Result};
use crate::{atomic, paths, Project};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    H264,
    ProRes,
}

impl Format {
    fn ext(self) -> &'static str {
        match self {
            Format::H264 => "mp4",
            Format::ProRes => "mov",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resolution {
    #[default]
    Source,
    Uhd,
    Hd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Framing {
    Crop,
    #[default]
    Fit,
}

#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub format: Format,
    pub resolution: Resolution,
    pub framing: Framing,
    /// If set, forces the whole compile to this playback rate — every frame gets a
    /// 1/fps duration regardless of the project or per-scene overrides. Left `None`,
    /// each scene uses its own fps (or the project's).
    pub fps_override: Option<u32>,
    /// ffmpeg executable; defaults to `ffmpeg` on PATH.
    pub ffmpeg: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Output {
    pub path: PathBuf,
    pub frames: usize,
    pub warnings: Vec<String>,
}

/// One input image and how long it stays on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub path: PathBuf,
    pub seconds: f64,
}

/// Builds the ordered frame list: one scene, or every scene in project order.
/// Mixed scene rates are handled by giving each frame its own duration.
///
/// `fps_override`, if set, forces every frame's duration to `1 / fps`, ignoring
/// per-scene rates. Otherwise each scene contributes at its own fps.
pub fn plan(project: &Project, scene: Option<&str>, fps_override: Option<u32>) -> Result<(Vec<Shot>, Vec<String>)> {
    let scenes = match scene {
        Some(key) => vec![project.find_scene(key)?],
        None => project.scenes()?,
    };
    let mut shots = Vec::new();
    let mut warnings = Vec::new();
    for s in &scenes {
        let fps = fps_override.unwrap_or_else(|| project.fps_for(s));
        let seconds = 1.0 / f64::from(fps);
        let frames = s.frames()?;
        if frames.is_empty() {
            warnings.push(format!("scene {:?} ({}) is empty; skipped", s.name(), s.id()));
            continue;
        }
        for f in frames {
            match f.jpeg() {
                Some(p) => shots.push(Shot { path: p.to_path_buf(), seconds }),
                None => warnings.push(format!("scene {} frame {} has no JPEG; skipped", s.id(), f.id)),
            }
        }
    }
    if shots.is_empty() {
        let what = scene.map_or("the project".to_string(), |s| format!("scene {s:?}"));
        return Err(Error::NothingToCompile(format!("{what} has no frames")));
    }
    Ok((shots, warnings))
}

pub fn compile(project: &Project, scene: Option<&str>, settings: &Settings) -> Result<Output> {
    compile_with_progress(project, scene, settings, |_| {})
}

/// Like [`compile`], calling `on_progress` with 0.0–1.0 as ffmpeg works through the film.
pub fn compile_with_progress(
    project: &Project,
    scene: Option<&str>,
    settings: &Settings,
    mut on_progress: impl FnMut(f32),
) -> Result<Output> {
    let (shots, warnings) = plan(project, scene, settings.fps_override)?;
    let total_secs: f64 = shots.iter().map(|s| s.seconds).sum();
    let exports = paths::exports_dir(&project.root);
    fs::create_dir_all(&exports).at(&exports)?;

    let label = match scene {
        Some(key) => project.find_scene(key)?.id().to_owned(),
        None => "all".into(),
    };
    let stamp = OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(format_description!("[year][month][day]-[hour][minute][second]"))
        .expect("valid format");
    let name = format!("{}_{}_{}.{}", sanitise(project.name()), label, stamp, settings.format.ext());
    let out = paths::unique_path(&exports, &name);

    let list = exports.join(format!(".{}.concat.txt", out.file_stem().unwrap().to_string_lossy()));
    atomic::write_atomic(&list, concat_list(&shots).as_bytes())?;

    let ffmpeg = settings.ffmpeg.clone().unwrap_or_else(|| "ffmpeg".into());
    let result = run_ffmpeg(
        &ffmpeg,
        ffmpeg_args(&list, &out, settings.fps_override.unwrap_or(project.file.fps), settings),
        total_secs,
        &mut on_progress,
    );
    let _ = fs::remove_file(&list);
    result?;
    on_progress(1.0);
    Ok(Output { path: out, frames: shots.len(), warnings })
}

fn run_ffmpeg(ffmpeg: &Path, args: Vec<String>, total_secs: f64, on_progress: &mut impl FnMut(f32)) -> Result<()> {
    let mut child = Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Ffmpeg(format!("could not run {}: {e}", ffmpeg.display())))?;

    // Drain stderr on its own thread so a chatty ffmpeg can't block on a full pipe.
    let mut stderr_pipe = child.stderr.take().expect("piped");
    let stderr = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr_pipe.read_to_string(&mut s);
        s
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
            if let Some(p) = progress_from_line(&line, total_secs) {
                on_progress(p);
            }
        }
    }

    let status = child.wait().map_err(|e| Error::Ffmpeg(e.to_string()))?;
    let stderr = stderr.join().unwrap_or_default();
    if !status.success() {
        let tail: Vec<&str> = stderr.lines().rev().take(8).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(Error::Ffmpeg(tail.join("\n")));
    }
    Ok(())
}

/// Reads one line of ffmpeg's `-progress` output. `out_time_ms` is really microseconds,
/// same as `out_time_us`; older ffmpeg builds only print the former.
pub fn progress_from_line(line: &str, total_secs: f64) -> Option<f32> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" if total_secs > 0.0 => {
            let us: f64 = value.parse().ok()?;
            Some((us / 1e6 / total_secs).clamp(0.0, 1.0) as f32)
        }
        "progress" if value == "end" => Some(1.0),
        _ => None,
    }
}

/// ffmpeg concat-demuxer script. The last file is repeated so its duration is honoured.
pub fn concat_list(shots: &[Shot]) -> String {
    let mut s = String::from("ffconcat version 1.0\n");
    for shot in shots {
        let _ = writeln!(s, "file '{}'\nduration {:.6}", escape(&shot.path), shot.seconds);
    }
    if let Some(last) = shots.last() {
        let _ = writeln!(s, "file '{}'", escape(&last.path));
    }
    s
}

pub fn ffmpeg_args(list: &Path, out: &Path, fps: u32, settings: &Settings) -> Vec<String> {
    let mut filters = vec![match (settings.resolution, settings.framing) {
        (Resolution::Source, _) => "scale=trunc(iw/2)*2:trunc(ih/2)*2".to_string(),
        (r, framing) => {
            let (w, h) = match r {
                Resolution::Uhd => (3840, 2160),
                Resolution::Hd => (1920, 1080),
                Resolution::Source => unreachable!(),
            };
            match framing {
                Framing::Fit => format!(
                    "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"
                ),
                Framing::Crop => {
                    format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}")
                }
            }
        }
    }];
    filters.push("setsar=1".into());
    filters.push(format!("fps={fps}"));

    let mut args: Vec<String> = [
        "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-n",
        "-f", "concat", "-safe", "0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push("-i".into());
    args.push(list.to_string_lossy().into_owned());
    args.push("-vf".into());
    args.push(filters.join(","));
    match settings.format {
        Format::H264 => args.extend(
            ["-c:v", "libx264", "-preset", "slow", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart"]
                .map(String::from),
        ),
        Format::ProRes => args.extend(
            ["-c:v", "prores_ks", "-profile:v", "2", "-pix_fmt", "yuv422p10le"].map(String::from),
        ),
    }
    args.push(out.to_string_lossy().into_owned());
    args
}

fn escape(p: &Path) -> String {
    // Forward slashes work for ffmpeg on Windows too; quotes are closed, escaped and reopened.
    p.to_string_lossy().replace('\\', "/").replace('\'', r"'\''")
}

fn sanitise(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() { "project".into() } else { s }
}

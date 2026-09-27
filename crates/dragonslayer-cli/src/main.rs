use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use clap::{Parser, Subcommand, ValueEnum};
use dragonslayer_camera::{mock::MockBackend, CameraBackend, FileKind, SettingKind};
use dragonslayer_core::compile::{self, Format, Framing, Resolution, Settings};
use dragonslayer_core::{capture, Project};

#[derive(Parser)]
#[command(name = "dragonslayer", version, about = "Free stop-motion capture for real cameras")]
struct Cli {
    /// Use the built-in mock camera instead of a real one.
    #[arg(long, global = true)]
    mock: bool,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a project folder.
    New {
        project: PathBuf,
        /// Display name (defaults to the folder name).
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = dragonslayer_core::project::DEFAULT_FPS)]
        fps: u32,
    },
    /// Add, rename, reorder, delete or list scenes.
    Scene {
        #[command(subcommand)]
        action: SceneCmd,
    },
    /// List detected cameras and what they support.
    Cameras,
    /// Show the camera's exposure settings, or change one.
    Settings {
        /// Setting to change; omit to list all.
        setting: Option<SettingArg>,
        /// New value, exactly as listed.
        value: Option<String>,
        /// Camera port from `dragonslayer cameras` (defaults to the first camera found).
        #[arg(long)]
        camera: Option<String>,
    },
    /// Capture frames into a scene (which becomes the active scene).
    Capture {
        project: PathBuf,
        scene: String,
        /// Number of frames to capture.
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// Seconds between captures when --count > 1.
        #[arg(long, default_value_t = 0.0)]
        interval: f64,
        /// Camera port from `dragonslayer cameras` (defaults to the first camera found).
        #[arg(long)]
        camera: Option<String>,
    },
    /// Move the last frame of a scene to its trash.
    DeleteLast { project: PathBuf, scene: String },
    /// Compile one scene, or the whole project, to video.
    Compile {
        project: PathBuf,
        scene: Option<String>,
        #[arg(long, value_enum, default_value_t = FormatArg::H264)]
        format: FormatArg,
        #[arg(long, value_enum, default_value_t = ResArg::Source)]
        resolution: ResArg,
        /// Crop to fill the frame instead of fitting with bars.
        #[arg(long)]
        crop: bool,
        /// Override the playback frame rate for this compile.
        /// Without this, the project fps (and per-scene overrides) are used.
        #[arg(long)]
        fps: Option<u32>,
    },
}

#[derive(Subcommand)]
enum SceneCmd {
    /// List scenes in order.
    List { project: PathBuf },
    /// Add a scene (at the end, or after another scene) and make it active.
    Add {
        project: PathBuf,
        name: String,
        #[arg(long)]
        after: Option<String>,
    },
    Rename { project: PathBuf, scene: String, name: String },
    /// Move a scene to a 1-based position.
    Move { project: PathBuf, scene: String, position: usize },
    /// Move a scene's folder to the project trash.
    Delete { project: PathBuf, scene: String },
    /// Set a scene's frame rate, or `project` to use the project rate.
    Fps { project: PathBuf, scene: String, fps: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum SettingArg {
    Aperture,
    Shutter,
    Iso,
    Wb,
    Format,
}

impl From<SettingArg> for SettingKind {
    fn from(a: SettingArg) -> Self {
        match a {
            SettingArg::Aperture => SettingKind::Aperture,
            SettingArg::Shutter => SettingKind::Shutter,
            SettingArg::Iso => SettingKind::Iso,
            SettingArg::Wb => SettingKind::WhiteBalance,
            SettingArg::Format => SettingKind::ImageFormat,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    H264,
    Prores,
}

#[derive(Clone, Copy, ValueEnum)]
enum ResArg {
    Source,
    #[value(name = "4k")]
    Uhd,
    #[value(name = "1080p")]
    Hd,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let mock = cli.mock;
    match cli.command {
        Cmd::New { project, name, fps } => {
            let name = name.unwrap_or_else(|| folder_name(&project));
            let p = Project::create(&project, &name, fps)?;
            println!("Created {:?} at {} ({} fps)", p.name(), project.display(), fps);
        }
        Cmd::Scene { action } => scene(action)?,
        Cmd::Cameras => cameras(&*backend(mock)?)?,
        Cmd::Settings { setting, value, camera } => {
            settings_cmd(&*backend(mock)?, setting.map(Into::into), value.as_deref(), camera.as_deref())?
        }
        Cmd::Capture { project, scene, count, interval, camera } => {
            capture_cmd(&*backend(mock)?, &project, &scene, count, interval, camera.as_deref())?
        }
        Cmd::DeleteLast { project, scene } => {
            let p = open(&project)?;
            let s = p.find_scene(&scene)?;
            let frame = s.delete_last()?;
            println!("Moved frame {frame} of {} to trash ({} left)", s.name(), s.frame_count()?);
        }
        Cmd::Compile { project, scene, format, resolution, crop, fps } => {
            let p = open(&project)?;
            let settings = Settings {
                format: match format {
                    FormatArg::H264 => Format::H264,
                    FormatArg::Prores => Format::ProRes,
                },
                resolution: match resolution {
                    ResArg::Source => Resolution::Source,
                    ResArg::Uhd => Resolution::Uhd,
                    ResArg::Hd => Resolution::Hd,
                },
                framing: if crop { Framing::Crop } else { Framing::Fit },
                fps_override: fps,
                ffmpeg: None,
            };
            let out = compile::compile(&p, scene.as_deref(), &settings)?;
            for w in &out.warnings {
                eprintln!("warning: {w}");
            }
            println!("Wrote {} ({} frames)", out.path.display(), out.frames);
        }
    }
    Ok(())
}

fn scene(action: SceneCmd) -> Result<()> {
    match action {
        SceneCmd::List { project } => {
            let p = open(&project)?;
            println!("{} ({} fps)", p.name(), p.file.fps);
            for (i, s) in p.scenes()?.iter().enumerate() {
                let active = if p.file.active_scene.as_deref() == Some(s.id()) { "*" } else { " " };
                let fps = s.file.fps.map(|f| format!(", {f} fps")).unwrap_or_default();
                println!("{active} {:>2}. {}  {}  ({} frames{fps})", i + 1, s.id(), s.name(), s.frame_count()?);
            }
        }
        SceneCmd::Add { project, name, after } => {
            let mut p = open(&project)?;
            let after = after.map(|a| p.find_scene(&a).map(|s| s.id().to_owned())).transpose()?;
            let s = p.add_scene(&name, after.as_deref())?;
            println!("Added {} {:?} (active)", s.id(), s.name());
        }
        SceneCmd::Rename { project, scene, name } => {
            let mut p = open(&project)?;
            let id = p.find_scene(&scene)?.id().to_owned();
            p.rename_scene(&id, &name)?;
            println!("Renamed {id} to {name:?}");
        }
        SceneCmd::Move { project, scene, position } => {
            let mut p = open(&project)?;
            let id = p.find_scene(&scene)?.id().to_owned();
            p.move_scene(&id, position.saturating_sub(1))?;
            println!("Moved {id} to position {}", p.file.scenes.iter().position(|s| *s == id).unwrap() + 1);
        }
        SceneCmd::Delete { project, scene } => {
            let mut p = open(&project)?;
            let id = p.find_scene(&scene)?.id().to_owned();
            p.delete_scene(&id)?;
            println!("Moved scene {id} to the project trash");
        }
        SceneCmd::Fps { project, scene, fps } => {
            let mut p = open(&project)?;
            let id = p.find_scene(&scene)?.id().to_owned();
            let fps = match fps.as_str() {
                "project" | "none" => None,
                n => Some(n.parse::<u32>().context("fps must be a number or `project`")?),
            };
            p.set_scene_fps(&id, fps)?;
            println!("{id}: {}", fps.map_or("uses project fps".into(), |f| format!("{f} fps")));
        }
    }
    Ok(())
}

fn cameras(backend: &dyn CameraBackend) -> Result<()> {
    let devices = backend.enumerate()?;
    if devices.is_empty() {
        println!("No cameras found.");
        print_connect_help();
        return Ok(());
    }
    for d in devices {
        print!("{}  [{}]", d.display_name(), d.port);
        match backend.open(&d) {
            Ok(cam) => {
                let c = cam.capabilities();
                let yn = |b: bool| if b { "yes" } else { "no" };
                println!();
                println!("  live view: {}  capture: {}  download: {}", yn(c.live_view), yn(c.capture), yn(c.download));
                if !c.usable() {
                    println!("  This camera can't be used (needs capture and download). See CAMERAS.md.");
                } else if !c.live_view {
                    println!("  No live view on this model: onion skin will run against the last frame.");
                }
                cam.close().ok();
            }
            Err(e) => println!("\n  could not open: {e}"),
        }
    }
    Ok(())
}

fn settings_cmd(
    backend: &dyn CameraBackend,
    setting: Option<SettingKind>,
    value: Option<&str>,
    port: Option<&str>,
) -> Result<()> {
    let devices = backend.enumerate()?;
    let device = match port {
        Some(port) => devices.into_iter().find(|d| d.port == port),
        None => devices.into_iter().next(),
    };
    let Some(device) = device else {
        print_connect_help();
        bail!("no camera found");
    };
    let mut cam = backend.open(&device)?;
    let result = (|| {
        match (setting, value) {
            (Some(kind), Some(value)) => {
                cam.set_setting(kind, value)?;
                let now = cam.settings()?.into_iter().find(|s| s.kind == kind);
                println!("{}: {}", kind.label(), now.map_or("(unknown)".into(), |s| s.value));
            }
            (Some(kind), None) => {
                let s = cam
                    .settings()?
                    .into_iter()
                    .find(|s| s.kind == kind)
                    .with_context(|| format!("{} doesn't report {}", device.display_name(), kind.label()))?;
                println!("{}: {}{}", kind.label(), s.value, if s.readonly { "  (read-only)" } else { "" });
                println!("  choices: {}", s.choices.join(", "));
            }
            (None, _) => {
                let all = cam.settings()?;
                if all.is_empty() {
                    println!("{} doesn't report any adjustable settings.", device.display_name());
                }
                for s in all {
                    let ro = if s.readonly { "  (read-only in this mode)" } else { "" };
                    println!("{:<14} {}{ro}", s.kind.label(), s.value);
                }
            }
        }
        Ok(())
    })();
    cam.close().ok();
    result
}

fn capture_cmd(
    backend: &dyn CameraBackend,
    project: &Path,
    scene: &str,
    count: u32,
    interval: f64,
    port: Option<&str>,
) -> Result<()> {
    let mut p = open(project)?;
    let id = p.find_scene(scene)?.id().to_owned();
    p.set_active(&id)?;

    let devices = backend.enumerate()?;
    let device = match port {
        Some(port) => devices.into_iter().find(|d| d.port == port),
        None => devices.into_iter().next(),
    };
    let Some(device) = device else {
        print_connect_help();
        bail!("no camera found");
    };
    let mut cam = backend.open(&device)?;
    if !cam.capabilities().usable() {
        bail!("{} can't capture and download; see CAMERAS.md", device.display_name());
    }
    let name = device.display_name();

    let mut warned_raw = false;
    for i in 0..count {
        if i > 0 && interval > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(interval));
        }
        let shot = capture::capture(&p, Some(&name), |dir| {
            dragonslayer_camera::shoot(cam.as_mut(), dir)
                .map(|files| files.into_iter().map(|f| f.path).collect())
                .map_err(|e| e.to_string())
        })?;
        let has_raw = shot.files.iter().any(|f| FileKind::of(f) == FileKind::Raw);
        if !has_raw && !warned_raw {
            eprintln!("warning: camera returned no RAW file; only the JPEG is kept (set RAW+JPEG on the camera to keep RAW)");
            warned_raw = true;
        }
        println!("{}: frame {} ({} files)", shot.scene, shot.frame, shot.files.len());
    }
    cam.close().ok();
    Ok(())
}

fn open(path: &Path) -> Result<Project> {
    let p = Project::open(path).with_context(|| format!("opening project {}", path.display()))?;
    let report = p.recover()?;
    for (scene, frame) in &report.recovered {
        eprintln!("recovered interrupted capture: {scene} frame {frame}");
    }
    for (scene, frame) in &report.abandoned {
        eprintln!("interrupted capture {scene} frame {frame} had no files; it may still be on the camera card");
    }
    Ok(p)
}

fn backend(mock: bool) -> Result<Box<dyn CameraBackend>> {
    if mock {
        return Ok(Box::new(MockBackend::default()));
    }
    dragonslayer_camera::default_backend().context(
        "this build has no camera support; rebuild with `--features gphoto2` (see README) or use --mock",
    )
}

fn print_connect_help() {
    if cfg!(windows) {
        println!("On Windows the camera must use the WinUSB driver: run Zadig once (see README, \"Windows: Zadig\").");
    } else if cfg!(target_os = "macos") {
        println!("On macOS, quit Image Capture and Photos, then run `killall ptpcamerad` and reconnect the camera.");
    }
    println!("Check the camera is on, in PTP/PC mode, and connected by USB. Supported models: http://www.gphoto.org/proj/libgphoto2/support.php");
}

fn folder_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into())
}

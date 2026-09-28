//! Reference audio: a sound file per scene, copied into the project, placed in the film
//! at compile time.

use std::fs;
use std::path::{Path, PathBuf};

use dragonslayer_core::compile::{self, AudioClip, Format, Settings};
use dragonslayer_core::{capture, Project};

fn project(dir: &Path) -> Project {
    Project::create(dir.join("Film"), "Film", 12).unwrap()
}

fn shoot(p: &Project, n: usize) {
    for _ in 0..n {
        capture::capture(p, None, |dir| {
            let f = dir.join("IMG.JPG");
            fs::write(&f, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
            Ok(vec![f])
        })
        .unwrap();
    }
}

fn sound(dir: &Path, name: &str, bytes: usize) -> PathBuf {
    let f = dir.join(name);
    fs::write(&f, vec![0u8; bytes]).unwrap();
    f
}

#[test]
fn audio_is_copied_into_the_project_and_saved_with_the_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    let song = sound(tmp.path(), "song.wav", 100);

    p.set_scene_audio("sc010", Some(&song)).unwrap();
    let copy = p.root.join("audio/song.wav");
    assert!(copy.exists(), "copied into audio/");
    assert_eq!(p.audio_for(&p.scene("sc010").unwrap()), Some((copy.clone(), 0.0)));

    p.set_audio_start("sc010", 1500).unwrap();
    let reopened = Project::open(&p.root).unwrap();
    assert_eq!(reopened.audio_for(&reopened.scene("sc010").unwrap()), Some((copy.clone(), 1.5)));

    // The same file again is reused; a different one with the same name gets its own copy.
    p.set_scene_audio("sc010", Some(&song)).unwrap();
    p.set_scene_audio("sc010", Some(&copy)).unwrap();
    assert_eq!(fs::read_dir(p.root.join("audio")).unwrap().count(), 1);
    let other = sound(&tmp.path().join("Film"), "song.wav", 7);
    p.set_scene_audio("sc010", Some(&other)).unwrap();
    assert_eq!(fs::read_dir(p.root.join("audio")).unwrap().count(), 2);

    // Removing leaves the file in audio/.
    p.set_scene_audio("sc010", None).unwrap();
    assert_eq!(p.audio_for(&p.scene("sc010").unwrap()), None);
    assert!(copy.exists());
}

#[test]
fn takes_use_their_scenes_audio() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    let take = p.add_take("sc010").unwrap();
    // Set through the take: it lands on the scene.
    p.set_scene_audio(take.id(), Some(&sound(tmp.path(), "line.mp3", 10))).unwrap();
    p.set_audio_start(take.id(), 250).unwrap();
    assert!(p.scene("sc010").unwrap().file.audio.is_some());
    assert!(p.scene(take.id()).unwrap().file.audio.is_none());
    let expect = Some((p.root.join("audio/line.mp3"), 0.25));
    assert_eq!(p.audio_for(&p.scene(take.id()).unwrap()), expect);
    assert_eq!(p.audio_for(&p.scene("sc010").unwrap()), expect);
}

#[test]
fn old_scene_files_without_audio_still_load() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let file = p.scene("sc010").unwrap().dir.join("scene.json");
    let text = fs::read_to_string(&file).unwrap();
    assert!(!text.contains("audio"), "nothing written when there is no audio: {text}");
    fs::write(&file, r#"{"id":"sc010","name":"Scene 1","fps":null}"#).unwrap();
    assert!(p.scene("sc010").unwrap().file.audio.is_none());
}

#[test]
fn each_scenes_audio_is_placed_where_the_scene_plays_in_the_film() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    shoot(&p, 3); // 3 frames at 12 fps = 0.25 s
    p.add_scene("Two", None).unwrap();
    p.set_scene_fps("sc020", Some(6)).unwrap();
    shoot(&p, 4); // 4 frames at 6 fps
    p.set_scene_audio("sc020", Some(&sound(tmp.path(), "talk.wav", 10))).unwrap();
    p.set_audio_start("sc020", 1500).unwrap();

    let (_, _, clips) = compile::plan_with_audio(&p, None, None, None).unwrap();
    let path = p.root.join("audio/talk.wav");
    assert_eq!(clips, [AudioClip { path: path.clone(), from: 1.5, at: 0.25, seconds: 4.0 / 6.0 }]);

    // Just frames 2–3 of the scene: the sound starts one frame (1/6 s) later, at 0.
    let (_, _, clips) = compile::plan_with_audio(&p, Some("sc020"), None, Some((1, 2))).unwrap();
    assert_eq!(clips.len(), 1);
    assert!((clips[0].from - (1.5 + 1.0 / 6.0)).abs() < 1e-9, "{clips:?}");
    assert_eq!((clips[0].at, clips[0].seconds), (0.0, 2.0 / 6.0));

    // A scene without audio has no clip.
    let (_, _, clips) = compile::plan_with_audio(&p, Some("sc010"), None, None).unwrap();
    assert!(clips.is_empty());
}

#[test]
fn ffmpeg_mixes_the_clips_into_one_track() {
    let clips = [
        AudioClip { path: "a.wav".into(), from: 1.5, at: 0.0, seconds: 2.0 },
        AudioClip { path: "b.mp3".into(), from: 0.0, at: 2.0, seconds: 1.0 },
    ];
    let (list, out) = (Path::new("list.txt"), Path::new("out.mp4"));
    let args = compile::ffmpeg_args_with_audio(list, out, 12, &Settings::default(), &clips);
    let joined = args.join(" ");
    assert!(!args.contains(&"-vf".to_string()), "video filters move into the graph: {joined}");
    let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
    assert!(graph.starts_with("[0:v]scale="), "{graph}");
    assert!(graph.contains("[1:a]atrim=start=1.500000:duration=2.000000,asetpts=PTS-STARTPTS,adelay=0:all=1[a0]"), "{graph}");
    assert!(graph.contains("[2:a]atrim=start=0.000000:duration=1.000000,asetpts=PTS-STARTPTS,adelay=2000:all=1[a1]"), "{graph}");
    assert!(graph.ends_with("[a0][a1]amix=inputs=2:normalize=0:duration=longest[a]"), "{graph}");
    assert!(joined.contains("-i a.wav -i b.mp3"), "{joined}");
    assert!(joined.contains("-map [v] -map [a]"), "{joined}");
    assert!(joined.contains("-c:a aac"), "{joined}");
    assert_eq!(args.last().unwrap(), "out.mp4");

    let prores = compile::ffmpeg_args_with_audio(list, out, 12, &Settings { format: Format::ProRes, ..Settings::default() }, &clips);
    assert!(prores.join(" ").contains("-c:a pcm_s16le"));

    // No clips: exactly the video-only arguments.
    assert_eq!(compile::ffmpeg_args_with_audio(list, out, 12, &Settings::default(), &[]), compile::ffmpeg_args(list, out, 12, &Settings::default()));
}

/// Real ffmpeg, if it is installed: the film comes out with a sound track.
#[test]
fn compiled_film_has_the_audio_when_ffmpeg_is_available() {
    let has = |bin: &str| std::process::Command::new(bin).arg("-version").output().is_ok_and(|o| o.status.success());
    if !has("ffmpeg") || !has("ffprobe") {
        eprintln!("ffmpeg/ffprobe not found; skipped");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    // Real JPEGs this time: ffmpeg decodes them.
    let jpeg = tmp.path().join("red.jpg");
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=red:s=64x48", "-frames:v", "1"])
        .arg(&jpeg)
        .status()
        .unwrap();
    assert!(made.success());
    for _ in 0..6 {
        capture::capture(&p, None, |dir| {
            let f = dir.join("IMG.JPG");
            fs::copy(&jpeg, &f).unwrap();
            Ok(vec![f])
        })
        .unwrap();
    }
    let wav = tmp.path().join("tone.wav");
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"])
        .arg(&wav)
        .status()
        .unwrap();
    assert!(made.success());
    p.set_scene_audio("sc010", Some(&wav)).unwrap();
    p.set_audio_start("sc010", 1000).unwrap();

    let out = compile::compile(&p, Some("sc010"), &Settings { audio: true, ..Settings::default() }).unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(&out.path)
        .output()
        .unwrap();
    let streams = String::from_utf8_lossy(&probe.stdout);
    assert!(streams.lines().any(|l| l.trim() == "audio"), "{streams}");
    assert!(streams.lines().any(|l| l.trim() == "video"), "{streams}");

    let silent = compile::compile(&p, Some("sc010"), &Settings::default()).unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(&silent.path)
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&probe.stdout).contains("audio"), "audio off: video only");
}

//! Property tests: compile helpers never panic and keep their shape on any input.

use std::path::PathBuf;

use dragonslayer_core::compile::{self, Format, Framing, Resolution, Settings, Shot};
use proptest::prelude::*;

fn shot() -> impl Strategy<Value = Shot> {
    // Any printable path (quotes, backslashes, unicode). Newlines are excluded: a path can't
    // contain one on Windows, and project folders never do.
    ("[^\\n\\r]{1,40}", 0.0001f64..10.0).prop_map(|(p, seconds)| Shot { path: PathBuf::from(p), seconds })
}

fn settings() -> impl Strategy<Value = Settings> {
    (
        prop_oneof![Just(Format::H264), Just(Format::ProRes)],
        prop_oneof![Just(Resolution::Source), Just(Resolution::Uhd), Just(Resolution::Hd)],
        prop_oneof![Just(Framing::Fit), Just(Framing::Crop)],
        proptest::option::of(1u32..240),
    )
        .prop_map(|(format, resolution, framing, fps_override)| Settings {
            format,
            resolution,
            framing,
            fps_override,
            frames: None,
            audio: false,
            ffmpeg: None,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn concat_list_has_one_entry_per_shot_plus_the_repeat(shots in proptest::collection::vec(shot(), 0..40)) {
        let list = compile::concat_list(&shots);
        prop_assert!(list.starts_with("ffconcat version 1.0\n"));
        let files = list.lines().filter(|l| l.starts_with("file '")).count();
        let durations = list.lines().filter(|l| l.starts_with("duration ")).count();
        prop_assert_eq!(files, if shots.is_empty() { 0 } else { shots.len() + 1 });
        prop_assert_eq!(durations, shots.len());
        // Every quoted path is closed: inside, ' only appears as the escape '\'' and a
        // backslash only as part of it (path backslashes become forward slashes).
        for l in list.lines().filter(|l| l.starts_with("file '")) {
            prop_assert!(l.len() > "file '".len() && l.ends_with('\''), "unterminated: {l}");
            let inner = l["file '".len()..l.len() - 1].replace("'\\''", "");
            prop_assert!(!inner.contains('\''), "stray quote: {l}");
            prop_assert!(!inner.contains('\\'), "stray backslash: {l}");
        }
    }

    #[test]
    fn progress_is_none_or_within_0_and_1(line in "\\PC{0,40}", total in any::<f64>()) {
        if let Some(p) = compile::progress_from_line(&line, total) {
            prop_assert!((0.0..=1.0).contains(&p), "{line:?} / {total} -> {p}");
        }
    }

    #[test]
    fn progress_on_real_looking_lines(
        key in prop_oneof![Just("out_time_us"), Just("out_time_ms"), Just("frame"), Just("progress")],
        value in prop_oneof![
            any::<i64>().prop_map(|v| v.to_string()),
            Just("N/A".to_string()),
            Just("NaN".to_string()),
            Just("inf".to_string()),
            Just("-inf".to_string()),
            Just("end".to_string()),
            Just("continue".to_string()),
        ],
        total in prop_oneof![Just(0.0), Just(-1.0), Just(f64::NAN), Just(f64::INFINITY), 0.001f64..10_000.0],
    ) {
        let r = compile::progress_from_line(&format!("{key}={value}"), total);
        if let Some(p) = r {
            prop_assert!((0.0..=1.0).contains(&p), "{key}={value} / {total} -> {p}");
        }
        if key == "progress" && value == "end" {
            prop_assert_eq!(r, Some(1.0));
        }
    }

    #[test]
    fn ffmpeg_args_always_well_formed(s in settings(), fps in 1u32..240, list in "[^\\n]{1,30}", out in "[^\\n]{1,30}") {
        let args = compile::ffmpeg_args(&PathBuf::from(&list), &PathBuf::from(&out), fps, &s);
        prop_assert_eq!(args.last().map(String::as_str), Some(out.as_str()), "output path is last");
        let i = args.iter().position(|a| a == "-i").unwrap();
        prop_assert_eq!(&args[i + 1], &list);
        let vf = &args[args.iter().position(|a| a == "-vf").unwrap() + 1];
        let expected_fps = format!("fps={fps}");
        prop_assert!(vf.contains(&expected_fps));
        prop_assert!(args.iter().any(|a| a == "-n"), "never overwrite an existing video");
        let codec = &args[args.iter().position(|a| a == "-c:v").unwrap() + 1];
        prop_assert_eq!(codec.as_str(), match s.format { Format::H264 => "libx264", Format::ProRes => "prores_ks" });
    }
}

/// Regression: "NaN" parses as an f64 and NaN survives clamp, so a garbled progress line
/// produced Some(NaN) and a NaN progress bar.
#[test]
fn nan_progress_is_ignored() {
    assert_eq!(compile::progress_from_line("out_time_us=NaN", 10.0), None);
    assert_eq!(compile::progress_from_line("out_time_us=inf", 10.0), None);
    assert_eq!(compile::progress_from_line("out_time_us=5000000", f64::INFINITY), Some(0.0));
}

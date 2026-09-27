//! Property tests: random capture / fail / crash / delete / recover sequences on a real
//! project, checked against a model after every step.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use dragonslayer_core::{capture, Project};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    /// Camera returns files; `raw` adds a RAW beside the JPEG.
    CaptureOk { raw: bool },
    /// Trigger failed before anything was downloaded.
    CaptureFailsEmpty,
    /// Files reached the incoming folder, then the app "crashed" before committing.
    CaptureCrashes,
    DeleteLast,
    /// Close and reopen the project, which runs recovery.
    ReopenAndRecover,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => any::<bool>().prop_map(|raw| Op::CaptureOk { raw }),
        1 => Just(Op::CaptureFailsEmpty),
        2 => Just(Op::CaptureCrashes),
        2 => Just(Op::DeleteLast),
        1 => Just(Op::ReopenAndRecover),
    ]
}

fn shoot(dir: &Path, raw: bool) -> Vec<PathBuf> {
    let jpg = dir.join("IMG_0001.JPG");
    fs::write(&jpg, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
    let mut out = vec![jpg];
    if raw {
        let r = dir.join("IMG_0001.CR2");
        fs::write(&r, b"raw").unwrap();
        out.push(r);
    }
    out
}

#[derive(Default, Debug)]
struct Model {
    frames: Vec<u64>,
    pending: BTreeSet<u64>,
    deleted: Vec<u64>,
    /// Every number ever handed out, committed or not.
    used: BTreeSet<u64>,
}

fn id(n: u64) -> String {
    format!("{n:06}")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn capture_sequences_keep_every_invariant(ops in proptest::collection::vec(op(), 1..18)) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Film");
        let mut p = Project::create(&root, "Film", 12).unwrap();
        let mut m = Model::default();

        for op in &ops {
            let scene = p.active_scene().unwrap();
            let next: u64 = scene.next_frame_id().unwrap().parse().unwrap();
            // A new number is always above every number ever used, so nothing is reused.
            prop_assert!(m.used.iter().all(|u| *u < next), "next {next} reuses one of {:?}", m.used);

            match op {
                Op::CaptureOk { raw } => {
                    let c = capture::capture(&p, None, |dir| Ok(shoot(dir, *raw))).unwrap();
                    prop_assert_eq!(&c.frame, &id(next));
                    prop_assert_eq!(c.files.len(), if *raw { 2 } else { 1 });
                    m.used.insert(next);
                    m.frames.push(next);
                }
                Op::CaptureFailsEmpty => {
                    prop_assert!(capture::capture(&p, None, |_| Err("busy".into())).is_err());
                    m.used.insert(next);
                }
                Op::CaptureCrashes => {
                    let r = capture::capture(&p, None, |dir| {
                        shoot(dir, true);
                        Err("crash".into())
                    });
                    prop_assert!(r.is_err());
                    m.used.insert(next);
                    m.pending.insert(next);
                }
                Op::DeleteLast => match m.frames.pop() {
                    Some(last) => {
                        prop_assert_eq!(scene.delete_last().unwrap(), id(last));
                        m.deleted.push(last);
                    }
                    None => prop_assert!(scene.delete_last().is_err()),
                },
                Op::ReopenAndRecover => {
                    p = Project::open(&root).unwrap();
                    let report = p.recover().unwrap();
                    let recovered: Vec<u64> = report.recovered.iter().map(|(_, f)| f.parse().unwrap()).collect();
                    // Recovery commits pending frames in number order, after the existing ones.
                    let expected: Vec<u64> = m.pending.iter().copied().collect();
                    prop_assert_eq!(recovered, expected.clone());
                    prop_assert!(report.abandoned.is_empty());
                    m.frames.extend(expected);
                    m.pending.clear();
                }
            }

            // Invariants after every step.
            let scene = p.active_scene().unwrap();
            let frames = scene.frames().unwrap();
            let ids: Vec<u64> = frames.iter().map(|f| f.id.parse().unwrap()).collect();
            prop_assert_eq!(&ids, &m.frames, "frame order");
            prop_assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len(), "a frame appears twice");
            prop_assert_eq!(scene.frame_count().unwrap(), m.frames.len());
            for f in &frames {
                prop_assert!(!f.files.is_empty(), "frame {} has no files", f.id);
                prop_assert!(f.files.iter().all(|x| x.is_file()));
                prop_assert!(f.jpeg().is_some());
            }
            let pending: BTreeSet<u64> = scene.pending().unwrap().iter().map(|f| f.parse().unwrap()).collect();
            prop_assert_eq!(&pending, &m.pending);
            // Deleted frames are in the scene's trash, never destroyed.
            for d in &m.deleted {
                prop_assert!(scene.dir.join("trash").join(format!("{}.jpg", id(*d))).is_file(), "frame {d} not in trash");
                prop_assert!(!ids.contains(d));
            }
        }

        // A final recovery always leaves nothing pending.
        let p = Project::open(&root).unwrap();
        p.recover().unwrap();
        prop_assert!(p.active_scene().unwrap().pending().unwrap().is_empty());
    }
}

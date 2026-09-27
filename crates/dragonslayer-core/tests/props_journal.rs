//! Property tests: journal replay against a reference model, and torn/garbage lines.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;

use dragonslayer_core::journal::{self, JournalEntry, JournalOp};
use dragonslayer_core::Project;
use proptest::prelude::*;

fn op() -> impl Strategy<Value = JournalOp> {
    prop_oneof![
        Just(JournalOp::Pending),
        Just(JournalOp::Capture),
        Just(JournalOp::Abandon),
        Just(JournalOp::Delete),
    ]
}

/// Small id space so ops collide on the same frame often.
fn entry() -> impl Strategy<Value = (JournalOp, u8, Option<u8>)> {
    (op(), 1u8..=6, proptest::option::of(0u8..3))
}

fn frame_id(n: u8) -> String {
    format!("{n:06}")
}

/// Reference replay: the documented semantics, written independently.
fn model(entries: &[(JournalOp, u8, Option<u8>)]) -> (Vec<(String, Option<String>)>, BTreeSet<String>) {
    let mut frames: Vec<(String, Option<String>)> = Vec::new();
    let mut pending = BTreeSet::new();
    for (op, n, cam) in entries {
        let id = frame_id(*n);
        let cam = cam.map(|c| format!("cam{c}"));
        match op {
            JournalOp::Pending => {
                pending.insert(id);
            }
            JournalOp::Capture => {
                pending.remove(&id);
                if !frames.iter().any(|(f, _)| *f == id) {
                    frames.push((id, cam));
                }
            }
            JournalOp::Abandon => {
                pending.remove(&id);
            }
            JournalOp::Delete => frames.retain(|(f, _)| *f != id),
        }
    }
    (frames, pending)
}

/// Lines that must never parse as an entry.
fn garbage() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Arbitrary bytes, including invalid UTF-8 (a torn multi-byte character).
        proptest::collection::vec(any::<u8>(), 0..40).prop_map(|mut v| {
            v.retain(|b| *b != b'\n');
            v
        }),
        // A real entry cut short, as a crash mid-append leaves it.
        (1usize..60).prop_map(|cut| {
            let full = serde_json::to_string(&JournalEntry::now(JournalOp::Capture, "000009", Some("Canon ÉOS".into())))
                .unwrap();
            let bytes = full.into_bytes();
            bytes[..cut.min(bytes.len() - 1)].to_vec()
        }),
        Just(b"{}".to_vec()),
        Just(b"   ".to_vec()),
        Just(b"\r".to_vec()),
        Just(b"null".to_vec()),
        Just(br#"{"op":"capture"}"#.to_vec()),
    ]
}

proptest! {
    // Every append fsyncs, which is slow on Windows: keep filesystem-heavy case counts modest.
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn replay_matches_reference_model(entries in proptest::collection::vec(entry(), 0..25)) {
        let tmp = tempfile::tempdir().unwrap();
        let p = Project::create(tmp.path().join("F"), "F", 12).unwrap();
        let scene = p.active_scene().unwrap();
        for (op, n, cam) in &entries {
            let cam = cam.map(|c| format!("cam{c}"));
            journal::append(&scene.journal_path(), &JournalEntry::now(*op, frame_id(*n), cam)).unwrap();
        }
        let (frames, pending) = model(&entries);
        let got: Vec<(String, Option<String>)> =
            scene.frames().unwrap().into_iter().map(|f| (f.id, f.camera)).collect();
        prop_assert_eq!(got, frames.clone());
        prop_assert_eq!(scene.frame_count().unwrap(), frames.len());
        prop_assert_eq!(scene.pending().unwrap().into_iter().collect::<BTreeSet<_>>(), pending);
        // Numbers are never reused: the next id is above every id ever journalled.
        let max = entries.iter().map(|(_, n, _)| *n as u64).max().unwrap_or(0);
        prop_assert_eq!(scene.next_frame_id().unwrap(), format!("{:06}", max + 1));
    }

    #[test]
    fn garbage_lines_anywhere_never_lose_good_entries(
        items in proptest::collection::vec(
            prop_oneof![entry().prop_map(Ok), garbage().prop_map(Err)],
            0..30,
        ),
        crlf in any::<bool>(),
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("journal.ndjson");
        let mut bytes = Vec::new();
        let mut good = Vec::new();
        for item in &items {
            match item {
                Ok((op, n, cam)) => {
                    let e = JournalEntry::now(*op, frame_id(*n), cam.map(|c| format!("cam{c}")));
                    bytes.extend_from_slice(serde_json::to_string(&e).unwrap().as_bytes());
                    good.push(e);
                }
                Err(g) => bytes.extend_from_slice(g),
            }
            bytes.extend_from_slice(if crlf { b"\r\n" } else { b"\n" });
        }
        fs::write(&path, &bytes).unwrap();
        let read = journal::read(&path);
        prop_assert!(read.is_ok(), "read failed: {:?}", read.err());
        prop_assert_eq!(read.unwrap(), good);
    }

    #[test]
    fn append_after_a_torn_tail_stays_readable(
        before in proptest::collection::vec(entry(), 0..6),
        tail in garbage(),
        after in proptest::collection::vec(entry(), 1..6),
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("journal.ndjson");
        let mut expected = Vec::new();
        for (op, n, _) in &before {
            let e = JournalEntry::now(*op, frame_id(*n), None);
            journal::append(&path, &e).unwrap();
            expected.push(e);
        }
        // Crash mid-append: a partial line with no newline.
        fs::OpenOptions::new().create(true).append(true).open(&path).unwrap().write_all(&tail).unwrap();
        for (op, n, _) in &after {
            let e = JournalEntry::now(*op, frame_id(*n), None);
            journal::append(&path, &e).unwrap();
            expected.push(e);
        }
        let read = journal::read(&path);
        prop_assert!(read.is_ok(), "read failed: {:?}", read.err());
        prop_assert_eq!(read.unwrap(), expected);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Same garbage property without fsync-ing appends, so it can run many more cases.
    #[test]
    fn garbage_lines_many_cases(
        items in proptest::collection::vec(
            prop_oneof![entry().prop_map(Ok), garbage().prop_map(Err)],
            0..30,
        ),
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("journal.ndjson");
        let mut bytes = Vec::new();
        let mut good = Vec::new();
        for item in &items {
            match item {
                Ok((op, n, cam)) => {
                    let e = JournalEntry::now(*op, frame_id(*n), cam.map(|c| format!("cam{c}")));
                    bytes.extend_from_slice(serde_json::to_string(&e).unwrap().as_bytes());
                    good.push(e);
                }
                Err(g) => bytes.extend_from_slice(g),
            }
            bytes.push(b'\n');
        }
        fs::write(&path, &bytes).unwrap();
        prop_assert_eq!(journal::read(&path).unwrap(), good);
    }
}

/// Regression: a crash mid-append that cut a non-ASCII camera name in half left invalid
/// UTF-8 in the journal, and journal::read failed outright, making the scene unreadable.
#[test]
fn torn_multibyte_character_does_not_make_the_journal_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("journal.ndjson");
    let e1 = JournalEntry::now(JournalOp::Capture, "000001", Some("Canon ÉOS".into()));
    journal::append(&path, &e1).unwrap();
    let full = serde_json::to_string(&JournalEntry::now(JournalOp::Capture, "000002", Some("É".into()))).unwrap();
    let cut = full.find('É').unwrap() + 1; // inside the 2-byte UTF-8 sequence
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(&full.as_bytes()[..cut]).unwrap();
    let e3 = JournalEntry::now(JournalOp::Capture, "000003", None);
    journal::append(&path, &e3).unwrap();
    assert_eq!(journal::read(&path).unwrap(), vec![e1, e3]);
}

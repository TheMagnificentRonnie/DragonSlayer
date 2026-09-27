//! Property tests: random scene edits against a model; project.json always reloads identically.

use std::collections::HashSet;

use dragonslayer_core::{Error, Project};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    AddEnd(String),
    /// Index into the current scene list (modulo its length).
    AddAfter(usize, String),
    Delete(usize),
    Move(usize, usize),
    Rename(usize, String),
    SetActive(usize),
    SetFps(usize, Option<u32>),
    Reopen,
}

fn name() -> impl Strategy<Value = String> {
    prop_oneof!["[A-Za-z ]{1,12}", "\\PC{1,8}", Just("Scene \"quoted\" / back\\slash".to_string())]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => name().prop_map(Op::AddEnd),
        3 => (any::<usize>(), name()).prop_map(|(i, n)| Op::AddAfter(i, n)),
        2 => any::<usize>().prop_map(Op::Delete),
        2 => (any::<usize>(), 0usize..12).prop_map(|(i, to)| Op::Move(i, to)),
        1 => (any::<usize>(), name()).prop_map(|(i, n)| Op::Rename(i, n)),
        1 => any::<usize>().prop_map(Op::SetActive),
        1 => (any::<usize>(), proptest::option::of(1u32..=60)).prop_map(|(i, f)| Op::SetFps(i, f)),
        1 => Just(Op::Reopen),
    ]
}

#[derive(Debug, Clone, PartialEq)]
struct SceneModel {
    id: String,
    name: String,
    fps: Option<u32>,
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn scene_edits_match_model_and_ids_are_never_reused(ops in proptest::collection::vec(op(), 1..25)) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Film");
        let mut p = Project::create(&root, "Film", 12).unwrap();
        let mut model = vec![SceneModel { id: "sc010".into(), name: "Scene 1".into(), fps: None }];
        let mut active: Option<String> = Some("sc010".into());
        let mut ever: HashSet<String> = HashSet::from(["sc010".to_string()]);

        for op in &ops {
            let pick = |i: usize| (!model.is_empty()).then(|| i % model.len());
            match op {
                Op::AddEnd(n) => {
                    let s = p.add_scene(n, None).unwrap();
                    prop_assert!(ever.insert(s.id().to_owned()), "id {} reused", s.id());
                    model.push(SceneModel { id: s.id().into(), name: n.clone(), fps: None });
                    active = Some(s.id().into());
                }
                Op::AddAfter(i, n) => {
                    let Some(i) = pick(*i) else { continue };
                    let after = model[i].id.clone();
                    let s = p.add_scene(n, Some(&after)).unwrap();
                    prop_assert!(ever.insert(s.id().to_owned()), "id {} reused", s.id());
                    model.insert(i + 1, SceneModel { id: s.id().into(), name: n.clone(), fps: None });
                    active = Some(s.id().into());
                }
                Op::Delete(i) => {
                    let Some(i) = pick(*i) else { continue };
                    let id = model[i].id.clone();
                    p.delete_scene(&id).unwrap();
                    prop_assert!(root.join("trash").join(&id).join("scene.json").is_file(), "{id} not in trash");
                    model.remove(i);
                    if active.as_deref() == Some(id.as_str()) {
                        active = model.get(i.min(model.len().saturating_sub(1))).map(|s| s.id.clone());
                    }
                }
                Op::Move(i, to) => {
                    let Some(i) = pick(*i) else { continue };
                    let s = model.remove(i);
                    p.move_scene(&s.id, *to).unwrap();
                    let to = (*to).min(model.len());
                    model.insert(to, s);
                }
                Op::Rename(i, n) => {
                    let Some(i) = pick(*i) else { continue };
                    p.rename_scene(&model[i].id.clone(), n).unwrap();
                    model[i].name = n.clone();
                }
                Op::SetActive(i) => {
                    let Some(i) = pick(*i) else { continue };
                    p.set_active(&model[i].id.clone()).unwrap();
                    active = Some(model[i].id.clone());
                }
                Op::SetFps(i, f) => {
                    let Some(i) = pick(*i) else { continue };
                    p.set_scene_fps(&model[i].id.clone(), *f).unwrap();
                    model[i].fps = *f;
                }
                Op::Reopen => {
                    p = Project::open(&root).unwrap();
                }
            }

            // Order, names and rates match the model.
            let ids: Vec<String> = model.iter().map(|s| s.id.clone()).collect();
            prop_assert_eq!(&p.file.scenes, &ids);
            for s in &model {
                let scene = p.scene(&s.id).unwrap();
                prop_assert_eq!(scene.name(), s.name.as_str());
                prop_assert_eq!(scene.file.fps, s.fps);
                prop_assert_eq!(p.fps_for(&scene), s.fps.unwrap_or(12));
            }
            prop_assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len(), "duplicate scene id");
            // Active is a live scene, or None only when there are no scenes.
            prop_assert_eq!(&p.file.active_scene, &active);
            match &active {
                Some(a) => prop_assert!(ids.contains(a)),
                None => prop_assert!(ids.is_empty()),
            }
            // What's on disk is exactly what's in memory.
            prop_assert_eq!(&Project::open(&root).unwrap().file, &p.file);
            // Unknown ids are errors, not panics.
            prop_assert!(matches!(p.scene("sc999999"), Err(Error::SceneNotFound(_))));
        }
    }
}

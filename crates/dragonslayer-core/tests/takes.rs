//! Multiple takes of a scene: shoot a move again without deleting the first attempt,
//! then choose which take goes in the film.

use std::fs;
use std::path::Path;

use dragonslayer_core::compile;
use dragonslayer_core::{capture, Project};

fn project(dir: &Path) -> Project {
    Project::create(dir.join("Film"), "Film", 12).unwrap()
}

fn shoot(p: &Project) {
    capture::capture(p, None, |dir| {
        let f = dir.join("IMG.JPG");
        fs::write(&f, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
        Ok(vec![f])
    })
    .unwrap();
}

fn stems(p: &Project, scene: Option<&str>) -> Vec<String> {
    let (shots, _) = compile::plan(p, scene, None).unwrap();
    shots
        .iter()
        .map(|s| {
            let scene = s.path.parent().unwrap().parent().unwrap().file_name().unwrap().to_string_lossy();
            format!("{scene}/{}", s.path.file_stem().unwrap().to_string_lossy())
        })
        .collect()
}

#[test]
fn a_new_take_is_empty_active_and_numbered_after_the_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.rename_scene("sc010", "Chase").unwrap();
    p.set_scene_fps("sc010", Some(15)).unwrap();
    shoot(&p);

    let t = p.add_take("sc010").unwrap();
    assert_eq!(t.id(), "sc010t2");
    assert_eq!(t.name(), "Chase · take 2");
    assert_eq!(t.file.fps, Some(15), "inherits the scene's frame rate");
    assert_eq!(p.file.active_scene.as_deref(), Some("sc010t2"));
    assert_eq!(p.take_number("sc010"), Some(1));
    assert_eq!(p.take_number("sc010t2"), Some(2));
    assert_eq!(p.take_owner("sc010t2"), Some("sc010"));
    assert_eq!(p.file.scenes, ["sc010"], "takes aren't scenes in the film order");

    // Captures now go into the take, not the scene.
    shoot(&p);
    shoot(&p);
    assert_eq!(p.scene("sc010").unwrap().frame_count().unwrap(), 1);
    assert_eq!(p.scene("sc010t2").unwrap().frame_count().unwrap(), 2);

    // A take of a take is another take of the scene.
    let t3 = p.add_take("sc010t2").unwrap();
    assert_eq!(t3.id(), "sc010t3");
    assert_eq!(p.takes_of("sc010").unwrap().len(), 2);
}

#[test]
fn the_film_uses_the_chosen_take_of_each_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    shoot(&p);
    p.add_scene("Second", None).unwrap();
    shoot(&p);
    p.set_active("sc010").unwrap();
    p.add_take("sc010").unwrap();
    shoot(&p);
    shoot(&p);

    assert_eq!(stems(&p, None), ["sc010/000001", "sc020/000001"], "take 1 until another is chosen");
    p.choose_take("sc010", Some("sc010t2")).unwrap();
    assert_eq!(stems(&p, None), ["sc010t2/000001", "sc010t2/000002", "sc020/000001"]);
    // Compiling one take on its own still works.
    assert_eq!(stems(&p, Some("sc010")), ["sc010/000001"]);
    p.choose_take("sc010", None).unwrap();
    assert_eq!(stems(&p, None), ["sc010/000001", "sc020/000001"]);

    // Survives a reload.
    p.choose_take("sc010", Some("sc010t2")).unwrap();
    let again = Project::open(&p.root).unwrap();
    assert_eq!(again.chosen_take("sc010"), "sc010t2");
}

#[test]
fn choosing_a_take_from_another_scene_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("Second", None).unwrap();
    p.add_take("sc020").unwrap();
    assert!(p.choose_take("sc010", Some("sc020t2")).is_err());
    assert!(p.choose_take("sc010", Some("nonsense")).is_err());
    assert_eq!(p.chosen_take("sc010"), "sc010");
}

#[test]
fn deleting_a_take_trashes_it_and_falls_back_to_take_one() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_take("sc010").unwrap();
    shoot(&p);
    p.choose_take("sc010", Some("sc010t2")).unwrap();

    p.delete_scene("sc010t2").unwrap();
    assert!(p.root.join("trash/sc010t2/scene.json").is_file());
    assert_eq!(p.file.scenes, ["sc010"], "the scene stays");
    assert_eq!(p.chosen_take("sc010"), "sc010");
    assert_eq!(p.file.active_scene.as_deref(), Some("sc010"));
    assert!(p.file.takes.is_empty());

    // Numbers aren't reused: the trashed take 2 keeps its folder name.
    assert_eq!(p.add_take("sc010").unwrap().id(), "sc010t3");
}

#[test]
fn deleting_a_scene_takes_its_takes_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("Second", None).unwrap();
    p.add_take("sc010").unwrap();
    p.add_take("sc010").unwrap();
    p.delete_scene("sc010").unwrap();
    for id in ["sc010", "sc010t2", "sc010t3"] {
        assert!(p.root.join("trash").join(id).is_dir(), "{id} in trash");
    }
    assert_eq!(p.file.scenes, ["sc020"]);
    assert_eq!(p.file.active_scene.as_deref(), Some("sc020"), "active take went with its scene");
    assert!(p.file.takes.is_empty());
}

#[test]
fn crash_recovery_covers_takes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_take("sc010").unwrap();
    let _ = capture::capture(&p, None, |dir| {
        fs::write(dir.join("IMG.JPG"), [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
        Err("simulated crash".into())
    });
    let report = Project::open(&p.root).unwrap().recover().unwrap();
    assert_eq!(report.recovered, [("sc010t2".to_string(), "000001".to_string())]);
}

#[test]
fn takes_can_be_found_by_id_or_name() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_take("sc010").unwrap();
    assert_eq!(p.find_scene("sc010t2").unwrap().id(), "sc010t2");
    assert_eq!(p.find_scene("Scene 1 · take 2").unwrap().id(), "sc010t2");
}

#[test]
fn projects_without_takes_read_and_write_the_old_format() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let text = fs::read_to_string(p.root.join("project.json")).unwrap();
    assert!(!text.contains("takes"), "no takes key unless there are takes: {text}");
    // A project.json written before takes existed opens fine.
    let old = r#"{"format":"dragonslayer/1","name":"Old","fps":12,"scenes":["sc010"],"active_scene":"sc010"}"#;
    fs::write(p.root.join("project.json"), old).unwrap();
    let opened = Project::open(&p.root).unwrap();
    assert!(opened.file.takes.is_empty());
    assert_eq!(opened.film_scenes().unwrap().len(), 1);
}

#[test]
fn adding_a_scene_while_a_take_is_active_goes_after_its_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("Last", None).unwrap();
    p.set_active("sc010").unwrap();
    p.add_take("sc010").unwrap();
    // The app passes the active row as "after": here, a take.
    let s = p.add_scene("Middle", Some("sc010t2")).unwrap();
    assert_eq!(p.file.scenes, ["sc010", s.id(), "sc020"]);
}

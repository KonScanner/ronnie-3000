//! File-backed preset store, ordering, migration, and host-program tests.

use ronie_core::{
    JuceCodec, JuceTree, JuceValue, PRESET_EXTENSION, PresetStore, read_preset,
    write_preset_v2_file,
};
use tempfile::TempDir;

fn preset(id: &str, name: &str, marker: &str) -> JuceTree {
    JuceTree {
        identifier: "T3KPreset".into(),
        properties: vec![
            ("id".into(), JuceValue::String(id.into())),
            ("name".into(), JuceValue::String(name.into())),
            ("marker".into(), JuceValue::String(marker.into())),
            ("unknownFuture".into(), JuceValue::Int(7)),
        ],
        children: Vec::new(),
    }
}

fn write(path: &std::path::Path, tree: &JuceTree) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
    write_preset_v2_file(path, tree, false).expect("write fixture");
}

fn write_legacy(path: &std::path::Path, tree: &JuceTree) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
    let mut bytes = b"T3KB".to_vec();
    bytes.extend_from_slice(&JuceCodec::default().write_tree(tree).expect("legacy tree"));
    std::fs::write(path, bytes).expect("legacy fixture");
}

#[test]
fn factory_override_user_order_and_host_mapping_match_reference() {
    let base = TempDir::new().expect("base");
    let system = TempDir::new().expect("system");
    write(
        &system.path().join(format!("clean{PRESET_EXTENSION}")),
        &preset("clean", "Shipped Clean", "system"),
    );
    write(
        &system.path().join(format!("lead{PRESET_EXTENSION}")),
        &preset("lead", "Shipped Lead", "system"),
    );
    write(
        &base.path().join(format!("Factory/clean{PRESET_EXTENSION}")),
        &preset("clean", "Local Clean", "local"),
    );
    let store = PresetStore::new(base.path(), system.path());
    let zulu = store.save("Zulu", preset("z", "Zulu", "z")).expect("zulu");
    let alpha = store
        .save("Alpha", preset("a", "Alpha", "a"))
        .expect("alpha");

    let list = store.list().expect("list");
    let names: Vec<_> = list.iter().map(|info| info.name.as_str()).collect();
    assert_eq!(names, ["Alpha", "Zulu", "Local Clean", "Shipped Lead"]);
    assert_eq!(list[2].id, "factory:clean");
    assert!(list[2].factory);
    assert_eq!(
        store.load("factory:clean").expect("override").properties[2].1,
        JuceValue::String("local".into())
    );

    let programs = store.host_programs();
    assert_eq!(programs[0].name, "Alpha");
    assert_eq!(programs[1].name, "Zulu");
    assert_eq!(programs[2].name, "Local Clean");
    assert_eq!(programs[127].name, "(empty)");
    assert!(
        store
            .select_host_program(0, &alpha.id)
            .expect("active no-op")
            .is_none()
    );
    assert_eq!(
        store
            .select_host_program(1, &alpha.id)
            .expect("select")
            .expect("info")
            .id,
        zulu.id
    );
    assert_eq!(store.current_program(&zulu.id), 1);
    assert_eq!(store.current_program("missing"), 0);
}

#[test]
fn same_name_update_and_rename_keep_stable_ids() {
    let base = TempDir::new().expect("base");
    let store = PresetStore::new(base.path(), base.path().join("system"));
    let first = store.save("Lead", preset("", "", "v1")).expect("first");
    let second = store.save("Lead", preset("", "", "v2")).expect("second");
    assert_eq!(first.id, second.id);

    assert!(store.rename(&first.id, "New Name").expect("rename"));
    let loaded = store.load(&first.id).expect("load renamed");
    assert_eq!(
        loaded
            .properties
            .iter()
            .find(|(name, _)| name == "name")
            .map(|(_, value)| value),
        Some(&JuceValue::String("New Name".into()))
    );
    assert_eq!(
        loaded
            .properties
            .iter()
            .find(|(name, _)| name == "id")
            .map(|(_, value)| value),
        Some(&JuceValue::String(
            first.id.trim_start_matches("user:").into()
        ))
    );
    assert_eq!(
        loaded
            .properties
            .iter()
            .find(|(name, _)| name == "unknownFuture")
            .map(|(_, value)| value),
        Some(&JuceValue::Int(7))
    );
    assert!(store.remove(&first.id).expect("remove"));
    assert!(store.load(&first.id).is_err());
    assert!(!store.remove("factory:clean").expect("factory refusal"));
}

#[test]
fn order_persists_and_movement_is_clamped_within_sections() {
    let base = TempDir::new().expect("base");
    let store = PresetStore::new(base.path(), base.path().join("system"));
    let a = store.save("A", preset("a", "A", "a")).expect("A");
    store.save("B", preset("b", "B", "b")).expect("B");
    let c = store.save("C", preset("c", "C", "c")).expect("C");

    assert!(store.move_by(&c.id, -1).expect("move earlier"));
    assert!(store.move_by(&c.id, -1).expect("move to section edge"));
    assert!(!store.move_by(&c.id, -100).expect("clamped no-op"));
    assert!(store.move_by(&a.id, 100).expect("move later"));
    let expected = ["C", "B", "A"];
    let names: Vec<_> = store
        .list()
        .expect("list")
        .iter()
        .map(|info| info.name.clone())
        .collect();
    assert_eq!(names, expected);
    let reopened = PresetStore::new(base.path(), base.path().join("system"));
    let persisted: Vec<_> = reopened
        .list()
        .expect("persisted")
        .iter()
        .map(|info| info.name.clone())
        .collect();
    assert_eq!(persisted, expected);
}

#[test]
fn header_cache_reparses_only_changed_or_new_headers() {
    let base = TempDir::new().expect("base");
    let path = base.path().join(format!("one{PRESET_EXTENSION}"));
    write(&path, &preset("one", "One", "one"));
    let store = PresetStore::new(base.path(), base.path().join("system"));
    store.list().expect("cold");
    let cold = store.header_reads();
    store.list().expect("warm");
    assert_eq!(store.header_reads(), cold);

    let file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append");
    file.set_len(file.metadata().expect("metadata").len() + 1)
        .expect("grow");
    drop(file);
    store.list().expect("changed");
    assert_eq!(store.header_reads(), cold + 1);

    std::fs::write(&path, b"corrupt").expect("corrupt");
    store.list().expect("corrupt read");
    let corrupt_reads = store.header_reads();
    store.list().expect("corrupt cached");
    assert_eq!(store.header_reads(), corrupt_reads);
}

#[test]
fn host_names_use_headers_without_decoding_model_bodies() {
    let base = TempDir::new().expect("base");
    let path = base.path().join(format!("one{PRESET_EXTENSION}"));
    write(&path, &preset("one", "One", "one"));
    let mut bytes = std::fs::read(&path).expect("preset");
    bytes.extend_from_slice(b"not a valid body");
    std::fs::write(&path, bytes).expect("corrupt body");
    let store = PresetStore::new(base.path(), base.path().join("system"));

    let programs = store.host_programs();

    assert_eq!(programs[0].name, "One");
    assert!(store.load("user:one").is_err());
}

#[test]
fn legacy_missing_id_migrates_lazily_with_stable_identity() {
    let base = TempDir::new().expect("base");
    let uuid = "0123456789abcdef0123456789abcdef";
    let legacy_path = base.path().join(format!("{uuid}{PRESET_EXTENSION}"));
    write_legacy(&legacy_path, &preset("", "Old Timer", "old"));
    let store = PresetStore::new(base.path(), base.path().join("system"));

    let listed = store.list().expect("legacy list");
    assert_eq!(listed[0].id, format!("user:{uuid}"));
    assert_eq!(listed[0].name, "Old Timer");
    assert!(
        store
            .rename(&listed[0].id, "Old Timer")
            .expect("lazy migration")
    );

    let migrated = base.path().join(format!("Old Timer{PRESET_EXTENSION}"));
    assert!(migrated.is_file());
    assert!(!legacy_path.exists());
    let decoded = read_preset(&std::fs::read(migrated).expect("migrated")).expect("migrated tree");
    assert_eq!(
        decoded
            .properties
            .iter()
            .find(|(name, _)| name == "id")
            .map(|(_, value)| value),
        Some(&JuceValue::String(uuid.into()))
    );
}

#[test]
fn duplicate_ids_fall_back_to_unique_stems() {
    let base = TempDir::new().expect("base");
    write(
        &base.path().join(format!("first{PRESET_EXTENSION}")),
        &preset("same", "First", "first"),
    );
    write(
        &base.path().join(format!("second{PRESET_EXTENSION}")),
        &preset("same", "Second", "second"),
    );
    let store = PresetStore::new(base.path(), base.path().join("system"));

    let list = store.list().expect("duplicates");
    let ids: Vec<_> = list.iter().map(|info| info.id.as_str()).collect();

    assert_eq!(ids, ["user:same", "user:second"]);
}

#[test]
fn corrupt_order_file_falls_back_to_name_order() {
    let base = TempDir::new().expect("base");
    let store = PresetStore::new(base.path(), base.path().join("system"));
    store.save("Zulu", preset("z", "Zulu", "z")).expect("zulu");
    store
        .save("Alpha", preset("a", "Alpha", "a"))
        .expect("alpha");
    std::fs::write(base.path().join("order.json"), b"not json").expect("corrupt order");

    let names: Vec<_> = store
        .list()
        .expect("name order")
        .iter()
        .map(|info| info.name.clone())
        .collect();

    assert_eq!(names, ["Alpha", "Zulu"]);
}

#[test]
#[ignore = "requires the official source fixture directory"]
fn official_factory_presets_scan_and_load() {
    let Some(root) = std::env::var_os("RONIE_REFERENCE_ROOT") else {
        return;
    };
    let factory = std::path::PathBuf::from(root).join("resources/factory-presets");
    let base = TempDir::new().expect("base");
    let store = PresetStore::new(base.path(), factory);

    let list = store.list().expect("official factory list");
    assert!(!list.is_empty());
    for info in &list {
        store
            .load(&info.id)
            .unwrap_or_else(|error| panic!("could not load {}: {error}", info.id));
    }
}

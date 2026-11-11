//! Preset framing, filename, collision, and atomic-write tests.

use ronie_core::{
    JuceTree, JuceValue, PresetError, PresetFraming, read_preset, read_preset_header,
    sanitize_stem, unique_preset_path, write_preset_v2, write_preset_v2_file,
};
use tempfile::TempDir;

#[test]
fn preset_file_reads_reject_excessive_aggregate_bytes_before_allocation() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("oversized.t3kpreset");
    let file = std::fs::File::create(&path).expect("sparse file");
    file.set_len(512 * 1024 * 1024 + 1).expect("sparse length");

    let result = ronie_core::read_preset_file(&path);

    assert!(matches!(result, Err(PresetError::InputTooLarge)));
}

fn preset() -> JuceTree {
    JuceTree {
        identifier: "T3KPreset".into(),
        properties: vec![
            ("id".into(), JuceValue::String("user:one".into())),
            ("name".into(), JuceValue::String("Lead".into())),
            ("marker".into(), JuceValue::String("v1".into())),
            (
                "unknownFuture".into(),
                JuceValue::Array(vec![JuceValue::Int(1), JuceValue::Int(2)]),
            ),
        ],
        children: Vec::new(),
    }
}

#[test]
fn official_filename_cases_match_reference() {
    let cases = [
        ("Marshall Bluesbreaker", "Marshall Bluesbreaker"),
        ("Für Élise 東京", "Für Élise 東京"),
        ("A/B\\C:D*E?F\"G<H>I|J", "A-B-C-D-E-F-G-H-I-J"),
        ("Clean // Lead", "Clean - Lead"),
        ("A -- B", "A -- B"),
        ("Tab\tNew\nLine", "Tab-New-Line"),
        ("  .hidden. ", "hidden"),
        ("~lock", "lock"),
        ("trailing...", "trailing"),
        ("con", "_con"),
        ("LPT1", "_LPT1"),
        ("con.backup", "_con.backup"),
        ("Console", "Console"),
        ("", "Preset"),
        ("///", "Preset"),
        (" . . ", "Preset"),
        ("---", "Preset"),
    ];
    for (input, expected) in cases {
        assert_eq!(sanitize_stem(input), expected, "input {input:?}");
    }
}

#[test]
fn unicode_cap_never_splits_a_code_point() {
    let cjk = sanitize_stem(&"東".repeat(60));
    assert_eq!(cjk.len(), 99);
    assert_eq!(cjk.chars().count(), 33);
    let accents = sanitize_stem(&"é".repeat(60));
    assert_eq!(accents.len(), 100);
    assert_eq!(accents.chars().count(), 50);
    assert_eq!(sanitize_stem(&"x".repeat(500)).len(), 100);
}

#[test]
fn collisions_get_numeric_suffixes_and_case_only_rename_uses_self() {
    let temp = TempDir::new().expect("temp dir");
    let first = temp.path().join("A-B.t3kpreset");
    let second = temp.path().join("A-B 2.t3kpreset");
    std::fs::write(&first, b"one").expect("first");
    std::fs::write(&second, b"two").expect("second");

    assert_eq!(
        unique_preset_path(temp.path(), "A-B", None),
        temp.path().join("A-B 3.t3kpreset")
    );
    assert_eq!(
        unique_preset_path(temp.path(), "A-B", Some(&second)),
        second
    );
    let case = temp.path().join("Lead.t3kpreset");
    std::fs::write(&case, b"lead").expect("case");
    assert_eq!(unique_preset_path(temp.path(), "Lead", Some(&case)), case);
}

#[test]
fn v2_round_trip_preserves_header_body_and_unknown_fields() {
    let encoded = write_preset_v2(&preset()).expect("v2 encode");
    assert_eq!(&encoded[..4], b"T3KH");
    let header = read_preset_header(&encoded).expect("v2 header");
    assert_eq!(header.id, "user:one");
    assert_eq!(header.name, "Lead");
    assert_eq!(header.framing(), PresetFraming::V2);
    assert_eq!(read_preset(&encoded).expect("v2 body"), preset());
}

#[test]
fn legacy_v1_uses_a_full_body_parse() {
    let mut encoded = b"T3KB".to_vec();
    encoded.extend_from_slice(
        &ronie_core::JuceCodec::default()
            .write_tree(&preset())
            .expect("body"),
    );
    let header = read_preset_header(&encoded).expect("legacy header");

    assert_eq!(header.id, "user:one");
    assert_eq!(header.name, "Lead");
    assert!(header.legacy);
    assert_eq!(header.framing(), PresetFraming::V1);
    assert_eq!(read_preset(&encoded).expect("legacy body"), preset());
}

#[test]
fn bad_lengths_tags_and_truncation_are_rejected() {
    for bytes in [
        b"T3KH\0\0\0\0".to_vec(),
        {
            let mut bytes = b"T3KH".to_vec();
            bytes.extend_from_slice(&65_537_i32.to_le_bytes());
            bytes
        },
        b"T3KX".to_vec(),
        b"T3KB\0".to_vec(),
    ] {
        assert!(
            read_preset_header(&bytes).is_err(),
            "accepted invalid framing"
        );
    }
    let mut wrong = b"T3KH".to_vec();
    wrong.extend_from_slice(&1_i32.to_le_bytes());
    wrong.push(0);
    assert!(read_preset(&wrong).is_err());
}

#[test]
fn atomic_write_replaces_user_presets_and_refuses_factory() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("Lead.t3kpreset");
    write_preset_v2_file(&path, &preset(), false).expect("initial write");
    let mut changed = preset();
    changed.properties[2].1 = JuceValue::String("v2".into());
    write_preset_v2_file(&path, &changed, false).expect("replacement");

    let decoded =
        read_preset(&std::fs::read(&path).expect("read replacement")).expect("replacement");
    assert_eq!(decoded.properties[2].1, JuceValue::String("v2".into()));

    let result = write_preset_v2_file(&path, &preset(), true);
    assert!(matches!(result, Err(PresetError::FactoryReadOnly)));
    assert_eq!(
        read_preset(&std::fs::read(&path).expect("factory unchanged")).expect("unchanged"),
        decoded
    );
}

#[cfg(unix)]
#[test]
fn interrupted_replacement_preserves_the_previous_file() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("Lead.t3kpreset");
    write_preset_v2_file(&path, &preset(), false).expect("initial write");
    let before = std::fs::read(&path).expect("before");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o555))
        .expect("lock directory");
    if std::fs::write(temp.path().join("probe"), b"").is_ok() {
        return;
    }

    let result = write_preset_v2_file(&path, &preset(), false);

    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).expect("old file"), before);
    assert!(
        std::fs::read_dir(temp.path())
            .expect("directory")
            .all(|entry| {
                let name = entry.expect("entry").file_name();
                !name.to_string_lossy().ends_with(".tmp")
            })
    );
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755))
        .expect("unlock directory");
}

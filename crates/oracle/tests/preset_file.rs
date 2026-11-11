//! Reference filename and v2 preset acceptance tests.

use ronie_core::{
    JuceCodec, JuceTree, JuceValue, read_preset_header, sanitize_stem, write_preset_v2,
};
use ronie_oracle::state::{StateCommand, StateOracle};

#[test]
#[ignore = "requires private Reference source and build directories"]
fn reference_matches_preset_filename_rules() {
    let Ok(oracle) = StateOracle::new() else {
        return;
    };
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
        let reference = oracle
            .run(&StateCommand::preset_name(input.as_bytes().to_vec()))
            .expect("Reference preset name");
        assert_eq!(reference, expected.as_bytes(), "input {input:?}");
        assert_eq!(sanitize_stem(input), expected, "input {input:?}");
    }

    let cjk = "東".repeat(60);
    let reference = oracle
        .run(&StateCommand::preset_name(cjk.as_bytes().to_vec()))
        .expect("Reference CJK name");
    assert_eq!(reference.len(), 99);
    assert_eq!(sanitize_stem(&cjk).as_bytes(), reference);
}

#[test]
#[ignore = "requires private Reference source and build directories"]
fn reference_accepts_rust_v2_header_and_body() {
    let Ok(oracle) = StateOracle::new() else {
        return;
    };
    let preset = JuceTree {
        identifier: "T3KPreset".into(),
        properties: vec![
            ("id".into(), JuceValue::String("user:one".into())),
            ("name".into(), JuceValue::String("Lead".into())),
            ("marker".into(), JuceValue::String("v1".into())),
        ],
        children: Vec::new(),
    };
    let encoded = write_preset_v2(&preset).expect("Rust v2");
    let header = read_preset_header(&encoded).expect("Rust header");
    let header_len = i32::from_le_bytes(encoded[4..8].try_into().expect("length")) as usize;

    let accepted_header = oracle
        .run(&StateCommand::header_decode(
            encoded[8..8 + header_len].to_vec(),
        ))
        .expect("Reference accepts Rust header");
    let accepted_body = oracle
        .run(&StateCommand::body_decode(
            encoded[8 + header_len..].to_vec(),
        ))
        .expect("Reference accepts Rust body");
    let header_xml = String::from_utf8(accepted_header).expect("header XML");
    let body_xml = String::from_utf8(accepted_body).expect("body XML");

    assert!(header_xml.contains(&header.id) && header_xml.contains(&header.name));
    assert!(body_xml.contains("T3KPreset") && body_xml.contains("v1"));
}

#[test]
#[ignore = "requires private Reference source and build directories"]
fn reference_accepts_rust_legacy_body() {
    let Ok(oracle) = StateOracle::new() else {
        return;
    };
    let preset = JuceTree {
        identifier: "T3KPreset".into(),
        properties: vec![
            ("id".into(), JuceValue::String("user:one".into())),
            ("name".into(), JuceValue::String("Lead".into())),
        ],
        children: Vec::new(),
    };
    let mut encoded = b"T3KB".to_vec();
    encoded.extend_from_slice(
        &JuceCodec::default()
            .write_tree(&preset)
            .expect("legacy body"),
    );

    let accepted = oracle
        .run(&StateCommand::body_decode(encoded[4..].to_vec()))
        .expect("Reference accepts Rust legacy body");
    let xml = String::from_utf8(accepted).expect("legacy XML");
    assert!(xml.contains("T3KPreset") && xml.contains("Lead"), "{xml}");
}

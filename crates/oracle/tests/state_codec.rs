//! Two-way Reference and Rust JUCE state-codec verification.

mod common;

use ronnie_core::{JuceCodec, JuceTree, JuceValue, StateCodec};
use ronnie_oracle::state::StateCommand;

#[test]
#[ignore = "requires private Reference source and build directories"]
fn reference_and_rust_accept_matching_string_trees() {
    let Some(oracle) = common::state_oracle() else {
        return;
    };
    let codec = JuceCodec::default();
    let tree = JuceTree {
        identifier: "Root".into(),
        properties: vec![("x".into(), JuceValue::String("1".into()))],
        children: Vec::new(),
    };

    let reference = oracle
        .run(&StateCommand::body_encode(b"<Root x=\"1\"/>".to_vec()))
        .expect("Reference encode");
    let rust = codec.write_tree(&tree).expect("Rust encode");
    assert_eq!(rust, reference);
    assert_eq!(codec.read_tree(&reference).expect("Rust decode"), tree);

    let accepted = oracle
        .run(&StateCommand::body_decode(rust))
        .expect("Reference accepts Rust bytes");
    let xml = String::from_utf8(accepted).expect("Reference XML");
    assert!(xml.contains("Root") && xml.contains("x=\"1\""), "{xml}");
}

#[test]
#[ignore = "requires private Reference source and build directories"]
fn reference_accepts_rust_session_tree() {
    let Some(oracle) = common::state_oracle() else {
        return;
    };
    let codec = StateCodec::new();
    let mut reference = b"T3KB".to_vec();
    reference.extend_from_slice(
        &oracle
            .run(&StateCommand::body_encode(
                b"<RonnieState schemaVersion=\"1\"><ChainSnapshot stereoEnabled=\"false\"/></RonnieState>"
                    .to_vec(),
            ))
            .expect("Reference session tree"),
    );
    let document = codec
        .decode_session(&reference)
        .expect("Rust session decode");
    let encoded = codec
        .encode_session(&document)
        .expect("Rust session encode");
    assert_eq!(&encoded[..4], b"T3KB");

    let accepted = oracle
        .run(&StateCommand::body_decode(encoded[4..].to_vec()))
        .expect("Reference accepts Rust session tree");
    let xml = String::from_utf8(accepted).expect("Reference XML");
    assert!(xml.contains("RonnieState"), "{xml}");
}

/// Asserts that a decoded block kept every dual-IR property built for the test below.
fn assert_dual_ir_settings(block: &ronnie_core::ChainBlock) {
    assert_eq!(
        block.ir_b,
        Some(ronnie_core::IrB {
            model_id: ronnie_core::ModelId(8)
        })
    );
    assert_eq!(block.params.ir_blend, 0.65);
    assert_eq!(block.params.ir_a_level, 0.4);
    assert_eq!(block.params.ir_b_level, 0.7);
    assert!(block.params.ir_b_invert);
    assert_eq!(block.skin.as_deref(), Some("british"));
}

/// A session saved with a cab's second IR: the Reference must still accept it, and its generic
/// `ValueTree` body decode/encode must preserve the dual-IR properties and the skin override.
///
/// The Reference state runner's body encode/decode is the generic JUCE `ValueTree` binary
/// write/read, so this proves that transform. It does not prove the Reference's private typed
/// session loader; that still needs the documented manual validation.
#[test]
#[ignore = "requires private Reference source and build directories"]
fn reference_accepts_a_session_written_with_ir_b() {
    let tone = ronnie_core::Tone::from_value(serde_json::json!({
        "id": 42,
        "title": "Two mics",
        "format": "ir",
        "models": [{"id": 7, "name": "A"}, {"id": 8, "name": "B"}]
    }))
    .expect("tone");
    let mut block = ronnie_core::ChainBlock::tone("cab", tone.clone(), ronnie_core::ModelId(7));
    block.ir_b = Some(ronnie_core::IrB {
        model_id: ronnie_core::ModelId(8),
    });
    block.params.ir_blend = 0.65;
    block.params.ir_a_level = 0.4;
    block.params.ir_b_level = 0.7;
    block.params.ir_b_invert = true;
    block.skin = Some("british".into());
    let cached = |model: i64, data: &[u8]| JuceTree {
        identifier: "CachedModel".into(),
        properties: vec![
            ("modelId".into(), JuceValue::Int64(model)),
            ("data".into(), JuceValue::Binary(data.to_vec())),
        ],
        children: Vec::new(),
    };
    let mut chain_block = JuceTree {
        identifier: "ChainBlock".into(),
        properties: vec![
            ("id".into(), JuceValue::String("cab".into())),
            ("type".into(), JuceValue::String("IR".into())),
            ("enabled".into(), JuceValue::Bool(true)),
            ("toneId".into(), JuceValue::Int64(42)),
            (
                "toneJson".into(),
                JuceValue::String(tone.to_json().expect("tone json")),
            ),
            ("activeModelId".into(), JuceValue::Int64(7)),
        ],
        children: vec![JuceTree {
            identifier: "ModelCache".into(),
            properties: Vec::new(),
            children: vec![cached(7, b"mic-a"), cached(8, b"mic-b")],
        }],
    };
    block.write_ir_pair_juce(&mut chain_block);
    block.write_skin_juce(&mut chain_block);
    let session = JuceTree {
        identifier: "RonnieState".into(),
        properties: vec![("schemaVersion".into(), JuceValue::Int(1))],
        children: vec![JuceTree {
            identifier: "ChainSnapshot".into(),
            properties: vec![("stereoEnabled".into(), JuceValue::Bool(false))],
            children: vec![JuceTree {
                identifier: "ChainBlocks".into(),
                properties: Vec::new(),
                children: vec![chain_block],
            }],
        }],
    };
    let body = JuceCodec::default()
        .write_tree(&session)
        .expect("Rust encode");
    let mut state = b"T3KB".to_vec();
    state.extend_from_slice(&body);
    let document = StateCodec::new()
        .decode_session(&state)
        .expect("Rust reads its own session");
    assert_dual_ir_settings(&document.snapshot.chain.left[0]);

    let Some(oracle) = common::state_oracle() else {
        return;
    };

    let accepted = oracle
        .run(&StateCommand::body_decode(body))
        .expect("Reference accepts a session written with IR B");
    let xml = String::from_utf8(accepted).expect("Reference XML");
    assert!(xml.contains("activeModelId=\"7\""), "{xml}");
    assert!(xml.contains("irBModelId=\"8\""), "{xml}");
    assert!(xml.contains("skinOverride=\"british\""), "{xml}");
    for attribute in ["irBlend", "irALevel", "irBLevel", "irBInvert"] {
        assert!(
            xml.contains(&format!("{attribute}=")),
            "the Reference generic decode dropped {attribute}: {xml}"
        );
    }

    let reencoded = oracle
        .run(&StateCommand::body_encode(xml.into_bytes()))
        .expect("Reference re-encodes its own XML");
    let mut round_tripped = b"T3KB".to_vec();
    round_tripped.extend_from_slice(&reencoded);
    let document = StateCodec::new()
        .decode_session(&round_tripped)
        .expect("Rust reads the Reference round trip");
    assert_dual_ir_settings(&document.snapshot.chain.left[0]);
}

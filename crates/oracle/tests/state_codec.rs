//! Two-way Reference and Rust JUCE state-codec verification.

use ronie_core::{JuceCodec, JuceTree, JuceValue, StateCodec};
use ronie_oracle::state::{StateCommand, StateOracle};

#[test]
#[ignore = "requires private Reference source and build directories"]
fn reference_and_rust_accept_matching_string_trees() {
    let Ok(oracle) = StateOracle::new() else {
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
    let Ok(oracle) = StateOracle::new() else {
        return;
    };
    let codec = StateCodec::new();
    let mut reference = b"T3KB".to_vec();
    reference.extend_from_slice(
        &oracle
            .run(&StateCommand::body_encode(
                b"<RonieState schemaVersion=\"1\"><ChainSnapshot stereoEnabled=\"false\"/></RonieState>"
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
    assert!(xml.contains("RonieState"), "{xml}");
}

/// A session saved with a cab's second IR: the Reference must still accept it, and the block
/// keeps its active model (A) for the Reference to play.
#[test]
#[ignore = "requires private Reference source and build directories"]
fn reference_accepts_a_session_written_with_ir_b() {
    let tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 42,
        "title": "Two mics",
        "format": "ir",
        "models": [{"id": 7, "name": "A"}, {"id": 8, "name": "B"}]
    }))
    .expect("tone");
    let mut block = ronie_core::ChainBlock::tone("cab", tone.clone(), ronie_core::ModelId(7));
    block.ir_b = Some(ronie_core::IrB {
        model_id: ronie_core::ModelId(8),
    });
    block.params.ir_blend = 0.65;
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
    let session = JuceTree {
        identifier: "RonieState".into(),
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
    assert_eq!(document.snapshot.chain.left[0].ir_b, block.ir_b);

    let Ok(oracle) = StateOracle::new() else {
        return;
    };

    let accepted = oracle
        .run(&StateCommand::body_decode(body))
        .expect("Reference accepts a session written with IR B");
    let xml = String::from_utf8(accepted).expect("Reference XML");
    assert!(xml.contains("activeModelId=\"7\""), "{xml}");
    assert!(xml.contains("irBModelId=\"8\""), "{xml}");
}

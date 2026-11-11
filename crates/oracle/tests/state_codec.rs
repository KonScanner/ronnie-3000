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

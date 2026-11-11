//! Deterministic JUCE variant and ValueTree binary codec tests.

use ronie_core::{CodecError, JuceCodec, JuceTree, JuceValue};

fn codec() -> JuceCodec {
    JuceCodec {
        max_depth: 64,
        max_nodes: 100_000,
        max_blob_bytes: 1024 * 1024,
    }
}

#[test]
fn simple_tree_matches_independently_derived_reference_bytes() {
    let tree = JuceTree {
        identifier: "Root".into(),
        properties: vec![("x".into(), JuceValue::Int(1))],
        children: Vec::new(),
    };
    let expected = [
        b'R', b'o', b'o', b't', 0, 1, 1, b'x', 0, 1, 5, 1, 1, 0, 0, 0, 0,
    ];

    let encoded = codec().write_tree(&tree).expect("encode");
    assert_eq!(encoded, expected);
    assert_eq!(codec().read_tree(&encoded).expect("decode"), tree);
}

#[test]
fn every_variant_round_trips_with_order_and_unknown_fields() {
    let tree = JuceTree {
        identifier: "Root".into(),
        properties: vec![
            ("void".into(), JuceValue::Void),
            ("undefined".into(), JuceValue::Undefined),
            ("int".into(), JuceValue::Int(-42)),
            ("int64".into(), JuceValue::Int64(-4_203_000_000)),
            ("false".into(), JuceValue::Bool(false)),
            ("true".into(), JuceValue::Bool(true)),
            ("double".into(), JuceValue::Double(-12.5)),
            ("unicode".into(), JuceValue::String("καλώς 世界".into())),
            (
                "array".into(),
                JuceValue::Array(vec![JuceValue::Int(1), JuceValue::String("two".into())]),
            ),
            ("emptyBinary".into(), JuceValue::Binary(Vec::new())),
            ("binary".into(), JuceValue::Binary(vec![0, 1, 2, 255])),
        ],
        children: vec![JuceTree {
            identifier: "UnknownFutureTree".into(),
            properties: vec![(
                "unknownFutureProperty".into(),
                JuceValue::String("retained".into()),
            )],
            children: Vec::new(),
        }],
    };

    let encoded = codec().write_tree(&tree).expect("encode");
    let decoded = codec().read_tree(&encoded).expect("decode");
    assert_eq!(decoded, tree);
    assert_eq!(codec().write_tree(&decoded).expect("re-encode"), encoded);
}

#[test]
fn special_double_bits_are_preserved() {
    let values = [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.0,
        f64::from_bits(1),
    ];
    for value in values {
        let tree = JuceTree {
            identifier: "Root".into(),
            properties: vec![("value".into(), JuceValue::Double(value))],
            children: Vec::new(),
        };
        let decoded = codec()
            .read_tree(&codec().write_tree(&tree).expect("encode"))
            .expect("decode");
        let JuceValue::Double(actual) = decoded.properties[0].1 else {
            panic!("wrong variant");
        };
        assert_eq!(actual.to_bits(), value.to_bits());
    }
}

#[test]
fn binary_limit_is_checked_before_allocation() {
    let small = JuceCodec {
        max_depth: 64,
        max_nodes: 100_000,
        max_blob_bytes: 8,
    };
    let valid = JuceTree {
        identifier: "Root".into(),
        properties: vec![("bytes".into(), JuceValue::Binary(vec![1; 8]))],
        children: Vec::new(),
    };
    assert!(small.write_tree(&valid).is_ok());
    let invalid = JuceTree {
        properties: vec![("bytes".into(), JuceValue::Binary(vec![1; 9]))],
        ..valid
    };
    assert!(matches!(
        small.write_tree(&invalid),
        Err(CodecError::BlobTooLarge { .. })
    ));

    let mut forged = b"Root\0".to_vec();
    forged.extend_from_slice(&[1]);
    forged.extend_from_slice(b"bytes\0");
    forged.extend_from_slice(&[1, 10, 8]);
    forged.extend_from_slice(&[1; 9]);
    forged.push(0);
    assert!(matches!(
        small.read_tree(&forged),
        Err(CodecError::BlobTooLarge { .. })
    ));
}

#[test]
fn depth_and_node_limits_are_enforced() {
    let mut tree = JuceTree {
        identifier: "Leaf".into(),
        properties: Vec::new(),
        children: Vec::new(),
    };
    for _ in 0..64 {
        tree = JuceTree {
            identifier: "Node".into(),
            properties: Vec::new(),
            children: vec![tree],
        };
    }
    assert!(matches!(
        codec().write_tree(&tree),
        Err(CodecError::DepthLimit { .. })
    ));

    let limited = JuceCodec {
        max_depth: 64,
        max_nodes: 1,
        max_blob_bytes: 1024,
    };
    let array = JuceTree {
        identifier: "Root".into(),
        properties: vec![(
            "array".into(),
            JuceValue::Array(vec![JuceValue::Int(1), JuceValue::Int(2)]),
        )],
        children: Vec::new(),
    };
    assert!(matches!(
        limited.write_tree(&array),
        Err(CodecError::NodeLimit { .. })
    ));
}

#[test]
fn large_binary_payload_round_trips_above_the_node_count() {
    let tree = JuceTree {
        identifier: "Root".into(),
        properties: vec![("bytes".into(), JuceValue::Binary(vec![0x5a; 1024 * 1024]))],
        children: Vec::new(),
    };

    let encoded = codec().write_tree(&tree).expect("encode large binary");
    let decoded = codec().read_tree(&encoded).expect("decode large binary");

    assert_eq!(decoded, tree);
}

#[test]
fn unsupported_tags_invalid_utf_and_trailing_bytes_are_rejected() {
    let mut unsupported = b"Root\0\x01\x01x\0\x01\x01\x63".to_vec();
    let result = codec().read_tree(&unsupported);
    assert!(
        matches!(result, Err(CodecError::UnsupportedType(99))),
        "actual result: {result:?}"
    );
    unsupported[0] = 0xff;
    assert!(matches!(
        codec().read_tree(&unsupported),
        Err(CodecError::InvalidUtf8)
    ));

    let valid = JuceTree {
        identifier: "Root".into(),
        properties: Vec::new(),
        children: Vec::new(),
    };
    let mut trailing = codec().write_tree(&valid).expect("encode");
    let tree_len = trailing.len();
    trailing.extend_from_slice(b"next");
    assert!(matches!(
        codec().read_tree(&trailing),
        Err(CodecError::TrailingBytes)
    ));
    let (prefix, consumed) = codec().read_tree_prefix(&trailing).expect("prefix");
    assert_eq!(prefix, valid);
    assert_eq!(consumed, tree_len);
}

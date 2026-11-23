//! Adversarial state-tree binary truncation and mutation tests.

use proptest::prelude::*;
use ronnie_core::{StateTree, StateTreeCodec, StateValue};

fn codec() -> StateTreeCodec {
    StateTreeCodec {
        max_depth: 64,
        max_nodes: 1000,
        max_blob_bytes: 4096,
    }
}

fn encoded_tree() -> Vec<u8> {
    let tree = StateTree {
        identifier: "Root".into(),
        properties: vec![
            (
                "array".into(),
                StateValue::Array(vec![StateValue::Int(1), StateValue::String("two".into())]),
            ),
            ("binary".into(), StateValue::Binary(vec![0, 1, 2, 3])),
        ],
        children: vec![StateTree {
            identifier: "Child".into(),
            properties: vec![("double".into(), StateValue::Double(1.5))],
            children: Vec::new(),
        }],
    };
    codec().write_tree(&tree).expect("fixture")
}

#[test]
fn every_truncation_is_bounded_and_panic_free() {
    let bytes = encoded_tree();
    for end in 0..bytes.len() {
        let _ = codec().read_tree(&bytes[..end]);
    }
}

#[test]
fn deterministic_mutations_are_bounded_and_panic_free() {
    let original = encoded_tree();
    for index in 0..original.len() {
        let mut changed = original.clone();
        changed[index] ^= 0xff;
        let _ = codec().read_tree(&changed);
    }
}

#[test]
fn forged_counts_and_depths_do_not_escape_bounds() {
    let cases: &[&[u8]] = &[
        b"Root\0\x7f",
        b"Root\0\x81\x01",
        b"Root\0\x01x\0\x81\x01\x07\x7f",
        b"Root\0\x01\xff\0\x01\x01\x01",
        b"Root\0\x01x\0\x06\x05hello\0\0",
    ];
    for bytes in cases {
        let _ = codec().read_tree(bytes);
    }
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in any::<Vec<u8>>()) {
        let _ = codec().read_tree(&bytes);
    }

    #[test]
    fn arbitrary_mutations_never_panic(mut bytes in any::<Vec<u8>>()) {
        if bytes.is_empty() {
            bytes.push(0);
        }
        let index = bytes.len() - 1;
        bytes[index] ^= 0x80;
        let _ = codec().read_tree(&bytes);
    }
}

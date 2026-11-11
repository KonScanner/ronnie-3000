//! Property-based malformed-input and parser-boundary tests.

use proptest::prelude::*;
use ronie_core::{NamError, parse_nam};
use serde_json::json;

fn document_with_depth(arrays: usize) -> Vec<u8> {
    let mut text = String::from(r#"{"version":"0.7.0","architecture":"WaveNet","config":"#);
    for _ in 0..arrays {
        text.push('[');
    }
    for _ in 0..arrays {
        text.push(']');
    }
    if arrays == 0 {
        text.push_str("null");
    }
    text.push_str(r#","weights":[]}"#);
    text.into_bytes()
}

fn document_with_weights(weights: &[i32]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "version": "0.7.0",
        "architecture": "WaveNet",
        "config": {},
        "weights": weights
    }))
    .expect("serialize integer weights")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn malformed_inputs_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let _ = parse_nam(&bytes);
    }

    #[test]
    fn truncated_documents_never_panic(prefix in 0..2048_usize) {
        let bytes = document_with_weights(&[1, 2, 3, 4]);
        let end = prefix.min(bytes.len());
        let _ = parse_nam(&bytes[..end]);
    }

    #[test]
    fn depth_limit_is_exact(arrays in 0..80_usize) {
        let result = parse_nam(&document_with_depth(arrays));
        if arrays <= 63 {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(NamError::NestingTooDeep)));
        }
    }

    #[test]
    fn arbitrary_weight_counts_remain_bounded(weights in prop::collection::vec(any::<i32>(), 0..1024)) {
        let parsed = parse_nam(&document_with_weights(&weights)).expect("valid integer weights");
        assert_eq!(parsed.weights.len(), weights.len());
    }
}

#[test]
fn oversized_input_is_rejected_before_json_allocation() {
    let bytes = vec![0_u8; 50 * 1024 * 1024 + 1];
    assert!(matches!(parse_nam(&bytes), Err(NamError::InputTooLarge)));
}

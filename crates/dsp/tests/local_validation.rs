//! Local A2 and slimmable-container validation tests.

use ronie_core::{DspError, parse_nam};
use ronie_dsp::validate_a2_document;
use ronie_oracle::synthetic::synthetic_a2_json;
use serde_json::json;

#[test]
fn valid_a2_document_passes_without_preparing_audio_state() {
    let document = parse_nam(&synthetic_a2_json(3).expect("synthetic A2")).expect("parse");

    validate_a2_document(&document).expect("valid A2 should pass");
}

#[test]
fn unsupported_a1_shape_is_rejected() {
    let document =
        parse_nam(br#"{"version":"0.7.0","architecture":"WaveNet","config":{},"weights":[]}"#)
            .expect("parse");

    assert!(matches!(
        validate_a2_document(&document),
        Err(DspError::InvalidA2Shape)
    ));
}

#[test]
fn every_container_child_must_pass_the_a2_gate() {
    let child: serde_json::Value =
        serde_json::from_slice(&synthetic_a2_json(3).expect("A2")).expect("child JSON");
    let container = json!({
        "version": "0.7.0",
        "architecture": "SlimmableContainer",
        "config": {"submodels": [
            {"max_value": 0.5, "model": child},
            {"max_value": 1.0, "model": child}
        ]},
        "weights": [],
        "sample_rate": 48000
    });
    let document = parse_nam(&serde_json::to_vec(&container).expect("container JSON"))
        .expect("container parse");

    validate_a2_document(&document).expect("valid children should pass");
}

#[test]
fn one_invalid_container_child_rejects_the_whole_document() {
    let child: serde_json::Value =
        serde_json::from_slice(&synthetic_a2_json(3).expect("A2")).expect("child JSON");
    let invalid: serde_json::Value = serde_json::from_slice(
        br#"{"version":"0.7.0","architecture":"WaveNet","config":{},"weights":[]}"#,
    )
    .expect("invalid child JSON");
    let container = json!({
        "version": "0.7.0",
        "architecture": "SlimmableContainer",
        "config": {"submodels": [
            {"max_value": 0.5, "model": child},
            {"max_value": 1.0, "model": invalid}
        ]},
        "weights": [],
        "sample_rate": 48000
    });
    let document = parse_nam(&serde_json::to_vec(&container).expect("container JSON"))
        .expect("container parse");

    assert!(validate_a2_document(&document).is_err());
}

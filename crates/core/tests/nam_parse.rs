//! Bounded NAM parser integration tests.

use ronie_core::{NamArchitecture, NamError, VersionSupport, parse_nam};
use serde as _;
use serde_json::json;
use thiserror as _;

fn document(version: &str, architecture: &str) -> String {
    format!(
        r#"{{
            "version": "{version}",
            "architecture": "{architecture}",
            "config": {{"name": "public-synthetic"}},
            "weights": [0.10000000149011612, -2.5, 3.0],
            "sample_rate": 48000,
            "metadata": {{
                "loudness": -12.5,
                "input_level_dbu": 1.5,
                "output_level_dbu": -1.5,
                "custom": "retained"
            }}
        }}"#
    )
}

fn document_with_nested_config(arrays: usize) -> Vec<u8> {
    let mut text = String::from(r#"{"version":"0.7.0","architecture":"WaveNet","config":"#);
    for _ in 0..arrays {
        text.push('[');
    }
    for _ in 0..arrays {
        text.push(']');
    }
    text.push_str(r#","weights":[]}"#);
    text.into_bytes()
}

#[test]
fn parses_wavenet_and_preserves_document_data() {
    let parsed = parse_nam(document("0.7.0", "WaveNet").as_bytes())
        .expect("valid WaveNet document should parse");

    assert_eq!(parsed.version, "0.7.0");
    assert_eq!(parsed.support, VersionSupport::Yes);
    assert_eq!(parsed.architecture, NamArchitecture::WaveNet);
    assert_eq!(parsed.sample_rate, Some(48_000.0));
    assert_eq!(parsed.config, json!({"name": "public-synthetic"}));
    assert_eq!(parsed.weights, vec![0.1_f32, -2.5, 3.0]);
    assert_eq!(parsed.weights[0].to_bits(), 0x3dcc_cccd);
    assert_eq!(parsed.metadata.loudness, Some(-12.5));
    assert_eq!(parsed.metadata.input_level_dbu, Some(1.5));
    assert_eq!(parsed.metadata.output_level_dbu, Some(-1.5));
    assert_eq!(parsed.metadata.extra["custom"], json!("retained"));
}

#[test]
fn parses_container_with_empty_weights_and_unknown_rate() {
    let input = br#"{
        "version":"0.5.0",
        "architecture":"SlimmableContainer",
        "config":{"models":[]},
        "weights":[]
    }"#;

    let parsed = parse_nam(input).expect("valid container should parse");

    assert_eq!(parsed.architecture, NamArchitecture::SlimmableContainer);
    assert_eq!(parsed.sample_rate, None);
    assert!(parsed.weights.is_empty());
    assert_eq!(parsed.support, VersionSupport::Yes);
}

#[test]
fn rejects_missing_required_fields_and_non_object_roots() {
    for input in [
        r#"{"architecture":"WaveNet","config":{},"weights":[]}"#,
        r#"{"version":"0.7.0","config":{},"weights":[]}"#,
        r#"{"version":"0.7.0","architecture":"WaveNet","weights":[]}"#,
        r#"{"version":"0.7.0","architecture":"WaveNet","config":{}}"#,
        r#"[]"#,
    ] {
        assert!(parse_nam(input.as_bytes()).is_err(), "accepted {input}");
    }
}

#[test]
fn rejects_invalid_version_syntax() {
    for version in ["", "0.7", "0.7.0.0", "0.7.0-beta", "a.7.0", "0.-1.0"] {
        let result = parse_nam(document(version, "WaveNet").as_bytes());
        assert!(
            matches!(result, Err(NamError::InvalidVersion(_))),
            "accepted invalid version {version:?}"
        );
    }
}

#[test]
fn applies_reference_version_support_rules() {
    for version in ["0.4.9", "0.8.0", "1.0.0"] {
        let result = parse_nam(document(version, "WaveNet").as_bytes());
        assert!(
            matches!(result, Err(NamError::UnsupportedVersion(_))),
            "accepted unsupported version {version}"
        );
    }

    let partial = parse_nam(document("0.7.1", "WaveNet").as_bytes())
        .expect("later patch should have partial support");
    assert_eq!(partial.support, VersionSupport::Partial);

    let older_minor = parse_nam(document("0.6.99", "WaveNet").as_bytes())
        .expect("older supported minor should parse fully");
    assert_eq!(older_minor.support, VersionSupport::Yes);
}

#[test]
fn rejects_non_numeric_sample_rates() {
    for value in ["\"48000\"", "null", "true", "[]"] {
        let input = format!(
            r#"{{"version":"0.7.0","architecture":"WaveNet","config":{{}},"weights":[],"sample_rate":{value}}}"#
        );
        assert!(parse_nam(input.as_bytes()).is_err(), "accepted {value}");
    }
}

#[test]
fn rejects_invalid_utf8_and_json() {
    assert!(parse_nam(&[0xff, 0xfe]).is_err());
    assert!(parse_nam(br#"{"version":"0.7.0""#).is_err());
}

#[test]
fn enforces_nesting_depth_64() {
    let at_limit = document_with_nested_config(63);
    let over_limit = document_with_nested_config(64);

    assert!(parse_nam(&at_limit).is_ok());
    assert!(matches!(
        parse_nam(&over_limit),
        Err(NamError::NestingTooDeep)
    ));
}

#[test]
fn ignores_brackets_inside_json_strings_when_measuring_depth() {
    let input = br#"{
        "version":"0.7.0",
        "architecture":"WaveNet",
        "config":{"text":"[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[{{{{"},
        "weights":[]
    }"#;

    assert!(parse_nam(input).is_ok());
}

#[test]
fn rejects_input_larger_than_50_mib_before_parsing() {
    let input = vec![b' '; 50 * 1024 * 1024 + 1];

    assert!(matches!(parse_nam(&input), Err(NamError::InputTooLarge)));
}

//! Combined persistence, filesystem, and network adversarial tests.

use proptest::{prelude::*, test_runner::FileFailurePersistence};
use ronie_core::{
    DownloadClient, FetchRequest, HttpError, ModelSource, OriginPolicy, RecordedTransport,
    SecretString, StateCodec, Tone, decode_wav, read_preset, read_preset_header,
};
use url::Url;

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Off)),
        ..ProptestConfig::default()
    })]

    #[test]
    fn combined_binary_and_json_inputs_are_bounded(bytes in any::<Vec<u8>>()) {
        let _ = StateCodec::new().decode_session(&bytes);
        let _ = read_preset_header(&bytes);
        let _ = read_preset(&bytes);
        let _ = decode_wav(&bytes);
        let _ = Tone::from_json(&String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn valid_prefix_truncation_is_bounded(bytes in any::<Vec<u8>>()) {
        let preset = ronie_core::JuceTree {
            identifier: "T3KPreset".into(),
            properties: vec![("id".into(), ronie_core::JuceValue::String("user:one".into()))],
            children: Vec::new(),
        };
        let encoded = ronie_core::write_preset_v2(&preset).expect("fixture");
        for end in 0..encoded.len() {
            let _ = read_preset_header(&encoded[..end]);
            let _ = read_preset(&encoded[..end]);
        }
        let _ = bytes;
    }

    #[test]
    fn a_persistence_fuzz_case_with_b(
        b in proptest::option::of(prop_oneof![0_i64..4, any::<i64>()]),
        blend in juce_value(),
        a_level in juce_value(),
        b_level in juce_value(),
        invert in juce_value(),
        cut in any::<usize>(),
    ) {
        let mut properties = vec![
            ("id".into(), ronie_core::JuceValue::String("cab".into())),
            ("type".into(), ronie_core::JuceValue::String("IR".into())),
            ("toneJson".into(), ronie_core::JuceValue::String(IR_TONE.into())),
            ("activeModelId".into(), ronie_core::JuceValue::Int64(1)),
            ("irBlend".into(), blend),
            ("irALevel".into(), a_level),
            ("irBLevel".into(), b_level),
            ("irBInvert".into(), invert),
        ];
        if let Some(b) = b {
            properties.push(("irBModelId".into(), ronie_core::JuceValue::Int64(b)));
        }
        let block = ronie_core::JuceTree {
            identifier: "ChainBlock".into(),
            properties,
            children: Vec::new(),
        };
        let root = ronie_core::JuceTree {
            identifier: "Root".into(),
            properties: Vec::new(),
            children: vec![ronie_core::JuceTree {
                identifier: "ChainSnapshot".into(),
                properties: Vec::new(),
                children: vec![ronie_core::JuceTree {
                    identifier: "ChainBlocks".into(),
                    properties: Vec::new(),
                    children: vec![block],
                }],
            }],
        };
        let mut encoded = b"T3KB".to_vec();
        encoded.extend(ronie_core::JuceCodec::default().write_tree(&root).expect("fixture"));
        let codec = StateCodec::new();
        let document = codec.decode_session(&encoded).expect("a B never breaks a session");
        let cab = &document.snapshot.chain.left[0];
        // A B that the stored tone does not list is dropped, so a later save cannot fail on it.
        let kept = b.filter(|id| [1, 2].contains(id));
        prop_assert_eq!(cab.ir_b.map(|ir_b| ir_b.model_id.0), kept);
        let again = codec
            .decode_session(&codec.encode_session(&document).expect("re-encode"))
            .expect("re-decode");
        prop_assert_eq!(&again.snapshot.chain.left[0], cab);
        let _ = codec.decode_session(&encoded[..cut % encoded.len()]);
    }
}

const IR_TONE: &str = r#"{"id":7,"title":"Cab","format":"ir","models":[{"id":1,"name":"A","model_url":"https://fixtures.local/1.wav"},{"id":2,"name":"B","model_url":"https://fixtures.local/2.wav"}]}"#;

fn juce_value() -> impl Strategy<Value = ronie_core::JuceValue> {
    prop_oneof![
        any::<bool>().prop_map(ronie_core::JuceValue::Bool),
        any::<i32>().prop_map(ronie_core::JuceValue::Int),
        any::<i64>().prop_map(ronie_core::JuceValue::Int64),
        any::<f64>().prop_map(ronie_core::JuceValue::Double),
        "[^\\x00]{0,8}".prop_map(ronie_core::JuceValue::String),
    ]
}

#[tokio::test]
async fn network_failures_never_expose_bearer_secrets() {
    let transport =
        RecordedTransport::new(vec![Err(HttpError::Transport), Err(HttpError::Transport)]);
    let client = DownloadClient::new(
        transport.clone(),
        OriginPolicy::new(
            Url::parse("https://api.example").expect("origin"),
            Vec::<Url>::new(),
        ),
    );
    let result = client
        .download_model(FetchRequest {
            source: ModelSource::Catalog {
                url: Url::parse("https://api.example/model.nam").expect("URL"),
            },
            bearer: Some(SecretString::from("super-secret-token")),
        })
        .await;

    let error = result.expect_err("failed download");
    let evidence = format!("{error}{:?}", transport.requests());
    assert!(!evidence.contains("super-secret-token"));
}

#[cfg(unix)]
#[test]
fn interrupted_preset_write_preserves_previous_bytes_and_secrets() {
    use std::os::unix::fs::PermissionsExt;

    use ronie_core::write_preset_v2_file;
    use tempfile::TempDir;

    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("secret.t3kpreset");
    let preset = ronie_core::JuceTree {
        identifier: "T3KPreset".into(),
        properties: vec![
            (
                "id".into(),
                ronie_core::JuceValue::String("user:secret".into()),
            ),
            (
                "token".into(),
                ronie_core::JuceValue::String("do-not-leak".into()),
            ),
        ],
        children: Vec::new(),
    };
    write_preset_v2_file(&path, &preset, false).expect("initial write");
    let before = std::fs::read(&path).expect("before");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o555))
        .expect("lock directory");
    if std::fs::write(temp.path().join("probe"), b"").is_ok() {
        return;
    }

    let result = write_preset_v2_file(&path, &preset, false);

    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).expect("old preset"), before);
    assert!(
        !result
            .expect_err("expected failure")
            .to_string()
            .contains("do-not-leak")
    );
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755))
        .expect("unlock directory");
}

#[test]
fn secret_bearing_debug_output_is_redacted() {
    let request = ronie_core::HttpRequest {
        headers: vec![("Authorization".into(), "Bearer request-secret".into())],
        ..ronie_core::HttpRequest::get(
            Url::parse("https://api.example/model").expect("URL"),
            ronie_core::ResponseKind::Model,
        )
    };
    let pkce = ronie_core::Pkce {
        verifier: "verifier-secret".into(),
        challenge: "public-challenge".into(),
        state: "state-secret".into(),
    };
    let callback = ronie_core::CallbackOutcome::Code("callback-secret".into());
    let response = ronie_core::HttpResponse::new(200, b"response-secret".to_vec());
    let mut properties = ronie_core::PropertiesFile::default();
    properties.set("tokens", "properties-secret");

    let response_evidence = format!("{response:?}");
    assert!(!response_evidence.contains("114, 101, 115, 112"));
    let evidence = format!("{request:?}{pkce:?}{callback:?}{response:?}{properties:?}");
    for secret in [
        "request-secret",
        "verifier-secret",
        "state-secret",
        "callback-secret",
        "response-secret",
        "properties-secret",
    ] {
        assert!(!evidence.contains(secret), "leaked {secret}");
    }
}

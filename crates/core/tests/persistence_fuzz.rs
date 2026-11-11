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

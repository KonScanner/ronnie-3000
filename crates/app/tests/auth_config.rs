//! Production community configuration and session-store wiring tests.

use std::path::Path;

use ronie_app::auth::configured_community_with_values;
use ronie_core::{
    OAuthConfig, OAuthFlow, OAuthTokens, RecordedTransport, SecretString, SessionStore,
    SettingsStore, UI_PREFERENCES_SETTINGS,
};
use ronie_ui::services::session::{AuthFlowAdapter, AuthPhase};
use url::Url;

const ORIGIN: Option<&str> = Some("https://api.example");
const CLIENT: Option<&str> = Some("public-client");

/// An invalid value holds these words in its credentials, path, query, and fragment.
const PRIVATE_WORDS: [&str; 4] = [
    "private-user",
    "private-word",
    "private-path",
    "private-query",
];

fn store(root: &Path) -> SessionStore<OAuthFlow<RecordedTransport>> {
    SessionStore::new(
        SettingsStore::standalone(root, UI_PREFERENCES_SETTINGS),
        OAuthFlow::new(
            OAuthConfig::new(Url::parse("https://api.example").expect("origin"), "other"),
            RecordedTransport::new(Vec::new()),
        ),
    )
}

fn stored_tokens() -> OAuthTokens {
    OAuthTokens {
        access: SecretString::from("stored-access"),
        refresh: SecretString::from("stored-refresh"),
        expires_at_ms: i64::MAX,
    }
}

fn error_text(root: &Path, model_origins: &str) -> String {
    match configured_community_with_values(root, ORIGIN, CLIENT, Some(model_origins)) {
        Ok(_) => panic!("accepted an invalid model-origin list"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn community_configuration_is_optional_but_must_be_complete_and_secure() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let root = directory.path();
    assert!(
        configured_community_with_values(root, None, None, None)
            .expect("no community configuration")
            .is_none()
    );
    assert!(configured_community_with_values(root, ORIGIN, None, None).is_err());
    assert!(configured_community_with_values(root, None, CLIENT, None).is_err());
    assert!(
        configured_community_with_values(root, Some("http://api.example"), CLIENT, None).is_err()
    );
    assert!(configured_community_with_values(root, ORIGIN, Some(""), None).is_err());

    let (auth, handle) = configured_community_with_values(root, ORIGIN, CLIENT, None)
        .expect("valid configuration")
        .expect("configured service")
        .into_parts();
    assert_eq!(auth.snapshot().phase, AuthPhase::Idle);
    assert!(!handle.has_stored_tokens());
}

#[test]
fn a_restored_session_and_both_parts_use_one_settings_file() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let root = directory.path();
    let other_process = store(root);
    other_process.set_tokens(stored_tokens()).expect("seed");

    let (mut auth, handle) = configured_community_with_values(root, ORIGIN, CLIENT, None)
        .expect("valid configuration")
        .expect("configured service")
        .into_parts();

    assert_eq!(auth.snapshot().phase, AuthPhase::Authenticated);
    assert!(handle.has_stored_tokens());

    auth.sign_out().expect("sign out");

    assert!(
        !handle.has_stored_tokens(),
        "the handle reads the same file"
    );
    assert!(
        other_process.read_tokens().expect("read").is_none(),
        "the service wrote the standalone settings file"
    );

    other_process.set_tokens(stored_tokens()).expect("seed");
    assert!(
        handle.has_stored_tokens(),
        "the handle reloads the same file"
    );
}

#[test]
fn model_origins_are_optional_and_accept_only_https_origins() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let root = directory.path();
    for accepted in [
        "",
        "  ",
        "https://models.example",
        "https://models.example/",
        " https://models.example , https://images.example:8443 ",
    ] {
        assert!(
            configured_community_with_values(root, ORIGIN, CLIENT, Some(accepted))
                .expect(accepted)
                .is_some()
        );
    }
}

#[test]
fn an_invalid_model_origin_stops_the_start_with_a_redacted_error() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let root = directory.path();
    for (invalid, position) in [
        ("http://models.example", 1),
        ("models.example", 1),
        ("file:///models", 1),
        ("https://private-user:private-word@models.example", 1),
        ("https://private-user@models.example", 1),
        ("https://models.example/private-path", 1),
        ("https://models.example/?private-query", 1),
        ("https://models.example/?", 1),
        ("https://models.example/#private-query", 1),
        (
            "https://models.example,https://models.example/private-path",
            2,
        ),
        ("https://models.example,", 2),
        (",https://models.example", 1),
    ] {
        let text = error_text(root, invalid);
        assert!(text.contains("RONIE_MODEL_ORIGINS"), "{text}");
        assert!(text.contains(&format!("entry {position}")), "{text}");
        assert!(!text.contains("models.example"), "{text}");
        for word in PRIVATE_WORDS {
            assert!(!text.contains(word), "{text}");
        }
    }
}

#[test]
fn an_invalid_model_origin_stops_the_start_without_oauth_configuration() {
    let directory = tempfile::tempdir().expect("temporary configuration root");

    let result = configured_community_with_values(
        directory.path(),
        None,
        None,
        Some("http://models.example"),
    );

    assert!(result.is_err());
}

/// The launch stops before a window opens. The test removes the display as a second guard.
#[test]
fn the_application_reports_an_invalid_model_origin_and_exits() {
    let directory = tempfile::tempdir().expect("temporary configuration root");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ronie"))
        .args(["--ui", "--dummy"])
        // Each operating system finds its settings folder in another variable.
        .env("XDG_CONFIG_HOME", directory.path())
        .env("HOME", directory.path())
        .env("APPDATA", directory.path())
        .env(
            "RONIE_MODEL_ORIGINS",
            "https://private-user:private-word@models.example/private-path?private-query",
        )
        .env_remove("RONIE_OAUTH_ORIGIN")
        .env_remove("RONIE_OAUTH_CLIENT_ID")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("the application runs");

    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("RONIE_MODEL_ORIGINS"), "{text}");
    assert!(!text.contains("models.example"), "{text}");
    for word in PRIVATE_WORDS {
        assert!(!text.contains(word), "{text}");
    }
}

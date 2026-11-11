//! Properties, settings, and session rotation tests.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use ronie_core::{
    HttpResponse, OAuthConfig, OAuthError, OAuthFlow, OAuthTokens, PropertiesFile,
    RecordedTransport, SecretString, SessionError, SessionStore, SettingsStore, TokenRefresher,
};
use tempfile::TempDir;
use tokio::sync::Notify;

fn tokens(access: &str, refresh: &str, expires_at_ms: i64) -> OAuthTokens {
    OAuthTokens {
        access: SecretString::from(access),
        refresh: SecretString::from(refresh),
        expires_at_ms,
    }
}

#[test]
fn properties_xml_preserves_reference_root_types_and_escaping() {
    let mut properties = PropertiesFile::default();
    properties.set("plain", "value");
    properties.set("escaped", "<&\"");
    properties.set_bool("enabled", true);
    properties.set("number", "42");

    let xml = properties.to_xml();
    let decoded = PropertiesFile::from_xml(&xml).expect("properties XML");

    assert!(xml.contains("<PROPERTIES>"));
    assert!(xml.contains("VALUE name="));
    assert_eq!(decoded.get("plain"), Some("value"));
    assert_eq!(decoded.get("escaped"), Some("<&\""));
    assert!(decoded.get_bool("enabled", false));
    assert!(!decoded.get_bool("missing", false));
}

#[tokio::test]
async fn two_stores_merge_distinct_keys_without_clobbering() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("ui-preferences.settings");
    let first = SettingsStore::new(&path);
    let second = SettingsStore::new(&path);

    let (first_result, second_result) = tokio::join!(
        tokio::task::spawn_blocking(move || first.merge_key("first", "one")),
        tokio::task::spawn_blocking(move || second.merge_key("second", "two")),
    );
    first_result.expect("first task").expect("first merge");
    second_result.expect("second task").expect("second merge");
    let merged = SettingsStore::new(&path).read().expect("read");

    assert_eq!(merged.get("first"), Some("one"));
    assert_eq!(merged.get("second"), Some("two"));
}

#[test]
fn malformed_xml_and_failed_writes_preserve_the_previous_file() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("preferences.settings");
    let store = SettingsStore::new(&path);
    store.merge_key("good", "old").expect("initial write");
    let before = std::fs::read(&path).expect("initial bytes");
    std::fs::write(&path, b"<PROPERTIES><VALUE").expect("corrupt");

    assert!(store.read().is_err());
    assert!(store.merge_key("new", "value").is_err());
    assert_eq!(
        std::fs::read(&path).expect("corrupt remains"),
        b"<PROPERTIES><VALUE"
    );

    std::fs::write(&path, before).expect("restore valid file");
    let valid_before = std::fs::read(&path).expect("valid bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o555))
            .expect("lock directory");
        if std::fs::write(temp.path().join("probe"), b"").is_ok() {
            return;
        }
        assert!(store.merge_key("good", "new").is_err());
        assert_eq!(
            std::fs::read(&path).expect("old file remains"),
            valid_before
        );
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755))
            .expect("unlock directory");
    }
    #[cfg(not(unix))]
    let _ = valid_before;
}

#[derive(Clone)]
struct ControlledRefresh {
    calls: Arc<AtomicUsize>,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl TokenRefresher for ControlledRefresh {
    async fn refresh(
        &self,
        _refresh: SecretString,
        now_ms: i64,
    ) -> Result<OAuthTokens, OAuthError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        self.release.notified().await;
        Ok(tokens(
            "rotated-access",
            "rotated-refresh",
            now_ms + 100_000,
        ))
    }
}

#[tokio::test]
async fn refresh_is_single_flight_and_rotates_tokens_once() {
    let temp = TempDir::new().expect("temp dir");
    let settings = SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let session = SessionStore::new(
        settings.clone(),
        ControlledRefresh {
            calls: Arc::new(AtomicUsize::new(0)),
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        },
    );
    session
        .set_tokens(tokens("old-access", "old-refresh", 0))
        .expect("seed");
    let refresher = session.refresher();
    let first = tokio::spawn({
        let session = session.clone();
        async move { session.access_token_at(1000).await }
    });
    refresher.started.notified().await;
    let second = tokio::spawn({
        let session = session.clone();
        async move { session.access_token_at(1000).await }
    });
    refresher.release.notify_one();

    let first = first.await.expect("task").expect("first");
    let first = secrecy::ExposeSecret::expose_secret(&first);
    let second = second.await.expect("task").expect("second");

    assert_eq!(first, "rotated-access");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&second),
        "rotated-access"
    );
    assert_eq!(refresher.calls.load(Ordering::SeqCst), 1);
    let stored = session.read_tokens().expect("tokens").expect("stored");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&stored.refresh),
        "rotated-refresh"
    );
}

#[derive(Clone)]
struct RejectRefresh;

impl TokenRefresher for RejectRefresh {
    async fn refresh(
        &self,
        _refresh: SecretString,
        _now_ms: i64,
    ) -> Result<OAuthTokens, OAuthError> {
        Err(OAuthError::RefreshFailed)
    }
}

#[tokio::test]
async fn another_process_token_is_adopted_and_cached_user_survives_offline() {
    let temp = TempDir::new().expect("temp dir");
    let settings = SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let session = SessionStore::new(settings.clone(), RejectRefresh);
    session
        .set_tokens(tokens("old-access", "old-refresh", 0))
        .expect("seed");
    session
        .set_cached_user(serde_json::json!({"username": "offline-user"}))
        .expect("user");
    let other = SessionStore::new(settings, RejectRefresh);
    other
        .set_tokens(tokens("other-access", "other-refresh", 100_000))
        .expect("other process");

    let adopted = session.access_token_at(1000).await.expect("adoption");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&adopted),
        "other-access"
    );
    assert_eq!(
        session.cached_user().expect("user")["username"],
        "offline-user"
    );

    let offline = SessionStore::new(
        SettingsStore::new(temp.path().join("offline.settings")),
        RejectRefresh,
    );
    offline
        .set_tokens(tokens("stale-access", "stale-refresh", 0))
        .expect("stale");
    offline
        .set_cached_user(serde_json::json!({"username": "cached"}))
        .expect("user");
    let result = offline.access_token_at(1000).await;

    assert!(matches!(result, Err(SessionError::Offline)));
    assert!(offline.read_tokens().expect("read").is_none());
    assert_eq!(offline.cached_user().expect("cached")["username"], "cached");
    offline.logout().expect("logout");
    assert_eq!(
        offline.cached_user().expect("cached after logout")["username"],
        "cached"
    );
}

#[derive(Clone)]
struct TransientRefresh;

impl TokenRefresher for TransientRefresh {
    async fn refresh(
        &self,
        _refresh: SecretString,
        _now_ms: i64,
    ) -> Result<OAuthTokens, OAuthError> {
        Err(OAuthError::Http(ronie_core::HttpError::ConnectTimeout))
    }
}

#[tokio::test]
async fn transient_refresh_failure_preserves_last_good_tokens() {
    let temp = TempDir::new().expect("temp dir");
    let session = SessionStore::new(
        SettingsStore::new(temp.path().join("ui-preferences.settings")),
        TransientRefresh,
    );
    session
        .set_tokens(tokens("last-access", "last-refresh", 0))
        .expect("seed");

    let result = session.access_token_at(1000).await;

    assert!(matches!(
        result,
        Err(SessionError::OAuth(OAuthError::Http(
            ronie_core::HttpError::ConnectTimeout
        )))
    ));
    assert!(session.read_tokens().expect("read").is_some());
}

fn flow_over(
    responses: Vec<Result<HttpResponse, ronie_core::HttpError>>,
) -> OAuthFlow<RecordedTransport> {
    OAuthFlow::new(
        OAuthConfig::new(
            url::Url::parse("https://api.example").expect("origin"),
            "public-key",
        ),
        RecordedTransport::new(responses),
    )
}

#[tokio::test]
async fn a_transient_server_failure_on_refresh_keeps_the_session() {
    for (status, body) in [
        (503, br#"{"error":"server_error"}"#.as_slice()),
        (500, br"{}".as_slice()),
        (502, b"<html>Bad Gateway</html>".as_slice()),
        (429, br#"{"error":"rate_limited"}"#.as_slice()),
    ] {
        let temp = TempDir::new().expect("temp dir");
        let session = SessionStore::new(
            SettingsStore::new(temp.path().join("ui-preferences.settings")),
            flow_over(vec![
                Ok(HttpResponse::new(status, body.to_vec())),
                Ok(HttpResponse::json(
                    200,
                    serde_json::json!({
                        "access_token": "next-access",
                        "refresh_token": "next-refresh",
                        "expires_in": 3600,
                    }),
                )),
            ]),
        );
        session
            .set_tokens(tokens("last-access", "last-refresh", 0))
            .expect("seed");

        let result = session.access_token_at(1000).await;

        assert!(
            matches!(
                result,
                Err(SessionError::OAuth(OAuthError::ServerFailure(found))) if found == status
            ),
            "HTTP {status}: {result:?}"
        );
        let kept = session.read_tokens().expect("read").unwrap_or_else(|| {
            panic!("HTTP {status} signed the user out");
        });
        assert_eq!(
            secrecy::ExposeSecret::expose_secret(&kept.refresh),
            "last-refresh"
        );

        let next = session.access_token_at(1000).await.expect("the retry");
        assert_eq!(secrecy::ExposeSecret::expose_secret(&next), "next-access");
    }
}

#[tokio::test]
async fn a_rejected_refresh_token_still_signs_the_user_out() {
    let temp = TempDir::new().expect("temp dir");
    let session = SessionStore::new(
        SettingsStore::new(temp.path().join("ui-preferences.settings")),
        flow_over(vec![Ok(HttpResponse::json(
            400,
            serde_json::json!({"error": "invalid_grant"}),
        ))]),
    );
    session
        .set_tokens(tokens("last-access", "last-refresh", 0))
        .expect("seed");

    let result = session.access_token_at(1000).await;

    assert!(matches!(result, Err(SessionError::Offline)), "{result:?}");
    assert!(session.read_tokens().expect("read").is_none());
}

#[derive(Clone)]
struct ControlledRejectRefresh {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl TokenRefresher for ControlledRejectRefresh {
    async fn refresh(
        &self,
        _refresh: SecretString,
        _now_ms: i64,
    ) -> Result<OAuthTokens, OAuthError> {
        self.started.notify_one();
        self.release.notified().await;
        Err(OAuthError::RefreshFailed)
    }
}

#[tokio::test]
async fn rejected_refresh_does_not_remove_newer_process_tokens() {
    let temp = TempDir::new().expect("temp dir");
    let settings = SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let session = SessionStore::new(
        settings.clone(),
        ControlledRejectRefresh {
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        },
    );
    let other = SessionStore::new(settings, RejectRefresh);
    session
        .set_tokens(tokens("old-access", "old-refresh", 0))
        .expect("seed");
    let refresher = session.refresher();
    let task = tokio::spawn(async move { session.access_token_at(1000).await });
    refresher.started.notified().await;
    other
        .set_tokens(tokens("other-access", "other-refresh", 60_000))
        .expect("other process");
    refresher.release.notify_one();

    let result = task.await.expect("task");

    assert!(matches!(result, Err(SessionError::Offline)));
    let stored = other.read_tokens().expect("read").expect("newer tokens");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&stored.refresh),
        "other-refresh"
    );
}

#[tokio::test]
async fn successful_refresh_does_not_overwrite_newer_process_tokens() {
    let temp = TempDir::new().expect("temp dir");
    let settings = SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let session = SessionStore::new(
        settings.clone(),
        ControlledRefresh {
            calls: Arc::new(AtomicUsize::new(0)),
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        },
    );
    let other = SessionStore::new(settings, RejectRefresh);
    session
        .set_tokens(tokens("old-access", "old-refresh", 0))
        .expect("seed");
    let refresher = session.refresher();
    let task = tokio::spawn(async move { session.access_token_at(1000).await });
    refresher.started.notified().await;
    other
        .set_tokens(tokens("other-access", "other-refresh", 60_000))
        .expect("other process");
    refresher.release.notify_one();
    task.await.expect("task").expect("refresh");

    let stored = other.read_tokens().expect("read").expect("newer tokens");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&stored.refresh),
        "other-refresh"
    );
}

#[test]
fn root_relative_settings_reject_traversal_and_absolute_paths() {
    let temp = TempDir::new().expect("temp dir");
    for filename in ["../outside.settings", ".", "/tmp/outside.settings"] {
        let store = SettingsStore::in_root(temp.path(), filename);
        assert!(store.read().is_err(), "accepted {filename}");
        assert!(store.merge_key("key", "value").is_err(), "wrote {filename}");
    }
    assert!(
        SettingsStore::standalone(temp.path(), "../outside.settings")
            .read()
            .is_err()
    );
    assert!(
        SettingsStore::in_root(temp.path(), ronie_core::UI_PREFERENCES_SETTINGS)
            .read()
            .is_ok()
    );
}

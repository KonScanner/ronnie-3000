//! OAuth, session recovery, and sensitive-value redaction tests.

use ronie_core::{HttpResponse, OAuthConfig, OAuthError, OAuthFlow, RecordedTransport};
use ronie_ui::{
    default::{AuthPhase, AuthService},
    services::session::{AuthFlowAdapter, FlowEvent},
};
use secrecy::ExposeSecret;
use tokio::runtime::Handle;
use url::Url;

fn flow(
    responses: Vec<Result<HttpResponse, ronie_core::HttpError>>,
) -> OAuthFlow<RecordedTransport> {
    OAuthFlow::new(
        OAuthConfig::new(
            Url::parse("https://api.example").expect("origin"),
            "public-client",
        ),
        RecordedTransport::new(responses),
    )
}

fn token_json(access: &str, refresh: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        serde_json::json!({"access_token": access, "refresh_token": refresh, "expires_in": 3600}),
    )
}

type Auth = AuthService<RecordedTransport>;

/// The socket of the listener. `localhost` can resolve to `::1` first, and the listener is not
/// there, so an address from name resolution would make a stopped listener look real.
fn listener_address(redirect: &Url) -> std::net::SocketAddr {
    let port = redirect.port().expect("redirect port");
    std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port))
}

/// Waits for the next flow event, while the flow runs on the test runtime.
async fn next_event(auth: &mut Auth) -> FlowEvent {
    for _ in 0..5_000 {
        if let Some(event) = auth.poll() {
            return event;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!("the flow reported nothing");
}

async fn start_browser(auth: &mut Auth) -> String {
    auth.start_browser(&Handle::current());
    match next_event(auth).await {
        FlowEvent::BrowserReady(url) => url,
        other => panic!("the browser flow did not start: {other:?}"),
    }
}

async fn finished(auth: &mut Auth) -> Result<(), OAuthError> {
    match next_event(auth).await {
        FlowEvent::Finished(result) => result,
        other => panic!("the flow did not finish: {other:?}"),
    }
}

#[tokio::test]
async fn browser_handoff_completes_without_exposing_secrets_in_public_state() {
    let flow = flow(vec![Ok(token_json("secret-access", "secret-refresh"))]);
    let mut auth = AuthService::new(flow);
    let authorize = start_browser(&mut auth).await;
    let parsed = Url::parse(&authorize).expect("authorize URL");
    let redirect = parsed
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .expect("redirect")
        .1
        .into_owned();
    let state = parsed
        .query_pairs()
        .find(|(key, _)| key == "state")
        .expect("state")
        .1
        .into_owned();
    let callback =
        Url::parse(&format!("{redirect}?code=secret-code&state={state}")).expect("callback URL");
    let address = listener_address(&callback);
    let mut stream = std::net::TcpStream::connect(address).expect("connect callback");
    use std::io::Write;
    write!(
        stream,
        "GET {}?{} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        callback.path(),
        callback.query().expect("callback query")
    )
    .expect("callback write");
    finished(&mut auth).await.expect("completion");
    assert_eq!(auth.snapshot().phase, AuthPhase::Authenticated);
    let public = format!("{:?}", auth.snapshot());
    assert!(!public.contains("secret-access"));
    assert!(!public.contains("secret-refresh"));
    assert!(!public.contains("secret-code"));
    assert!(!public.contains(&state));
}

#[tokio::test]
async fn copy_link_has_feedback_but_stays_out_of_the_view_model() {
    let flow = flow(vec![]);
    let mut auth = AuthService::new(flow);
    let authorize = start_browser(&mut auth).await;
    assert_eq!(auth.copy_link(), Some(authorize));
    assert!(auth.snapshot().copied);
    let public = format!("{:?}", auth.snapshot());
    assert!(!public.contains("redirect_uri"));
    assert!(!public.contains("code_challenge"));
    auth.close();
}

#[tokio::test]
async fn device_code_expiry_and_replacement_follow_public_phases() {
    let flow = flow(vec![
        Ok(HttpResponse::json(
            200,
            serde_json::json!({
                "device_code": "secret-device-code",
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://api.example/activate",
                "expires_in": 0,
                "interval": 1
            }),
        )),
        Ok(HttpResponse::json(
            400,
            serde_json::json!({"error": "expired_token"}),
        )),
        Ok(HttpResponse::json(
            200,
            serde_json::json!({
                "device_code": "second-device-code",
                "user_code": "IJKL-MNOP",
                "verification_uri": "https://api.example/activate",
                "expires_in": 3600,
                "interval": 1
            }),
        )),
        Ok(token_json("device-access", "device-refresh")),
    ]);
    let mut auth = AuthService::new(flow);
    auth.start_device(&Handle::current());
    while auth.snapshot().phase != AuthPhase::DeviceWaiting {
        assert!(auth.poll().is_none(), "the device code comes first");
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert_eq!(auth.snapshot().user_code.as_deref(), Some("ABCD-EFGH"));
    assert!(matches!(
        finished(&mut auth).await,
        Err(OAuthError::ExpiredDeviceCode)
    ));
    assert_eq!(auth.snapshot().phase, AuthPhase::DeviceFailed);
    let public = format!("{:?}", auth.snapshot());
    assert!(!public.contains("secret-device-code"));
    auth.start_device(&Handle::current());
    finished(&mut auth).await.expect("device tokens");
    assert_eq!(auth.snapshot().phase, AuthPhase::Authenticated);
}

#[tokio::test]
async fn close_cancels_loopback_and_preserves_browse_intent_without_callbacks() {
    let flow = flow(vec![]);
    let mut auth = AuthService::new(flow);
    let _ = start_browser(&mut auth).await;
    auth.close();
    assert_eq!(auth.snapshot().phase, AuthPhase::Idle);
    assert_eq!(auth.snapshot().pending_intent, "browse");
    assert!(auth.poll().is_none(), "a closed flow reports nothing");
}

#[tokio::test]
async fn invalid_state_and_flow_errors_are_public_but_codes_are_not() {
    let flow = flow(vec![]);
    let mut auth = AuthService::new(flow);
    auth.fail("StateMismatch");
    assert_eq!(auth.snapshot().phase, AuthPhase::Error);
    let public = format!("{:?}", auth.snapshot());
    assert!(public.contains("StateMismatch") || public.contains("state"));
    assert!(!public.contains("secret"));
}

#[tokio::test]
async fn browser_completion_can_be_polled_without_blocking() {
    let flow = flow(vec![Ok(token_json("polled-access", "polled-refresh"))]);
    let mut auth = AuthService::new(flow);
    let authorize = start_browser(&mut auth).await;
    assert!(auth.poll().is_none());
    let parsed = Url::parse(&authorize).expect("authorize URL");
    let redirect = parsed
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .expect("redirect")
        .1
        .into_owned();
    let state = parsed
        .query_pairs()
        .find(|(key, _)| key == "state")
        .expect("state")
        .1
        .into_owned();
    let callback =
        Url::parse(&format!("{redirect}?code=polled-code&state={state}")).expect("callback URL");
    let address = listener_address(&callback);
    let mut stream = std::net::TcpStream::connect(address).expect("connect callback");
    use std::io::Write;
    write!(
        stream,
        "GET {}?{} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        callback.path(),
        callback.query().expect("callback query")
    )
    .expect("callback write");
    finished(&mut auth).await.expect("polled completion");
    assert_eq!(auth.snapshot().phase, AuthPhase::Authenticated);
}

#[test]
fn cached_user_survives_signed_out_offline_state() {
    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let session = ronie_core::SessionStore::new(settings, flow(vec![]));
    session
        .set_cached_user(serde_json::json!({"username": "offline-user"}))
        .expect("cached user");
    session.logout().expect("logout");
    assert_eq!(
        session.cached_user().expect("user")["username"],
        "offline-user"
    );
    assert!(session.read_tokens().expect("tokens").is_none());
}

#[test]
fn another_process_rotation_is_adopted_before_refresh_failure() {
    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let session = ronie_core::SessionStore::new(settings.clone(), flow(vec![]));
    session
        .set_tokens(ronie_core::OAuthTokens {
            access: secrecy::SecretString::from("old-access"),
            refresh: secrecy::SecretString::from("old-refresh"),
            expires_at_ms: 0,
        })
        .expect("seed");
    let other = ronie_core::SessionStore::new(settings, flow(vec![]));
    other
        .set_tokens(ronie_core::OAuthTokens {
            access: secrecy::SecretString::from("new-access"),
            refresh: secrecy::SecretString::from("new-refresh"),
            expires_at_ms: 100_000,
        })
        .expect("rotation");
    let adopted = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(session.access_token_at(1000))
        .expect("adopted");
    assert_eq!(adopted.expose_secret(), "new-access");
}

fn complete_browser_callback(authorize: &str) {
    let parsed = Url::parse(authorize).expect("authorize URL");
    let redirect = parsed
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .expect("redirect")
        .1
        .into_owned();
    let state = parsed
        .query_pairs()
        .find(|(key, _)| key == "state")
        .expect("state")
        .1
        .into_owned();
    let callback =
        Url::parse(&format!("{redirect}?code=returning-code&state={state}")).expect("callback URL");
    let address = listener_address(&callback);
    let mut stream = std::net::TcpStream::connect(address).expect("connect callback");
    use std::io::Write;
    write!(
        stream,
        "GET {}?{} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        callback.path(),
        callback.query().expect("callback query")
    )
    .expect("callback write");
}

#[tokio::test]
async fn callback_shows_returning_while_token_exchange_is_stalled() {
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let transport = RecordedTransport::gated(
        vec![Ok(token_json("returning-access", "returning-refresh"))],
        started.clone(),
        release.clone(),
    );
    let mut auth = AuthService::new(OAuthFlow::new(
        OAuthConfig::new(
            Url::parse("https://api.example").expect("origin"),
            "public-client",
        ),
        transport,
    ));
    let authorize = start_browser(&mut auth).await;
    complete_browser_callback(&authorize);
    started.notified().await;
    assert_eq!(auth.snapshot().phase, AuthPhase::Returning);
    release.notify_one();
    finished(&mut auth).await.expect("completion");
}

#[tokio::test]
async fn dropping_auth_cancels_the_loopback_listener() {
    let flow = flow(vec![]);
    let mut auth = AuthService::new(flow);
    let authorize = start_browser(&mut auth).await;
    let parsed = Url::parse(&authorize).expect("authorize URL");
    let redirect = parsed
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .expect("redirect")
        .1
        .into_owned();
    let address = listener_address(&Url::parse(&redirect).expect("redirect URL"));
    let _before =
        std::net::TcpStream::connect(address).expect("the listener must accept before the drop");
    drop(auth);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(std::net::TcpStream::connect(address).is_err());
}

#[test]
fn unconfigured_chrome_rejects_sign_in_instead_of_faking_success() {
    let scenario =
        ronie_ui::testbed::load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "main-empty")
            .expect("main-empty");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario));
    let mut chrome = ronie_ui::default::ChromeController::new(backend, Vec::new(), false, true);
    assert!(chrome.sign_in().is_err());
    assert!(!chrome.session().signed_in());
}

#[tokio::test]
async fn device_request_is_nonblocking_and_pending_polls_continue() {
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::json(
            200,
            serde_json::json!({
                "device_code": "background-device-code",
                "user_code": "WXYZ-1234",
                "verification_uri": "https://api.example/activate",
                "expires_in": 600,
                "interval": 1
            }),
        )),
        Ok(HttpResponse::json(
            400,
            serde_json::json!({"error": "authorization_pending"}),
        )),
        Ok(token_json("approved-access", "approved-refresh")),
    ]);
    let requests = transport.clone();
    let mut auth = AuthService::new(OAuthFlow::new(
        OAuthConfig::new(
            Url::parse("https://api.example").expect("origin"),
            "public-client",
        ),
        transport,
    ));
    auth.start_device(&Handle::current());
    assert_eq!(auth.snapshot().phase, AuthPhase::DeviceRequesting);
    for _ in 0..100 {
        if auth.snapshot().phase == AuthPhase::DeviceWaiting {
            break;
        }
        assert!(auth.poll().is_none());
        tokio::task::yield_now().await;
    }
    assert_eq!(auth.snapshot().phase, AuthPhase::DeviceWaiting);
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    for _ in 0..100 {
        let _ = auth.poll();
        if auth.authenticated() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(auth.authenticated());
    assert_eq!(requests.requests().len(), 3);
}

#[tokio::test]
async fn session_tokens_are_saved_restored_and_removed_on_logout() {
    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let mut auth = AuthService::with_session(
        flow(vec![
            Ok(HttpResponse::json(
                200,
                serde_json::json!({
                    "device_code": "saved-device-code",
                    "user_code": "SAVE-1234",
                    "verification_uri": "https://api.example/activate",
                    "expires_in": 600,
                    "interval": 1
                }),
            )),
            Ok(token_json("saved-access", "saved-refresh")),
        ]),
        settings.clone(),
    );
    auth.start_device(&Handle::current());
    finished(&mut auth).await.expect("tokens");

    let mut restored = AuthService::with_session(flow(vec![]), settings.clone());
    assert!(restored.authenticated());
    assert_eq!(restored.snapshot().phase, AuthPhase::Authenticated);
    restored.sign_out().expect("logout");
    assert!(!restored.authenticated());
    let session = ronie_core::SessionStore::new(settings, flow(vec![]));
    assert!(session.read_tokens().expect("tokens").is_none());
}

//! OAuth PKCE, loopback, device, and token-exchange tests.

use ronie_core::{
    CallbackOutcome, HttpResponse, OAuthConfig, OAuthError, OAuthFlow, Pkce, RecordedTransport,
    parse_callback,
};
use tokio::{io::AsyncWriteExt, net::TcpStream, sync::watch};
use url::Url;

fn flow(
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

/// The socket of the listener. A test uses it to reach the listener without name resolution.
fn listener_address(redirect_uri: &str) -> String {
    let redirect = Url::parse(redirect_uri).expect("redirect URI");
    format!("127.0.0.1:{}", redirect.port().expect("redirect port"))
}

#[test]
fn pkce_matches_rfc_vector_and_generated_material_is_base64url() {
    let challenge = Pkce::challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");

    let generated = Pkce::generate().expect("system randomness");
    assert_eq!(generated.verifier.len(), 43);
    assert_eq!(generated.state.len(), 22);
    assert!(
        generated
            .verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    );
    assert!(
        generated
            .state
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    );
}

#[test]
fn callback_parser_handles_code_cancel_bad_state_and_server_errors() {
    assert_eq!(
        parse_callback("code=abc&state=good", "good").expect("code"),
        CallbackOutcome::Code("abc".into())
    );
    assert_eq!(
        parse_callback("canceled=true&state=good", "good").expect("cancel"),
        CallbackOutcome::Canceled
    );
    assert_eq!(
        parse_callback("code=abc&state=bad", "good").expect_err("state"),
        OAuthError::StateMismatch
    );
    assert_eq!(
        parse_callback("error=access_denied&state=good", "good").expect_err("server"),
        OAuthError::Callback("access_denied".into())
    );
    assert_eq!(
        parse_callback("state=good", "good").expect_err("missing"),
        OAuthError::MissingCode
    );
}

#[tokio::test]
async fn loopback_accepts_one_callback_and_builds_reference_authorize_url() {
    let oauth = flow(Vec::new());
    let browser = oauth.start_loopback().await.expect("loopback");
    let authorize = browser.authorization_url();
    assert!(authorize.starts_with("https://api.example/api/v1/oauth/authorize?"));
    assert!(authorize.contains("code_challenge_method=S256"));
    assert!(authorize.contains("menubar=true"));
    let address = listener_address(&browser.redirect_uri);
    let redirect_uri = browser.redirect_uri.clone();
    let state = browser.state().to_owned();
    let waiter = tokio::spawn(browser.wait_for_callback());

    let mut client = TcpStream::connect(address).await.expect("connect callback");
    client
        .write_all(
            format!("GET /?code=abc&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        )
        .await
        .expect("write callback");
    let callback = waiter.await.expect("waiter").expect("callback");

    assert_eq!(callback.code, "abc");
    assert_eq!(
        callback.redirect_uri, redirect_uri,
        "the token exchange must send the redirect from the authorization request"
    );
}

#[tokio::test]
async fn authorization_url_uses_documented_localhost_redirect() {
    let oauth = flow(Vec::new());
    let browser = oauth.start_loopback().await.expect("loopback");

    let authorize = Url::parse(&browser.authorization_url()).expect("authorize URL");
    let (_, redirect) = authorize
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .expect("redirect field");
    let redirect = Url::parse(&redirect).expect("redirect URI");

    assert_eq!(redirect.scheme(), "http");
    assert_eq!(redirect.host_str(), Some("localhost"));
    assert_eq!(redirect.path(), "/");
    assert_ne!(redirect.port().expect("ephemeral port"), 0);
}

#[tokio::test]
async fn cancellation_stops_the_loopback_listener() {
    let oauth = flow(Vec::new());
    let browser = oauth.start_loopback().await.expect("loopback");
    let address = listener_address(&browser.redirect_uri);
    let (cancel, canceled) = watch::channel(false);
    let waiter = tokio::spawn(browser.wait_for_callback_with_cancel(canceled));
    let _before = TcpStream::connect(&address)
        .await
        .expect("the listener must accept before the cancellation");

    cancel.send(true).expect("cancel");
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("the cancellation must end the wait")
        .expect("waiter");

    assert!(matches!(result, Err(OAuthError::Cancelled)));
    let after = TcpStream::connect(&address)
        .await
        .expect_err("the listener must stop");
    assert_eq!(after.kind(), std::io::ErrorKind::ConnectionRefused);
}

proptest::proptest! {
    #[test]
    fn callback_parser_gives_a_code_only_for_the_expected_state(
        query in ".{0,200}",
        code in "[a-zA-Z0-9]{1,16}",
    ) {
        // Any text from a local process must not panic.
        let _ = ronie_core::parse_callback(&query, "expected");
        let wrong_state = format!("code={code}&state=wrong");
        let right_state = format!("code={code}&state=expected");
        let wrong = ronie_core::parse_callback(&wrong_state, "expected");
        let right = ronie_core::parse_callback(&right_state, "expected");
        proptest::prop_assert!(matches!(wrong, Err(OAuthError::StateMismatch)));
        proptest::prop_assert!(matches!(right, Ok(ronie_core::CallbackOutcome::Code(_))));
    }

    #[test]
    fn garbage_connections_do_not_end_the_loopback_flow(
        junk in proptest::collection::vec(
            proptest::collection::vec(proptest::num::u8::ANY, 0..300),
            0..4,
        ),
    ) {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async move {
            let oauth = flow(Vec::new());
            let browser = oauth.start_loopback().await.expect("loopback");
            let address = listener_address(&browser.redirect_uri);
            let state = browser.state().to_owned();
            let waiter = tokio::spawn(browser.wait_for_callback());
            for bytes in junk {
                let mut stream = TcpStream::connect(&address).await.expect("junk socket");
                // Bytes that are not a request line, ended so that the reader finishes.
                let mut request = b"X ".to_vec();
                request.extend(bytes);
                request.extend(b"\r\n\r\n");
                let _ = stream.write_all(&request).await;
            }
            let mut client = TcpStream::connect(&address).await.expect("callback socket");
            client
                .write_all(format!("GET /?code=abc&state={state} HTTP/1.1\r\n\r\n").as_bytes())
                .await
                .expect("write callback");
            let callback = tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
                .await
                .expect("the flow must still answer")
                .expect("waiter")
                .expect("callback");
            assert_eq!(callback.code, "abc");
        });
    }
}

#[tokio::test]
async fn loopback_survives_idle_sockets_and_favicon_requests_from_a_browser() {
    let oauth = flow(Vec::new());
    let browser = oauth.start_loopback().await.expect("loopback");
    let address = listener_address(&browser.redirect_uri);
    let state = browser.state().to_owned();
    let waiter = tokio::spawn(browser.wait_for_callback());

    // Browsers often open a spare socket first and never write to it.
    let _idle = TcpStream::connect(&address).await.expect("idle socket");
    let mut favicon = TcpStream::connect(&address).await.expect("favicon socket");
    favicon
        .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("write favicon request");
    let mut client = TcpStream::connect(&address)
        .await
        .expect("connect callback");
    client
        .write_all(
            format!("GET /?code=abc&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        )
        .await
        .expect("write callback");

    let callback = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("an idle socket must not delay the callback")
        .expect("waiter")
        .expect("callback");
    assert_eq!(callback.code, "abc");
}

#[tokio::test]
async fn a_callback_with_a_wrong_state_gets_400_and_the_sign_in_continues() {
    let oauth = flow(Vec::new());
    let browser = oauth.start_loopback().await.expect("loopback");
    let address = listener_address(&browser.redirect_uri);
    let state = browser.state().to_owned();
    let waiter = tokio::spawn(browser.wait_for_callback());

    for query in [
        "code=forged&state=wrong",
        "canceled=true&state=wrong",
        "error=access_denied&state=wrong",
        "code=forged",
    ] {
        let mut client = TcpStream::connect(&address).await.expect("connect");
        client
            .write_all(format!("GET /?{query} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .expect("write");
        let mut answer = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut client, &mut answer)
            .await
            .expect("answer");
        assert!(answer.starts_with("HTTP/1.1 400"), "{query}: {answer}");
        assert!(!waiter.is_finished(), "{query} ended the sign-in");
    }

    let mut client = TcpStream::connect(&address).await.expect("connect");
    client
        .write_all(format!("GET /?code=real&state={state} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .expect("write");
    let callback = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("the real callback must end the wait")
        .expect("task")
        .expect("callback");
    assert_eq!(callback.code, "real");
}

#[tokio::test]
async fn loopback_stops_on_a_cancellation_before_any_callback() {
    let oauth = flow(Vec::new());
    let browser = oauth.start_loopback().await.expect("loopback");
    let (_cancel, canceled) = watch::channel(true);
    let result = browser.wait_for_callback_with_cancel(canceled).await;
    assert!(matches!(result, Err(OAuthError::Cancelled)));
}

#[tokio::test]
async fn expired_device_code_and_exchange_failure_are_typed() {
    let oauth = flow(vec![
        Ok(HttpResponse::json(
            400,
            serde_json::json!({"error": "expired_token"}),
        )),
        Ok(HttpResponse::json(
            400,
            serde_json::json!({"error": "invalid_grant"}),
        )),
    ]);

    let expired = oauth
        .poll_device_token(&ronie_core::SecretString::from("device-secret"), 1000)
        .await;
    let exchange = oauth
        .exchange_code("code-secret", "verifier", "http://127.0.0.1:1/", 1000)
        .await;

    assert!(matches!(expired, Err(OAuthError::ExpiredDeviceCode)));
    assert!(matches!(exchange, Err(OAuthError::ExchangeFailed)));
}

#[tokio::test]
async fn a_transient_status_on_refresh_says_nothing_about_the_refresh_token() {
    let refresh = ronie_core::SecretString::from("refresh-secret");
    for (status, body) in [
        (503, br#"{"error":"server_error"}"#.as_slice()),
        (500, br"{}".as_slice()),
        (502, b"<html>Bad Gateway</html>".as_slice()),
        (429, br#"{"error":"slow_down"}"#.as_slice()),
    ] {
        let oauth = flow(vec![Ok(HttpResponse::new(status, body.to_vec()))]);

        let result = oauth.refresh_token(&refresh, 1000).await;

        assert!(
            matches!(result, Err(OAuthError::ServerFailure(found)) if found == status),
            "HTTP {status}: {result:?}"
        );
    }
}

#[tokio::test]
async fn a_client_rejection_of_a_refresh_stays_final() {
    let refresh = ronie_core::SecretString::from("refresh-secret");
    for status in [400, 401, 403] {
        let oauth = flow(vec![Ok(HttpResponse::json(
            status,
            serde_json::json!({"error": "invalid_grant"}),
        ))]);

        let result = oauth.refresh_token(&refresh, 1000).await;

        assert!(
            matches!(result, Err(OAuthError::RefreshFailed)),
            "HTTP {status}: {result:?}"
        );
    }
}

#[tokio::test]
async fn device_pending_and_slow_down_responses_are_typed() {
    let oauth = flow(vec![
        Ok(HttpResponse::json(
            400,
            serde_json::json!({"error": "authorization_pending"}),
        )),
        Ok(HttpResponse::json(
            400,
            serde_json::json!({"error": "slow_down"}),
        )),
    ]);

    let pending = oauth
        .poll_device_token(&ronie_core::SecretString::from("device-secret"), 1000)
        .await;
    let slow_down = oauth
        .poll_device_token(&ronie_core::SecretString::from("device-secret"), 1000)
        .await;

    assert!(matches!(pending, Err(OAuthError::AuthorizationPending)));
    assert!(matches!(slow_down, Err(OAuthError::SlowDown)));
}

#[tokio::test]
async fn device_and_token_requests_match_reference_and_never_log_secrets() {
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::json(
            200,
            serde_json::json!({
                "device_code": "device-secret",
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://api.example/activate",
                "expires_in": 600,
                "interval": 5
            }),
        )),
        Ok(HttpResponse::json(
            200,
            serde_json::json!({
                "access_token": "access-secret",
                "refresh_token": "refresh-secret",
                "expires_in": 3600
            }),
        )),
    ]);
    let oauth = OAuthFlow::new(
        OAuthConfig::new(
            url::Url::parse("https://api.example").expect("origin"),
            "public-key",
        ),
        transport.clone(),
    );

    let device = oauth.request_device_authorization().await.expect("device");
    let tokens = oauth
        .poll_device_token(&device.device_code, 1000)
        .await
        .expect("tokens");

    assert_eq!(tokens.expires_at_ms, 3_601_000);
    let requests = transport.requests();
    assert_eq!(requests[0].url.path(), "/api/v1/oauth/device_authorization");
    assert_eq!(requests[0].method, ronie_core::HttpMethod::Post);
    assert_eq!(
        requests[0].content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(requests[1].url.path(), "/api/v1/oauth/token");
    let evidence = format!("{requests:?}{tokens:?}");
    for secret in ["device-secret", "access-secret", "refresh-secret"] {
        assert!(!evidence.contains(secret), "leaked {secret}");
    }
}

#[tokio::test]
async fn a_device_poll_reports_a_server_failure_with_or_without_json() {
    let oauth = flow(vec![
        Ok(HttpResponse {
            status: 503,
            body: b"<html>Service Unavailable</html>".to_vec(),
        }),
        Ok(HttpResponse::json(
            500,
            serde_json::json!({"error": "server_error"}),
        )),
    ]);
    let code = ronie_core::SecretString::from("device-secret");

    let gateway = oauth.poll_device_token(&code, 1000).await;
    let server = oauth.poll_device_token(&code, 1000).await;

    assert!(matches!(gateway, Err(OAuthError::ServerFailure(503))));
    assert!(matches!(server, Err(OAuthError::ServerFailure(500))));
}

#[tokio::test]
async fn a_device_poll_reports_a_rate_limit_with_or_without_json() {
    let oauth = flow(vec![
        Ok(HttpResponse::json(
            429,
            serde_json::json!({"error": "rate_limited"}),
        )),
        Ok(HttpResponse {
            status: 429,
            body: b"Too Many Requests".to_vec(),
        }),
    ]);
    let code = ronie_core::SecretString::from("device-secret");

    let with_json = oauth.poll_device_token(&code, 1000).await;
    let without_json = oauth.poll_device_token(&code, 1000).await;

    assert!(
        matches!(with_json, Err(OAuthError::ServerFailure(429))),
        "{with_json:?}"
    );
    assert!(
        matches!(without_json, Err(OAuthError::ServerFailure(429))),
        "{without_json:?}"
    );
}

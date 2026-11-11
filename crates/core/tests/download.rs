//! Bounded model/image download and origin-policy tests.

use base64::{Engine, engine::general_purpose::STANDARD};
use std::time::Duration;

use ronie_core::{
    DownloadClient, DownloadError, FetchRequest, HttpError, HttpRequest, HttpResponse,
    HttpTransport, ModelSource, OAuthConfig, OAuthError, OAuthFlow, OAuthTokens, OriginPolicy,
    RecordedTransport, ReqwestTransport, ResponseKind, SecretString, SessionError, SessionStore,
    SettingsStore,
};
use tokio::{net::TcpListener, time::timeout};
use url::Url;

fn source() -> ModelSource {
    ModelSource::Catalog {
        url: Url::parse("https://api.example/model.nam").expect("URL"),
    }
}

fn client(transport: RecordedTransport) -> DownloadClient<RecordedTransport> {
    DownloadClient::new(
        transport,
        OriginPolicy::new(
            Url::parse("https://api.example").expect("upstream"),
            [
                Url::parse("https://legacy.example").expect("legacy"),
                Url::parse("https://images.example").expect("images"),
            ],
        ),
    )
}

/// A local listener that speaks no TLS. A request that reaches it shows that the transport
/// connected. No service answers, so a short deadline ends each request.
async fn local_listener() -> (TcpListener, Url) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let port = listener.local_addr().expect("fixture address").port();
    let url = Url::parse(&format!("https://127.0.0.1:{port}/model.nam")).expect("fixture URL");
    (listener, url)
}

async fn a_connection_arrives(listener: &TcpListener) -> bool {
    timeout(Duration::from_millis(200), listener.accept())
        .await
        .is_ok()
}

fn short_deadline_transport(policy: OriginPolicy) -> ReqwestTransport {
    ReqwestTransport::with_timeouts(
        policy,
        Duration::from_millis(200),
        Duration::from_millis(200),
    )
    .expect("transport")
}

fn bearer_request(url: Url) -> HttpRequest {
    HttpRequest {
        headers: vec![("Authorization".into(), "Bearer secret-token".into())],
        ..HttpRequest::get(url, ResponseKind::Model)
    }
}

#[tokio::test]
async fn transport_refuses_a_bearer_to_an_unapproved_origin_before_transmission() {
    let (listener, url) = local_listener().await;
    let transport = short_deadline_transport(OriginPolicy::new(
        Url::parse("https://api.example").expect("upstream"),
        [Url::parse("https://legacy.example").expect("legacy")],
    ));

    let result = transport.execute(bearer_request(url)).await;

    assert!(matches!(result, Err(HttpError::UnsafeOrigin)), "{result:?}");
    assert!(
        !a_connection_arrives(&listener).await,
        "the transport opened a connection to an unapproved origin"
    );
}

#[tokio::test]
async fn transport_refuses_an_anonymous_request_to_an_unapproved_origin() {
    let (listener, url) = local_listener().await;
    let transport = short_deadline_transport(OriginPolicy::new(
        Url::parse("https://api.example").expect("upstream"),
        Vec::<Url>::new(),
    ));

    let result = transport
        .execute(HttpRequest::get(url, ResponseKind::Image))
        .await;

    assert!(matches!(result, Err(HttpError::UnsafeOrigin)), "{result:?}");
    assert!(!a_connection_arrives(&listener).await);
}

#[tokio::test]
async fn transport_refuses_a_bearer_to_a_public_model_origin_before_transmission() {
    let (listener, url) = local_listener().await;
    let transport = short_deadline_transport(OriginPolicy::new(
        Url::parse("https://api.example").expect("upstream"),
        [url.clone()],
    ));

    let result = transport.execute(bearer_request(url)).await;

    assert!(matches!(result, Err(HttpError::UnsafeOrigin)), "{result:?}");
    assert!(!a_connection_arrives(&listener).await);
}

/// The control for the refusal tests: the same listener sees a request that the policy approves.
#[tokio::test]
async fn transport_connects_to_an_approved_origin() {
    let (listener, url) = local_listener().await;
    let transport = short_deadline_transport(OriginPolicy::new(
        Url::parse("https://api.example").expect("upstream"),
        [url.clone()],
    ));

    let (result, connected) = tokio::join!(
        transport.execute(HttpRequest::get(url, ResponseKind::Model)),
        a_connection_arrives(&listener),
    );

    assert!(connected, "the approved request did not reach the listener");
    assert!(
        !matches!(result, Err(HttpError::UnsafeOrigin)),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_public_model_origin_receives_no_bearer() {
    let transport = RecordedTransport::new(vec![Ok(HttpResponse::new(200, b"public".to_vec()))]);
    let client = client(transport.clone());

    let bytes = client
        .download_model(FetchRequest {
            source: ModelSource::Catalog {
                url: Url::parse("https://legacy.example/model.nam").expect("URL"),
            },
            bearer: Some(SecretString::from("secret-token")),
        })
        .await
        .expect("public model");

    assert_eq!(bytes.as_ref(), b"public");
    assert!(transport.requests()[0].headers.is_empty());
}

#[tokio::test]
async fn an_unapproved_image_origin_never_reaches_the_transport() {
    let transport = RecordedTransport::new(Vec::new());
    let client = client(transport.clone());

    for url in [
        "https://attacker.example/art.png",
        "http://images.example/art.png",
    ] {
        let result = client.download_image(&Url::parse(url).expect("URL")).await;
        assert!(matches!(result, Err(DownloadError::UnsafeOrigin)), "{url}");
    }
    assert!(transport.requests().is_empty());
}

#[tokio::test]
async fn model_download_supports_anonymous_and_redacted_bearer_requests() {
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::new(200, b"anonymous".to_vec())),
        Ok(HttpResponse::new(200, b"bearer".to_vec())),
    ]);
    let client = client(transport.clone());

    let anonymous = client
        .download_model(FetchRequest {
            source: source(),
            bearer: None,
        })
        .await
        .expect("anonymous");
    let bearer = client
        .download_model(FetchRequest {
            source: source(),
            bearer: Some(SecretString::from("secret-token")),
        })
        .await
        .expect("bearer");

    assert_eq!(anonymous.as_ref(), b"anonymous");
    assert_eq!(bearer.as_ref(), b"bearer");
    let requests = transport.requests();
    assert!(
        requests[0]
            .headers
            .iter()
            .all(|(name, _)| name != "Authorization")
    );
    assert_eq!(requests[1].headers[0].1, "Bearer [redacted]");
    assert!(!format!("{requests:?}").contains("secret-token"));
}

#[tokio::test]
async fn empty_download_is_an_error() {
    let client = client(RecordedTransport::new(vec![Ok(HttpResponse::new(
        200,
        Vec::new(),
    ))]));

    let result = client
        .download_model(FetchRequest {
            source: source(),
            bearer: None,
        })
        .await;

    assert!(matches!(result, Err(DownloadError::EmptyDownload)));
}

#[tokio::test]
async fn transient_failure_retries_once_and_permanent_status_does_not_retry() {
    let transport = RecordedTransport::new(vec![
        Err(HttpError::Transport),
        Ok(HttpResponse::new(200, b"model".to_vec())),
        Ok(HttpResponse::new(404, Vec::new())),
    ]);
    let client = client(transport.clone());

    let bytes = client
        .download_model(FetchRequest {
            source: source(),
            bearer: None,
        })
        .await
        .expect("retry succeeds");
    assert_eq!(bytes.as_ref(), b"model");
    let failure = client
        .download_model(FetchRequest {
            source: source(),
            bearer: None,
        })
        .await;

    assert!(matches!(failure, Err(DownloadError::HttpStatus(404))));
    assert_eq!(transport.requests().len(), 3);
}

#[tokio::test]
async fn model_download_enforces_byte_cap() {
    let transport =
        RecordedTransport::with_model_limit(vec![Ok(HttpResponse::new(200, vec![1; 9]))], 8);
    let client = client(transport);

    let result = client
        .download_model(FetchRequest {
            source: source(),
            bearer: None,
        })
        .await;

    assert!(matches!(
        result,
        Err(DownloadError::ResponseTooLarge { max: 8 })
    ));
}

#[tokio::test]
async fn image_404_falls_back_and_valid_png_decodes() {
    let png = STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")
        .expect("PNG fixture");
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::new(404, Vec::new())),
        Ok(HttpResponse::new(200, png)),
    ]);
    let client = client(transport);
    let url = Url::parse("https://images.example/tone.png").expect("image URL");

    assert!(
        client
            .download_image(&url)
            .await
            .expect("404 fallback")
            .is_none()
    );
    let image = client
        .download_image(&url)
        .await
        .expect("PNG decode")
        .expect("image");

    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.rgba.len(), 4);
}

#[test]
fn origin_policy_accepts_only_https_and_documented_origins() {
    let policy = OriginPolicy::new(
        Url::parse("https://api.example").expect("upstream"),
        [Url::parse("https://legacy.example").expect("legacy")],
    );

    assert!(
        policy
            .validate(&Url::parse("https://api.example/model").expect("URL"))
            .is_ok()
    );
    assert!(
        policy
            .validate(&Url::parse("https://legacy.example/model").expect("URL"))
            .is_ok()
    );
    for url in [
        "http://api.example/model",
        "https://attacker.example/model",
        "file:///tmp/model.nam",
        "https://user:secret@api.example/model",
        "https://user@legacy.example/model",
    ] {
        assert!(
            policy.validate(&Url::parse(url).expect("URL")).is_err(),
            "accepted {url}"
        );
    }
}

#[tokio::test]
async fn refused_origin_never_reaches_transport() {
    let transport = RecordedTransport::new(Vec::new());
    let client = client(transport.clone());

    let result = client
        .download_model(FetchRequest {
            source: ModelSource::Catalog {
                url: Url::parse("https://attacker.example/model.nam").expect("URL"),
            },
            bearer: None,
        })
        .await;

    assert!(matches!(result, Err(DownloadError::UnsafeOrigin)));
    assert!(transport.requests().is_empty());
}

#[derive(Clone)]
struct RefreshTokens {
    forced: std::sync::Arc<std::sync::Mutex<Vec<bool>>>,
}

impl ronie_core::TokenProvider for RefreshTokens {
    async fn access_token(
        &self,
        force_refresh: bool,
    ) -> Result<Option<SecretString>, ronie_core::ApiError> {
        self.forced.lock().expect("force calls").push(force_refresh);
        Ok(Some(SecretString::from(if force_refresh {
            "fresh-token"
        } else {
            "stale-token"
        })))
    }
}

#[tokio::test]
async fn model_download_refreshes_once_after_unauthorized_response() {
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::new(401, Vec::new())),
        Ok(HttpResponse::new(200, b"model".to_vec())),
    ]);
    let client = client(transport.clone());
    let tokens = RefreshTokens {
        forced: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let bytes = client
        .download_model_with_tokens(
            FetchRequest {
                source: source(),
                bearer: None,
            },
            &tokens,
        )
        .await
        .expect("refreshed model");

    assert_eq!(bytes.as_ref(), b"model");
    assert_eq!(transport.requests().len(), 2);
    assert_eq!(*tokens.forced.lock().expect("force calls"), [false, true]);
}

#[tokio::test]
async fn unauthorized_download_has_a_typed_outcome() {
    let client = client(RecordedTransport::new(vec![Ok(HttpResponse::new(
        401,
        Vec::new(),
    ))]));

    let result = client
        .download_model(FetchRequest {
            source: source(),
            bearer: None,
        })
        .await;

    assert!(matches!(result, Err(DownloadError::Unauthorized)));
}

#[derive(Clone)]
struct AnonymousTokens;

impl ronie_core::TokenProvider for AnonymousTokens {
    async fn access_token(
        &self,
        _force_refresh: bool,
    ) -> Result<Option<SecretString>, ronie_core::ApiError> {
        Ok(None)
    }
}

fn session_over(
    transport: RecordedTransport,
    directory: &tempfile::TempDir,
) -> SessionStore<OAuthFlow<RecordedTransport>> {
    SessionStore::new(
        SettingsStore::new(directory.path().join("ui-preferences.settings")),
        OAuthFlow::new(
            OAuthConfig::new(
                Url::parse("https://api.example").expect("origin"),
                "public-key",
            ),
            transport,
        ),
    )
}

fn signed_in(session: &SessionStore<OAuthFlow<RecordedTransport>>) {
    session
        .set_tokens(OAuthTokens {
            access: SecretString::from("old-access"),
            refresh: SecretString::from("old-refresh"),
            expires_at_ms: i64::MAX,
        })
        .expect("seed");
}

fn catalog_request(url: &str) -> FetchRequest {
    FetchRequest {
        source: ModelSource::Catalog {
            url: Url::parse(url).expect("URL"),
        },
        bearer: None,
    }
}

#[tokio::test]
async fn a_signed_out_session_downloads_a_public_model_anonymously() {
    let directory = tempfile::tempdir().expect("settings root");
    let refreshes = RecordedTransport::new(Vec::new());
    let session = session_over(refreshes.clone(), &directory);
    let transport = RecordedTransport::new(vec![Ok(HttpResponse::new(200, b"public".to_vec()))]);

    let bytes = client(transport.clone())
        .download_model_with_tokens(catalog_request("https://api.example/model.nam"), &session)
        .await
        .expect("anonymous model");

    assert_eq!(bytes.as_ref(), b"public");
    assert!(transport.requests()[0].headers.is_empty());
    assert!(refreshes.requests().is_empty());
}

#[tokio::test]
async fn an_upstream_model_401_refreshes_the_session_once() {
    let directory = tempfile::tempdir().expect("settings root");
    let refreshes = RecordedTransport::new(vec![Ok(HttpResponse::json(
        200,
        serde_json::json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600}),
    ))]);
    let session = session_over(refreshes.clone(), &directory);
    signed_in(&session);
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::new(401, Vec::new())),
        Ok(HttpResponse::new(401, Vec::new())),
    ]);

    let result = client(transport.clone())
        .download_model_with_tokens(catalog_request("https://api.example/model.nam"), &session)
        .await;

    assert!(
        matches!(result, Err(DownloadError::Unauthorized)),
        "{result:?}"
    );
    assert_eq!(transport.requests().len(), 2, "one retry");
    assert_eq!(refreshes.requests().len(), 1, "one refresh");
    assert_eq!(transport.requests()[0].headers[0].1, "Bearer [redacted]");
}

#[tokio::test]
async fn an_approved_public_origin_works_and_does_not_touch_the_session() {
    let directory = tempfile::tempdir().expect("settings root");
    let refreshes = RecordedTransport::new(Vec::new());
    let session = session_over(refreshes.clone(), &directory);
    signed_in(&session);
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::new(200, b"public".to_vec())),
        Ok(HttpResponse::new(401, Vec::new())),
    ]);
    let client = client(transport.clone());

    let bytes = client
        .download_model_with_tokens(
            catalog_request("https://legacy.example/model.nam"),
            &session,
        )
        .await
        .expect("public model");
    let refused = client
        .download_model_with_tokens(
            catalog_request("https://legacy.example/model.nam"),
            &session,
        )
        .await;

    assert_eq!(bytes.as_ref(), b"public");
    assert!(matches!(refused, Err(DownloadError::Unauthorized)));
    assert!(
        transport
            .requests()
            .iter()
            .all(|request| request.headers.is_empty())
    );
    assert!(
        refreshes.requests().is_empty(),
        "a refresh cannot help here"
    );
}

#[tokio::test]
async fn an_unapproved_model_origin_needs_no_token_and_no_transmission() {
    let directory = tempfile::tempdir().expect("settings root");
    let refreshes = RecordedTransport::new(Vec::new());
    let session = session_over(refreshes.clone(), &directory);
    signed_in(&session);
    let transport = RecordedTransport::new(Vec::new());

    let result = client(transport.clone())
        .download_model_with_tokens(
            catalog_request("https://attacker.example/model.nam"),
            &session,
        )
        .await;

    assert!(
        matches!(result, Err(DownloadError::UnsafeOrigin)),
        "{result:?}"
    );
    assert!(transport.requests().is_empty());
    assert!(refreshes.requests().is_empty());
}

#[tokio::test]
async fn a_failed_refresh_during_a_download_is_a_session_error() {
    let directory = tempfile::tempdir().expect("settings root");
    let refreshes = RecordedTransport::new(vec![Ok(HttpResponse::json(
        503,
        serde_json::json!({"error": "server_error"}),
    ))]);
    let session = session_over(refreshes, &directory);
    signed_in(&session);
    let transport = RecordedTransport::new(vec![Ok(HttpResponse::new(401, Vec::new()))]);

    let result = client(transport)
        .download_model_with_tokens(catalog_request("https://api.example/model.nam"), &session)
        .await;

    assert!(
        matches!(
            result,
            Err(DownloadError::Session(SessionError::OAuth(
                OAuthError::ServerFailure(503)
            )))
        ),
        "{result:?}"
    );
    assert!(session.read_tokens().expect("read").is_some());
}

#[tokio::test]
async fn clones_of_a_download_client_share_one_transport() {
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::new(200, b"first".to_vec())),
        Ok(HttpResponse::new(200, b"second".to_vec())),
    ]);
    let first = client(transport.clone());
    let second = first.clone();

    let one = first
        .download_model(catalog_request("https://api.example/model.nam"))
        .await
        .expect("first");
    let two = second
        .download_model(catalog_request("https://api.example/model.nam"))
        .await
        .expect("second");

    assert_eq!(one.as_ref(), b"first");
    assert_eq!(two.as_ref(), b"second");
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn token_aware_download_preserves_anonymous_public_fallback() {
    let transport =
        RecordedTransport::new(vec![Ok(HttpResponse::new(200, b"public-model".to_vec()))]);
    let client = client(transport.clone());

    let bytes = client
        .download_model_with_tokens(
            FetchRequest {
                source: source(),
                bearer: None,
            },
            &AnonymousTokens,
        )
        .await
        .expect("anonymous public model");

    assert_eq!(bytes.as_ref(), b"public-model");
    assert!(transport.requests()[0].headers.is_empty());
}

//! Catalog query, authentication, parsing, and generation-race tests.

use std::sync::{Arc, Mutex};

use ronie_core::{
    ApiConfig, ApiError, CatalogClient, HttpResponse, Profile, RecordedTransport, SecretString,
    Taxonomy, TokenProvider, ToneId, ToneQuery, ToneSort,
};
use tokio::sync::Notify;
use url::Url;

fn config() -> ApiConfig {
    ApiConfig::new(
        Url::parse("https://api.example").expect("origin"),
        "public-key",
    )
}

#[tokio::test]
async fn catalog_config_applies_request_deadlines() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let transport = RecordedTransport::gated(
        vec![Ok(HttpResponse::json(
            200,
            serde_json::json!({"items": []}),
        ))],
        Arc::clone(&started),
        Arc::clone(&release),
    );
    let mut config = config();
    config.connect_timeout = std::time::Duration::from_millis(7);
    config.body_deadline = std::time::Duration::from_millis(5);
    let client = CatalogClient::new(config, transport.clone());
    let task = tokio::spawn(async move { client.list_trending(None, None).await });
    started.notified().await;

    let outcome = tokio::time::timeout(std::time::Duration::from_millis(100), task).await;
    release.notify_one();

    let response = outcome
        .expect("configured body deadline was not applied")
        .expect("task");
    assert!(matches!(
        response,
        Err(ApiError::Http(ronie_core::HttpError::BodyTimeout))
    ));
    assert_eq!(
        transport.requests()[0].connect_timeout,
        Some(std::time::Duration::from_millis(7))
    );
}

fn tone_json(id: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": "Fixture",
        "format": "nam",
        "models": [{"id": 10, "name": "Standard", "model_url": "https://api.example/model.nam"}]
    })
}

#[derive(Default)]
struct Tokens {
    calls: Mutex<Vec<bool>>,
}

impl TokenProvider for Tokens {
    async fn access_token(&self, force_refresh: bool) -> Result<Option<SecretString>, ApiError> {
        self.calls.lock().expect("calls").push(force_refresh);
        Ok(Some(SecretString::from(if force_refresh {
            "fresh"
        } else {
            "stored"
        })))
    }
}

#[test]
fn tone_query_matches_reference_paths_and_escaping() {
    let empty = ToneQuery::default();
    assert_eq!(
        empty.request_path(1, 12, 2),
        "/api/v1/tones/search?page=1&page_size=12&architecture=2"
    );

    let query = ToneQuery {
        text: " fender twin ".into(),
        sort: Some(ToneSort::Popular),
        gear: "amp-cab".into(),
        format: "nam".into(),
        tags: vec!["metal".into(), "high gain".into()],
        makes: vec!["Fender Twin Reverb".into(), "1965 Vox AC30".into()],
        creators: vec!["creator".into(), "amalgam_audio".into()],
        calibrated: true,
        verified: true,
        profile: Profile::None,
    };
    let path = query.request_path(2, 12, 2);
    assert!(path.contains("page=2&page_size=12&query=fender%20twin&sort=downloads-all-time"));
    assert!(path.contains("&gears=amp-cab&format=nam&tags=metal_high%20gain"));
    assert!(path.contains("&makes=Fender%20Twin%20Reverb_1965%20Vox%20AC30"));
    assert!(
        path.contains(
            "&creators=creator,amalgam_audio&calibrated=true&verified=true&architecture=2"
        )
    );

    let profile = ToneQuery {
        text: " plexi ".into(),
        gear: "pedal".into(),
        profile: Profile::Favorited,
        ..query
    };
    assert_eq!(
        profile.request_path(3, 12, 2),
        "/api/v1/tones/favorited?page=3&page_size=12&query=plexi&gear=pedal"
    );
}

#[tokio::test]
async fn trending_supports_anonymous_and_redacted_bearer_requests() {
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::json(200, serde_json::json!({"data": []}))),
        Ok(HttpResponse::json(200, serde_json::json!({"data": []}))),
    ]);
    let client = CatalogClient::new(config(), transport.clone());

    client
        .list_trending(Some("amp"), None)
        .await
        .expect("anonymous");
    client
        .list_trending(None, Some(SecretString::from("secret-token")))
        .await
        .expect("bearer");

    let requests = transport.requests();
    assert_eq!(
        requests[0].url.as_str(),
        "https://api.example/api/v1/tones/trending?gear=amp"
    );
    assert!(
        requests[0]
            .headers
            .iter()
            .all(|(name, _)| name != "Authorization")
    );
    assert_eq!(
        requests[1].url.as_str(),
        "https://api.example/api/v1/tones/trending"
    );
    assert_eq!(
        requests[1]
            .headers
            .iter()
            .find(|(name, _)| name == "Authorization")
            .map(|(_, value)| value.as_str()),
        Some("Bearer [redacted]")
    );
    assert!(!format!("{requests:?}").contains("secret-token"));
}

#[tokio::test]
async fn unauthorized_response_refreshes_and_retries_exactly_once() {
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::new(401, Vec::new())),
        Ok(HttpResponse::json(200, tone_json(1))),
        Ok(HttpResponse::new(401, Vec::new())),
        Ok(HttpResponse::new(401, Vec::new())),
    ]);
    let client = CatalogClient::new(config(), transport.clone());
    let tokens = Tokens::default();

    client
        .get_tone(ToneId(1), &tokens)
        .await
        .expect("refresh should succeed");
    let second = client.get_tone(ToneId(1), &tokens).await;

    assert!(matches!(second, Err(ApiError::HttpStatus(401))));
    assert_eq!(transport.requests().len(), 4);
    assert_eq!(
        *tokens.calls.lock().expect("calls"),
        [false, true, false, true]
    );
}

#[tokio::test]
async fn malformed_and_oversized_json_have_bounded_errors() {
    let malformed = CatalogClient::new(
        config(),
        RecordedTransport::new(vec![Ok(HttpResponse::new(200, b"{".to_vec()))]),
    );
    let oversized = CatalogClient::new(
        config(),
        RecordedTransport::with_json_limit(vec![Ok(HttpResponse::new(200, vec![b' '; 17]))], 16),
    );

    assert!(matches!(
        malformed.list_trending(None, None).await,
        Err(ApiError::Json(_))
    ));
    assert!(matches!(
        oversized.list_trending(None, None).await,
        Err(ApiError::ResponseTooLarge { max: 16 })
    ));
}

#[tokio::test]
async fn older_page_completion_is_superseded() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let transport = RecordedTransport::gated(
        vec![
            Ok(HttpResponse::json(
                200,
                serde_json::json!({"data": [tone_json(1)], "page": 1, "page_size": 1, "total": 2, "total_pages": 2}),
            )),
            Ok(HttpResponse::json(
                200,
                serde_json::json!({"data": [tone_json(2)], "page": 2, "page_size": 1, "total": 2, "total_pages": 2}),
            )),
        ],
        started.clone(),
        release.clone(),
    );
    let client = Arc::new(CatalogClient::new(config(), transport));
    let tokens = Arc::new(Tokens::default());
    let first = tokio::spawn({
        let client = (*client).clone();
        let tokens = Arc::clone(&tokens);
        async move {
            client
                .list_tones(ToneQuery::default(), 1, 1, 2, tokens.as_ref())
                .await
        }
    });
    started.notified().await;

    let second = client
        .list_tones(ToneQuery::default(), 2, 1, 2, tokens.as_ref())
        .await
        .expect("new page");
    assert_eq!(second.data[0].id.0, 2);
    release.notify_one();
    let first = first.await.expect("first task");

    assert!(matches!(first, Err(ApiError::Superseded { .. })));
}

#[tokio::test]
async fn older_taxonomy_completion_is_superseded() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let transport = RecordedTransport::gated(
        vec![
            Ok(HttpResponse::json(
                200,
                serde_json::json!({"data": [{"id": 1, "name": "old"}]}),
            )),
            Ok(HttpResponse::json(
                200,
                serde_json::json!({"data": [{"id": 2, "name": "new"}]}),
            )),
        ],
        started.clone(),
        release.clone(),
    );
    let client = Arc::new(CatalogClient::new(config(), transport));
    let tokens = Arc::new(Tokens::default());
    let first = tokio::spawn({
        let client = Arc::clone(&client);
        let tokens = Arc::clone(&tokens);
        async move {
            client
                .list_taxonomy(Taxonomy::Tags, "", 20, tokens.as_ref())
                .await
        }
    });
    started.notified().await;

    let second = client
        .list_taxonomy(Taxonomy::Tags, "new", 20, tokens.as_ref())
        .await
        .expect("new taxonomy");
    assert_eq!(second.data[0].name, "new");
    release.notify_one();

    assert!(matches!(
        first.await.expect("task"),
        Err(ApiError::Superseded { .. })
    ));
}

#[tokio::test]
async fn model_listing_returns_typed_models() {
    let transport = RecordedTransport::new(vec![Ok(HttpResponse::json(
        200,
        serde_json::json!({"data": [{"id": 10, "name": "Standard", "model_url": "https://api.example/model.nam"}], "page": 1, "page_size": 20, "total": 1, "total_pages": 1}),
    ))]);
    let client = CatalogClient::new(config(), transport);
    let tokens = Tokens::default();

    let page = client
        .list_models(ToneId(1), 20, 2, &tokens)
        .await
        .expect("models");

    assert_eq!(page.data.len(), 1);
    assert_eq!(page.data[0].id.0, 10);
    assert_eq!(page.total_pages, 1);
}

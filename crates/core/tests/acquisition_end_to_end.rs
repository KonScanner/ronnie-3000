//! End-to-end acquisition, compiled-cache, session, and preset closure tests.

use ronie_core::{
    ApiConfig, ApiError, BlockId, ByteCacheKey, CatalogClient, JuceTree, JuceValue, ModelByteCache,
    ModelId, ModelNamespace, OAuthConfig, OAuthError, OAuthFlow, OAuthTokens, PresetStore,
    RestoreCaches, SecretString, SessionError, SessionStore, SettingsStore, StateCodec,
    TokenProvider, Tone, load_local_path, read_preset_header, restore_offline,
};
use serde_json::json;
use tempfile::TempDir;

fn tree(
    identifier: &str,
    properties: Vec<(String, JuceValue)>,
    children: Vec<JuceTree>,
) -> JuceTree {
    JuceTree {
        identifier: identifier.into(),
        properties,
        children,
    }
}

fn string(value: &str) -> JuceValue {
    JuceValue::String(value.into())
}

fn catalog_tone() -> Tone {
    Tone::from_value(json!({
        "id": 1,
        "title": "Catalog",
        "format": "nam",
        "models": [{"id": 100, "name": "Standard", "model_url": "https://fixtures.local/model.nam"}]
    }))
    .expect("catalog tone")
}

fn block(tone: &Tone, bytes: &[u8]) -> JuceTree {
    let active = tone.models[0].id;
    tree(
        "ChainBlock",
        vec![
            ("id".into(), string("block")),
            ("type".into(), string("NAM")),
            (
                "toneJson".into(),
                string(&tone.to_json().expect("tone JSON")),
            ),
            ("activeModelId".into(), JuceValue::Int64(active.0)),
            ("unknownBlockField".into(), JuceValue::Int(7)),
        ],
        vec![tree(
            "ModelCache",
            Vec::new(),
            vec![tree(
                "CachedModel",
                vec![
                    ("modelId".into(), JuceValue::Int64(active.0)),
                    ("data".into(), JuceValue::Binary(bytes.to_vec())),
                ],
                Vec::new(),
            )],
        )],
    )
}

fn session_bytes(block_tree: JuceTree) -> Vec<u8> {
    let snapshot = tree(
        "ChainSnapshot",
        vec![("stereoEnabled".into(), JuceValue::Bool(false))],
        vec![
            tree("ChainBlocks", Vec::new(), vec![block_tree]),
            tree("RightChainBlocks", Vec::new(), Vec::new()),
        ],
    );
    let root = tree(
        "RonieState",
        vec![
            ("schemaVersion".into(), JuceValue::Int(1)),
            ("unknownRootField".into(), string("retained")),
        ],
        vec![snapshot],
    );
    let mut bytes = b"T3KB".to_vec();
    bytes.extend_from_slice(
        &ronie_core::JuceCodec::default()
            .write_tree(&root)
            .expect("session tree"),
    );
    bytes
}

#[test]
#[ignore = "requires the official source fixture directory"]
fn signed_out_local_model_flows_through_compilation_session_and_offline_restore() {
    let Some(root) = std::env::var_os("RONIE_REFERENCE_ROOT") else {
        return;
    };
    let official = std::fs::read(std::path::PathBuf::from(root).join("test/files/a2-amp-test.nam"))
        .expect("official A2 fixture");
    let temp = TempDir::new().expect("temp dir");
    let input = temp.path().join("input");
    std::fs::create_dir(&input).expect("input dir");
    std::fs::write(input.join("amp.nam"), &official).expect("local fixture");
    let stash = ronie_core::LocalStash::new(temp.path().join("stash")).expect("stash");

    let loaded = load_local_path(&input, &stash, &|_| Ok(())).expect("signed-out local load");
    assert_eq!(loaded.tone.models.len(), 1);
    let bytes = std::fs::read(&loaded.stashed[0].path).expect("stashed bytes");
    let tone = Tone::from_value(loaded.tone.raw.clone()).expect("local tone");
    let document = StateCodec::new()
        .decode_session(&session_bytes(block(&tone, &bytes)))
        .expect("session");
    let byte_cache = ModelByteCache::new(64 * 1024 * 1024);
    let plan = restore_offline(
        &document,
        RestoreCaches {
            bytes: &byte_cache,
            current: None,
        },
    )
    .expect("offline restore");

    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(
        plan.jobs[0].bytes.as_ref().expect("offline bytes").as_ref(),
        bytes
    );
    let byte_key = ByteCacheKey {
        block_id: BlockId("block".into()),
        model_id: plan.jobs[0].model_id,
        namespace: ModelNamespace::Local,
    };
    assert_eq!(byte_cache.get(&byte_key).expect("cache").as_ref(), bytes);

    let store = PresetStore::new(temp.path(), temp.path().join("system"));
    let saved = store
        .save(
            "Offline",
            tree(
                "T3KPreset",
                document.raw.properties.clone(),
                document.raw.children.clone(),
            ),
        )
        .expect("preset save");
    let preset = store.load(&saved.id).expect("preset load");
    assert_eq!(
        preset
            .properties
            .iter()
            .find(|(name, _)| name == "unknownRootField")
            .map(|(_, value)| value),
        Some(&string("retained"))
    );
    assert_eq!(
        read_preset_header(
            &std::fs::read(temp.path().join("Offline.t3kpreset")).expect("preset bytes")
        )
        .expect("header")
        .name,
        "Offline"
    );
}

#[test]
fn cached_catalog_model_restores_without_network() {
    let bytes = b"catalog model";
    let document = StateCodec::new()
        .decode_session(&session_bytes(block(&catalog_tone(), bytes)))
        .expect("session");
    let cache = ModelByteCache::new(1024);

    let plan = restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: None,
        },
    )
    .expect("catalog restore");

    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(
        plan.jobs[0].bytes.as_ref().expect("cached bytes").as_ref(),
        bytes
    );
    let key = ByteCacheKey {
        block_id: BlockId("block".into()),
        model_id: ModelId(100),
        namespace: ModelNamespace::Catalog,
    };
    assert_eq!(cache.get(&key).expect("cached").as_ref(), bytes);
}

#[derive(Clone)]
struct SessionTokens {
    session: SessionStore<OAuthFlow<ronie_core::RecordedTransport>>,
}

impl TokenProvider for SessionTokens {
    async fn access_token(&self, force_refresh: bool) -> Result<Option<SecretString>, ApiError> {
        let result = if force_refresh {
            self.session.refresh_access_token_at(1000).await
        } else {
            self.session.access_token_at(1000).await
        };
        result.map(Some).map_err(|_| ApiError::Unauthorized)
    }
}

#[tokio::test]
async fn authenticated_model_loading_rotates_refresh_once() {
    let refresh_transport = ronie_core::RecordedTransport::new(vec![Ok(
        ronie_core::HttpResponse::json(
            200,
            json!({"access_token": "fresh-access", "refresh_token": "fresh-refresh", "expires_in": 3600}),
        ),
    )]);
    let settings = SettingsStore::new(
        TempDir::new()
            .expect("temp dir")
            .path()
            .join("ui-preferences.settings"),
    );
    let session = SessionStore::new(
        settings,
        OAuthFlow::new(
            OAuthConfig::new(
                url::Url::parse("https://api.example").expect("origin"),
                "public-key",
            ),
            refresh_transport,
        ),
    );
    session
        .set_tokens(OAuthTokens {
            access: SecretString::from("old-access"),
            refresh: SecretString::from("old-refresh"),
            expires_at_ms: 100_000,
        })
        .expect("seed");
    let catalog_transport = ronie_core::RecordedTransport::new(vec![
        Ok(ronie_core::HttpResponse::new(401, Vec::new())),
        Ok(ronie_core::HttpResponse::json(
            200,
            json!({
                "id": 1,
                "title": "Catalog",
                "format": "nam",
                "models": [{"id": 100, "name": "Standard", "model_url": "https://fixtures.local/model.nam"}]
            }),
        )),
    ]);
    let client = CatalogClient::new(
        ApiConfig::new(
            url::Url::parse("https://api.example").expect("origin"),
            "public-key",
        ),
        catalog_transport.clone(),
    );

    let tokens = SessionTokens { session };
    let tone = client
        .get_tone(ronie_core::ToneId(1), &tokens)
        .await
        .expect("refresh and load");

    assert_eq!(tone.id.0, 1);
    assert_eq!(catalog_transport.requests().len(), 2);
    assert_eq!(
        catalog_transport.requests()[1].headers[0].1,
        "Bearer [redacted]"
    );
    let evidence = format!("{:?}", catalog_transport.requests());
    for secret in ["old-access", "old-refresh", "fresh-access", "fresh-refresh"] {
        assert!(!evidence.contains(secret));
    }
    let stored = tokens
        .session
        .read_tokens()
        .expect("read tokens")
        .expect("rotated tokens");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&stored.refresh),
        "fresh-refresh"
    );
}

/// An access token that stays fresh for the length of a test.
const FAR_FUTURE_MS: i64 = i64::MAX;

fn rotated(access: &str, refresh: &str) -> ronie_core::HttpResponse {
    ronie_core::HttpResponse::json(
        200,
        json!({"access_token": access, "refresh_token": refresh, "expires_in": 3600}),
    )
}

fn tone_json() -> ronie_core::HttpResponse {
    ronie_core::HttpResponse::json(200, json!({"id": 1, "title": "Catalog", "format": "nam"}))
}

fn stored(access: &str, refresh: &str, expires_at_ms: i64) -> OAuthTokens {
    OAuthTokens {
        access: SecretString::from(access),
        refresh: SecretString::from(refresh),
        expires_at_ms,
    }
}

fn expose(token: &SecretString) -> &str {
    secrecy::ExposeSecret::expose_secret(token)
}

/// A session over its own settings file, and the transport of its refresh adapter.
struct SessionFixture {
    _directory: TempDir,
    settings: SettingsStore,
    session: SessionStore<OAuthFlow<ronie_core::RecordedTransport>>,
    refreshes: ronie_core::RecordedTransport,
}

impl SessionFixture {
    fn over(refreshes: ronie_core::RecordedTransport) -> Self {
        let directory = TempDir::new().expect("temp dir");
        let settings = SettingsStore::new(directory.path().join("ui-preferences.settings"));
        let session = SessionStore::new(settings.clone(), oauth_flow(refreshes.clone()));
        Self {
            _directory: directory,
            settings,
            session,
            refreshes,
        }
    }

    fn with(responses: Vec<ronie_core::HttpResponse>) -> Self {
        Self::over(ronie_core::RecordedTransport::new(
            responses.into_iter().map(Ok).collect(),
        ))
    }
}

fn oauth_flow(
    transport: ronie_core::RecordedTransport,
) -> OAuthFlow<ronie_core::RecordedTransport> {
    OAuthFlow::new(
        OAuthConfig::new(
            url::Url::parse("https://api.example").expect("origin"),
            "public-key",
        ),
        transport,
    )
}

fn catalog<T: ronie_core::HttpTransport>(transport: T) -> CatalogClient<T> {
    CatalogClient::new(
        ApiConfig::new(
            url::Url::parse("https://api.example").expect("origin"),
            "public-key",
        ),
        transport,
    )
}

fn recorded(responses: Vec<ronie_core::HttpResponse>) -> ronie_core::RecordedTransport {
    ronie_core::RecordedTransport::new(responses.into_iter().map(Ok).collect())
}

#[tokio::test]
async fn a_signed_out_session_gives_no_token_and_sends_no_refresh() {
    let fixture = SessionFixture::with(Vec::new());

    let current = fixture.session.access_token(false).await.expect("lookup");
    let forced = fixture.session.access_token(true).await.expect("forced");

    assert!(current.is_none());
    assert!(forced.is_none());
    assert!(fixture.refreshes.requests().is_empty());
}

#[tokio::test]
async fn a_catalog_401_forces_one_refresh_and_the_retry_uses_the_new_token() {
    let fixture = SessionFixture::with(vec![rotated("new-access", "new-refresh")]);
    fixture
        .session
        .set_tokens(stored("old-access", "old-refresh", FAR_FUTURE_MS))
        .expect("seed");
    let transport = recorded(vec![
        ronie_core::HttpResponse::new(401, Vec::new()),
        tone_json(),
    ]);

    let tone = catalog(transport.clone())
        .get_tone(ronie_core::ToneId(1), &fixture.session)
        .await
        .expect("the retry");

    assert_eq!(tone.id.0, 1);
    assert_eq!(transport.requests().len(), 2);
    assert_eq!(fixture.refreshes.requests().len(), 1);
    let kept = fixture
        .session
        .read_tokens()
        .expect("read")
        .expect("tokens");
    assert_eq!(expose(&kept.access), "new-access");
    assert_eq!(expose(&kept.refresh), "new-refresh");
}

#[tokio::test]
async fn a_second_401_ends_the_request_without_a_second_refresh() {
    let fixture = SessionFixture::with(vec![rotated("new-access", "new-refresh")]);
    fixture
        .session
        .set_tokens(stored("old-access", "old-refresh", FAR_FUTURE_MS))
        .expect("seed");
    let transport = recorded(vec![
        ronie_core::HttpResponse::new(401, Vec::new()),
        ronie_core::HttpResponse::new(401, Vec::new()),
    ]);

    let result = catalog(transport.clone())
        .get_tone(ronie_core::ToneId(1), &fixture.session)
        .await;

    assert!(
        matches!(result, Err(ApiError::HttpStatus(401))),
        "{result:?}"
    );
    assert_eq!(transport.requests().len(), 2);
    assert_eq!(fixture.refreshes.requests().len(), 1);
}

#[tokio::test]
async fn two_callers_with_an_expired_token_share_one_refresh() {
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let fixture = SessionFixture::over(ronie_core::RecordedTransport::gated(
        vec![Ok(rotated("new-access", "new-refresh"))],
        started.clone(),
        release.clone(),
    ));
    fixture
        .session
        .set_tokens(stored("old-access", "old-refresh", 0))
        .expect("seed");

    let first = tokio::spawn({
        let session = fixture.session.clone();
        async move { session.access_token(false).await }
    });
    started.notified().await;
    let second = tokio::spawn({
        let session = fixture.session.clone();
        async move { session.access_token(false).await }
    });
    release.notify_one();

    let first = first.await.expect("task").expect("first").expect("token");
    let second = second.await.expect("task").expect("second").expect("token");
    assert_eq!(expose(&first), "new-access");
    assert_eq!(expose(&second), "new-access");
    assert_eq!(fixture.refreshes.requests().len(), 1);
}

/// A transport that holds its first requests until all of them are in flight, so that callers
/// get their answers at the same time.
struct TogetherTransport {
    inner: ronie_core::RecordedTransport,
    barrier: tokio::sync::Barrier,
    held: std::sync::atomic::AtomicUsize,
}

impl ronie_core::HttpTransport for TogetherTransport {
    async fn execute(
        &self,
        request: ronie_core::HttpRequest,
    ) -> Result<ronie_core::HttpResponse, ronie_core::HttpError> {
        let response = self.inner.execute(request).await;
        let held = self
            .held
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok();
        if held {
            self.barrier.wait().await;
        }
        response
    }
}

#[tokio::test]
async fn two_catalog_calls_rejected_together_share_one_refresh() {
    let fixture = SessionFixture::with(vec![rotated("new-access", "new-refresh")]);
    fixture
        .session
        .set_tokens(stored("old-access", "old-refresh", FAR_FUTURE_MS))
        .expect("seed");
    let recorded_calls = recorded(vec![
        ronie_core::HttpResponse::new(401, Vec::new()),
        ronie_core::HttpResponse::new(401, Vec::new()),
        tone_json(),
        tone_json(),
    ]);
    let client = catalog(TogetherTransport {
        inner: recorded_calls.clone(),
        barrier: tokio::sync::Barrier::new(2),
        held: std::sync::atomic::AtomicUsize::new(2),
    });

    let (first, second) = tokio::join!(
        client.get_tone(ronie_core::ToneId(1), &fixture.session),
        client.get_tone(ronie_core::ToneId(1), &fixture.session),
    );

    first.expect("first");
    second.expect("second");
    assert_eq!(recorded_calls.requests().len(), 4);
    assert_eq!(fixture.refreshes.requests().len(), 1);
}

#[tokio::test]
async fn a_rejected_token_adopts_a_newer_token_from_another_process() {
    let fixture = SessionFixture::with(Vec::new());
    fixture
        .session
        .set_tokens(stored("old-access", "old-refresh", FAR_FUTURE_MS))
        .expect("seed");
    let rejected = fixture
        .session
        .access_token(false)
        .await
        .expect("lookup")
        .expect("token");
    let other = SessionStore::new(fixture.settings.clone(), oauth_flow(recorded(Vec::new())));
    other
        .set_tokens(stored("other-access", "other-refresh", FAR_FUTURE_MS))
        .expect("other process");

    let adopted = fixture
        .session
        .refreshed_token(&rejected)
        .await
        .expect("adoption")
        .expect("token");

    assert_eq!(expose(&adopted), "other-access");
    assert!(fixture.refreshes.requests().is_empty(), "no rotation");
    let current = fixture
        .session
        .access_token(false)
        .await
        .expect("lookup")
        .expect("token");
    assert_eq!(expose(&current), "other-access");
}

#[tokio::test]
async fn a_rejected_token_that_is_still_stored_refreshes() {
    let fixture = SessionFixture::with(vec![rotated("new-access", "new-refresh")]);
    fixture
        .session
        .set_tokens(stored("old-access", "old-refresh", FAR_FUTURE_MS))
        .expect("seed");

    let fresh = fixture
        .session
        .refreshed_token(&SecretString::from("old-access"))
        .await
        .expect("refresh")
        .expect("token");

    assert_eq!(expose(&fresh), "new-access");
    assert_eq!(fixture.refreshes.requests().len(), 1);
}

#[tokio::test]
async fn a_dead_session_stops_after_one_refresh_and_leaks_nothing() {
    let fixture = SessionFixture::with(vec![ronie_core::HttpResponse::json(
        400,
        json!({"error": "invalid_grant"}),
    )]);
    fixture
        .session
        .set_tokens(stored("old-access", "old-refresh", FAR_FUTURE_MS))
        .expect("seed");
    let transport = recorded(vec![ronie_core::HttpResponse::new(401, Vec::new())]);

    let result = catalog(transport.clone())
        .get_tone(ronie_core::ToneId(1), &fixture.session)
        .await;

    assert!(matches!(result, Err(ApiError::Unauthorized)), "{result:?}");
    assert_eq!(transport.requests().len(), 1);
    assert_eq!(fixture.refreshes.requests().len(), 1);
    assert!(fixture.session.read_tokens().expect("read").is_none());
    let evidence = format!(
        "{result:?}{:?}{:?}",
        transport.requests(),
        fixture.refreshes.requests()
    );
    for secret in ["old-access", "old-refresh"] {
        assert!(!evidence.contains(secret));
    }
}

#[tokio::test]
async fn a_failed_refresh_is_not_reported_as_a_sign_out() {
    for (response, expected) in [
        (
            Ok(ronie_core::HttpResponse::json(
                503,
                json!({"error": "server_error"}),
            )),
            OAuthError::ServerFailure(503),
        ),
        (
            Err(ronie_core::HttpError::Transport),
            OAuthError::Http(ronie_core::HttpError::Transport),
        ),
    ] {
        let fixture = SessionFixture::over(ronie_core::RecordedTransport::new(vec![response]));
        fixture
            .session
            .set_tokens(stored("old-access", "old-refresh", FAR_FUTURE_MS))
            .expect("seed");
        let transport = recorded(vec![ronie_core::HttpResponse::new(401, Vec::new())]);

        let result = catalog(transport)
            .get_tone(ronie_core::ToneId(1), &fixture.session)
            .await;

        assert!(
            matches!(
                &result,
                Err(ApiError::Session(SessionError::OAuth(error))) if *error == expected
            ),
            "{result:?}"
        );
        assert!(
            fixture.session.read_tokens().expect("read").is_some(),
            "{expected:?} signed the user out"
        );
    }
}

#[test]
fn folder_retry_and_preset_round_trip_preserve_valid_data() {
    let temp = TempDir::new().expect("temp dir");
    let input = temp.path().join("pack");
    std::fs::create_dir(&input).expect("input dir");
    std::fs::write(input.join("bad.nam"), b"bad").expect("bad file");
    std::fs::write(input.join("good.wav"), wav(&[1, 2, 3])).expect("good file");
    let stash = ronie_core::LocalStash::new(temp.path().join("stash")).expect("stash");

    assert!(load_local_path(&input, &stash, &|_| Ok(())).is_err());
    std::fs::remove_file(input.join("bad.nam")).expect("remove bad");
    let loaded = load_local_path(&input, &stash, &|_| Ok(())).expect("retry");
    assert_eq!(loaded.tone.models.len(), 1);

    let store = PresetStore::new(temp.path(), temp.path().join("system"));
    let saved = store.save("Retry", preset_with_unknown()).expect("save");
    let loaded = store.load(&saved.id).expect("preset round trip");
    assert_eq!(
        loaded
            .properties
            .iter()
            .find(|(name, _)| name == "unknownFuture")
            .map(|(_, value)| value),
        Some(&JuceValue::Int(7))
    );
}

fn preset_with_unknown() -> JuceTree {
    tree(
        "T3KPreset",
        vec![
            ("name".into(), string("Retry")),
            ("unknownFuture".into(), JuceValue::Int(7)),
        ],
        Vec::new(),
    )
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data_len as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[test]
fn token_and_settings_merges_keep_newer_rotation_and_cached_user() {
    let temp = TempDir::new().expect("temp dir");
    let settings = SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let first = SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let second = SettingsStore::new(temp.path().join("ui-preferences.settings"));
    first.merge_key("first", "one").expect("first");
    second.merge_key("second", "two").expect("second");
    let session = SessionStore::new(
        settings,
        OAuthFlow::new(
            OAuthConfig::new(
                url::Url::parse("https://api.example").expect("origin"),
                "public-key",
            ),
            ronie_core::RecordedTransport::new(Vec::new()),
        ),
    );
    session
        .set_cached_user(json!({"username": "offline"}))
        .expect("cached user");
    session
        .set_tokens(OAuthTokens {
            access: SecretString::from("new-access"),
            refresh: SecretString::from("new-refresh"),
            expires_at_ms: 100_000,
        })
        .expect("tokens");

    let merged = session.read_tokens().expect("tokens").expect("present");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&merged.refresh),
        "new-refresh"
    );
    assert_eq!(session.cached_user().expect("user")["username"], "offline");
    let properties = SettingsStore::new(temp.path().join("ui-preferences.settings"))
        .read()
        .expect("properties");
    assert_eq!(properties.get("first"), Some("one"));
    assert_eq!(properties.get("second"), Some("two"));
}

#[tokio::test]
async fn a_malformed_or_oversized_catalog_answer_leaves_the_stored_tokens() {
    let oversized = ronie_core::RecordedTransport::with_json_limit(
        vec![Ok(ronie_core::HttpResponse::new(200, vec![b' '; 17]))],
        16,
    );
    let malformed = recorded(vec![ronie_core::HttpResponse::new(
        200,
        b"{not json".to_vec(),
    )]);
    for (name, transport) in [("oversized", oversized), ("malformed", malformed)] {
        let fixture = SessionFixture::with(Vec::new());
        fixture
            .session
            .set_tokens(stored("kept-access", "kept-refresh", FAR_FUTURE_MS))
            .expect("seed");

        let result = catalog(transport.clone())
            .get_tone(ronie_core::ToneId(1), &fixture.session)
            .await;

        assert!(
            matches!(
                result,
                Err(ApiError::Json(_) | ApiError::ResponseTooLarge { .. })
            ),
            "{name}: {result:?}"
        );
        assert_eq!(transport.requests().len(), 1, "{name}");
        assert!(fixture.refreshes.requests().is_empty(), "{name}");
        let kept = fixture
            .session
            .read_tokens()
            .expect("read")
            .expect("tokens");
        assert_eq!(expose(&kept.access), "kept-access", "{name}");
        assert_eq!(expose(&kept.refresh), "kept-refresh", "{name}");
    }
}

#[tokio::test]
async fn a_rate_limit_or_a_server_failure_of_a_refresh_keeps_the_stored_tokens() {
    for status in [429, 503] {
        let fixture = SessionFixture::with(vec![ronie_core::HttpResponse::new(
            status,
            b"busy".to_vec(),
        )]);
        fixture
            .session
            .set_tokens(stored("kept-access", "kept-refresh", FAR_FUTURE_MS))
            .expect("seed");
        let transport = recorded(vec![ronie_core::HttpResponse::new(401, Vec::new())]);

        let result = catalog(transport.clone())
            .get_tone(ronie_core::ToneId(1), &fixture.session)
            .await;

        assert!(result.is_err(), "{status}");
        assert_eq!(fixture.refreshes.requests().len(), 1, "{status}");
        let kept = fixture
            .session
            .read_tokens()
            .expect("read")
            .expect("tokens");
        assert_eq!(expose(&kept.access), "kept-access", "{status}");
        assert_eq!(expose(&kept.refresh), "kept-refresh", "{status}");
    }
}

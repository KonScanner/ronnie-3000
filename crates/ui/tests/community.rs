//! One shared session behind sign-in, catalog, and downloads. These tests use recorded
//! responses, so that no test contacts a service.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use ronie_core::{
    BlockId, ChainSide, DownloadError, FetchRequest, HttpError, HttpMethod, HttpRequest,
    HttpResponse, HttpTransport, ModelBytes, ModelId, ModelKind, ModelSource, OAuthConfig,
    OAuthTokens, OriginPolicy, Profile, RecordedTransport, SecretString, SessionStore,
    SettingsStore, Taxonomy, TokenProvider, Tone, ToneId, ToneModel, ToneQuery,
    UI_PREFERENCES_SETTINGS,
    params::{ParameterId, ParameterValue},
};
use ronie_ui::{
    backend::{
        ChainViewState, GesturePhase, MeterSnapshot, MockBackend, UiAction, UiBackend, UiError,
        UiReceipt,
    },
    default::{ART_SIDE, AuthPhase, ChromeController, FilterMenu, ModelOption, TileVisualState},
    driver::UiDriver,
    services::{
        AuthService, CatalogStatus, LoadFlow, LoadPhase, ModelSwitchFlow,
        community::{CommunityAccess, CommunityHandle, CommunityService},
        community_worker::{
            CATALOG_PAGE_SIZE, CommunityEvent, CommunityFailure, CommunityScope, CommunityWorker,
            LoadRequest, ModelSwitchRequest, ModelsRequest, QueryRequest, TAXONOMY_PAGE_SIZE,
            TaxonomyRequest,
        },
        session::{AuthFlowAdapter, FlowEvent},
    },
    testbed::load_scenarios,
};
use tempfile::TempDir;
use tokio::{
    runtime::Runtime,
    sync::{Notify, watch},
};
use url::Url;

fn settings(directory: &TempDir) -> SettingsStore {
    SettingsStore::standalone(directory.path(), UI_PREFERENCES_SETTINGS)
}

fn service_over<T: HttpTransport + Clone + Send + Sync + 'static>(
    transport: T,
    directory: &TempDir,
) -> CommunityService<T> {
    CommunityService::new(
        OAuthConfig::new(
            Url::parse("https://api.example").expect("origin"),
            "public-client",
        ),
        OriginPolicy::new(
            Url::parse("https://api.example").expect("upstream"),
            [Url::parse("https://models.example").expect("public models")],
        ),
        transport,
        settings(directory),
    )
}

fn recorded(responses: Vec<HttpResponse>) -> RecordedTransport {
    RecordedTransport::new(responses.into_iter().map(Ok).collect())
}

fn token_response(access: &str, refresh: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        serde_json::json!({"access_token": access, "refresh_token": refresh, "expires_in": 3600}),
    )
}

fn device_response() -> HttpResponse {
    HttpResponse::json(
        200,
        serde_json::json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://api.example/device",
            "interval": 1,
            "expires_in": 600,
        }),
    )
}

fn user_response() -> HttpResponse {
    HttpResponse::json(200, serde_json::json!({"username": "player"}))
}

/// Writes tokens as an earlier launch did, through a store that this launch does not own.
fn earlier_launch_signed_in(directory: &TempDir, expires_at_ms: i64) {
    let earlier = SessionStore::new(
        settings(directory),
        ronie_core::OAuthFlow::new(
            OAuthConfig::new(
                Url::parse("https://api.example").expect("origin"),
                "public-client",
            ),
            recorded(Vec::new()),
        ),
    );
    earlier
        .set_tokens(OAuthTokens {
            access: SecretString::from("stored-access"),
            refresh: SecretString::from("stored-refresh"),
            expires_at_ms,
        })
        .expect("seed");
}

fn finish_device_sign_in(
    auth: &mut AuthService<RecordedTransport>,
    runtime: &tokio::runtime::Runtime,
) {
    auth.start_device(runtime.handle());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(FlowEvent::Finished(result)) = auth.poll() {
            result.expect("device sign-in");
            return;
        }
        assert!(Instant::now() < deadline, "the flow did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn paths(transport: &RecordedTransport) -> Vec<String> {
    transport
        .requests()
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect()
}

#[test]
fn a_service_is_signed_out_and_needs_no_runtime_before_a_flow_starts() {
    let directory = tempfile::tempdir().expect("settings root");
    let transport = recorded(Vec::new());

    let (auth, handle) = service_over(transport.clone(), &directory).into_parts();

    assert_eq!(auth.snapshot().phase, AuthPhase::Idle);
    assert!(!auth.authenticated());
    assert!(!handle.has_stored_tokens());
    assert!(transport.requests().is_empty());
}

#[test]
fn a_sign_in_reaches_the_catalog_through_one_store_and_one_transport() {
    let directory = tempfile::tempdir().expect("settings root");
    let transport = recorded(vec![
        device_response(),
        token_response("access", "refresh"),
        user_response(),
    ]);
    let (mut auth, handle) = service_over(transport.clone(), &directory).into_parts();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    finish_device_sign_in(&mut auth, &runtime);

    assert!(handle.has_stored_tokens());
    let user = runtime
        .block_on(handle.catalog().get_user(handle.tokens()))
        .expect("signed-in user");
    assert_eq!(user["username"], "player");
    assert_eq!(
        paths(&transport),
        [
            "/api/v1/oauth/device_authorization",
            "/api/v1/oauth/token",
            "/api/v1/user",
        ],
        "one transport carries the flow and the catalog"
    );
    let last = transport.requests().pop().expect("catalog request");
    assert_eq!(last.headers[0].1, "Bearer [redacted]");
}

#[test]
fn a_restored_session_serves_both_parts() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let transport = recorded(vec![user_response()]);

    let (auth, handle) = service_over(transport.clone(), &directory).into_parts();

    assert_eq!(auth.snapshot().phase, AuthPhase::Authenticated);
    assert!(handle.has_stored_tokens());
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime
        .block_on(handle.catalog().get_user(handle.tokens()))
        .expect("restored user");
    assert_eq!(paths(&transport), ["/api/v1/user"], "no refresh was needed");
}

#[test]
fn a_sign_out_removes_the_token_for_both_parts() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let (mut auth, handle) = service_over(recorded(Vec::new()), &directory).into_parts();

    auth.sign_out().expect("sign out");

    assert!(!handle.has_stored_tokens());
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let token = runtime
        .block_on(handle.tokens().access_token(false))
        .expect("lookup");
    assert!(token.is_none(), "a signed-out lookup gives no token");
}

#[test]
fn a_catalog_401_rotates_tokens_that_the_next_launch_restores() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let transport = recorded(vec![
        HttpResponse::new(401, Vec::new()),
        token_response("rotated-access", "rotated-refresh"),
        user_response(),
    ]);
    let (_auth, handle) = service_over(transport.clone(), &directory).into_parts();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    runtime
        .block_on(handle.catalog().get_user(handle.tokens()))
        .expect("the retry");

    assert_eq!(
        paths(&transport),
        ["/api/v1/user", "/api/v1/oauth/token", "/api/v1/user"]
    );
    let (next_auth, next_handle) = service_over(recorded(Vec::new()), &directory).into_parts();
    assert_eq!(next_auth.snapshot().phase, AuthPhase::Authenticated);
    let kept = next_handle
        .tokens()
        .read_tokens()
        .expect("read")
        .expect("rotated tokens");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&kept.refresh),
        "rotated-refresh"
    );
}

#[tokio::test]
async fn clones_of_a_handle_share_one_refresh() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, 0);
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let transport = RecordedTransport::gated(
        vec![Ok(token_response("rotated-access", "rotated-refresh"))],
        started.clone(),
        release.clone(),
    );
    let (_auth, handle) = service_over(transport.clone(), &directory).into_parts();
    let token_of = |handle: CommunityHandle<RecordedTransport>| async move {
        handle.tokens().access_token(false).await
    };

    let first = tokio::spawn(token_of(handle.clone()));
    started.notified().await;
    let second = tokio::spawn(token_of(handle.clone()));
    release.notify_one();

    for task in [first, second] {
        let token = task.await.expect("task").expect("token").expect("present");
        assert_eq!(
            secrecy::ExposeSecret::expose_secret(&token),
            "rotated-access"
        );
    }
    assert_eq!(paths(&transport), ["/api/v1/oauth/token"]);
}

#[tokio::test]
async fn the_handle_downloads_from_a_public_origin_without_a_bearer() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let transport = recorded(vec![HttpResponse::new(200, b"model".to_vec())]);
    let (_auth, handle) = service_over(transport.clone(), &directory).into_parts();

    let bytes = handle
        .downloads()
        .download_model_with_tokens(
            FetchRequest {
                source: ModelSource::Catalog {
                    url: Url::parse("https://models.example/model.nam").expect("URL"),
                },
                bearer: None,
            },
            handle.tokens(),
        )
        .await
        .expect("public model");

    assert_eq!(bytes.as_ref(), b"model");
    assert!(transport.requests()[0].headers.is_empty());
}

#[tokio::test]
async fn the_handle_refuses_an_unapproved_model_origin_before_transmission() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let transport = recorded(Vec::new());
    let (_auth, handle) = service_over(transport.clone(), &directory).into_parts();

    let result = handle
        .downloads()
        .download_model_with_tokens(
            FetchRequest {
                source: ModelSource::Catalog {
                    url: Url::parse("https://attacker.example/model.nam").expect("URL"),
                },
                bearer: None,
            },
            handle.tokens(),
        )
        .await;

    assert!(
        matches!(result, Err(DownloadError::UnsafeOrigin)),
        "{result:?}"
    );
    assert!(transport.requests().is_empty());
}

fn backend() -> Arc<MockBackend> {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-mono")
        .expect("main scenario");
    Arc::new(MockBackend::from_scenario(&scenario))
}

#[test]
fn a_chrome_with_a_community_service_holds_the_flow_and_the_handle() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let transport = recorded(Vec::new());

    let mut chrome = ChromeController::with_community(
        backend(),
        Vec::new(),
        false,
        true,
        service_over(transport.clone(), &directory),
    );

    assert!(chrome.session().signed_in(), "the flow restored its tokens");
    assert!(
        chrome
            .community()
            .is_some_and(CommunityAccess::has_stored_tokens)
    );

    chrome.sign_out().expect("sign out");

    assert!(!chrome.session().signed_in());
    assert!(
        !chrome
            .community()
            .is_some_and(CommunityAccess::has_stored_tokens),
        "the handle reads the store that the flow changed"
    );
    assert!(
        transport.requests().is_empty(),
        "this controller starts no catalog work"
    );
}

#[test]
fn a_chrome_with_only_a_fixture_flow_has_no_community_service() {
    let directory = tempfile::tempdir().expect("settings root");
    let (auth, _handle) = service_over(recorded(Vec::new()), &directory).into_parts();

    let chrome =
        ChromeController::with_auth_flow(backend(), Vec::new(), false, true, Box::new(auth));

    assert!(chrome.community().is_none());
}

/// One request that the routed transport saw. It keeps no token.
#[derive(Debug, Clone)]
struct Seen {
    method: HttpMethod,
    target: String,
    path: String,
    bearer: bool,
}

#[derive(Default)]
struct RouteState {
    responses: BTreeMap<String, VecDeque<HttpResponse>>,
    gates: BTreeMap<String, watch::Sender<bool>>,
    seen: Vec<Seen>,
    finished: Vec<String>,
}

/// A transport that answers by path, because concurrent requests have no fixed order. A held
/// path keeps each request until a test releases the path.
#[derive(Clone, Default)]
struct Routes(Arc<Mutex<RouteState>>);

impl Routes {
    fn state(&self) -> std::sync::MutexGuard<'_, RouteState> {
        self.0.lock().expect("route state")
    }

    fn respond(&self, path: &str, response: HttpResponse) -> &Self {
        self.state()
            .responses
            .entry(path.to_owned())
            .or_default()
            .push_back(response);
        self
    }

    fn hold(&self, path: &str) {
        self.state()
            .gates
            .insert(path.to_owned(), watch::channel(false).0);
    }

    fn release(&self, path: &str) {
        if let Some(gate) = self.state().gates.remove(path) {
            gate.send_replace(true);
        }
    }

    fn seen(&self, path: &str) -> Vec<Seen> {
        self.state()
            .seen
            .iter()
            .filter(|seen| seen.path == path)
            .cloned()
            .collect()
    }

    fn count(&self, path: &str) -> usize {
        self.seen(path).len()
    }

    fn finished(&self, path: &str) -> usize {
        self.state()
            .finished
            .iter()
            .filter(|finished| *finished == path)
            .count()
    }
}

impl HttpTransport for Routes {
    fn execute(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send {
        let path = request.url.path().to_owned();
        let (response, gate) = {
            let mut state = self.state();
            state.seen.push(Seen {
                method: request.method,
                target: format!(
                    "{path}?{query}",
                    query = request.url.query().unwrap_or_default()
                ),
                path: path.clone(),
                bearer: request
                    .headers
                    .iter()
                    .any(|(name, _)| name == "Authorization"),
            });
            let response = state
                .responses
                .get_mut(&path)
                .and_then(VecDeque::pop_front)
                .ok_or(HttpError::Transport);
            (
                response,
                state.gates.get(&path).map(watch::Sender::subscribe),
            )
        };
        let routes = self.clone();
        async move {
            if let Some(mut gate) = gate {
                let _ = gate.wait_for(|open| *open).await;
            }
            routes.state().finished.push(path);
            response
        }
    }
}

const SEARCH: &str = "/api/v1/tones/search";
const TRENDING: &str = "/api/v1/tones/trending";
const USER: &str = "/api/v1/user";
const TOKEN: &str = "/api/v1/oauth/token";

fn tone_row(id: i64, title: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "format": "nam",
        "models": [],
        "favorites_count": 4,
        "user": {"username": "maker", "avatar_url": "https://models.example/maker.png"},
    })
}

fn page_of(rows: &[(i64, &str)]) -> HttpResponse {
    HttpResponse::json(
        200,
        serde_json::json!({
            "data": rows.iter().map(|(id, title)| tone_row(*id, title)).collect::<Vec<_>>(),
            "page": 1,
            "page_size": 12,
            "total": 36,
            "total_pages": 3,
        }),
    )
}

fn rejected_refresh() -> HttpResponse {
    HttpResponse::json(400, serde_json::json!({"error": "invalid_grant"}))
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Gives a task that has an answer the time to finish, so that its output waits for a poll.
fn let_tasks_finish() {
    std::thread::sleep(Duration::from_millis(40));
}

fn request(generation: u64, query: ToneQuery, signed_in: bool) -> QueryRequest {
    QueryRequest {
        generation,
        query,
        page: 1,
        signed_in,
    }
}

fn worker_over(
    routes: &Routes,
    directory: &TempDir,
) -> (CommunityWorker<Routes>, CommunityHandle<Routes>) {
    let (_auth, handle) = service_over(routes.clone(), directory).into_parts();
    (CommunityWorker::new(handle.clone()), handle)
}

fn events_until(
    worker: &mut CommunityWorker<Routes>,
    mut done: impl FnMut(&[CommunityEvent]) -> bool,
) -> Vec<CommunityEvent> {
    let mut events = Vec::new();
    wait_for("worker events", || {
        events.extend(worker.poll());
        done(&events)
    });
    events
}

fn query_generations(events: &[CommunityEvent]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|event| match event {
            CommunityEvent::Query { generation, .. } => Some(*generation),
            _ => None,
        })
        .collect()
}

#[test]
fn a_signed_out_query_requests_only_the_public_trending_preview() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes.respond(TRENDING, page_of(&[(1, "Public")]));
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    let query = ToneQuery {
        text: "secret words".to_owned(),
        gear: "pedal".to_owned(),
        tags: vec!["rock".to_owned()],
        profile: Profile::Favorited,
        ..ToneQuery::default()
    };
    worker.start_query(runtime.handle(), request(7, query, false));
    let events = events_until(&mut worker, |events| !events.is_empty());

    let [CommunityEvent::Query { generation, result }] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert_eq!(*generation, 7);
    let reply = result.as_ref().expect("preview");
    assert!(reply.trending);
    assert_eq!(reply.page.data[0].title, "Public");
    let seen = routes.seen(TRENDING);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].target, "/api/v1/tones/trending?gear=pedal");
    assert!(!seen[0].bearer, "the preview is public");
    assert_eq!(routes.count(SEARCH), 0);
}

#[test]
fn a_signed_in_search_uses_the_catalog_page_size_and_a_bearer() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes.respond(SEARCH, page_of(&[(2, "Mine")]));
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    let query = ToneQuery {
        text: "fuzz".to_owned(),
        gear: "pedal".to_owned(),
        ..ToneQuery::default()
    };
    worker.start_query(
        runtime.handle(),
        QueryRequest {
            generation: 3,
            query,
            page: 2,
            signed_in: true,
        },
    );
    let events = events_until(&mut worker, |events| !events.is_empty());

    assert_eq!(CATALOG_PAGE_SIZE, 12);
    let CommunityEvent::Query { result, .. } = &events[0] else {
        panic!("{events:?}");
    };
    assert!(!result.as_ref().expect("page").trending);
    let seen = routes.seen(SEARCH);
    assert_eq!(
        seen[0].target,
        "/api/v1/tones/search?page=2&page_size=12&query=fuzz&gears=pedal"
    );
    assert!(seen[0].bearer);
    assert_eq!(routes.count(TRENDING), 0);
}

#[test]
fn profile_streams_use_their_exact_endpoints_and_park_catalog_filters() {
    for (profile, path) in [
        (Profile::Downloaded, "/api/v1/tones/downloaded"),
        (Profile::Favorited, "/api/v1/tones/favorited"),
        (Profile::Created, "/api/v1/tones/created"),
    ] {
        let directory = tempfile::tempdir().expect("settings root");
        earlier_launch_signed_in(&directory, i64::MAX);
        let routes = Routes::default();
        routes.respond(path, page_of(&[(3, "Stream")]));
        let (mut worker, _handle) = worker_over(&routes, &directory);
        let runtime = Runtime::new().expect("runtime");

        let query = ToneQuery {
            gear: "amp".to_owned(),
            tags: vec!["rock".to_owned()],
            makes: vec!["maker".to_owned()],
            format: "ir".to_owned(),
            verified: true,
            profile,
            ..ToneQuery::default()
        };
        worker.start_query(runtime.handle(), request(1, query, true));
        events_until(&mut worker, |events| !events.is_empty());

        assert_eq!(
            routes.seen(path)[0].target,
            format!("{path}?page=1&page_size=12&gear=amp")
        );
    }
}

#[test]
fn a_taxonomy_request_uses_its_page_size_and_its_kind() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes.respond(
        "/api/v1/makes",
        HttpResponse::json(
            200,
            serde_json::json!({"data": [{"id": 9, "name": "Maker"}], "page": 1, "total_pages": 1}),
        ),
    );
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_taxonomy(
        runtime.handle(),
        TaxonomyRequest {
            generation: 5,
            kind: Taxonomy::Makes,
            text: "ma".to_owned(),
        },
    );
    let events = events_until(&mut worker, |events| !events.is_empty());

    assert_eq!(TAXONOMY_PAGE_SIZE, 25);
    let [
        CommunityEvent::Taxonomy {
            generation: 5,
            kind: Taxonomy::Makes,
            result: Ok(rows),
        },
    ] = events.as_slice()
    else {
        panic!("{events:?}");
    };
    assert_eq!(rows[0].name, "Maker");
    assert_eq!(
        routes.seen("/api/v1/makes")[0].target,
        "/api/v1/makes?page=1&page_size=25&query=ma"
    );
}

#[test]
fn only_the_newest_query_generation_completes() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes
        .respond(SEARCH, page_of(&[(1, "Old")]))
        .respond(SEARCH, page_of(&[(2, "New")]));
    routes.hold(SEARCH);
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_query(runtime.handle(), request(1, ToneQuery::default(), true));
    wait_for("the old request", || routes.count(SEARCH) == 1);
    worker.start_query(runtime.handle(), request(2, ToneQuery::default(), true));
    routes.release(SEARCH);
    let events = events_until(&mut worker, |events| !events.is_empty());
    let_tasks_finish();

    assert_eq!(query_generations(&events), [2]);
    assert!(worker.poll().is_empty());
}

#[test]
fn a_query_that_finished_before_a_newer_query_is_not_delivered() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes
        .respond(SEARCH, page_of(&[(1, "Old")]))
        .respond(SEARCH, page_of(&[(2, "New")]));
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_query(runtime.handle(), request(1, ToneQuery::default(), true));
    wait_for("the old answer", || routes.finished(SEARCH) == 1);
    let_tasks_finish();
    worker.start_query(runtime.handle(), request(2, ToneQuery::default(), true));
    let events = events_until(&mut worker, |events| !events.is_empty());

    assert_eq!(query_generations(&events), [2], "{events:?}");
}

#[test]
fn a_burst_of_keystrokes_transmits_at_most_two_queries() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    for _ in 0..200 {
        routes.respond(SEARCH, page_of(&[(1, "Tone")]));
    }
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    for generation in 1..=200 {
        let query = ToneQuery {
            text: format!("text {generation}"),
            ..ToneQuery::default()
        };
        worker.start_query(runtime.handle(), request(generation, query, true));
    }
    let events = events_until(&mut worker, |events| !events.is_empty());
    let_tasks_finish();

    assert_eq!(query_generations(&events), [200]);
    assert!(routes.count(SEARCH) <= 2, "{}", routes.count(SEARCH));
    assert!(
        routes
            .seen(SEARCH)
            .last()
            .expect("newest")
            .target
            .contains("text%20200")
    );
}

#[test]
fn a_poll_returns_at_once_while_a_request_waits() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes.respond(SEARCH, page_of(&[(1, "Late")]));
    routes.hold(SEARCH);
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_query(runtime.handle(), request(1, ToneQuery::default(), true));
    wait_for("the request", || routes.count(SEARCH) == 1);
    let started = Instant::now();
    for _ in 0..1000 {
        assert!(worker.poll().is_empty());
    }

    assert!(started.elapsed() < Duration::from_secs(1));
    routes.release(SEARCH);
    let events = events_until(&mut worker, |events| !events.is_empty());
    assert_eq!(query_generations(&events), [1]);
}

#[test]
fn a_cancelled_query_keeps_the_token_that_a_refresh_rotated() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, 0);
    let routes = Routes::default();
    routes.respond(TOKEN, token_response("rotated-access", "rotated-refresh"));
    routes.hold(TOKEN);
    let (mut worker, handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_query(runtime.handle(), request(1, ToneQuery::default(), true));
    wait_for("the refresh", || routes.count(TOKEN) == 1);
    worker.cancel(CommunityScope::Browser);
    let_tasks_finish();
    routes.release(TOKEN);

    wait_for("the rotated token", || {
        handle
            .tokens()
            .read_tokens()
            .ok()
            .flatten()
            .is_some_and(|tokens| {
                secrecy::ExposeSecret::expose_secret(&tokens.refresh) == "rotated-refresh"
            })
    });
    assert_eq!(routes.count(SEARCH), 0, "the query was cancelled");
    assert!(worker.poll().is_empty());
}

#[test]
fn a_rejected_refresh_reports_a_lost_session_one_time() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, 0);
    let routes = Routes::default();
    routes.respond(TOKEN, rejected_refresh());
    let (mut worker, handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_query(runtime.handle(), request(1, ToneQuery::default(), true));
    let events = events_until(&mut worker, |events| !events.is_empty());

    assert_eq!(
        events,
        [CommunityEvent::Query {
            generation: 1,
            result: Err(CommunityFailure::SessionLost),
        }]
    );
    assert_eq!(routes.count(TOKEN), 1);
    assert_eq!(routes.count(SEARCH), 0);
    assert!(!handle.has_stored_tokens());
}

#[test]
fn a_failure_of_the_service_or_the_network_is_not_a_lost_session() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes.respond(SEARCH, HttpResponse::new(500, b"secret detail".to_vec()));
    let (mut worker, handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_query(runtime.handle(), request(1, ToneQuery::default(), true));
    let answered = events_until(&mut worker, |events| !events.is_empty());
    worker.start_query(runtime.handle(), request(2, ToneQuery::default(), true));
    let offline = events_until(&mut worker, |events| !events.is_empty());

    for event in answered.iter().chain(&offline) {
        let CommunityEvent::Query {
            result: Err(CommunityFailure::Failed(message)),
            ..
        } = event
        else {
            panic!("{event:?}");
        };
        assert!(!message.contains("secret"), "{message}");
    }
    assert!(handle.has_stored_tokens());
}

#[test]
fn a_refresh_that_cannot_reach_the_service_keeps_the_session() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, 0);
    let routes = Routes::default();
    let (mut worker, handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_query(runtime.handle(), request(1, ToneQuery::default(), true));
    let events = events_until(&mut worker, |events| !events.is_empty());

    assert!(
        matches!(
            events[0],
            CommunityEvent::Query {
                result: Err(CommunityFailure::Failed(_)),
                ..
            }
        ),
        "{events:?}"
    );
    assert!(handle.has_stored_tokens(), "offline is not a sign-out");
}

#[test]
fn a_favorite_write_uses_its_method_and_reports_its_tone() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    let path = "/api/v1/tones/4/favorite";
    routes
        .respond(path, HttpResponse::new(204, Vec::new()))
        .respond(path, HttpResponse::new(500, Vec::new()));
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.start_favorite(runtime.handle(), ToneId(4), true);
    let first = events_until(&mut worker, |events| !events.is_empty());
    worker.start_favorite(runtime.handle(), ToneId(4), false);
    let second = events_until(&mut worker, |events| !events.is_empty());

    assert!(matches!(
        first[0],
        CommunityEvent::Favorite {
            tone: ToneId(4),
            result: Ok(()),
            ..
        }
    ));
    assert!(matches!(
        second[0],
        CommunityEvent::Favorite {
            tone: ToneId(4),
            result: Err(CommunityFailure::Failed(_)),
            ..
        }
    ));
    let seen = routes.seen(path);
    assert_eq!(seen[0].method, HttpMethod::Put);
    assert_eq!(seen[1].method, HttpMethod::Delete);
}

#[test]
fn favorite_writes_are_bounded_and_the_refused_write_fails() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");
    for id in 0..100 {
        routes.hold(&format!("/api/v1/tones/{id}/favorite"));
    }

    for id in 0..100 {
        worker.start_favorite(runtime.handle(), ToneId(id), true);
    }
    let refused = events_until(&mut worker, |events| !events.is_empty());
    let count_writes = || {
        (0..100)
            .map(|id| routes.count(&format!("/api/v1/tones/{id}/favorite")))
            .sum::<usize>()
    };
    // An accepted write first reads its token, so wait until each one reaches the transport.
    wait_for("accepted writes", || count_writes() + refused.len() >= 100);

    let writes = count_writes();
    assert_eq!(writes + refused.len(), 100, "{writes}");
    assert!(writes <= 8, "{writes}");
    assert!(refused.iter().all(|event| matches!(
        event,
        CommunityEvent::Favorite {
            result: Err(CommunityFailure::Failed(_)),
            ..
        }
    )));
}

fn favorite_path(id: i64) -> String {
    format!("/api/v1/tones/{id}/favorite")
}

#[test]
fn a_browser_close_keeps_held_writes_in_the_limit() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");
    for id in 0..10 {
        routes.hold(&favorite_path(id));
        routes.respond(&favorite_path(id), HttpResponse::new(204, Vec::new()));
    }

    for id in 0..8 {
        worker.start_favorite(runtime.handle(), ToneId(id), true);
    }
    wait_for("held writes", || {
        (0..8).all(|id| routes.count(&favorite_path(id)) == 1)
    });
    worker.cancel(CommunityScope::Browser);
    worker.start_favorite(runtime.handle(), ToneId(8), true);
    let refused = events_until(&mut worker, |events| !events.is_empty());

    assert!(matches!(
        refused[0],
        CommunityEvent::Favorite {
            tone: ToneId(8),
            result: Err(CommunityFailure::Failed(_)),
            ..
        }
    ));
    assert_eq!(routes.count(&favorite_path(8)), 0);

    for id in 0..8 {
        routes.release(&favorite_path(id));
    }
    wait_for("detached writes", || {
        (0..8).all(|id| routes.finished(&favorite_path(id)) == 1)
    });
    let mut accepted = None;
    wait_for("a free write slot", || {
        worker.start_favorite(runtime.handle(), ToneId(9), true);
        let events = worker.poll();
        if events.is_empty() {
            accepted = Some(());
            return true;
        }
        false
    });
    assert!(accepted.is_some());
    wait_for("the new write", || routes.count(&favorite_path(9)) == 1);
}

#[test]
fn a_browser_close_keeps_one_write_for_each_tone() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");
    let path = favorite_path(5);
    routes.hold(&path);
    routes.respond(&path, HttpResponse::new(204, Vec::new()));

    worker.start_favorite(runtime.handle(), ToneId(5), true);
    wait_for("the held write", || routes.count(&path) == 1);
    worker.cancel(CommunityScope::Browser);
    worker.start_favorite(runtime.handle(), ToneId(5), false);
    let refused = events_until(&mut worker, |events| !events.is_empty());

    assert!(matches!(
        refused[0],
        CommunityEvent::Favorite {
            tone: ToneId(5),
            result: Err(CommunityFailure::Failed(_)),
            ..
        }
    ));
    assert_eq!(routes.count(&path), 1, "the second write must not be sent");
    routes.release(&path);
}

#[test]
fn the_public_user_is_cached_without_private_fields() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes.respond(
        USER,
        HttpResponse::json(
            200,
            serde_json::json!({
                "username": "player",
                "avatar_url": "https://models.example/player.png",
                "email": "private@example.test",
            }),
        ),
    );
    let (mut worker, handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.refresh_user(runtime.handle());
    let events = events_until(&mut worker, |events| !events.is_empty());

    let CommunityEvent::User {
        result: Ok(user), ..
    } = &events[0]
    else {
        panic!("{events:?}");
    };
    assert_eq!(user["username"], "player");
    assert!(user.get("email").is_none());
    wait_for("the cached user", || {
        worker.poll();
        handle.tokens().cached_user().expect("read")["username"] == "player"
    });
    assert!(
        handle
            .tokens()
            .cached_user()
            .expect("read")
            .get("email")
            .is_none()
    );
}

#[test]
fn a_user_that_finished_before_a_sign_out_is_not_delivered_or_cached() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes.respond(USER, user_response());
    let (mut worker, handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");

    worker.refresh_user(runtime.handle());
    wait_for("the answer", || routes.finished(USER) == 1);
    let_tasks_finish();
    worker.cancel(CommunityScope::Session);

    assert!(worker.poll().is_empty());
    let_tasks_finish();
    assert!(handle.tokens().cached_user().expect("read").is_null());
}

fn empty_chain_backend() -> MockBackend {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-empty")
        .expect("empty scenario");
    MockBackend::from_scenario(&scenario)
}

fn chrome_over(routes: &Routes, directory: &TempDir) -> ChromeController {
    chrome_with(backend(), routes, directory)
}

fn chrome_with(
    backend: Arc<MockBackend>,
    routes: &Routes,
    directory: &TempDir,
) -> ChromeController {
    ChromeController::with_community(
        backend,
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), directory),
    )
}

fn open_browser(chrome: &mut ChromeController) {
    chrome
        .gallery_mut()
        .select_tone(ChainSide::Left, BlockId("left-insert-new".to_owned()));
    chrome.sync_browser();
}

fn pump(
    chrome: &mut ChromeController,
    what: &str,
    mut done: impl FnMut(&ChromeController) -> bool,
) {
    wait_for(what, || {
        chrome.poll_auth();
        chrome.poll_community();
        done(chrome)
    });
}

fn result_titles(chrome: &ChromeController) -> Vec<String> {
    chrome
        .browser()
        .and_then(|browser| browser.store().result())
        .map(|page| page.data.iter().map(|tone| tone.title.clone()).collect())
        .unwrap_or_default()
}

fn signed_in_routes(directory: &TempDir) -> Routes {
    earlier_launch_signed_in(directory, i64::MAX);
    let routes = Routes::default();
    routes.respond(USER, user_response());
    routes
}

fn first_tone(chrome: &ChromeController) -> Tone {
    chrome
        .browser()
        .and_then(|browser| browser.store().result())
        .map(|page| page.data[0].clone())
        .expect("a tone")
}

#[test]
fn opening_the_browser_signed_out_shows_the_trending_preview() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes.respond(TRENDING, page_of(&[(1, "Public")]));
    let mut chrome = chrome_over(&routes, &directory);

    open_browser(&mut chrome);
    pump(&mut chrome, "the preview", |chrome| {
        !result_titles(chrome).is_empty()
    });

    let store = chrome.browser().expect("browser").store();
    assert!(store.result_is_trending());
    assert_eq!(store.status(), CatalogStatus::Ready);
    assert_eq!(routes.count(TRENDING), 1);
    assert_eq!(routes.count(SEARCH), 0);
    assert_eq!(routes.count(USER), 0, "a signed-out user has no account");
}

#[test]
fn a_real_gear_click_signed_out_filters_only_the_preview() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes
        .respond(TRENDING, page_of(&[(1, "Any gear")]))
        .respond(TRENDING, page_of(&[(2, "Pedal gear")]));
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend);

    let _ = driver.click_first("Add Tone").expect("open");
    wait_for("the preview", || {
        driver.frame();
        result_titles(driver.chrome()) == ["Any gear"]
    });
    let _ = driver.click("Pedal").expect("gear");
    wait_for("the gear preview", || {
        driver.frame();
        result_titles(driver.chrome()) == ["Pedal gear"]
    });

    let targets = routes.seen(TRENDING);
    assert_eq!(targets[1].target, "/api/v1/tones/trending?gear=pedal");
    assert_eq!(routes.count(SEARCH), 0);
    assert!(!driver.is_visible("Search tones"));
}

#[test]
fn a_signed_in_browser_searches_and_pins_creator_avatars() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes.respond(SEARCH, page_of(&[(1, "Mine")]));
    let mut chrome = chrome_over(&routes, &directory);

    open_browser(&mut chrome);
    pump(&mut chrome, "the results and the user", |chrome| {
        !result_titles(chrome).is_empty() && chrome.public_user().is_some()
    });

    let store = chrome.browser().expect("browser").store();
    assert!(!store.result_is_trending());
    assert_eq!(
        store.creator_avatar("maker"),
        Some("https://models.example/maker.png")
    );
    assert_eq!(routes.count(TRENDING), 0);
    assert_eq!(
        routes.seen(SEARCH)[0].target,
        "/api/v1/tones/search?page=1&page_size=12"
    );
    assert_eq!(chrome.public_user().expect("user")["username"], "player");
}

#[test]
fn a_chrome_without_a_service_starts_no_query_and_keeps_todays_state() {
    let mut chrome = ChromeController::new(backend(), Vec::new(), true, true);
    open_browser(&mut chrome);
    for _ in 0..3 {
        chrome.poll_community();
    }

    let browser = chrome.browser_mut().expect("browser");
    assert_eq!(browser.store().status(), CatalogStatus::Idle);
    assert_eq!(browser.store().current_generation(), 0);
    let generation = browser.store_mut().begin_query();
    chrome.poll_community();
    let store = chrome.browser().expect("browser").store();
    assert_eq!(store.status(), CatalogStatus::Loading);
    assert_eq!(store.current_generation(), generation);
    assert!(chrome.public_user().is_none());
}

#[test]
fn a_new_query_asks_for_page_one_and_a_turned_page_asks_for_that_page() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    for title in ["First", "Second", "Third"] {
        routes.respond(SEARCH, page_of(&[(1, title)]));
    }
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "page one", |chrome| {
        result_titles(chrome) == ["First"]
    });

    let store = chrome.browser_mut().expect("browser").store_mut();
    assert!(store.turn_page(3));
    pump(&mut chrome, "page three", |chrome| {
        result_titles(chrome) == ["Second"]
    });
    let store = chrome.browser_mut().expect("browser").store_mut();
    store.query_mut().text = "fuzz".to_owned();
    store.begin_query();
    pump(&mut chrome, "a new search", |chrome| {
        result_titles(chrome) == ["Third"]
    });

    let seen = routes.seen(SEARCH);
    assert!(seen[1].target.contains("page=3&page_size=12"), "{seen:?}");
    assert!(
        seen[2].target.contains("page=1&page_size=12&query=fuzz"),
        "{seen:?}"
    );
}

#[test]
fn a_return_keeps_the_query_the_result_and_the_scroll_with_no_request() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes.respond(SEARCH, page_of(&[(1, "Kept")]));
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    let browser = chrome.browser_mut().expect("browser");
    browser.store_mut().query_mut().text = "kept text".to_owned();
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Kept"]
    });
    chrome.browser_mut().expect("browser").set_scroll(240.0);
    let generation = chrome
        .browser()
        .expect("browser")
        .store()
        .current_generation();

    chrome.go_to_main();
    assert!(chrome.browser().is_none());
    open_browser(&mut chrome);
    for _ in 0..5 {
        chrome.poll_community();
    }
    let_tasks_finish();
    chrome.poll_community();

    let browser = chrome.browser().expect("browser");
    assert_eq!(browser.scroll(), 240.0);
    assert_eq!(browser.store().query().text, "kept text");
    assert_eq!(result_titles(&chrome), ["Kept"]);
    assert_eq!(browser.store().status(), CatalogStatus::Ready);
    assert_eq!(routes.count(SEARCH), 1);
    assert_eq!(browser.store().current_generation(), generation + 1);
}

#[test]
fn a_query_that_finished_before_the_browser_closed_is_not_delivered() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Late")]))
        .respond(SEARCH, page_of(&[(2, "Fresh")]));
    routes.hold(SEARCH);
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the request", |_| routes.count(SEARCH) == 1);
    routes.release(SEARCH);
    wait_for("the answer", || routes.finished(SEARCH) == 1);
    let_tasks_finish();

    chrome.go_to_main();
    chrome.poll_community();
    open_browser(&mut chrome);
    assert!(result_titles(&chrome).is_empty(), "the close wins");

    pump(&mut chrome, "a fresh query", |chrome| {
        !result_titles(chrome).is_empty()
    });
    assert_eq!(result_titles(&chrome), ["Fresh"], "the return asks again");
}

#[test]
fn a_query_of_a_browser_that_a_load_replaced_cannot_reach_the_next_browser() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Old store")]))
        .respond(SEARCH, page_of(&[(2, "New store")]));
    routes.hold(SEARCH);
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the request", |_| routes.count(SEARCH) == 1);
    routes.release(SEARCH);
    wait_for("the answer", || routes.finished(SEARCH) == 1);
    let_tasks_finish();

    let tone = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-detail")
        .map(|scenario| {
            ronie_ui::backend::UiBackend::chain_state(&MockBackend::from_scenario(&scenario), 0)
                .chain
                .left[0]
                .tone
                .clone()
                .expect("tone")
        })
        .expect("scenario");
    let generation = chrome
        .load_flow_mut()
        .begin_load(tone.clone())
        .expect("load");
    assert!(chrome.load_flow_mut().complete_cached(generation, tone));
    chrome.sync_browser();
    assert!(chrome.browser().is_none(), "the load closed the browser");

    open_browser(&mut chrome);
    let store = chrome.browser_mut().expect("browser").store_mut();
    assert_eq!(
        store.begin_query(),
        1,
        "a new store counts from the same number as the old store"
    );
    pump(&mut chrome, "the new result", |chrome| {
        !result_titles(chrome).is_empty()
    });
    assert_eq!(result_titles(&chrome), ["New store"]);
}

#[test]
fn a_query_that_a_close_interrupted_runs_again_on_return_for_the_newest_text() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Old text")]))
        .respond(SEARCH, page_of(&[(2, "New text")]))
        .respond(SEARCH, page_of(&[(3, "Again")]));
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Old text"]
    });
    routes.hold(SEARCH);
    let store = chrome.browser_mut().expect("browser").store_mut();
    store.query_mut().text = "new".to_owned();
    store.begin_query();
    pump(&mut chrome, "the second request", |_| {
        routes.count(SEARCH) == 2
    });

    chrome.go_to_main();
    routes.release(SEARCH);
    let_tasks_finish();
    chrome.poll_community();
    open_browser(&mut chrome);
    pump(&mut chrome, "the rerun", |chrome| {
        result_titles(chrome) == ["Again"]
    });

    assert!(routes.seen(SEARCH)[2].target.contains("query=new"));
}

#[test]
fn a_query_that_finished_before_a_sign_out_is_not_delivered() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes.respond(SEARCH, page_of(&[(1, "Private")]));
    routes.respond(TRENDING, page_of(&[(2, "Public")]));
    routes.hold(SEARCH);
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the request", |_| routes.count(SEARCH) == 1);
    routes.release(SEARCH);
    wait_for("the answer", || routes.finished(SEARCH) == 1);
    let_tasks_finish();

    chrome.sign_out().expect("sign out");
    chrome.poll_community();
    assert!(!result_titles(&chrome).contains(&"Private".to_owned()));

    pump(&mut chrome, "the preview", |chrome| {
        !result_titles(chrome).is_empty()
    });
    assert_eq!(result_titles(&chrome), ["Public"]);
    assert!(
        chrome
            .browser()
            .expect("browser")
            .store()
            .result_is_trending()
    );
}

#[test]
fn a_closed_editor_stops_its_work_and_writes_nothing() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes.respond(SEARCH, page_of(&[(1, "Late")]));
    routes.hold(SEARCH);
    routes.hold(USER);
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the requests", |_| {
        routes.count(SEARCH) == 1 && routes.count(USER) == 1
    });

    drop(chrome);
    routes.release(SEARCH);
    routes.release(USER);
    let_tasks_finish();

    assert_eq!(routes.finished(SEARCH), 0, "the task stopped");
    assert_eq!(routes.finished(USER), 0, "the task stopped");
    let (_auth, handle) = service_over(routes.clone(), &directory).into_parts();
    assert!(handle.tokens().cached_user().expect("read").is_null());
    assert!(handle.has_stored_tokens(), "a close is not a sign-out");
}

#[test]
fn a_query_that_finds_no_session_leaves_the_signed_in_state_and_its_results() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Private")]))
        .respond(SEARCH, HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, rejected_refresh())
        .respond(TRENDING, page_of(&[(2, "Public")]));
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Private"]
    });

    chrome
        .browser_mut()
        .expect("browser")
        .store_mut()
        .begin_query();
    pump(&mut chrome, "the lost session", |chrome| {
        !chrome.session().signed_in()
    });

    assert!(
        !result_titles(&chrome).contains(&"Private".to_owned()),
        "session-specific results must go"
    );
    assert!(chrome.toast().is_some(), "the user must see why");
    assert!(chrome.public_user().is_none());
    assert!(!chrome.community().expect("service").has_stored_tokens());

    pump(&mut chrome, "the preview", |chrome| {
        result_titles(chrome) == ["Public"]
    });
    assert!(
        chrome
            .browser()
            .expect("browser")
            .store()
            .result_is_trending()
    );
    for _ in 0..20 {
        chrome.poll_community();
    }
    let_tasks_finish();
    chrome.poll_community();
    assert_eq!(routes.count(TOKEN), 1, "a dead session must not loop");
    assert_eq!(routes.count(SEARCH), 2, "a dead session must not loop");
    assert_eq!(routes.count(TRENDING), 1);
}

#[test]
fn a_lost_session_also_clears_a_parked_browser() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes
        .respond(SEARCH, page_of(&[(1, "Private")]))
        .respond(USER, HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, rejected_refresh())
        .respond(TRENDING, page_of(&[(2, "Public")]));
    routes.hold(USER);
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Private"]
    });

    chrome.go_to_main();
    routes.release(USER);
    pump(&mut chrome, "the lost session", |chrome| {
        !chrome.session().signed_in()
    });

    open_browser(&mut chrome);
    assert!(result_titles(&chrome).is_empty());
    pump(&mut chrome, "the preview", |chrome| {
        result_titles(chrome) == ["Public"]
    });
}

#[test]
fn a_network_failure_keeps_the_session_and_the_last_result() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Kept")]))
        .respond(SEARCH, HttpResponse::new(503, Vec::new()));
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Kept"]
    });

    chrome
        .browser_mut()
        .expect("browser")
        .store_mut()
        .begin_query();
    pump(&mut chrome, "the failure", |chrome| {
        chrome.browser().expect("browser").store().status() == CatalogStatus::Error
    });

    assert!(chrome.session().signed_in());
    assert_eq!(result_titles(&chrome), ["Kept"]);
    assert_eq!(
        routes.count(SEARCH),
        2,
        "a failure does not retry by itself"
    );
}

#[test]
fn a_sign_in_resumes_the_pending_browse_query_and_caches_the_user() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes
        .respond(TRENDING, page_of(&[(1, "Public")]))
        .respond("/api/v1/oauth/device_authorization", device_response())
        .respond(TOKEN, token_response("access", "refresh"))
        .respond(SEARCH, page_of(&[(2, "Mine")]))
        .respond(USER, user_response());
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the preview", |chrome| {
        result_titles(chrome) == ["Public"]
    });
    let store = chrome.browser_mut().expect("browser").store_mut();
    store.query_mut().text = "pending".to_owned();
    store.query_mut().gear = "amp".to_owned();

    chrome.start_device_sign_in().expect("start sign-in");
    pump(&mut chrome, "the sign-in", |chrome| {
        chrome.session().signed_in()
    });
    pump(&mut chrome, "the resumed query", |chrome| {
        result_titles(chrome) == ["Mine"] && chrome.public_user().is_some()
    });

    let store = chrome.browser().expect("browser").store();
    assert!(!store.result_is_trending());
    assert_eq!(store.query().text, "pending");
    assert_eq!(
        routes.seen(SEARCH)[0].target,
        "/api/v1/tones/search?page=1&page_size=12&query=pending&gears=amp"
    );
    let (_auth, handle) = service_over(routes.clone(), &directory).into_parts();
    wait_for("the cached user", || {
        chrome.poll_community();
        handle.tokens().cached_user().expect("read")["username"] == "player"
    });
}

#[test]
fn a_favorite_updates_at_once_and_a_failure_restores_the_raw_tone() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    let path = "/api/v1/tones/1/favorite";
    routes
        .respond(SEARCH, page_of(&[(1, "Tone")]))
        .respond(path, HttpResponse::new(500, Vec::new()))
        .respond(path, HttpResponse::new(204, Vec::new()));
    routes.hold(path);
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Tone"]
    });
    let previous = first_tone(&chrome).raw;

    chrome.set_favorite(ToneId(1), true);
    let optimistic = first_tone(&chrome).raw;
    assert_eq!(optimistic["is_favorite"], true);
    assert_eq!(optimistic["favorites_count"], 5);
    pump(&mut chrome, "the failed write", |_| routes.count(path) == 1);
    routes.release(path);
    pump(&mut chrome, "the rollback", |chrome| {
        first_tone(chrome).raw == previous
    });

    chrome.set_favorite(ToneId(1), true);
    wait_for("the second write", || routes.finished(path) == 2);
    let_tasks_finish();
    chrome.poll_community();
    assert_eq!(
        first_tone(&chrome).raw["is_favorite"],
        true,
        "the write held"
    );
    assert_eq!(routes.seen(path)[1].method, HttpMethod::Put);
}

#[test]
fn a_favorite_that_finishes_after_a_close_does_not_publish_and_the_return_asks_again() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    let path = "/api/v1/tones/1/favorite";
    routes
        .respond(SEARCH, page_of(&[(1, "Tone")]))
        .respond(SEARCH, page_of(&[(1, "Fresh")]))
        .respond(path, HttpResponse::new(500, Vec::new()));
    routes.hold(path);
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Tone"]
    });
    let previous = first_tone(&chrome).raw;

    chrome.set_favorite(ToneId(1), true);
    pump(&mut chrome, "the write", |_| routes.count(path) == 1);
    chrome.go_to_main();
    routes.release(path);
    wait_for("the failed write", || routes.finished(path) == 1);
    let_tasks_finish();
    chrome.poll_community();

    open_browser(&mut chrome);
    assert_eq!(
        first_tone(&chrome).raw,
        previous,
        "a closed browser keeps no unconfirmed state"
    );
    pump(&mut chrome, "the fresh result", |chrome| {
        result_titles(chrome) == ["Fresh"]
    });
}

#[test]
fn a_favorite_that_fails_after_a_sign_out_does_not_publish() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    let path = "/api/v1/tones/1/favorite";
    routes
        .respond(SEARCH, page_of(&[(1, "Tone")]))
        .respond(TRENDING, page_of(&[(2, "Public")]))
        .respond(path, HttpResponse::new(500, Vec::new()));
    routes.hold(path);
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Tone"]
    });

    chrome.set_favorite(ToneId(1), true);
    pump(&mut chrome, "the write", |_| routes.count(path) == 1);
    chrome.sign_out().expect("sign out");
    routes.release(path);
    pump(&mut chrome, "the preview", |chrome| {
        result_titles(chrome) == ["Public"]
    });
    let_tasks_finish();
    chrome.poll_community();

    assert_eq!(result_titles(&chrome), ["Public"]);
    assert!(!chrome.session().signed_in());
}

#[test]
fn a_favorite_that_finds_no_session_rolls_back_and_signs_out() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    let path = "/api/v1/tones/1/favorite";
    routes
        .respond(SEARCH, page_of(&[(1, "Tone")]))
        .respond(TRENDING, page_of(&[(2, "Public")]))
        .respond(path, HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, rejected_refresh());
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the result", |chrome| {
        result_titles(chrome) == ["Tone"]
    });

    chrome.set_favorite(ToneId(1), true);
    pump(&mut chrome, "the lost session", |chrome| {
        !chrome.session().signed_in()
    });
    pump(&mut chrome, "the preview", |chrome| {
        result_titles(chrome) == ["Public"]
    });
}

#[test]
fn a_signed_out_favorite_asks_for_a_sign_in_and_sends_nothing() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes.respond(TRENDING, page_of(&[(1, "Public")]));
    let mut chrome = chrome_over(&routes, &directory);
    open_browser(&mut chrome);
    pump(&mut chrome, "the preview", |chrome| {
        result_titles(chrome) == ["Public"]
    });

    chrome.set_favorite(ToneId(1), true);
    let_tasks_finish();
    chrome.poll_community();

    assert!(chrome.browser().expect("browser").sign_in_page());
    assert!(first_tone(&chrome).raw["is_favorite"].is_null());
    assert_eq!(routes.count("/api/v1/tones/1/favorite"), 0);
}

const ADD_FAVORITE: &str = "Add Tone to favorites";
const REMOVE_FAVORITE: &str = "Remove Tone from favorites";
const FAVORITE_PATH: &str = "/api/v1/tones/1/favorite";

/// Opens the real browser through a real click, and waits for its one tone on the screen.
fn driver_on_tone_card(routes: &Routes, directory: &TempDir) -> UiDriver {
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), directory),
    );
    let mut driver = UiDriver::new(chrome, backend);
    let _ = driver.click_first("Add Tone").expect("open");
    wait_for("the tone card", || {
        driver.frame();
        driver.is_visible("Tone")
    });
    driver.settle();
    driver
}

/// What a click on a heart must not start.
fn assert_no_tone_selected(driver: &UiDriver, routes: &Routes) {
    assert!(driver.chrome().browser().is_some(), "the browser stays");
    assert_eq!(driver.chrome().load_flow_phase(), LoadPhase::Idle);
    assert_eq!(routes.count("/api/v1/tones/1"), 0, "no tone request");
    assert_eq!(routes.count("/api/v1/models"), 0, "no model request");
}

#[test]
fn a_click_on_the_heart_changes_the_favorite_at_once_and_selects_no_tone() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Tone")]))
        .respond(FAVORITE_PATH, HttpResponse::new(204, Vec::new()))
        .respond(FAVORITE_PATH, HttpResponse::new(204, Vec::new()));
    routes.hold(FAVORITE_PATH);
    let mut driver = driver_on_tone_card(&routes, &directory);
    assert!(driver.is_visible(ADD_FAVORITE));
    assert!(!driver.is_visible(REMOVE_FAVORITE));

    let interaction = driver.click(ADD_FAVORITE).expect("heart");

    // The held write has no answer yet, so this state is the optimistic one.
    assert_eq!(routes.finished(FAVORITE_PATH), 0);
    assert!(
        interaction
            .visible
            .iter()
            .any(|label| label == REMOVE_FAVORITE)
    );
    assert!(!driver.is_visible(ADD_FAVORITE));
    let tone = first_tone(driver.chrome());
    assert_eq!(tone.raw["is_favorite"], true);
    assert_eq!(tone.raw["favorites_count"], 5);
    assert_no_tone_selected(&driver, &routes);

    routes.release(FAVORITE_PATH);
    wait_for("the write", || {
        driver.frame();
        routes.finished(FAVORITE_PATH) == 1
    });
    let_tasks_finish();
    driver.settle();
    assert!(driver.is_visible(REMOVE_FAVORITE), "the write held");
    assert_eq!(routes.seen(FAVORITE_PATH)[0].method, HttpMethod::Put);

    let _ = driver.click(REMOVE_FAVORITE).expect("heart");
    assert!(driver.is_visible(ADD_FAVORITE));
    wait_for("the second write", || {
        driver.frame();
        routes.finished(FAVORITE_PATH) == 2
    });
    assert_eq!(routes.seen(FAVORITE_PATH)[1].method, HttpMethod::Delete);
    assert_eq!(first_tone(driver.chrome()).raw["is_favorite"], false);
    assert_no_tone_selected(&driver, &routes);
}

#[test]
fn a_failed_favorite_write_restores_the_heart_and_the_tone() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Tone")]))
        .respond(FAVORITE_PATH, HttpResponse::new(500, Vec::new()));
    routes.hold(FAVORITE_PATH);
    let mut driver = driver_on_tone_card(&routes, &directory);
    let previous = first_tone(driver.chrome()).raw;

    let _ = driver.click(ADD_FAVORITE).expect("heart");
    assert!(
        driver.is_visible(REMOVE_FAVORITE),
        "the change shows at once"
    );

    routes.release(FAVORITE_PATH);
    wait_for("the rollback", || {
        driver.frame();
        driver.is_visible(ADD_FAVORITE)
    });

    assert!(!driver.is_visible(REMOVE_FAVORITE));
    assert_eq!(first_tone(driver.chrome()).raw, previous);
    assert!(driver.chrome().session().signed_in());
    assert_no_tone_selected(&driver, &routes);
}

#[test]
fn a_favorite_tone_opens_with_the_filled_heart_label() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    let mut row = tone_row(1, "Tone");
    row["is_favorite"] = serde_json::json!(true);
    routes
        .respond(
            SEARCH,
            HttpResponse::json(
                200,
                serde_json::json!({
                    "data": [row], "page": 1, "page_size": 12, "total": 1, "total_pages": 1,
                }),
            ),
        )
        .respond(FAVORITE_PATH, HttpResponse::new(204, Vec::new()));
    let mut driver = driver_on_tone_card(&routes, &directory);
    assert!(driver.is_visible(REMOVE_FAVORITE));
    assert!(!driver.is_visible(ADD_FAVORITE));

    let _ = driver.click(REMOVE_FAVORITE).expect("heart");

    assert!(driver.is_visible(ADD_FAVORITE));
    wait_for("the write", || {
        driver.frame();
        routes.count(FAVORITE_PATH) == 1
    });
    assert_eq!(routes.seen(FAVORITE_PATH)[0].method, HttpMethod::Delete);
}

#[test]
fn a_signed_out_card_has_no_heart_and_its_click_still_asks_for_a_sign_in() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes.respond(TRENDING, page_of(&[(1, "Tone")]));
    let mut driver = driver_on_tone_card(&routes, &directory);

    assert!(!driver.is_visible(ADD_FAVORITE));
    assert!(!driver.is_visible(REMOVE_FAVORITE));

    let _ = driver.click("Tone").expect("card");

    assert!(driver.chrome().browser().expect("browser").sign_in_page());
    assert_eq!(routes.count(FAVORITE_PATH), 0);
}

#[test]
fn each_heart_names_its_own_tone_on_one_clean_line() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes.respond(
        SEARCH,
        page_of(&[(1, "Tone"), (2, "Bad\u{202E}\n\tTitle\u{200B}")]),
    );
    let driver = driver_on_tone_card(&routes, &directory);

    assert!(
        driver.is_visible(ADD_FAVORITE),
        "{:?}",
        driver.visible_labels()
    );
    assert!(
        driver.is_visible("Add Bad Title to favorites"),
        "{:?}",
        driver.visible_labels()
    );
}

#[test]
fn a_heart_takes_no_click_while_a_tone_loads() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes.respond(SEARCH, page_of(&[(1, "Tone"), (2, "Other")]));
    routes.hold("/api/v1/tones/2");
    let mut driver = driver_on_tone_card(&routes, &directory);

    let _ = driver.click("Other").expect("card");
    assert_eq!(
        driver.chrome().browser().expect("browser").pending_tone(),
        Some(ToneId(2))
    );

    let refused = driver.click_first(ADD_FAVORITE);
    assert!(
        matches!(
            refused,
            Err(ronie_ui::driver::DriverError::NotInteractive { .. })
        ),
        "{refused:?}"
    );
    assert_eq!(routes.count(FAVORITE_PATH), 0);
    assert!(first_tone(driver.chrome()).raw["is_favorite"].is_null());
}

#[test]
fn a_heart_takes_a_click_in_a_zoomed_browser() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Tone")]))
        .respond(FAVORITE_PATH, HttpResponse::new(204, Vec::new()));
    let mut driver = driver_on_tone_card(&routes, &directory);

    driver
        .chrome_mut()
        .browser_mut()
        .expect("browser")
        .set_zoom(0.75);
    driver.settle();
    let _ = driver.click(ADD_FAVORITE).expect("heart");

    assert!(driver.is_visible(REMOVE_FAVORITE));
    assert_no_tone_selected(&driver, &routes);
}

#[test]
fn opening_a_filter_menu_fetches_its_taxonomy_and_shows_the_rows() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes.respond(SEARCH, page_of(&[(1, "Tone")])).respond(
        "/api/v1/tags",
        HttpResponse::json(
            200,
            serde_json::json!({"data": [{"id": 1, "name": "fetched-tag"}], "total_pages": 1}),
        ),
    );
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend);
    let _ = driver.click_first("Add Tone").expect("open");
    wait_for("the result", || {
        driver.frame();
        result_titles(driver.chrome()) == ["Tone"]
    });

    let browser = driver.chrome_mut().browser_mut().expect("browser");
    browser.set_filters_expanded(true);
    browser.set_filter_menu(Some(FilterMenu::Tags));
    wait_for("the fetched row", || {
        driver.frame();
        driver.is_visible("fetched-tag")
    });

    assert_eq!(
        routes.count("/api/v1/tags"),
        1,
        "one open makes one request"
    );
    assert_eq!(
        routes.seen("/api/v1/tags")[0].target,
        "/api/v1/tags?page=1&page_size=25"
    );
}

#[test]
fn a_real_search_keystroke_reaches_the_service_through_the_browser() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(1, "Everything")]))
        .respond(SEARCH, page_of(&[(2, "Only fuzz")]));
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend);
    let _ = driver.click_first("Add Tone").expect("open");
    wait_for("the result", || {
        driver.frame();
        result_titles(driver.chrome()) == ["Everything"]
    });

    let _ = driver.fill("Search tones", "fuzz").expect("type");
    wait_for("the search", || {
        driver.frame();
        result_titles(driver.chrome()) == ["Only fuzz"]
    });

    let seen = routes.seen(SEARCH);
    assert!(
        seen.last().expect("search").target.contains("query=fuzz"),
        "{seen:?}"
    );
}

const TONE_FIVE: &str = "/api/v1/tones/5";
const TONE_SIX: &str = "/api/v1/tones/6";
const MODELS: &str = "/api/v1/models";
const PUBLIC_FILE: &str = "/files/7.nam";
const PUBLIC_URL: &str = "https://models.example/files/7.nam";
const UPSTREAM_URL: &str = "https://api.example/files/7.nam";

fn tone_answer(id: i64, format: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        serde_json::json!({"id": id, "title": "Remote", "format": format, "models": [], "note": 1}),
    )
}

fn models_answer(rows: serde_json::Value) -> HttpResponse {
    HttpResponse::json(
        200,
        serde_json::json!({"data": rows, "page": 1, "page_size": 1, "total": 1, "total_pages": 1}),
    )
}

fn one_model(id: i64, url: &str) -> HttpResponse {
    models_answer(serde_json::json!([{"id": id, "name": "Amp", "model_url": url}]))
}

fn file_answer(body: &[u8]) -> HttpResponse {
    HttpResponse::new(200, body.to_vec())
}

fn catalog_row(id: i64) -> Tone {
    Tone::from_value(tone_row(id, "Remote")).expect("row")
}

/// One flow with a target over the fixture chain, as a chrome holds it.
fn load_flow_over(backend: Arc<dyn UiBackend>) -> LoadFlow {
    let target = backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|block| block.is_insert)
        .expect("insert")
        .id
        .0
        .clone();
    let mut flow = LoadFlow::new(backend);
    flow.select(ChainSide::Left, &target);
    flow
}

fn start_load(
    worker: &mut CommunityWorker<Routes>,
    runtime: &Runtime,
    flow: &mut LoadFlow,
    id: i64,
) -> u64 {
    let ticket = flow.begin_remote_load(catalog_row(id)).expect("load");
    let generation = ticket.generation();
    worker.start_tone_load(
        runtime.handle(),
        LoadRequest {
            tone: ToneId(id),
            ticket,
        },
    );
    generation
}

fn tone_load_result(events: &[CommunityEvent]) -> Option<Result<(), CommunityFailure>> {
    events.iter().find_map(|event| match event {
        CommunityEvent::ToneLoad { result, .. } => Some(*result),
        _ => None,
    })
}

fn load_result(worker: &mut CommunityWorker<Routes>) -> Result<(), CommunityFailure> {
    let events = events_until(worker, |events| tone_load_result(events).is_some());
    tone_load_result(&events).expect("a result")
}

fn published_bytes(backend: &MockBackend) -> Vec<Vec<u8>> {
    backend
        .actions()
        .into_iter()
        .filter_map(|action| match action {
            UiAction::LoadToneWithBytes { bytes, .. } => Some(bytes.to_vec()),
            _ => None,
        })
        .collect()
}

/// One load over a fresh signed-in store and fixture chain.
struct LoadRig {
    _directory: TempDir,
    routes: Routes,
    worker: CommunityWorker<Routes>,
    handle: CommunityHandle<Routes>,
    runtime: Runtime,
    backend: Arc<MockBackend>,
    flow: LoadFlow,
}

impl LoadRig {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("settings root");
        earlier_launch_signed_in(&directory, i64::MAX);
        let routes = Routes::default();
        let (worker, handle) = worker_over(&routes, &directory);
        let backend = backend();
        let flow = load_flow_over(backend.clone());
        Self {
            _directory: directory,
            routes,
            worker,
            handle,
            runtime: Runtime::new().expect("runtime"),
            backend,
            flow,
        }
    }

    fn start(&mut self, id: i64) -> u64 {
        start_load(&mut self.worker, &self.runtime, &mut self.flow, id)
    }

    fn result(&mut self) -> Result<(), CommunityFailure> {
        load_result(&mut self.worker)
    }

    fn assert_chain_unchanged(&self, revision: u64) {
        assert!(published_bytes(&self.backend).is_empty());
        assert_eq!(self.backend.chain_state(0).revision, revision);
    }
}

/// One way to stop a load, with a name for the assertion text.
type NamedCancel = (&'static str, fn(&mut LoadRig));

#[test]
fn a_nam_load_selects_one_wavenet_model_and_publishes_its_bytes() {
    let mut rig = LoadRig::new();
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b"nam bytes"));

    let generation = rig.start(5);
    assert_eq!(rig.result(), Ok(()));

    assert_eq!(
        rig.routes.seen(MODELS)[0].target,
        "/api/v1/models?tone_id=5&page=1&page_size=1&architecture=2"
    );
    assert!(rig.routes.seen(TONE_FIVE)[0].bearer);
    assert!(rig.routes.seen(MODELS)[0].bearer);
    assert!(!rig.routes.seen(PUBLIC_FILE)[0].bearer, "a public origin");
    let actions = rig.backend.actions();
    let Some(UiAction::LoadToneWithBytes {
        tone,
        model_id,
        bytes,
        ..
    }) = actions.last()
    else {
        panic!("{} actions", actions.len());
    };
    assert_eq!(*model_id, ModelId(7));
    assert_eq!(bytes.as_ref(), b"nam bytes");
    assert_eq!(tone.id, ToneId(5));
    assert_eq!(tone.models.len(), 1);
    assert_eq!(tone.models[0].id, ModelId(7));
    assert_eq!(tone.raw["models"][0]["id"], 7, "the stored tone agrees");
    assert_eq!(tone.raw["note"], 1, "the tone keeps its unknown fields");
    assert!(rig.flow.finish_remote(generation));
    assert_eq!(rig.flow.phase(), LoadPhase::Ready);
}

#[test]
fn an_ir_load_has_no_architecture_filter() {
    let mut rig = LoadRig::new();
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "ir"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b"wav bytes"));

    rig.start(5);
    assert_eq!(rig.result(), Ok(()));

    assert_eq!(
        rig.routes.seen(MODELS)[0].target,
        "/api/v1/models?tone_id=5&page=1&page_size=1"
    );
    assert_eq!(published_bytes(&rig.backend), [b"wav bytes".to_vec()]);
}

#[test]
fn a_tone_with_no_usable_model_keeps_the_chain() {
    for rows in [
        serde_json::json!([]),
        serde_json::json!([{"id": 7, "name": "No address"}]),
    ] {
        let mut rig = LoadRig::new();
        let revision = rig.backend.chain_state(0).revision;
        rig.routes
            .respond(TONE_FIVE, tone_answer(5, "nam"))
            .respond(MODELS, models_answer(rows));

        rig.start(5);

        assert!(matches!(rig.result(), Err(CommunityFailure::Failed(_))));
        rig.assert_chain_unchanged(revision);
        assert!(rig.handle.has_stored_tokens());
    }
}

#[test]
fn a_tone_that_is_not_the_requested_tone_is_refused() {
    let mut rig = LoadRig::new();
    let revision = rig.backend.chain_state(0).revision;
    rig.routes.respond(TONE_FIVE, tone_answer(6, "nam"));

    rig.start(5);

    assert!(matches!(rig.result(), Err(CommunityFailure::Failed(_))));
    assert_eq!(rig.routes.count(MODELS), 0);
    rig.assert_chain_unchanged(revision);
}

#[test]
fn an_empty_download_keeps_the_chain_and_the_session() {
    let mut rig = LoadRig::new();
    let revision = rig.backend.chain_state(0).revision;
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b""));

    rig.start(5);

    assert!(matches!(rig.result(), Err(CommunityFailure::Failed(_))));
    rig.assert_chain_unchanged(revision);
    assert!(rig.handle.has_stored_tokens());
}

#[test]
fn a_model_address_that_is_not_approved_fails_before_transmission() {
    for url in [
        "https://attacker.example/files/7.nam",
        "http://models.example/files/7.nam",
        "https://user:word@models.example/files/7.nam",
        "file:///etc/files/7.nam",
    ] {
        let mut rig = LoadRig::new();
        let revision = rig.backend.chain_state(0).revision;
        rig.routes
            .respond(TONE_FIVE, tone_answer(5, "nam"))
            .respond(MODELS, one_model(7, url))
            .respond(PUBLIC_FILE, file_answer(b"never"));

        rig.start(5);

        assert!(
            matches!(rig.result(), Err(CommunityFailure::Failed(_))),
            "{url}"
        );
        assert_eq!(rig.routes.count(PUBLIC_FILE), 0, "{url}");
        assert_eq!(rig.routes.state().seen.len(), 2, "{url}");
        rig.assert_chain_unchanged(revision);
        assert!(rig.handle.has_stored_tokens(), "{url}");
    }
}

#[test]
fn a_model_larger_than_its_limit_keeps_the_chain() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let transport = RecordedTransport::with_model_limit(
        vec![
            Ok(tone_answer(5, "nam")),
            Ok(one_model(7, PUBLIC_URL)),
            Ok(HttpResponse::new(200, vec![1; 9])),
        ],
        8,
    );
    let (_auth, handle) = service_over(transport, &directory).into_parts();
    let mut worker = CommunityWorker::new(handle);
    let runtime = Runtime::new().expect("runtime");
    let backend = backend();
    let revision = backend.chain_state(0).revision;
    let mut flow = load_flow_over(backend.clone());
    let ticket = flow.begin_remote_load(catalog_row(5)).expect("load");
    worker.start_tone_load(
        runtime.handle(),
        LoadRequest {
            tone: ToneId(5),
            ticket,
        },
    );

    let mut events = Vec::new();
    wait_for("the load", || {
        events.extend(worker.poll());
        tone_load_result(&events).is_some()
    });

    assert!(matches!(
        tone_load_result(&events),
        Some(Err(CommunityFailure::Failed(_)))
    ));
    assert!(published_bytes(&backend).is_empty());
    assert_eq!(backend.chain_state(0).revision, revision);
}

#[test]
fn a_model_401_refreshes_one_time_and_loads() {
    let mut rig = LoadRig::new();
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, UPSTREAM_URL))
        .respond(PUBLIC_FILE, HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, token_response("rotated-access", "rotated-refresh"))
        .respond(PUBLIC_FILE, file_answer(b"after refresh"));

    rig.start(5);
    assert_eq!(rig.result(), Ok(()));

    let seen = rig.routes.seen(PUBLIC_FILE);
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|request| request.bearer));
    assert_eq!(rig.routes.count(TOKEN), 1);
    assert_eq!(published_bytes(&rig.backend), [b"after refresh".to_vec()]);
}

#[test]
fn a_second_model_401_fails_the_load_and_keeps_the_session() {
    let mut rig = LoadRig::new();
    let revision = rig.backend.chain_state(0).revision;
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, UPSTREAM_URL))
        .respond(PUBLIC_FILE, HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, token_response("rotated-access", "rotated-refresh"))
        .respond(PUBLIC_FILE, HttpResponse::new(401, Vec::new()));

    rig.start(5);

    assert!(matches!(rig.result(), Err(CommunityFailure::Failed(_))));
    assert_eq!(rig.routes.count(PUBLIC_FILE), 2);
    assert_eq!(rig.routes.count(TOKEN), 1);
    rig.assert_chain_unchanged(revision);
    assert!(rig.handle.has_stored_tokens());
}

#[test]
fn a_refresh_that_the_service_rejects_ends_the_session_with_no_loop() {
    let mut rig = LoadRig::new();
    let revision = rig.backend.chain_state(0).revision;
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, UPSTREAM_URL))
        .respond(PUBLIC_FILE, HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, rejected_refresh());

    rig.start(5);

    assert_eq!(rig.result(), Err(CommunityFailure::SessionLost));
    assert_eq!(rig.routes.count(PUBLIC_FILE), 1);
    assert_eq!(rig.routes.count(TOKEN), 1);
    rig.assert_chain_unchanged(revision);
}

#[test]
fn a_download_failure_keeps_the_chain_and_the_session() {
    for status in [404, 500] {
        let mut rig = LoadRig::new();
        let revision = rig.backend.chain_state(0).revision;
        rig.routes
            .respond(TONE_FIVE, tone_answer(5, "nam"))
            .respond(MODELS, one_model(7, PUBLIC_URL))
            .respond(PUBLIC_FILE, HttpResponse::new(status, Vec::new()))
            .respond(PUBLIC_FILE, HttpResponse::new(status, Vec::new()));

        rig.start(5);

        assert!(matches!(rig.result(), Err(CommunityFailure::Failed(_))));
        rig.assert_chain_unchanged(revision);
        assert!(rig.handle.has_stored_tokens());
        assert_eq!(rig.routes.count(TOKEN), 0);
    }
}

#[test]
fn a_late_download_after_each_cancellation_reaches_no_processor() {
    let cancels: [NamedCancel; 3] = [
        ("browser", |rig| {
            rig.flow.close();
            rig.worker.cancel(CommunityScope::Browser);
        }),
        ("session", |rig| {
            rig.flow.abandon();
            rig.worker.cancel(CommunityScope::Session);
        }),
        ("worker only", |rig| {
            rig.worker.cancel(CommunityScope::Session);
        }),
    ];
    for (name, cancel) in cancels {
        let mut rig = LoadRig::new();
        let revision = rig.backend.chain_state(0).revision;
        rig.routes
            .respond(TONE_FIVE, tone_answer(5, "nam"))
            .respond(MODELS, one_model(7, PUBLIC_URL))
            .respond(PUBLIC_FILE, file_answer(b"late"));
        rig.routes.hold(PUBLIC_FILE);
        rig.start(5);
        wait_for("the download", || rig.routes.count(PUBLIC_FILE) == 1);

        cancel(&mut rig);
        rig.routes.release(PUBLIC_FILE);
        let_tasks_finish();

        assert!(rig.worker.poll().is_empty(), "{name}");
        rig.assert_chain_unchanged(revision);
    }
}

#[test]
fn a_finished_download_that_a_cancellation_beats_before_the_poll_is_not_delivered() {
    let mut rig = LoadRig::new();
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b"landed"));
    let generation = rig.start(5);
    wait_for("the publication", || {
        !published_bytes(&rig.backend).is_empty()
    });
    let_tasks_finish();

    rig.flow.close();
    rig.worker.cancel(CommunityScope::Browser);

    assert!(rig.worker.poll().is_empty());
    assert!(!rig.flow.finish_remote(generation));
}

#[test]
fn a_panic_in_the_backend_ends_the_load_with_a_failure() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b"bytes"));
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");
    let backend = backend();
    let mut flow = load_flow_over(Arc::new(HeldBackend {
        inner: backend.clone(),
        compile: Arc::default(),
        panics: true,
    }));

    start_load(&mut worker, &runtime, &mut flow, 5);

    assert!(matches!(
        load_result(&mut worker),
        Err(CommunityFailure::Failed(_))
    ));
    assert!(published_bytes(&backend).is_empty());
}

/// A backend that holds a publication at its compile, where an abort cannot reach it.
#[derive(Default)]
struct Compile {
    held: Mutex<bool>,
    open: Condvar,
    entered: AtomicUsize,
    active: AtomicUsize,
    most_active: AtomicUsize,
    threads: Mutex<Vec<std::thread::ThreadId>>,
}

impl Compile {
    fn hold(&self) {
        *self.held.lock().expect("lock") = true;
    }

    fn release(&self) {
        *self.held.lock().expect("lock") = false;
        self.open.notify_all();
    }

    fn record_thread(&self) {
        self.threads
            .lock()
            .expect("lock")
            .push(std::thread::current().id());
    }

    fn enter(&self) {
        self.record_thread();
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_active.fetch_max(active, Ordering::SeqCst);
        self.entered.fetch_add(1, Ordering::SeqCst);
        let mut held = self.held.lock().expect("lock");
        while *held {
            held = self.open.wait(held).expect("lock");
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

struct HeldBackend {
    inner: Arc<MockBackend>,
    compile: Arc<Compile>,
    panics: bool,
}

impl UiBackend for HeldBackend {
    fn chain_state(&self, known_revision: u64) -> ChainViewState {
        self.inner.chain_state(known_revision)
    }

    fn meter_levels(&self) -> MeterSnapshot {
        self.inner.meter_levels()
    }

    fn dispatch(&self, action: UiAction) -> Result<UiReceipt, UiError> {
        if matches!(
            action,
            UiAction::LoadToneWithBytes { .. } | UiAction::SelectModelWithBytes { .. }
        ) {
            self.compile.record_thread();
        }
        self.inner.dispatch(action)
    }

    fn validate_model(
        &self,
        _tone: &Tone,
        _model_id: ModelId,
        _bytes: &ModelBytes,
    ) -> Result<(), UiError> {
        assert!(!self.panics, "a compile failure");
        self.compile.enter();
        Ok(())
    }

    fn settle_audio(&self) -> Result<(), UiError> {
        self.compile.record_thread();
        Ok(())
    }

    fn parameter(&self, id: ParameterId) -> Option<ParameterValue> {
        self.inner.parameter(id)
    }

    fn set_parameter(
        &self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<(), UiError> {
        self.inner.set_parameter(id, value, gesture)
    }
}

struct HeldRig {
    _directory: TempDir,
    routes: Routes,
    inner: Arc<MockBackend>,
    compile: Arc<Compile>,
    chrome: ChromeController,
}

impl HeldRig {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("settings root");
        let routes = signed_in_routes(&directory);
        routes.respond(SEARCH, page_of(&[(5, "Remote")]));
        routes
            .respond(TONE_FIVE, tone_answer(5, "nam"))
            .respond(MODELS, one_model(7, PUBLIC_URL))
            .respond(PUBLIC_FILE, file_answer(b"old"));
        let inner = backend();
        let compile = Arc::new(Compile::default());
        compile.hold();
        let chrome = ChromeController::with_community(
            Arc::new(HeldBackend {
                inner: inner.clone(),
                compile: compile.clone(),
                panics: false,
            }),
            Vec::new(),
            false,
            true,
            service_over(routes.clone(), &directory),
        );
        let mut rig = Self {
            _directory: directory,
            routes,
            inner,
            compile,
            chrome,
        };
        open_browser(&mut rig.chrome);
        rig
    }

    fn pick_and_wait_for_the_compile(&mut self) {
        self.chrome.pick_tone(catalog_row(5));
        let compile = self.compile.clone();
        pump(&mut self.chrome, "the compile", move |_| {
            compile.entered.load(Ordering::SeqCst) == 1
        });
    }
}

#[test]
fn a_compile_that_finishes_after_a_close_reaches_no_processor() {
    for leave in [
        |chrome: &mut ChromeController| chrome.go_to_main(),
        |chrome: &mut ChromeController| {
            chrome.load_flow_mut().close();
            chrome.sync_browser();
        },
    ] {
        let mut rig = HeldRig::new();
        let revision = rig.inner.chain_state(0).revision;
        rig.pick_and_wait_for_the_compile();

        leave(&mut rig.chrome);
        rig.compile.release();
        let_tasks_finish();
        rig.chrome.poll_community();

        assert!(published_bytes(&rig.inner).is_empty());
        assert_eq!(rig.inner.chain_state(0).revision, revision);
        assert_eq!(rig.chrome.load_flow_phase(), LoadPhase::Cancelled);
    }
}

#[test]
fn a_compile_that_finishes_after_a_sign_out_reaches_no_processor() {
    let mut rig = HeldRig::new();
    rig.pick_and_wait_for_the_compile();

    rig.chrome.sign_out().expect("sign out");
    rig.compile.release();
    let_tasks_finish();
    rig.chrome.poll_community();

    assert!(published_bytes(&rig.inner).is_empty());
    assert_eq!(rig.chrome.load_flow_phase(), LoadPhase::Idle);
    assert!(rig.chrome.browser().is_some(), "the browser stays open");
}

#[test]
fn a_compile_that_finishes_after_a_newer_load_gives_way_to_the_newer_bytes() {
    let mut rig = HeldRig::new();
    rig.routes
        .respond(TONE_SIX, tone_answer(6, "nam"))
        .respond(MODELS, one_model(8, "https://models.example/files/8.nam"))
        .respond("/files/8.nam", file_answer(b"new"));
    rig.pick_and_wait_for_the_compile();

    rig.chrome.pick_tone(catalog_row(6));
    let routes = rig.routes.clone();
    pump(&mut rig.chrome, "the newer download", move |_| {
        routes.finished("/files/8.nam") == 1
    });
    let_tasks_finish();
    assert_eq!(
        rig.compile.entered.load(Ordering::SeqCst),
        1,
        "one compile runs at a time, so a queue of loads cannot hold more bytes"
    );
    rig.compile.release();
    pump(&mut rig.chrome, "the end of the load", |chrome| {
        chrome.load_flow_phase() == LoadPhase::Ready
    });

    assert_eq!(published_bytes(&rig.inner), [b"new".to_vec()]);
    assert_eq!(rig.compile.most_active.load(Ordering::SeqCst), 1);
}

#[test]
fn each_compile_and_each_publication_runs_away_from_the_polling_thread() {
    let mut rig = HeldRig::new();
    rig.compile.release();

    rig.chrome.pick_tone(catalog_row(5));
    pump(&mut rig.chrome, "the end of the load", |chrome| {
        chrome.load_flow_phase() == LoadPhase::Ready
    });

    let here = std::thread::current().id();
    let threads = rig.compile.threads.lock().expect("lock");
    assert!(!threads.is_empty());
    assert!(threads.iter().all(|thread| *thread != here), "{threads:?}");
    assert_eq!(published_bytes(&rig.inner), [b"old".to_vec()]);
}

#[test]
fn a_chrome_with_a_service_lands_only_after_the_bytes_arrive() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b"bytes"));
    routes.hold(PUBLIC_FILE);
    let inner = backend();
    let mut chrome = chrome_with(inner.clone(), &routes, &directory);
    open_browser(&mut chrome);

    chrome.pick_tone(catalog_row(5));
    chrome.sync_browser();

    assert_eq!(chrome.load_flow_phase(), LoadPhase::Loading);
    assert_eq!(
        chrome.browser().expect("browser").pending_tone(),
        Some(ToneId(5)),
        "the card stays busy"
    );
    let waiting = routes.clone();
    pump(&mut chrome, "the download", move |_| {
        waiting.count(PUBLIC_FILE) == 1
    });
    assert!(
        !inner
            .actions()
            .iter()
            .any(|action| matches!(action, UiAction::LoadTone { .. })),
        "a remote pick does not complete from a cache that it does not have"
    );
    assert!(published_bytes(&inner).is_empty());

    routes.release(PUBLIC_FILE);
    pump(&mut chrome, "the end of the load", |chrome| {
        chrome.load_flow_phase() == LoadPhase::Ready
    });
    chrome.sync_browser();

    assert!(chrome.browser().is_none());
    assert_eq!(published_bytes(&inner), [b"bytes".to_vec()]);
    assert!(
        inner
            .chain_state(0)
            .chain
            .left
            .iter()
            .any(|block| block.tone.as_ref().is_some_and(|tone| tone.id == ToneId(5)))
    );
}

#[test]
fn a_failed_load_shows_its_failure_and_the_same_tone_retries() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, HttpResponse::new(404, Vec::new()));
    let inner = backend();
    let revision = inner.chain_state(0).revision;
    let mut chrome = chrome_with(inner.clone(), &routes, &directory);
    open_browser(&mut chrome);

    chrome.pick_tone(catalog_row(5));
    pump(&mut chrome, "the failure", |chrome| {
        chrome.load_flow_phase() == LoadPhase::Failed
    });
    chrome.sync_browser();

    assert!(
        chrome
            .toast()
            .is_some_and(|text| text.contains("try again"))
    );
    let browser = chrome.browser().expect("the browser stays open");
    assert_eq!(browser.pending_tone(), None, "the card takes a click again");
    assert_eq!(inner.chain_state(0).revision, revision);
    assert!(chrome.session().signed_in());
    assert!(
        chrome
            .community()
            .is_some_and(CommunityAccess::has_stored_tokens)
    );

    routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b"second try"));
    chrome.retry_load();
    assert_eq!(chrome.load_flow_phase(), LoadPhase::Loading);
    pump(&mut chrome, "the retry", |chrome| {
        chrome.load_flow_phase() == LoadPhase::Ready
    });
    assert_eq!(published_bytes(&inner), [b"second try".to_vec()]);
}

#[test]
fn a_load_that_finds_no_session_asks_for_a_sign_in() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes
        .respond(USER, user_response())
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, UPSTREAM_URL))
        .respond(PUBLIC_FILE, HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, rejected_refresh());
    let inner = backend();
    let revision = inner.chain_state(0).revision;
    let mut chrome = chrome_with(inner.clone(), &routes, &directory);
    open_browser(&mut chrome);

    chrome.pick_tone(catalog_row(5));
    pump(&mut chrome, "the end of the session", |chrome| {
        !chrome.session().signed_in()
    });

    assert_eq!(routes.count(PUBLIC_FILE), 1, "no loop");
    assert_eq!(chrome.load_flow_phase(), LoadPhase::Idle);
    assert!(chrome.toast().is_some_and(|text| text.contains("Sign in")));
    assert!(published_bytes(&inner).is_empty());
    assert_eq!(inner.chain_state(0).revision, revision);
}

#[test]
fn a_chrome_with_no_service_keeps_the_fixture_completion() {
    let inner = backend();
    let tone = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-detail")
        .map(|scenario| {
            UiBackend::chain_state(&MockBackend::from_scenario(&scenario), 0)
                .chain
                .left[0]
                .tone
                .clone()
                .expect("tone")
        })
        .expect("scenario");
    let mut chrome = ChromeController::new(inner.clone(), Vec::new(), true, true);
    open_browser(&mut chrome);

    chrome.pick_tone(tone);

    assert_eq!(chrome.load_flow_phase(), LoadPhase::Ready);
    assert!(
        inner
            .actions()
            .iter()
            .any(|action| matches!(action, UiAction::LoadTone { .. }))
    );
    assert!(published_bytes(&inner).is_empty());
}

#[test]
fn a_local_tone_with_a_service_needs_no_network() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    let inner = backend();
    let mut chrome = chrome_with(inner.clone(), &routes, &directory);
    open_browser(&mut chrome);
    let tone = Tone::from_value(serde_json::json!({
        "id": 0,
        "title": "Local",
        "format": "nam",
        "local": true,
        "models": [{"id": 3, "name": "Local", "model_url": "file:///stash/3.nam"}],
    }))
    .expect("tone");

    chrome.pick_tone(tone);

    assert_eq!(chrome.load_flow_phase(), LoadPhase::Ready);
    assert!(routes.state().seen.iter().all(|seen| seen.path == TRENDING));
    assert!(published_bytes(&inner).is_empty());
}

#[test]
fn a_card_click_in_a_real_frame_shows_the_failure_and_the_same_card_retries() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(SEARCH, page_of(&[(5, "Remote")]))
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, HttpResponse::new(404, Vec::new()))
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b"second try"));
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend.clone());
    let _ = driver.click_first("Add Tone").expect("open");
    wait_for("the result", || {
        driver.frame();
        driver.is_visible("Remote")
    });

    let _ = driver.click("Remote").expect("the card");
    wait_for("the failure", || {
        driver.frame();
        driver.chrome().load_flow_phase() == LoadPhase::Failed
    });
    driver.settle();

    assert!(
        driver
            .chrome()
            .visible_toast()
            .is_some_and(|text| text.contains("try again")),
        "{:?}",
        driver.chrome().visible_toast()
    );
    assert!(driver.is_visible("Remote"), "the browser stays open");
    assert!(published_bytes(&backend).is_empty());

    let _ = driver.click("Remote").expect("the same card");
    wait_for("the second try", || {
        driver.frame();
        !published_bytes(&backend).is_empty() && driver.chrome().browser().is_none()
    });

    assert_eq!(published_bytes(&backend), [b"second try".to_vec()]);
    assert!(driver.is_visible("Remote"), "the tile shows the tone");
}

// Model lists, model switches, and remote artwork.

const BLOCK: &str = "blk-1";
const ACTIVE_MODEL: ModelId = ModelId(1104);

fn detail_backend() -> Arc<MockBackend> {
    Arc::new(detail_mock())
}

fn detail_mock() -> MockBackend {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-detail")
        .expect("detail scenario");
    MockBackend::from_scenario(&scenario)
}

fn model_file(id: i64) -> String {
    format!("/files/{id}.nam")
}

fn model_rows(ids: &[i64]) -> HttpResponse {
    let rows = ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "name": format!("Model {id}"),
                "model_url": format!("https://models.example{}", model_file(*id)),
            })
        })
        .collect::<Vec<_>>();
    models_answer(serde_json::Value::Array(rows))
}

fn switched_bytes(backend: &MockBackend) -> Vec<Vec<u8>> {
    backend
        .actions()
        .into_iter()
        .filter_map(|action| match action {
            UiAction::SelectModelWithBytes { bytes, .. } => Some(bytes.to_vec()),
            _ => None,
        })
        .collect()
}

fn active_model(backend: &MockBackend) -> Option<ModelId> {
    backend
        .chain_state(0)
        .chain
        .block(&BlockId(BLOCK.to_owned()))
        .and_then(|block| block.active_model_id)
}

/// One open block detail over a signed-in store, with a compile that a test can hold.
struct DetailRig {
    _directory: TempDir,
    routes: Routes,
    inner: Arc<MockBackend>,
    compile: Arc<Compile>,
    chrome: ChromeController,
}

impl DetailRig {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("settings root");
        let routes = signed_in_routes(&directory);
        let inner = detail_backend();
        let compile = Arc::new(Compile::default());
        let chrome = ChromeController::with_community(
            Arc::new(HeldBackend {
                inner: inner.clone(),
                compile: compile.clone(),
                panics: false,
            }),
            Vec::new(),
            false,
            true,
            service_over(routes.clone(), &directory),
        );
        Self {
            _directory: directory,
            routes,
            inner,
            compile,
            chrome,
        }
    }

    fn open(&mut self, ids: &[i64]) {
        self.routes.respond(MODELS, model_rows(ids));
        self.chrome.open_detail(BLOCK).expect("detail");
        let wanted = ids.len();
        pump(&mut self.chrome, "the model list", move |chrome| {
            chrome
                .detail()
                .is_some_and(|detail| detail.model_select().options().len() >= wanted)
        });
    }

    fn pick(&mut self, id: i64) {
        let session = self.chrome.session();
        self.chrome
            .detail_mut()
            .expect("detail")
            .select_model(ModelId(id), session)
            .expect("selection");
        // The switch starts at the next poll.
        self.chrome.poll_community();
    }

    fn offer_file(&self, id: i64, body: &[u8]) {
        self.routes.respond(&model_file(id), file_answer(body));
    }

    fn wait_for_download(&mut self, id: i64) {
        let routes = self.routes.clone();
        let file = model_file(id);
        pump(&mut self.chrome, "the download", move |_| {
            routes.count(&file) == 1
        });
    }

    fn wait_for_compile(&mut self) {
        let compile = self.compile.clone();
        pump(&mut self.chrome, "the compile", move |_| {
            compile.entered.load(Ordering::SeqCst) == 1
        });
    }

    fn wait_for_the_end_of_the_switch(&mut self) {
        pump(&mut self.chrome, "the end of the switch", |chrome| {
            chrome
                .detail()
                .is_none_or(|detail| detail.model_select().target().is_none())
        });
    }

    fn other_tone() -> Tone {
        Tone::from_value(serde_json::json!({
            "id": 900,
            "title": "Other",
            "format": "nam",
            "models": [{"id": 901, "name": "Other", "model_url": "https://models.example/files/901.nam"}],
        }))
        .expect("tone")
    }
}

/// One way that a late completion becomes out of date, with a name for the assertion text.
type DetailLeave = (&'static str, fn(&mut DetailRig));

fn detail_leaves() -> [DetailLeave; 6] {
    [
        ("the chrome leaves the detail", |rig| {
            rig.chrome.go_to_main();
        }),
        ("the detail closes and no frame runs", |rig| {
            rig.chrome.detail_mut().expect("detail").close();
        }),
        ("the block goes away and no frame runs", |rig| {
            rig.inner
                .dispatch(UiAction::RemoveBlock(BlockId(BLOCK.to_owned())))
                .expect("remove");
        }),
        ("another tone replaces the block and no frame runs", |rig| {
            rig.routes.respond(MODELS, model_rows(&[901]));
            rig.inner
                .dispatch(UiAction::LoadTone {
                    target_insert_id: BlockId(BLOCK.to_owned()),
                    tone: DetailRig::other_tone(),
                    model_id: ModelId(901),
                })
                .expect("replace");
        }),
        ("another model becomes active and no frame runs", |rig| {
            rig.inner
                .dispatch(UiAction::SelectModel {
                    block_id: BlockId(BLOCK.to_owned()),
                    model_id: ModelId(1106),
                })
                .expect("undo");
        }),
        ("the session ends", |rig| {
            rig.chrome.sign_out().expect("sign out");
        }),
    ]
}

fn assert_stays_out_of_date(name: &str, rig: &mut DetailRig, before: &ChainViewState) {
    let_tasks_finish();
    rig.chrome.poll_community();
    let_tasks_finish();
    rig.chrome.poll_community();

    assert!(switched_bytes(&rig.inner).is_empty(), "{name}");
    assert_eq!(rig.inner.chain_state(0).revision, before.revision, "{name}");
    assert_eq!(
        rig.chrome.toast(),
        None,
        "{name}: a user who left needs no failure"
    );
}

#[test]
fn a_worker_lists_at_most_300_models_with_the_filter_of_the_tone() {
    for (kind, target) in [
        (
            ModelKind::Nam,
            "/api/v1/models?tone_id=5&page=1&page_size=300&architecture=2",
        ),
        (
            ModelKind::Ir,
            "/api/v1/models?tone_id=5&page=1&page_size=300",
        ),
    ] {
        let mut rig = LoadRig::new();
        rig.routes.respond(MODELS, model_rows(&[7, 8]));

        rig.worker.start_models(
            rig.runtime.handle(),
            ModelsRequest {
                generation: 3,
                tone: ToneId(5),
                kind,
            },
        );
        let events = events_until(&mut rig.worker, |events| !events.is_empty());

        assert_eq!(rig.routes.seen(MODELS)[0].target, target);
        assert!(rig.routes.seen(MODELS)[0].bearer);
        let [
            CommunityEvent::Models {
                generation: 3,
                tone: ToneId(5),
                result: Ok(list),
            },
        ] = events.as_slice()
        else {
            panic!("{events:?}");
        };
        assert_eq!(
            list.models.iter().map(|model| model.id).collect::<Vec<_>>(),
            [ModelId(7), ModelId(8)]
        );
    }
}

#[test]
fn a_page_with_more_than_300_models_is_refused() {
    let mut rig = LoadRig::new();
    let ids = (0..301).collect::<Vec<_>>();
    rig.routes.respond(MODELS, model_rows(&ids));

    rig.worker.start_models(
        rig.runtime.handle(),
        ModelsRequest {
            generation: 1,
            tone: ToneId(5),
            kind: ModelKind::Nam,
        },
    );
    let events = events_until(&mut rig.worker, |events| !events.is_empty());

    assert!(matches!(
        events.as_slice(),
        [CommunityEvent::Models {
            result: Err(CommunityFailure::Failed(_)),
            ..
        }]
    ));
}

#[test]
fn a_newer_model_list_supersedes_a_list_that_finished_first() {
    let mut rig = LoadRig::new();
    rig.routes
        .respond(MODELS, model_rows(&[7]))
        .respond(MODELS, model_rows(&[8]));
    let request = |generation| ModelsRequest {
        generation,
        tone: ToneId(5),
        kind: ModelKind::Nam,
    };

    rig.worker.start_models(rig.runtime.handle(), request(1));
    wait_for("the first list", || rig.routes.finished(MODELS) == 1);
    let_tasks_finish();
    rig.worker.start_models(rig.runtime.handle(), request(2));
    let events = events_until(&mut rig.worker, |events| !events.is_empty());

    assert!(
        matches!(
            events.as_slice(),
            [CommunityEvent::Models { generation: 2, .. }]
        ),
        "{events:?}"
    );
}

#[test]
fn a_detail_shows_the_model_list_of_its_tone_and_keeps_the_active_model() {
    let mut rig = DetailRig::new();
    let revision = rig.inner.chain_state(0).revision;

    rig.open(&[1104, 1105, 1106]);

    assert_eq!(
        rig.routes.seen(MODELS)[0].target,
        "/api/v1/models?tone_id=104&page=1&page_size=300&architecture=2"
    );
    let detail = rig.chrome.detail().expect("detail");
    assert_eq!(detail.model_select().options().len(), 3);
    assert_eq!(detail.model_select().active(), Some(ACTIVE_MODEL));
    assert!(!detail.model_select().loading());
    assert_eq!(
        rig.inner.chain_state(0).revision,
        revision,
        "a list changes nothing"
    );
}

#[test]
fn a_failed_model_list_keeps_the_active_model_and_does_not_ask_again_in_each_poll() {
    let mut rig = DetailRig::new();
    rig.routes
        .respond(MODELS, HttpResponse::new(500, Vec::new()));
    rig.chrome.open_detail(BLOCK).expect("detail");

    pump(&mut rig.chrome, "the failure", |chrome| {
        chrome.toast().is_some()
    });
    for _ in 0..20 {
        rig.chrome.poll_community();
    }
    let_tasks_finish();
    rig.chrome.poll_community();

    assert_eq!(rig.routes.count(MODELS), 1);
    let detail = rig.chrome.detail().expect("detail");
    assert_eq!(detail.model_select().active(), Some(ACTIVE_MODEL));
    assert!(!detail.model_select().loading());
    assert!(rig.chrome.session().signed_in());
}

#[test]
fn a_signed_out_detail_asks_for_no_model_list() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    let mut chrome = chrome_with(detail_backend(), &routes, &directory);
    chrome.open_detail(BLOCK).expect("detail");

    chrome.poll_community();
    let_tasks_finish();

    assert_eq!(routes.count(MODELS), 0);
}

#[test]
fn a_model_list_that_finishes_after_a_block_change_is_not_applied() {
    let mut rig = DetailRig::new();
    rig.routes.hold(MODELS);
    rig.routes.respond(MODELS, model_rows(&[1104, 1105]));
    rig.routes.respond(MODELS, model_rows(&[901, 902, 903]));
    rig.chrome.open_detail(BLOCK).expect("detail");
    let routes = rig.routes.clone();
    pump(&mut rig.chrome, "the first list", move |_| {
        routes.count(MODELS) == 1
    });

    rig.inner
        .dispatch(UiAction::LoadTone {
            target_insert_id: BlockId(BLOCK.to_owned()),
            tone: DetailRig::other_tone(),
            model_id: ModelId(901),
        })
        .expect("replace");
    rig.chrome.detail_mut().expect("detail").sync();
    let routes = rig.routes.clone();
    pump(&mut rig.chrome, "the list of the new tone", move |_| {
        routes.count(MODELS) == 2
    });
    rig.routes.release(MODELS);
    pump(&mut rig.chrome, "the new list", |chrome| {
        !chrome.detail().expect("detail").model_select().loading()
    });
    let_tasks_finish();
    rig.chrome.poll_community();

    assert!(rig.routes.seen(MODELS)[1].target.contains("tone_id=900"));
    let select = rig.chrome.detail().expect("detail");
    let ids = select
        .model_select()
        .options()
        .iter()
        .map(|option| option.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, [ModelId(901), ModelId(902), ModelId(903)]);
    assert_eq!(select.model_select().active(), Some(ModelId(901)));
}

#[test]
fn a_model_list_of_a_closed_detail_cannot_reach_the_next_detail_of_the_same_block() {
    let mut rig = DetailRig::new();
    rig.routes.hold(MODELS);
    rig.routes.respond(MODELS, model_rows(&[1104, 1105]));
    rig.routes.respond(MODELS, model_rows(&[1104, 1105, 1106]));
    rig.chrome.open_detail(BLOCK).expect("detail");
    let routes = rig.routes.clone();
    pump(&mut rig.chrome, "the first list", move |_| {
        routes.count(MODELS) == 1
    });

    rig.chrome.go_to_main();
    rig.chrome.open_detail(BLOCK).expect("the same detail");
    let routes = rig.routes.clone();
    pump(&mut rig.chrome, "the second list", move |_| {
        routes.count(MODELS) == 2
    });
    rig.routes.release(MODELS);
    pump(&mut rig.chrome, "the list", |chrome| {
        !chrome.detail().expect("detail").model_select().loading()
    });

    assert_eq!(
        rig.chrome
            .detail()
            .expect("detail")
            .model_select()
            .options()
            .len(),
        3
    );
}

#[test]
fn a_remote_switch_publishes_different_bytes_only_after_they_arrive() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105]);
    rig.offer_file(1105, b"other bytes");
    rig.routes.hold(&model_file(1105));
    let revision = rig.inner.chain_state(0).revision;

    rig.pick(1105);
    rig.wait_for_download(1105);

    assert_eq!(active_model(&rig.inner), Some(ACTIVE_MODEL));
    assert_eq!(rig.inner.chain_state(0).revision, revision);
    assert!(switched_bytes(&rig.inner).is_empty());
    let detail = rig.chrome.detail().expect("detail");
    assert_eq!(detail.model_select().active(), Some(ACTIVE_MODEL));
    assert_eq!(detail.load_state(), TileVisualState::Loading);
    assert!(
        !rig.routes.seen(&model_file(1105))[0].bearer,
        "a public origin"
    );

    rig.routes.release(&model_file(1105));
    rig.wait_for_the_end_of_the_switch();

    assert_eq!(switched_bytes(&rig.inner), [b"other bytes".to_vec()]);
    assert_eq!(active_model(&rig.inner), Some(ModelId(1105)));
    let block = rig
        .inner
        .chain_state(0)
        .chain
        .block(&BlockId(BLOCK.to_owned()))
        .cloned()
        .expect("block");
    let tone = block.tone.expect("tone");
    assert_eq!(tone.id, ToneId(104));
    assert_eq!(
        tone.models.len(),
        1,
        "a catalog tone keeps its active model"
    );
    assert_eq!(tone.models[0].id, ModelId(1105));
    assert_eq!(tone.raw["models"][0]["id"], 1105, "the stored tone agrees");
    assert_eq!(tone.raw["gear"], "pedal", "the tone keeps its other fields");
    let detail = rig.chrome.detail().expect("detail");
    assert_eq!(detail.model_select().active(), Some(ModelId(1105)));
    assert_eq!(detail.load_state(), TileVisualState::Ready);
}

#[test]
fn a_switch_finishes_only_after_its_audio_is_prepared_and_away_from_the_polling_thread() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105]);
    rig.offer_file(1105, b"bytes");

    rig.pick(1105);
    rig.wait_for_the_end_of_the_switch();

    let here = std::thread::current().id();
    let threads = rig.compile.threads.lock().expect("lock");
    assert_eq!(
        threads.len(),
        3,
        "the compile, the dispatch, and the preparation: {threads:?}"
    );
    assert!(threads.iter().all(|thread| *thread != here), "{threads:?}");
}

#[test]
fn a_failed_switch_keeps_the_active_model_and_the_same_model_can_retry() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105]);
    rig.routes
        .respond(&model_file(1105), HttpResponse::new(404, Vec::new()));
    let revision = rig.inner.chain_state(0).revision;

    rig.pick(1105);
    rig.wait_for_the_end_of_the_switch();

    assert!(
        rig.chrome
            .toast()
            .is_some_and(|text| text.contains("try again")),
        "{:?}",
        rig.chrome.toast()
    );
    assert_eq!(active_model(&rig.inner), Some(ACTIVE_MODEL));
    assert_eq!(rig.inner.chain_state(0).revision, revision);
    let detail = rig.chrome.detail().expect("detail");
    assert_eq!(detail.model_select().active(), Some(ACTIVE_MODEL));
    assert_eq!(detail.load_state(), TileVisualState::Ready);
    assert!(rig.chrome.session().signed_in());

    rig.offer_file(1105, b"second try");
    rig.pick(1105);
    rig.wait_for_the_end_of_the_switch();

    assert_eq!(switched_bytes(&rig.inner), [b"second try".to_vec()]);
    assert_eq!(active_model(&rig.inner), Some(ModelId(1105)));
}

#[test]
fn a_model_address_that_is_not_approved_fails_the_switch_before_transmission() {
    for url in [
        "http://models.example/files/1105.nam",
        "https://elsewhere.example/files/1105.nam",
    ] {
        let mut rig = DetailRig::new();
        rig.routes.respond(MODELS, one_model(1105, url));
        rig.chrome.open_detail(BLOCK).expect("detail");
        pump(&mut rig.chrome, "the model list", |chrome| {
            !chrome.detail().expect("detail").model_select().loading()
                && chrome
                    .detail()
                    .expect("detail")
                    .model_select()
                    .options()
                    .len()
                    == 2
        });

        rig.pick(1105);
        rig.wait_for_the_end_of_the_switch();

        assert_eq!(rig.routes.count(&model_file(1105)), 0, "{url}");
        assert!(switched_bytes(&rig.inner).is_empty(), "{url}");
        assert_eq!(active_model(&rig.inner), Some(ACTIVE_MODEL), "{url}");
    }
}

#[test]
fn an_empty_download_fails_the_switch_and_keeps_the_chain() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105]);
    rig.offer_file(1105, b"");

    rig.pick(1105);
    rig.wait_for_the_end_of_the_switch();

    assert!(switched_bytes(&rig.inner).is_empty());
    assert_eq!(active_model(&rig.inner), Some(ACTIVE_MODEL));
}

#[test]
fn a_switch_that_finds_no_session_keeps_the_active_model_and_asks_for_a_sign_in() {
    let mut rig = DetailRig::new();
    rig.routes.respond(
        MODELS,
        one_model(1105, "https://api.example/files/1105.nam"),
    );
    rig.routes
        .respond(&model_file(1105), HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, rejected_refresh());
    rig.chrome.open_detail(BLOCK).expect("detail");
    pump(&mut rig.chrome, "the model list", |chrome| {
        chrome
            .detail()
            .expect("detail")
            .model_select()
            .options()
            .len()
            == 2
    });

    rig.pick(1105);
    pump(&mut rig.chrome, "the end of the session", |chrome| {
        !chrome.session().signed_in()
    });

    assert_eq!(rig.routes.count(&model_file(1105)), 1, "no loop");
    assert!(switched_bytes(&rig.inner).is_empty());
    assert_eq!(active_model(&rig.inner), Some(ACTIVE_MODEL));
    assert!(
        rig.chrome
            .toast()
            .is_some_and(|text| text.contains("Sign in"))
    );
    assert_eq!(
        rig.chrome.detail().expect("detail").model_select().target(),
        None
    );
}

#[test]
fn a_late_download_after_each_change_reaches_no_processor() {
    for (name, leave) in detail_leaves() {
        let mut rig = DetailRig::new();
        rig.open(&[1104, 1105, 1106]);
        rig.offer_file(1105, b"late");
        rig.routes.hold(&model_file(1105));
        rig.pick(1105);
        rig.wait_for_download(1105);

        leave(&mut rig);
        let before = rig.inner.chain_state(0);
        rig.routes.release(&model_file(1105));

        assert_stays_out_of_date(name, &mut rig, &before);
    }
}

#[test]
fn a_late_compile_after_each_change_reaches_no_processor() {
    for (name, leave) in detail_leaves() {
        let mut rig = DetailRig::new();
        rig.open(&[1104, 1105, 1106]);
        rig.offer_file(1105, b"late");
        rig.compile.hold();
        rig.pick(1105);
        rig.wait_for_compile();

        leave(&mut rig);
        let before = rig.inner.chain_state(0);
        rig.compile.release();

        assert_stays_out_of_date(name, &mut rig, &before);
    }
}

#[test]
fn a_late_download_gives_way_to_a_newer_switch() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105, 1106]);
    rig.offer_file(1105, b"old");
    rig.offer_file(1106, b"new");
    rig.routes.hold(&model_file(1105));
    rig.pick(1105);
    rig.wait_for_download(1105);

    rig.pick(1106);
    rig.wait_for_the_end_of_the_switch();
    rig.routes.release(&model_file(1105));
    let_tasks_finish();
    rig.chrome.poll_community();

    assert_eq!(switched_bytes(&rig.inner), [b"new".to_vec()]);
    assert_eq!(active_model(&rig.inner), Some(ModelId(1106)));
}

#[test]
fn a_late_compile_gives_way_to_a_newer_switch_and_one_compile_runs_at_a_time() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105, 1106]);
    rig.offer_file(1105, b"old");
    rig.offer_file(1106, b"new");
    rig.compile.hold();
    rig.pick(1105);
    rig.wait_for_compile();

    rig.pick(1106);
    let routes = rig.routes.clone();
    let file = model_file(1106);
    pump(&mut rig.chrome, "the newer download", move |_| {
        routes.finished(&file) == 1
    });
    let_tasks_finish();
    assert_eq!(
        rig.compile.entered.load(Ordering::SeqCst),
        1,
        "the newer switch waits for the compile"
    );
    rig.compile.release();
    rig.wait_for_the_end_of_the_switch();

    assert_eq!(switched_bytes(&rig.inner), [b"new".to_vec()]);
    assert_eq!(active_model(&rig.inner), Some(ModelId(1106)));
    assert_eq!(rig.compile.most_active.load(Ordering::SeqCst), 1);
}

#[test]
fn a_tone_load_and_a_switch_share_one_compile() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105]);
    rig.offer_file(1105, b"switch");
    rig.compile.hold();
    rig.pick(1105);
    rig.wait_for_compile();
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, PUBLIC_URL))
        .respond(PUBLIC_FILE, file_answer(b"tone"));

    rig.chrome
        .gallery_mut()
        .select_tone(ChainSide::Left, BlockId("ins-1".to_owned()));
    rig.chrome.sync_browser();
    rig.chrome.pick_tone(catalog_row(5));
    let routes = rig.routes.clone();
    pump(&mut rig.chrome, "the download of the tone", move |_| {
        routes.finished(PUBLIC_FILE) == 1
    });
    let_tasks_finish();

    assert_eq!(rig.compile.entered.load(Ordering::SeqCst), 1);
    rig.compile.release();
    pump(&mut rig.chrome, "the end of the load", |chrome| {
        chrome.load_flow_phase() == LoadPhase::Ready
    });
    assert_eq!(rig.compile.most_active.load(Ordering::SeqCst), 1);
}

#[test]
fn a_finished_switch_that_a_close_beats_before_the_poll_is_not_applied_to_the_next_detail() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105]);
    rig.routes
        .respond(&model_file(1105), HttpResponse::new(404, Vec::new()));
    rig.pick(1105);
    let routes = rig.routes.clone();
    wait_for("the failed download", || {
        routes.finished(&model_file(1105)) == 1
    });
    let_tasks_finish();

    rig.chrome.go_to_main();
    rig.routes.respond(MODELS, model_rows(&[1104, 1105]));
    rig.chrome.open_detail(BLOCK).expect("detail");
    rig.chrome.poll_community();

    assert_eq!(rig.chrome.toast(), None);
}

#[test]
fn a_signed_out_local_switch_uses_the_stash_path_with_no_network() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    let inner = detail_backend();
    let local = Tone::from_value(serde_json::json!({
        "id": 0,
        "title": "Local",
        "format": "nam",
        "local": true,
        "models": [
            {"id": 3, "name": "One", "model_url": "file:///stash/3.nam"},
            {"id": 4, "name": "Two", "model_url": "file:///stash/4.nam"},
        ],
    }))
    .expect("tone");
    inner
        .dispatch(UiAction::LoadTone {
            target_insert_id: BlockId(BLOCK.to_owned()),
            tone: local,
            model_id: ModelId(3),
        })
        .expect("local tone");
    let mut chrome = chrome_with(inner.clone(), &routes, &directory);
    chrome.open_detail(BLOCK).expect("detail");
    let session = chrome.session();
    assert!(!session.signed_in());

    chrome
        .detail_mut()
        .expect("detail")
        .select_model(ModelId(4), session)
        .expect("local switch");
    chrome.poll_community();
    let_tasks_finish();
    chrome.poll_community();

    assert_eq!(active_model(&inner), Some(ModelId(4)));
    assert!(inner.actions().iter().any(|action| matches!(
        action,
        UiAction::SelectModel {
            model_id: ModelId(4),
            ..
        }
    )));
    assert!(switched_bytes(&inner).is_empty());
    assert!(routes.state().seen.is_empty(), "no network");
}

#[test]
fn a_chrome_with_no_service_keeps_the_action_that_selects_a_model() {
    let inner = detail_backend();
    let mut chrome = ChromeController::new(inner.clone(), Vec::new(), true, true);
    chrome.open_detail(BLOCK).expect("detail");
    let session = chrome.session();
    chrome
        .detail_mut()
        .expect("detail")
        .model_select_mut()
        .set_options(
            vec![
                ModelOption {
                    id: ModelId(1104),
                    name: "One".to_owned(),
                },
                ModelOption {
                    id: ModelId(1105),
                    name: "Two".to_owned(),
                },
            ],
            2,
        );

    chrome
        .detail_mut()
        .expect("detail")
        .select_model(ModelId(1105), session)
        .expect("selection");
    chrome.poll_community();

    assert_eq!(active_model(&inner), Some(ModelId(1105)));
    assert!(switched_bytes(&inner).is_empty());
}

#[test]
fn a_click_on_the_next_model_in_a_real_frame_switches_after_the_bytes_arrive() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(MODELS, model_rows(&[1104, 1105]))
        .respond(&model_file(1105), file_answer(b"clicked"));
    let backend = detail_mock();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend.clone());
    driver.chrome_mut().open_detail(BLOCK).expect("detail");
    wait_for("the model list", || {
        driver.frame();
        driver
            .chrome()
            .detail()
            .is_some_and(|detail| detail.model_select().options().len() == 2)
    });

    let _ = driver.click("Next model").expect("the stepper");
    wait_for("the switch", || {
        driver.frame();
        active_model(&backend) == Some(ModelId(1105))
            && driver
                .chrome()
                .detail()
                .is_some_and(|detail| detail.model_select().target().is_none())
    });

    assert_eq!(switched_bytes(&backend), [b"clicked".to_vec()]);
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([9, 8, 7, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("png");
    bytes.into_inner()
}

const ART: &str = "/art.png";
const MAKER: &str = "/maker.png";

fn tone_with_images() -> Tone {
    Tone::from_value(serde_json::json!({
        "id": 104,
        "title": "Pictured",
        "format": "nam",
        "images": ["https://models.example/art.png"],
        "user": {"username": "maker", "avatar_url": "https://models.example/maker.png"},
        "models": [{"id": 1104, "name": "One", "model_url": "https://models.example/files/1104.nam"}],
    }))
    .expect("tone")
}

fn pictured_driver(routes: &Routes, directory: &TempDir) -> UiDriver {
    let backend = detail_mock();
    backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: BlockId(BLOCK.to_owned()),
            tone: tone_with_images(),
            model_id: ModelId(1104),
        })
        .expect("tone");
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), directory),
    );
    UiDriver::new(chrome, backend)
}

#[test]
fn a_tile_and_a_detail_show_remote_artwork_with_no_token() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(ART, file_answer(&png(1024, 1024)))
        .respond(MAKER, file_answer(&png(32, 32)))
        .respond(MODELS, model_rows(&[1104]));
    let mut driver = pictured_driver(&routes, &directory);
    let block = BlockId(BLOCK.to_owned());

    wait_for("the artwork of the tile", || {
        driver.frame();
        driver.chrome().gallery().has_artwork(&block)
    });
    driver.chrome_mut().open_detail(BLOCK).expect("detail");
    wait_for("the artwork of the detail", || {
        driver.frame();
        driver
            .chrome()
            .detail()
            .is_some_and(|detail| detail.artwork().is_some() && detail.avatar().is_some())
    });

    let detail = driver.chrome().detail().expect("detail");
    assert_eq!(
        detail.artwork().expect("art").size(),
        [ART_SIDE as usize, ART_SIDE as usize]
    );
    assert_eq!(
        detail.avatar().expect("avatar").size(),
        [32, 32],
        "no upscale"
    );
    assert_eq!(
        routes.count(ART),
        1,
        "one fetch for the tile and the detail"
    );
    assert_eq!(routes.count(MAKER), 1);
    assert!(!routes.seen(ART)[0].bearer);
    assert!(!routes.seen(MAKER)[0].bearer);
}

#[test]
fn a_failed_image_keeps_the_placeholder_and_is_not_fetched_in_each_frame() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(ART, HttpResponse::new(404, Vec::new()))
        .respond(MAKER, HttpResponse::new(500, Vec::new()))
        .respond(MODELS, model_rows(&[1104]));
    let mut driver = pictured_driver(&routes, &directory);
    let block = BlockId(BLOCK.to_owned());
    driver.chrome_mut().open_detail(BLOCK).expect("detail");

    wait_for("the fetches", || {
        driver.frame();
        routes.finished(ART) == 1 && routes.finished(MAKER) == 1
    });
    for _ in 0..30 {
        driver.frame();
    }
    let_tasks_finish();
    for _ in 0..5 {
        driver.frame();
    }

    assert_eq!(routes.count(ART), 1);
    assert_eq!(routes.count(MAKER), 1);
    assert!(!driver.chrome().gallery().has_artwork(&block));
    let detail = driver.chrome().detail().expect("detail");
    assert!(detail.artwork().is_none());
    assert!(detail.avatar().is_none());
}

#[test]
fn an_image_on_an_address_that_is_not_approved_is_not_requested() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    let backend = detail_mock();
    let tone = Tone::from_value(serde_json::json!({
        "id": 104,
        "title": "Pictured",
        "format": "nam",
        "images": ["http://models.example/art.png"],
        "user": {"username": "maker", "avatar_url": "https://elsewhere.example/maker.png"},
        "models": [{"id": 1104, "name": "One", "model_url": "https://models.example/files/1104.nam"}],
    }))
    .expect("tone");
    backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: BlockId(BLOCK.to_owned()),
            tone,
            model_id: ModelId(1104),
        })
        .expect("tone");
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend);

    for _ in 0..10 {
        driver.frame();
    }
    let_tasks_finish();
    driver.frame();

    assert_eq!(routes.count(ART), 0);
    assert_eq!(routes.count(MAKER), 0);
    assert!(
        !driver
            .chrome()
            .gallery()
            .has_artwork(&BlockId(BLOCK.to_owned()))
    );
}

#[test]
fn a_tile_loses_the_artwork_of_a_tone_that_left_the_block() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes.respond(ART, file_answer(&png(16, 16)));
    let backend = detail_mock();
    backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: BlockId(BLOCK.to_owned()),
            tone: tone_with_images(),
            model_id: ModelId(1104),
        })
        .expect("tone");
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend.clone());
    let block = BlockId(BLOCK.to_owned());
    wait_for("the artwork", || {
        driver.frame();
        driver.chrome().gallery().has_artwork(&block)
    });

    backend
        .dispatch(UiAction::RemoveBlock(block.clone()))
        .expect("remove");
    driver.frame();

    assert!(!driver.chrome().gallery().has_artwork(&block));
}

#[test]
fn the_account_avatar_of_the_public_user_arrives_and_a_sign_out_removes_it() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes
        .respond(
            USER,
            HttpResponse::json(
                200,
                serde_json::json!({
                    "username": "player",
                    "avatar_url": "https://models.example/player.png",
                }),
            ),
        )
        .respond("/player.png", file_answer(&png(64, 64)));
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend);

    wait_for("the avatar", || {
        driver.frame();
        driver.chrome().account_avatar().is_some()
    });
    assert!(!routes.seen("/player.png")[0].bearer);

    driver.chrome_mut().sign_out().expect("sign out");
    driver.frame();
    assert!(driver.chrome().account_avatar().is_none());
}

#[test]
fn browser_cards_and_creator_rows_show_remote_avatars_with_no_token() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    let mut row = tone_row(5, "Pictured");
    row["images"] = serde_json::json!(["https://models.example/art.png"]);
    routes
        .respond(
            SEARCH,
            HttpResponse::json(
                200,
                serde_json::json!({
                    "data": [row],
                    "page": 1,
                    "page_size": 12,
                    "total": 1,
                    "total_pages": 1,
                }),
            ),
        )
        .respond(ART, file_answer(&png(600, 600)))
        .respond(MAKER, file_answer(&png(64, 64)))
        .respond(
            "/api/v1/users",
            HttpResponse::json(
                200,
                serde_json::json!({
                    "data": [{
                        "id": 2,
                        "username": "other",
                        "name": "other",
                        "avatar_url": "https://models.example/other.png",
                    }],
                    "total_pages": 1,
                }),
            ),
        )
        .respond("/other.png", file_answer(&png(64, 64)));
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend);
    let _ = driver.click_first("Add Tone").expect("open");

    wait_for("the images of the card", || {
        driver.frame();
        let browser = driver.chrome().browser().expect("browser");
        browser.has_artwork(ToneId(5)) && browser.has_avatar(ToneId(5))
    });

    let browser = driver.chrome_mut().browser_mut().expect("browser");
    browser.set_filters_expanded(true);
    browser.set_filter_menu(Some(FilterMenu::Creators));
    wait_for("the avatar of the creator", || {
        driver.frame();
        driver
            .chrome()
            .browser()
            .expect("browser")
            .has_creator_avatar("other")
    });

    assert!(!routes.seen("/other.png")[0].bearer);
    assert!(!routes.seen(ART)[0].bearer);
    assert_eq!(routes.count(MAKER), 1);
}

#[test]
fn a_browser_keeps_textures_only_for_the_tones_of_the_current_page() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    let page = |id: i64, art: &str| {
        let mut row = tone_row(id, "Pictured");
        row["images"] = serde_json::json!([format!("https://models.example{art}")]);
        HttpResponse::json(
            200,
            serde_json::json!({
                "data": [row],
                "page": 1,
                "page_size": 12,
                "total": 24,
                "total_pages": 2,
            }),
        )
    };
    routes
        .respond(SEARCH, page(5, ART))
        .respond(SEARCH, page(6, "/art-two.png"))
        .respond(ART, file_answer(&png(16, 16)))
        .respond("/art-two.png", file_answer(&png(16, 16)))
        .respond(MAKER, file_answer(&png(16, 16)));
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, backend);
    let _ = driver.click_first("Add Tone").expect("open");
    wait_for("the first page", || {
        driver.frame();
        driver
            .chrome()
            .browser()
            .expect("browser")
            .has_artwork(ToneId(5))
    });

    let browser = driver.chrome_mut().browser_mut().expect("browser");
    assert!(browser.store_mut().turn_page(2));
    wait_for("the second page", || {
        driver.frame();
        driver
            .chrome()
            .browser()
            .expect("browser")
            .has_artwork(ToneId(6))
    });

    assert!(
        !driver
            .chrome()
            .browser()
            .expect("browser")
            .has_artwork(ToneId(5))
    );
}

#[test]
fn a_closed_detail_stops_the_download_of_its_switch() {
    let mut rig = DetailRig::new();
    rig.open(&[1104, 1105]);
    rig.offer_file(1105, b"late");
    rig.routes.hold(&model_file(1105));
    rig.pick(1105);
    rig.wait_for_download(1105);

    rig.chrome.go_to_main();
    rig.chrome.poll_community();
    let_tasks_finish();
    rig.routes.release(&model_file(1105));
    let_tasks_finish();

    assert_eq!(rig.routes.finished(&model_file(1105)), 0);
}

/// One switch that a worker holds at its compile, with a flow that nothing else changes.
struct HeldSwitch {
    rig: LoadRig,
    inner: Arc<MockBackend>,
    compile: Arc<Compile>,
}

fn worker_switch_at_its_compile() -> HeldSwitch {
    let mut rig = LoadRig::new();
    rig.routes.respond("/files/8.nam", file_answer(b"late"));
    let inner = detail_backend();
    let compile = Arc::new(Compile::default());
    compile.hold();
    let tone = inner.chain_state(0).chain.left[0]
        .tone
        .clone()
        .expect("tone");
    let model = ToneModel::from_value(
        &serde_json::json!({
            "id": 8,
            "name": "Other",
            "model_url": "https://models.example/files/8.nam",
        }),
        false,
    )
    .expect("model");
    let flow = ModelSwitchFlow::new(Arc::new(HeldBackend {
        inner: inner.clone(),
        compile: compile.clone(),
        panics: false,
    }));
    let ticket = flow.begin(BlockId(BLOCK.to_owned()), Some(ACTIVE_MODEL));
    rig.worker.start_model_switch(
        rig.runtime.handle(),
        ModelSwitchRequest {
            tone,
            model,
            ticket,
        },
    );
    wait_for("the compile", || {
        compile.entered.load(Ordering::SeqCst) == 1
    });
    HeldSwitch {
        rig,
        inner,
        compile,
    }
}

#[test]
fn a_late_compile_after_a_cancellation_of_the_worker_reaches_no_processor() {
    for scope in [CommunityScope::Detail, CommunityScope::Session] {
        let mut held = worker_switch_at_its_compile();

        held.rig.worker.cancel(scope);
        held.compile.release();
        let_tasks_finish();

        assert!(held.rig.worker.poll().is_empty(), "{scope:?}");
        assert!(switched_bytes(&held.inner).is_empty(), "{scope:?}");
    }
}

#[test]
fn a_browser_close_leaves_the_switch_of_a_detail_alone() {
    let mut held = worker_switch_at_its_compile();

    held.rig.worker.cancel(CommunityScope::Browser);
    held.compile.release();
    let events = events_until(&mut held.rig.worker, |events| !events.is_empty());

    assert!(
        matches!(
            events.as_slice(),
            [CommunityEvent::ModelSwitch { result: Ok(()), .. }]
        ),
        "{events:?}"
    );
    assert_eq!(switched_bytes(&held.inner), [b"late".to_vec()]);
    assert_eq!(active_model(&held.inner), Some(ModelId(8)));
}

#[test]
fn a_panic_in_the_compile_of_a_switch_ends_it_with_a_failure() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    routes.respond("/files/8.nam", file_answer(b"bytes"));
    let (mut worker, _handle) = worker_over(&routes, &directory);
    let runtime = Runtime::new().expect("runtime");
    let flow = ModelSwitchFlow::new(Arc::new(HeldBackend {
        inner: detail_backend(),
        compile: Arc::default(),
        panics: true,
    }));
    let tone = Tone::from_value(tone_row(104, "Remote")).expect("tone");
    let model = ToneModel::from_value(
        &serde_json::json!({
            "id": 8,
            "name": "Other",
            "model_url": "https://models.example/files/8.nam",
        }),
        false,
    )
    .expect("model");
    worker.start_model_switch(
        runtime.handle(),
        ModelSwitchRequest {
            tone,
            model,
            ticket: flow.begin(BlockId(BLOCK.to_owned()), Some(ACTIVE_MODEL)),
        },
    );

    let events = events_until(&mut worker, |events| !events.is_empty());

    assert!(matches!(
        events.as_slice(),
        [CommunityEvent::ModelSwitch {
            result: Err(CommunityFailure::Failed(_)),
            ..
        }]
    ));
}

// The rest of the adversarial matrix. Each case checks its public outcome, and also that the
// chain and the stored session did not change.

#[test]
fn an_editor_close_at_each_network_stage_reaches_no_processor() {
    for held in [TONE_FIVE, MODELS, PUBLIC_FILE, TOKEN] {
        let directory = tempfile::tempdir().expect("settings root");
        let routes = signed_in_routes(&directory);
        routes
            .respond(TONE_FIVE, tone_answer(5, "nam"))
            .respond(MODELS, one_model(7, UPSTREAM_URL))
            .respond(PUBLIC_FILE, HttpResponse::new(401, Vec::new()))
            .respond(TOKEN, token_response("rotated-access", "rotated-refresh"))
            .respond(PUBLIC_FILE, file_answer(b"late"));
        routes.hold(held);
        let inner = backend();
        let revision = inner.chain_state(0).revision;
        let mut chrome = chrome_with(inner.clone(), &routes, &directory);
        open_browser(&mut chrome);
        chrome.pick_tone(catalog_row(5));
        pump(&mut chrome, "the held stage", |_| routes.count(held) == 1);
        let sent = routes.count(TONE_FIVE) + routes.count(MODELS) + routes.count(PUBLIC_FILE);

        drop(chrome);
        routes.release(held);
        let_tasks_finish();

        assert!(published_bytes(&inner).is_empty(), "{held}");
        assert_eq!(inner.chain_state(0).revision, revision, "{held}");
        assert_eq!(
            routes.count(TONE_FIVE) + routes.count(MODELS) + routes.count(PUBLIC_FILE),
            sent,
            "{held}: the closed editor sent another request"
        );
        let (_auth, handle) = service_over(routes.clone(), &directory).into_parts();
        assert!(handle.has_stored_tokens(), "{held}: a close is no sign-out");
    }
}

/// A load over a transport that a test scripts, with one flow and one fixture chain.
fn load_over_recorded(
    transport: RecordedTransport,
) -> (
    Result<(), CommunityFailure>,
    Arc<MockBackend>,
    u64,
    CommunityHandle<RecordedTransport>,
    TempDir,
) {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let (_auth, handle) = service_over(transport, &directory).into_parts();
    let mut worker = CommunityWorker::new(handle.clone());
    let runtime = Runtime::new().expect("runtime");
    let backend = backend();
    let revision = backend.chain_state(0).revision;
    let mut flow = load_flow_over(backend.clone());
    let ticket = flow.begin_remote_load(catalog_row(5)).expect("load");
    worker.start_tone_load(
        runtime.handle(),
        LoadRequest {
            tone: ToneId(5),
            ticket,
        },
    );
    let mut events = Vec::new();
    wait_for("the load", || {
        events.extend(worker.poll());
        tone_load_result(&events).is_some()
    });
    let result = tone_load_result(&events).expect("a result");
    (result, backend, revision, handle, directory)
}

#[test]
fn a_malformed_catalog_answer_keeps_the_chain_and_the_session() {
    let cases: [(&str, HttpResponse, Option<HttpResponse>); 4] = [
        ("a tone that is not JSON", file_answer(b"{not json"), None),
        (
            "a tone of the wrong shape",
            HttpResponse::json(200, serde_json::json!({"id": "five"})),
            None,
        ),
        (
            "models that are not JSON",
            tone_answer(5, "nam"),
            Some(file_answer(b"[")),
        ),
        (
            "models of the wrong shape",
            tone_answer(5, "nam"),
            Some(HttpResponse::json(200, serde_json::json!({"data": "none"}))),
        ),
    ];
    for (name, tone, models) in cases {
        let mut rig = LoadRig::new();
        let revision = rig.backend.chain_state(0).revision;
        rig.routes.respond(TONE_FIVE, tone);
        if let Some(models) = models {
            rig.routes.respond(MODELS, models);
        }

        rig.start(5);

        assert!(
            matches!(rig.result(), Err(CommunityFailure::Failed(_))),
            "{name}"
        );
        rig.assert_chain_unchanged(revision);
        assert!(rig.handle.has_stored_tokens(), "{name}");
        assert_eq!(rig.routes.count(PUBLIC_FILE), 0, "{name}");
        assert_eq!(rig.routes.count(TOKEN), 0, "{name}");
    }
}

#[test]
fn an_oversized_catalog_answer_keeps_the_chain_and_the_session() {
    let padding = vec![b' '; 4096];
    let padded = |mut answer: HttpResponse| {
        answer.body.extend_from_slice(&padding);
        answer
    };
    let cases = [
        ("the tone", vec![Ok(padded(tone_answer(5, "nam")))]),
        (
            "the models",
            vec![
                Ok(tone_answer(5, "nam")),
                Ok(padded(one_model(7, PUBLIC_URL))),
            ],
        ),
    ];
    for (name, answers) in cases {
        let transport = RecordedTransport::with_json_limit(answers, 2048);

        let (result, backend, revision, handle, _directory) = load_over_recorded(transport.clone());

        assert!(matches!(result, Err(CommunityFailure::Failed(_))), "{name}");
        assert!(published_bytes(&backend).is_empty(), "{name}");
        assert_eq!(backend.chain_state(0).revision, revision, "{name}");
        assert!(handle.has_stored_tokens(), "{name}");
        assert!(
            transport
                .requests()
                .iter()
                .all(|request| request.url.path() != PUBLIC_FILE),
            "{name}"
        );
    }
}

#[test]
fn a_model_transport_failure_retries_one_time_and_then_keeps_the_chain() {
    let first_answers = || {
        vec![
            Ok(tone_answer(5, "nam")),
            Ok(one_model(7, PUBLIC_URL)),
            Err(HttpError::Transport),
        ]
    };
    let mut retried = first_answers();
    retried.push(Ok(file_answer(b"second try")));
    let transport = RecordedTransport::new(retried);
    let (result, backend, _, _, _directory) = load_over_recorded(transport.clone());
    assert_eq!(result, Ok(()));
    assert_eq!(published_bytes(&backend), [b"second try".to_vec()]);
    assert_eq!(transport.requests().len(), 4);

    let mut failed = first_answers();
    failed.push(Err(HttpError::Transport));
    // The test would see a third attempt take these bytes.
    failed.push(Ok(file_answer(b"third try")));
    let transport = RecordedTransport::new(failed);
    let (result, backend, revision, handle, _directory) = load_over_recorded(transport.clone());
    assert!(matches!(result, Err(CommunityFailure::Failed(_))));
    assert!(published_bytes(&backend).is_empty());
    assert_eq!(backend.chain_state(0).revision, revision);
    assert!(handle.has_stored_tokens());
    assert_eq!(transport.requests().len(), 4, "a failure retries one time");
}

#[test]
fn a_model_401_adopts_the_token_of_another_process_with_no_refresh() {
    let mut rig = LoadRig::new();
    rig.routes
        .respond(TONE_FIVE, tone_answer(5, "nam"))
        .respond(MODELS, one_model(7, UPSTREAM_URL))
        .respond(PUBLIC_FILE, HttpResponse::new(401, Vec::new()))
        .respond(PUBLIC_FILE, file_answer(b"with the new token"));
    rig.routes.hold(PUBLIC_FILE);
    rig.start(5);
    wait_for("the first download", || rig.routes.count(PUBLIC_FILE) == 1);

    let other_process = SessionStore::new(
        settings(&rig._directory),
        ronie_core::OAuthFlow::new(
            OAuthConfig::new(
                Url::parse("https://api.example").expect("origin"),
                "public-client",
            ),
            recorded(Vec::new()),
        ),
    );
    other_process
        .set_tokens(OAuthTokens {
            access: SecretString::from("other-access"),
            refresh: SecretString::from("other-refresh"),
            expires_at_ms: i64::MAX,
        })
        .expect("the rotation of the other process");
    rig.routes.release(PUBLIC_FILE);

    assert_eq!(rig.result(), Ok(()));
    assert_eq!(rig.routes.count(PUBLIC_FILE), 2);
    assert_eq!(
        rig.routes.count(TOKEN),
        0,
        "the newer token needs no refresh"
    );
    assert_eq!(
        published_bytes(&rig.backend),
        [b"with the new token".to_vec()]
    );
}

#[test]
fn a_cancelled_browser_sign_in_stops_the_listener_and_stores_nothing() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes.respond(TOKEN, token_response("never", "never"));
    let (mut auth, handle) = service_over(routes.clone(), &directory).into_parts();
    let runtime = Runtime::new().expect("runtime");

    auth.start_browser(runtime.handle());
    let mut opened = None;
    wait_for("the listener", || {
        if let Some(FlowEvent::BrowserReady(url)) = auth.poll() {
            opened = Some(url);
        }
        opened.is_some()
    });
    let opened = Url::parse(&opened.expect("authorize page")).expect("authorize URL");
    let redirect = opened
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .expect("redirect field")
        .1
        .into_owned();
    let port = Url::parse(&redirect)
        .expect("redirect URI")
        .port()
        .expect("port");
    let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let _open = std::net::TcpStream::connect(address).expect("the listener accepts");

    auth.close();

    wait_for("the end of the listener", || {
        std::net::TcpStream::connect(address).is_err()
    });
    assert_eq!(auth.snapshot().phase, AuthPhase::Idle);
    assert!(!handle.has_stored_tokens());
    assert_eq!(routes.count(TOKEN), 0);
}

#[test]
fn a_device_code_that_waits_and_then_expires_stores_nothing() {
    let directory = tempfile::tempdir().expect("settings root");
    let transport = recorded(vec![
        device_response(),
        HttpResponse::json(400, serde_json::json!({"error": "authorization_pending"})),
        HttpResponse::json(400, serde_json::json!({"error": "expired_token"})),
    ]);
    let (mut auth, handle) = service_over(transport.clone(), &directory).into_parts();
    let runtime = Runtime::new().expect("runtime");

    auth.start_device(runtime.handle());
    let mut finished = None;
    wait_for("the end of the flow", || {
        if let Some(FlowEvent::Finished(result)) = auth.poll() {
            finished = Some(result);
        }
        finished.is_some()
    });

    assert!(matches!(
        finished,
        Some(Err(ronie_core::OAuthError::ExpiredDeviceCode))
    ));
    assert_eq!(auth.snapshot().phase, AuthPhase::DeviceFailed);
    assert!(!handle.has_stored_tokens());
    assert_eq!(transport.requests().len(), 3);
}

#[test]
fn an_offline_launch_keeps_the_session_the_cached_user_and_the_chain() {
    let directory = tempfile::tempdir().expect("settings root");
    earlier_launch_signed_in(&directory, i64::MAX);
    let routes = Routes::default();
    let (_auth, earlier) = service_over(routes.clone(), &directory).into_parts();
    earlier
        .tokens()
        .set_cached_user(serde_json::json!({"username": "offline player"}))
        .expect("cached user");
    let inner = backend();
    let revision = inner.chain_state(0).revision;

    // This transport answers nothing, as a computer with no network does.
    let mut chrome = chrome_with(inner.clone(), &routes, &directory);
    pump(&mut chrome, "the user refresh", |_| {
        routes.finished(USER) == 1
    });
    let_tasks_finish();
    chrome.poll_community();

    assert!(chrome.session().signed_in(), "offline is not a sign-out");
    // The service gave no answer, so that the chip shows the cache and no fresh user exists.
    assert!(chrome.public_user().is_none());
    assert_eq!(chrome.account_name().as_deref(), Some("offline player"));
    let (_auth, handle) = service_over(routes.clone(), &directory).into_parts();
    assert!(handle.has_stored_tokens());
    assert_eq!(
        handle.tokens().cached_user().expect("read")["username"],
        "offline player"
    );
    assert_eq!(inner.chain_state(0).revision, revision);
    assert!(published_bytes(&inner).is_empty());
}

const OFFLINE_NAME: &str = "offline player";

/// Stores tokens and a cached user as an earlier launch did, and starts this launch.
fn launch_with_cached_user(
    routes: &Routes,
    directory: &TempDir,
    user: serde_json::Value,
) -> UiDriver {
    earlier_launch_signed_in(directory, i64::MAX);
    let (_auth, earlier) = service_over(routes.clone(), directory).into_parts();
    earlier.tokens().set_cached_user(user).expect("cached user");
    launch(routes, directory)
}

fn launch(routes: &Routes, directory: &TempDir) -> UiDriver {
    let backend = empty_chain_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), directory),
    );
    UiDriver::new(chrome, backend)
}

#[test]
fn the_chip_shows_the_cached_name_until_a_fresh_user_replaces_it() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes.respond(USER, user_response());
    routes.hold(USER);
    let mut driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({"username": OFFLINE_NAME}),
    );

    assert!(driver.is_visible(OFFLINE_NAME));
    assert!(driver.chrome().public_user().is_none());

    routes.release(USER);
    wait_for("the fresh user", || {
        driver.frame();
        driver.is_visible("player")
    });

    assert!(!driver.is_visible(OFFLINE_NAME));
    assert!(driver.chrome().public_user().is_some());
}

#[test]
fn a_failed_user_refresh_keeps_the_cached_name() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    let mut driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({"username": OFFLINE_NAME}),
    );

    wait_for("the user refresh", || {
        driver.frame();
        routes.finished(USER) == 1
    });
    let_tasks_finish();
    driver.settle();

    assert!(driver.chrome().session().signed_in());
    assert!(driver.is_visible(OFFLINE_NAME));
}

#[test]
fn a_sign_out_removes_the_cached_name_and_keeps_the_cache() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    let mut driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({"username": OFFLINE_NAME}),
    );
    assert!(driver.is_visible(OFFLINE_NAME));

    let _ = driver.click("Account").expect("menu");
    let _ = driver.click("Logout").expect("sign out");

    assert!(!driver.chrome().session().signed_in());
    assert!(!driver.is_visible(OFFLINE_NAME));
    assert_eq!(driver.chrome().account_name(), None);
    let (_auth, handle) = service_over(routes.clone(), &directory).into_parts();
    assert_eq!(
        handle.tokens().cached_user().expect("read")["username"],
        OFFLINE_NAME
    );
}

#[test]
fn a_lost_session_removes_the_cached_name() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes
        .respond(USER, HttpResponse::new(401, Vec::new()))
        .respond(TOKEN, rejected_refresh());
    routes.hold(USER);
    let mut driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({"username": OFFLINE_NAME}),
    );
    assert!(driver.is_visible(OFFLINE_NAME));

    routes.release(USER);
    wait_for("the lost session", || {
        driver.frame();
        !driver.chrome().session().signed_in()
    });
    driver.settle();

    assert!(!driver.is_visible(OFFLINE_NAME));
    assert_eq!(driver.chrome().account_name(), None);
}

#[test]
fn a_launch_with_no_session_shows_no_cached_name_and_reads_no_avatar() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    let (_auth, earlier) = service_over(routes.clone(), &directory).into_parts();
    earlier
        .tokens()
        .set_cached_user(serde_json::json!({
            "username": OFFLINE_NAME,
            "avatar_url": "https://models.example/cached.png",
        }))
        .expect("cached user");

    let mut driver = launch(&routes, &directory);
    driver.settle();

    assert!(!driver.chrome().session().signed_in());
    assert!(!driver.is_visible(OFFLINE_NAME));
    assert_eq!(routes.count("/cached.png"), 0);
}

#[test]
fn the_chip_reads_the_cache_one_time() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    let mut driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({"username": OFFLINE_NAME}),
    );
    let (_auth, other) = service_over(routes.clone(), &directory).into_parts();
    other
        .tokens()
        .set_cached_user(serde_json::json!({"username": "later value"}))
        .expect("cached user");

    driver.settle();

    assert!(driver.is_visible(OFFLINE_NAME));
    assert!(!driver.is_visible("later value"));
}

#[test]
fn the_chip_takes_only_a_clean_name_and_an_avatar_from_the_cache() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    let driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({
            "id": 7,
            "username": "plain-handle",
            "display_name": "Clean\u{202e}\n  Name",
            "email": "person@example.test",
            "access_token": "fake-cached-token",
        }),
    );

    assert_eq!(
        driver.chrome().account_name().as_deref(),
        Some("Clean Name")
    );
    assert!(driver.is_visible("Clean Name"));
    for field in [
        "plain-handle",
        "person@example.test",
        "fake-cached-token",
        "7",
    ] {
        assert!(!driver.is_visible(field), "{field}");
    }
}

#[test]
fn a_cache_with_no_usable_name_shows_no_name() {
    for user in [
        serde_json::json!({"display_name": " \u{200b} "}),
        serde_json::json!({"username": 7}),
        serde_json::json!("not an object"),
    ] {
        let directory = tempfile::tempdir().expect("settings root");
        let driver = launch_with_cached_user(&Routes::default(), &directory, user);
        assert_eq!(driver.chrome().account_name(), None);
    }
}

#[test]
fn the_cached_avatar_arrives_through_the_image_path_with_no_token() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes.respond("/cached.png", file_answer(&png(64, 64)));
    let mut driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({
            "username": OFFLINE_NAME,
            "avatar_url": "https://models.example/cached.png",
        }),
    );

    wait_for("the cached avatar", || {
        driver.frame();
        driver.chrome().account_avatar().is_some()
    });
    assert!(!routes.seen("/cached.png")[0].bearer);

    let _ = driver.click("Account").expect("menu");
    let _ = driver.click("Logout").expect("sign out");
    assert!(driver.chrome().account_avatar().is_none());
}

#[test]
fn a_failed_cached_avatar_keeps_the_placeholder_and_the_name() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes.respond("/cached.png", HttpResponse::new(404, Vec::new()));
    let mut driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({
            "username": OFFLINE_NAME,
            "avatar_url": "https://models.example/cached.png",
        }),
    );

    wait_for("the failed avatar", || {
        driver.frame();
        routes.finished("/cached.png") == 1
    });
    let_tasks_finish();
    driver.settle();

    assert!(driver.chrome().account_avatar().is_none());
    assert!(driver.is_visible(OFFLINE_NAME));
    assert!(driver.chrome().session().signed_in());
    assert!(driver.chrome().toast().is_none());
    assert_eq!(routes.count("/cached.png"), 1, "no fetch in each frame");
}

#[test]
fn a_fresh_user_with_no_avatar_removes_the_cached_avatar() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = Routes::default();
    routes
        .respond("/cached.png", file_answer(&png(64, 64)))
        .respond(USER, user_response());
    routes.hold(USER);
    let mut driver = launch_with_cached_user(
        &routes,
        &directory,
        serde_json::json!({
            "username": OFFLINE_NAME,
            "avatar_url": "https://models.example/cached.png",
        }),
    );
    wait_for("the cached avatar", || {
        driver.frame();
        driver.chrome().account_avatar().is_some()
    });

    routes.release(USER);
    wait_for("the fresh user", || {
        driver.frame();
        driver.chrome().public_user().is_some()
    });
    driver.settle();

    assert!(driver.chrome().account_avatar().is_none());
}

#[test]
fn a_long_name_stays_in_the_chip_and_the_preset_controls_do_not_move() {
    let rect = |driver: &UiDriver, label: &str| {
        driver
            .controls()
            .iter()
            .find(|control| control.label == label)
            .unwrap_or_else(|| panic!("{label}"))
            .rect
    };
    let plain_directory = tempfile::tempdir().expect("settings root");
    let plain = launch(&Routes::default(), &plain_directory);
    let directory = tempfile::tempdir().expect("settings root");
    let long = "a very long display name that no chip can hold on one line";
    let named = launch_with_cached_user(
        &Routes::default(),
        &directory,
        serde_json::json!({"display_name": long}),
    );

    for label in ["Previous preset", "Next preset", "Browse presets"] {
        assert_eq!(rect(&plain, label), rect(&named, label), "{label}");
    }
    let chip = rect(&named, "Account");
    assert_eq!(chip.right(), rect(&plain, "Account").right());
    assert!(chip.left() < rect(&plain, "Account").left(), "{chip:?}");
    assert!(rect(&named, "Redo").right() <= chip.left());
    assert!(rect(&named, "Tuner").right() <= rect(&named, "Undo").left());
    assert!(
        chip.contains_rect(rect(&named, long)),
        "the name stays inside"
    );
}

#[test]
fn an_image_404_keeps_the_placeholder_the_session_and_the_chain() {
    let directory = tempfile::tempdir().expect("settings root");
    let routes = signed_in_routes(&directory);
    routes
        .respond(ART, HttpResponse::new(404, Vec::new()))
        .respond(MAKER, HttpResponse::new(404, Vec::new()))
        .respond(MODELS, model_rows(&[1104]));
    let mock = detail_mock();
    mock.dispatch(UiAction::LoadTone {
        target_insert_id: BlockId(BLOCK.to_owned()),
        tone: tone_with_images(),
        model_id: ModelId(1104),
    })
    .expect("tone");
    let revision = mock.chain_state(0).revision;
    let chrome = ChromeController::with_community(
        Arc::new(mock.clone()),
        Vec::new(),
        false,
        true,
        service_over(routes.clone(), &directory),
    );
    let mut driver = UiDriver::new(chrome, mock.clone());

    wait_for("the image", || {
        driver.frame();
        routes.finished(ART) == 1
    });
    let_tasks_finish();
    driver.frame();

    assert!(
        !driver
            .chrome()
            .gallery()
            .has_artwork(&BlockId(BLOCK.to_owned()))
    );
    assert!(driver.chrome().session().signed_in());
    assert!(driver.chrome().toast().is_none());
    assert_eq!(mock.chain_state(0).revision, revision);
    assert!(published_bytes(&mock).is_empty());
    assert_eq!(routes.count(TOKEN), 0);
}

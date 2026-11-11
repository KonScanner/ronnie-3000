//! Sign-in through the real chrome. The session owns its runtime, so these tests can run on
//! any thread, and one test runs both on a plain thread and inside a test runtime.

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    sync::Arc,
    time::{Duration, Instant},
};

use ronie_core::{
    BlockId, ChainSide, HttpError, HttpResponse, OAuthConfig, OAuthError, OAuthFlow,
    RecordedTransport, SettingsStore, UI_PREFERENCES_SETTINGS,
};
use ronie_ui::{
    backend::MockBackend,
    default::{AuthPhase, AuthService, BrowserView, ChromeController, Overlay},
    driver::UiDriver,
    services::session::{AuthFlowAdapter, AuthSnapshot, FlowEvent},
    testbed::load_scenarios,
};
use url::Url;

fn flow(responses: Vec<HttpResponse>) -> OAuthFlow<RecordedTransport> {
    OAuthFlow::new(
        OAuthConfig::new(
            Url::parse("https://api.example").expect("origin"),
            "public-client",
        ),
        RecordedTransport::new(responses.into_iter().map(Ok).collect()),
    )
}

fn token_response() -> HttpResponse {
    HttpResponse::json(
        200,
        serde_json::json!({"access_token": "access", "refresh_token": "refresh", "expires_in": 3600}),
    )
}

fn scenario_backend(id: &str) -> MockBackend {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario");
    MockBackend::from_scenario(&scenario)
}

fn main_mono_backend() -> MockBackend {
    scenario_backend("main-mono")
}

fn chrome(auth: Box<dyn AuthFlowAdapter>) -> ChromeController {
    ChromeController::with_auth_flow(Arc::new(main_mono_backend()), Vec::new(), false, true, auth)
}

/// The real UI behind real input.
fn driver(auth: Box<dyn AuthFlowAdapter>) -> UiDriver {
    let backend = main_mono_backend();
    let chrome =
        ChromeController::with_auth_flow(Arc::new(backend.clone()), Vec::new(), false, true, auth);
    UiDriver::new(chrome, backend)
}

fn frames_until(
    driver: &mut UiDriver,
    mut done: impl FnMut(&ChromeController, &[String]) -> bool,
) -> Vec<String> {
    let started = Instant::now();
    let mut opened = Vec::new();
    while started.elapsed() < Duration::from_secs(10) {
        driver.frame();
        opened.extend(driver.interaction().opened_urls);
        if done(driver.chrome(), &opened) {
            return opened;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the sign-in flow did not reach the expected state");
}

/// Clicks a control that starts the browser flow, and waits for the page to open.
fn click_and_wait_for_browser(driver: &mut UiDriver, label: &str) -> String {
    let mut opened = driver.click(label).expect("sign-in control").opened_urls;
    if opened.is_empty() {
        opened = frames_until(driver, |_, opened| !opened.is_empty());
    }
    assert_eq!(opened.len(), 1, "the click must open the system browser");
    opened.remove(0)
}

/// The socket of the listener. `localhost` can resolve to `::1` first, and the listener is not
/// there, so an address from name resolution would make a stopped listener look real.
fn listener_address(redirect: &Url) -> SocketAddr {
    let port = redirect.port().expect("redirect port");
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// Plays the system browser: a spare socket, a favicon request, then the redirect.
fn browser_returns_to(authorize_url: &str) {
    let authorize = Url::parse(authorize_url).expect("authorize URL");
    let query = |name: &str| {
        authorize
            .query_pairs()
            .find(|(key, _)| key == name)
            .expect("query field")
            .1
            .into_owned()
    };
    let redirect = Url::parse(&query("redirect_uri")).expect("redirect URI");
    let address = listener_address(&redirect);
    let _spare = TcpStream::connect(address).expect("spare socket");
    let mut favicon = TcpStream::connect(address).expect("favicon socket");
    write!(favicon, "GET /favicon.ico HTTP/1.1\r\nHost: x\r\n\r\n").expect("favicon request");
    let mut callback = TcpStream::connect(address).expect("callback socket");
    write!(
        callback,
        "GET /?code=secret-code&state={} HTTP/1.1\r\nHost: x\r\n\r\n",
        query("state")
    )
    .expect("callback request");
    let mut page = String::new();
    callback.read_to_string(&mut page).expect("callback page");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
}

#[test]
fn mouse_clicks_open_each_header_popover_and_an_outside_click_closes_it() {
    let mut driver = driver(Box::new(AuthService::new(flow(Vec::new()))));
    for (button, overlay) in [
        ("Account", Overlay::Account),
        ("Browse presets", Overlay::PresetBrowser),
        ("Save preset", Overlay::SavePreset),
    ] {
        let _ = driver.click(button).expect("header button");
        assert_eq!(
            driver.chrome().overlay(),
            overlay,
            "the opening click must not close it"
        );
        let _ = driver.click_empty_space();
        assert_eq!(
            driver.chrome().overlay(),
            Overlay::None,
            "an outside click must close it"
        );
    }
}

#[test]
fn mouse_sign_in_from_the_account_menu_reaches_the_browser_and_back() {
    let mut driver = driver(Box::new(AuthService::new(flow(vec![token_response()]))));

    let _ = driver.click("Account").expect("account button");
    assert_eq!(driver.chrome().overlay(), Overlay::Account);
    let authorize = click_and_wait_for_browser(&mut driver, "Sign in");

    browser_returns_to(&authorize);
    frames_until(&mut driver, |chrome, _| chrome.session().signed_in());

    let _ = driver.click("Account").expect("account button");
    let _ = driver.click("Logout").expect("sign-out row");
    assert!(
        !driver.chrome().session().signed_in(),
        "the second row must sign out now"
    );
}

#[test]
fn sign_in_finishes_and_persists_while_a_browser_covers_the_window() {
    let directory = tempfile::tempdir().expect("settings root");
    let settings = || SettingsStore::standalone(directory.path(), UI_PREFERENCES_SETTINGS);
    let mut covered = driver(Box::new(AuthService::with_session(
        flow(vec![token_response()]),
        settings(),
    )));
    covered.chrome_mut().sign_in().expect("start sign-in");
    let opened = frames_until(&mut covered, |_, opened| !opened.is_empty());
    browser_returns_to(&opened[0]);

    // eframe runs no UI frame for an occluded window, only `App::logic`.
    let started = Instant::now();
    while !covered.chrome().session().signed_in() {
        covered.chrome_mut().poll_auth();
        assert!(started.elapsed() < Duration::from_secs(10), "no completion");
        std::thread::sleep(Duration::from_millis(10));
    }
    let next_launch = chrome(Box::new(AuthService::with_session(
        flow(Vec::new()),
        settings(),
    )));
    assert!(next_launch.session().signed_in());
}

#[test]
fn back_arrow_leaves_the_sign_in_page_and_cancels_the_flow() {
    let mut driver = driver(Box::new(AuthService::new(flow(Vec::new()))));
    driver.chrome_mut().sign_in().expect("start sign-in");
    assert!(driver.chrome().auth_active(), "a start must not wait");
    let opened = frames_until(&mut driver, |_, opened| !opened.is_empty());
    let authorize = Url::parse(&opened[0]).expect("authorize URL");
    let redirect = authorize
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .expect("redirect field")
        .1
        .into_owned();
    let address = listener_address(&Url::parse(&redirect).expect("redirect URI"));
    let _before = TcpStream::connect(address).expect("the listener must accept before the close");

    let _ = driver.click("Back").expect("back arrow");
    assert!(
        !driver.chrome().auth_active(),
        "the back arrow must close the page"
    );

    // Closing aborts the listener task, and the abort finishes on a runtime thread.
    let started = Instant::now();
    while TcpStream::connect(address).is_ok() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the loopback listener must stop"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

const TONE_PROMPT: &str = "Sign in or create free account";

fn takes_a_click(driver: &UiDriver, label: &str) -> bool {
    driver
        .controls()
        .iter()
        .any(|control| control.label == label && control.clickable)
}

fn sign_out_from_the_account_menu(driver: &mut UiDriver) {
    let _ = driver.click("Account").expect("account button");
    let _ = driver.click("Logout").expect("sign-out row");
    driver.settle();
    assert!(!driver.chrome().session().signed_in());
}

fn finish_sign_in(driver: &mut UiDriver, label: &str) {
    let authorize = click_and_wait_for_browser(driver, label);
    assert!(driver.chrome().auth_active(), "the page shows the wait");
    browser_returns_to(&authorize);
    frames_until(driver, |chrome, _| chrome.session().signed_in());
    driver.settle();
    assert!(!driver.chrome().auth_active(), "a finished flow leaves");
}

/// One session, and every surface follows it without a copy of its own.
fn every_surface_follows_sign_in_and_sign_out() {
    let backend = scenario_backend("main-detail");
    let chrome = ChromeController::with_auth_flow(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        Box::new(AuthService::new(flow(vec![
            token_response(),
            token_response(),
            token_response(),
            token_response(),
        ]))),
    );
    let mut driver = UiDriver::new(chrome, backend);

    // Account menu.
    let _ = driver.click("Account").expect("account button");
    assert!(driver.is_visible("Sign in"));
    assert!(!driver.is_visible("Logout"));
    finish_sign_in(&mut driver, "Sign in");
    let _ = driver.click("Account").expect("account button");
    assert!(driver.is_visible("Logout"), "{:?}", driver.visible_labels());
    assert!(!driver.is_visible("Sign in"));
    let _ = driver.click_empty_space();
    sign_out_from_the_account_menu(&mut driver);

    // Model select: disabled while signed out, for a tone from the catalog.
    driver.chrome_mut().open_detail("blk-1").expect("detail");
    driver.settle();
    assert!(driver.is_visible("Select model"));
    assert!(!takes_a_click(&driver, "Select model"));
    let _ = driver.click("Account").expect("account button");
    finish_sign_in(&mut driver, "Sign in");
    assert!(takes_a_click(&driver, "Select model"));
    sign_out_from_the_account_menu(&mut driver);
    assert!(!takes_a_click(&driver, "Select model"));

    // Block detail: the metadata prompt starts the flow, and it leaves with the sign-in.
    let _ = driver.click("Tone info").expect("info tab");
    assert!(driver.is_visible("Log In"), "{:?}", driver.visible_labels());
    finish_sign_in(&mut driver, "Log In");
    assert!(!driver.is_visible("Log In"));
    sign_out_from_the_account_menu(&mut driver);
    assert!(driver.is_visible("Log In"));

    // Tone browser: the prompt starts the flow, and the signed-in search replaces it.
    let _ = driver.click("Swap tone").expect("swap");
    driver.settle();
    assert!(
        driver.is_visible(TONE_PROMPT),
        "{:?}",
        driver.visible_labels()
    );
    assert!(!driver.is_visible("Search tones"));
    let browser = driver.chrome_mut().browser_mut().expect("browser");
    hold_a_catalog_result(browser);
    browser.set_sign_in_page(true);
    driver.settle();
    finish_sign_in(&mut driver, TONE_PROMPT);
    let browser = driver.chrome().browser().expect("browser");
    assert!(!browser.sign_in_page(), "a sign-in leaves the card prompt");
    assert!(browser.store().result().is_none(), "the session changed");
    assert!(driver.is_visible("Search tones"));
    assert!(!driver.is_visible(TONE_PROMPT));
    hold_a_catalog_result(driver.chrome_mut().browser_mut().expect("browser"));
    sign_out_from_the_account_menu(&mut driver);
    let browser = driver.chrome().browser().expect("browser");
    assert!(browser.store().result().is_none(), "the session changed");
    assert!(driver.is_visible(TONE_PROMPT));
    assert!(!driver.is_visible("Search tones"));
}

fn hold_a_catalog_result(browser: &mut BrowserView) {
    let generation = browser.store_mut().begin_query();
    assert!(browser.store_mut().complete_query(
        generation,
        ronie_core::Page {
            data: Vec::new(),
            page: 1,
            page_size: 20,
            total: 0,
            total_pages: 0,
        },
        true,
    ));
}

#[test]
fn every_surface_follows_the_session_on_a_plain_thread() {
    every_surface_follows_sign_in_and_sign_out();
}

#[tokio::test]
async fn every_surface_follows_the_session_inside_a_test_runtime() {
    every_surface_follows_sign_in_and_sign_out();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_starts_a_device_flow_and_drops_inside_a_test_runtime() {
    let mut driver = driver(Box::new(AuthService::new(flow(vec![
        device_response(),
        token_response(),
    ]))));
    driver
        .chrome_mut()
        .start_device_sign_in()
        .expect("start device flow");
    frames_until(&mut driver, |chrome, _| chrome.session().signed_in());
}

#[test]
fn browser_sign_in_opens_the_page_and_persists_a_session_for_the_next_launch() {
    let directory = tempfile::tempdir().expect("settings root");
    let settings = || SettingsStore::standalone(directory.path(), UI_PREFERENCES_SETTINGS);
    let mut signed_out = driver(Box::new(AuthService::with_session(
        flow(vec![token_response()]),
        settings(),
    )));
    assert!(!signed_out.chrome().session().signed_in());

    signed_out.chrome_mut().sign_in().expect("start sign-in");
    let opened = frames_until(&mut signed_out, |_, opened| !opened.is_empty());
    assert!(opened[0].starts_with("https://api.example/api/v1/oauth/authorize?"));

    browser_returns_to(&opened[0]);
    frames_until(&mut signed_out, |chrome, _| chrome.session().signed_in());

    let mut next_launch = chrome(Box::new(AuthService::with_session(
        flow(Vec::new()),
        settings(),
    )));
    assert!(
        next_launch.session().signed_in(),
        "the session must survive a restart"
    );

    next_launch.sign_out().expect("sign out");
    assert!(!next_launch.session().signed_in());
    let after_sign_out = chrome(Box::new(AuthService::with_session(
        flow(Vec::new()),
        settings(),
    )));
    assert!(!after_sign_out.session().signed_in());
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

#[test]
fn device_sign_in_runs_on_a_thread_without_a_runtime_context() {
    let pending = HttpResponse::json(400, serde_json::json!({"error": "authorization_pending"}));
    let mut driver = driver(Box::new(AuthService::new(flow(vec![
        device_response(),
        pending,
        token_response(),
    ]))));

    driver
        .chrome_mut()
        .start_device_sign_in()
        .expect("start device flow");
    let opened = frames_until(&mut driver, |chrome, _| chrome.session().signed_in());
    assert_eq!(opened, ["https://api.example/device"]);
}

#[test]
fn an_expired_device_code_keeps_the_page_and_a_new_code_finishes_the_sign_in() {
    let expired = HttpResponse::json(400, serde_json::json!({"error": "expired_token"}));
    let mut driver = driver(Box::new(AuthService::new(flow(vec![
        device_response(),
        expired,
        device_response(),
        token_response(),
    ]))));
    driver
        .chrome_mut()
        .start_device_sign_in()
        .expect("start device flow");

    let started = Instant::now();
    while !driver.is_visible("New code") {
        driver.frame();
        assert!(started.elapsed() < Duration::from_secs(10), "no failure");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(driver.chrome().auth_active(), "the failure keeps the page");
    assert!(!driver.chrome().session().signed_in());

    let _ = driver.click("New code").expect("new code");
    frames_until(&mut driver, |chrome, _| chrome.session().signed_in());
    assert!(!driver.chrome().auth_active(), "a finished flow leaves");
}

#[test]
fn failed_device_polling_offers_a_new_code() {
    let mut auth = AuthService::new(flow(vec![
        HttpResponse::json(
            200,
            serde_json::json!({
                "device_code": "device-secret",
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://api.example/device",
            }),
        ),
        HttpResponse::json(400, serde_json::json!({"error": "expired_token"})),
    ]));
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    auth.start_device(runtime.handle());
    let started = Instant::now();
    while auth.snapshot().phase != AuthPhase::DeviceFailed {
        let _ = auth.poll();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no failure seen"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        auth.copy_link().as_deref(),
        Some("https://api.example/device"),
        "the failure page offers a link to copy"
    );
}

#[test]
fn failed_device_request_stays_on_the_device_flow() {
    let mut auth = AuthService::new(flow(vec![HttpResponse::json(
        500,
        serde_json::json!({"error": "server_error"}),
    )]));
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    auth.start_device(runtime.handle());
    let started = Instant::now();
    while auth.poll().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no failure seen"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(auth.snapshot().phase, AuthPhase::DeviceFailed);
}

/// This test waits for one real retry of about two seconds, because it checks the wiring from
/// the service to the poll loop. The unit tests of the loop have no real waits.
#[test]
fn a_timeout_during_device_polling_does_not_end_the_sign_in() {
    let flow = OAuthFlow::new(
        OAuthConfig::new(
            Url::parse("https://api.example").expect("origin"),
            "public-client",
        ),
        RecordedTransport::new(vec![
            Ok(device_response()),
            Err(HttpError::ConnectTimeout),
            Ok(token_response()),
        ]),
    );
    let mut driver = driver(Box::new(AuthService::new(flow)));
    driver
        .chrome_mut()
        .start_device_sign_in()
        .expect("start device flow");

    frames_until(&mut driver, |chrome, _| chrome.session().signed_in());
}

#[test]
fn a_device_page_that_is_not_a_web_page_does_not_open_and_shows_the_problem() {
    for page in [
        serde_json::json!({"verification_uri": "file:///etc/passwd"}),
        serde_json::json!({
            "verification_uri": "https://api.example/device",
            "verification_uri_complete": "javascript:alert(1)",
        }),
    ] {
        let mut device = serde_json::json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "interval": 1,
        });
        for (key, value) in page.as_object().expect("page fields") {
            device[key] = value.clone();
        }
        let mut driver = driver(Box::new(AuthService::new(flow(vec![
            HttpResponse::json(200, device),
            token_response(),
        ]))));
        driver
            .chrome_mut()
            .start_device_sign_in()
            .expect("start device flow");

        let mut opened = Vec::new();
        let started = Instant::now();
        while !driver.is_visible("New code") {
            driver.frame();
            opened.extend(driver.interaction().opened_urls);
            assert!(started.elapsed() < Duration::from_secs(10), "no failure");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(opened, Vec::<String>::new(), "{page}");
        assert!(driver.chrome().auth_active(), "the page shows the problem");
        assert!(!driver.chrome().session().signed_in());
        assert!(!driver.is_visible("Copy Link"), "no link stays to copy");
    }
}

/// A flow that only reports the page of a system browser.
struct ReadyFlow(Option<FlowEvent>);

impl AuthFlowAdapter for ReadyFlow {
    fn snapshot(&self) -> AuthSnapshot {
        AuthSnapshot {
            phase: AuthPhase::BrowserWaiting,
            copied: false,
            user_code: None,
            verification_uri: None,
            browser_problem: None,
            pending_intent: "browse".to_owned(),
            error: None,
        }
    }

    fn start_browser(&mut self, _runtime: &tokio::runtime::Handle) {}

    fn start_device(&mut self, _runtime: &tokio::runtime::Handle) {}

    fn poll(&mut self) -> Option<FlowEvent> {
        self.0.take()
    }

    fn copy_link(&mut self) -> Option<String> {
        None
    }

    fn sign_out(&mut self) -> Result<(), OAuthError> {
        Ok(())
    }

    fn close(&mut self) {}
}

#[test]
fn the_session_opens_only_a_web_page_from_any_flow() {
    let opened_for = |url: &str| {
        let ready = FlowEvent::BrowserReady(url.to_owned());
        let mut driver = driver(Box::new(ReadyFlow(Some(ready))));
        driver.chrome_mut().sign_in().expect("start sign-in");
        driver.frame();
        driver.interaction().opened_urls
    };
    assert_eq!(
        opened_for("https://api.example/a"),
        ["https://api.example/a"]
    );
    assert_eq!(opened_for("file:///etc/passwd"), Vec::<String>::new());
}

#[test]
fn a_closed_browser_drops_the_results_of_the_previous_session() {
    let mut chrome = ChromeController::new(Arc::new(main_mono_backend()), Vec::new(), true, true);
    let open = |chrome: &mut ChromeController| {
        let target = BlockId("left-insert-new".to_owned());
        chrome.gallery_mut().select_tone(ChainSide::Left, target);
        chrome.sync_browser();
    };
    open(&mut chrome);
    hold_a_catalog_result(chrome.browser_mut().expect("browser"));

    chrome.go_to_main();
    assert!(chrome.browser().is_none());
    open(&mut chrome);
    let browser = chrome.browser().expect("browser");
    assert!(
        browser.store().result().is_some(),
        "the same session keeps the results of a closed browser"
    );

    chrome.go_to_main();
    chrome.sign_out().expect("sign out");
    assert!(!chrome.session().signed_in());
    open(&mut chrome);
    let browser = chrome.browser().expect("browser");
    assert!(
        browser.store().result().is_none(),
        "the results of the old session must not stay"
    );
}

#[test]
fn a_rejected_refresh_returns_the_real_browser_to_the_signed_out_preview() {
    let directory = tempfile::tempdir().expect("settings root");
    let settings = SettingsStore::standalone(directory.path(), ronie_core::UI_PREFERENCES_SETTINGS);
    ronie_core::SessionStore::new(settings.clone(), flow(Vec::new()))
        .set_tokens(ronie_core::OAuthTokens {
            access: ronie_core::SecretString::from("dead-access"),
            refresh: ronie_core::SecretString::from("dead-refresh"),
            expires_at_ms: 0,
        })
        .expect("seed");
    let transport = RecordedTransport::new(vec![
        Ok(HttpResponse::json(
            400,
            serde_json::json!({"error": "invalid_grant"}),
        )),
        Ok(HttpResponse::json(
            200,
            serde_json::json!({
                "data": [{"id": 1, "title": "Public", "format": "nam", "models": []}],
                "page": 1,
                "total_pages": 1,
            }),
        )),
    ]);
    let origin = Url::parse("https://api.example").expect("origin");
    let community = ronie_ui::services::community::CommunityService::new(
        OAuthConfig::new(origin.clone(), "public-client"),
        ronie_core::OriginPolicy::new(origin, []),
        transport.clone(),
        settings,
    );
    let backend = main_mono_backend();
    let chrome = ChromeController::with_community(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        community,
    );
    assert!(
        chrome.session().signed_in(),
        "the stored tokens look usable"
    );
    let mut driver = UiDriver::new(chrome, backend);
    driver
        .chrome_mut()
        .gallery_mut()
        .select_tone(ChainSide::Left, BlockId("left-insert-new".to_owned()));

    let started = Instant::now();
    while driver.chrome().session().signed_in() || !driver.is_visible("Public") {
        assert!(started.elapsed() < Duration::from_secs(10), "no preview");
        driver.frame();
    }

    assert!(driver.is_visible(TONE_PROMPT));
    assert!(!driver.is_visible("Search tones"));
    assert!(
        driver
            .chrome()
            .browser()
            .expect("browser")
            .store()
            .result_is_trending()
    );
}

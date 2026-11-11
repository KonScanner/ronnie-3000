//! A build with no community service: nothing invites a sign-in, and local tones stay available.

use std::sync::Arc;

use eframe::egui;
use ronie_core::{OAuthError, Page, Tone};
use ronie_ui::{
    backend::{MockBackend, UiBackend},
    default::{AuthPhase, CatalogStatus, ChromeController, popover},
    driver::UiDriver,
    native_scenario::reach_scenario,
    services::load_flow::LoadPhase,
    services::session::{AuthFlowAdapter, AuthSnapshot, FlowEvent},
    testbed::{UiScenario, drive_scenario, load_scenarios},
};
use tokio::runtime::Handle;

const TONE_PROMPT: &str = "Sign in or create free account";
/// The row height of the account menu.
const ACCOUNT_ROW: f32 = 38.0;

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario")
}

/// A flow that never finishes, so that a test sees only what a service makes visible.
struct QuietFlow;

impl AuthFlowAdapter for QuietFlow {
    fn snapshot(&self) -> AuthSnapshot {
        AuthSnapshot {
            phase: AuthPhase::Idle,
            copied: false,
            user_code: None,
            verification_uri: None,
            browser_problem: None,
            pending_intent: "browse".to_owned(),
            error: None,
        }
    }

    fn start_browser(&mut self, _runtime: &Handle) {}

    fn start_device(&mut self, _runtime: &Handle) {}

    fn poll(&mut self) -> Option<FlowEvent> {
        None
    }

    fn copy_link(&mut self) -> Option<String> {
        None
    }

    fn sign_out(&mut self) -> Result<(), OAuthError> {
        Ok(())
    }

    fn close(&mut self) {}
}

fn no_service(id: &str) -> UiDriver {
    let backend = MockBackend::from_scenario(&scenario(id));
    let chrome = ChromeController::new(Arc::new(backend.clone()), Vec::new(), false, true);
    UiDriver::new(chrome, backend)
}

fn with_service(id: &str) -> UiDriver {
    let backend = MockBackend::from_scenario(&scenario(id));
    let chrome = ChromeController::with_auth_flow(
        Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
        Box::new(QuietFlow),
    );
    UiDriver::new(chrome, backend)
}

/// No control on the screen asks the user to sign in.
fn assert_no_invitation(driver: &UiDriver) {
    let labels = driver.visible_labels();
    for label in &labels {
        let text = label.to_lowercase();
        assert!(
            !text.contains("sign in") && !text.contains("log in") && !text.contains("login"),
            "{label:?} in {labels:?}"
        );
    }
}

fn account_panel(driver: &UiDriver) -> egui::Rect {
    let panels = popover::open_rects(driver.context());
    assert_eq!(panels.len(), 1, "the account menu is the one open panel");
    panels[0]
}

fn on_the_browser(mut driver: UiDriver) -> UiDriver {
    let _ = driver.click_first("Add Tone").expect("open the browser");
    driver.settle();
    assert!(driver.chrome().browser().is_some());
    driver
}

fn on_the_info_tab(mut driver: UiDriver) -> UiDriver {
    driver.chrome_mut().open_detail("blk-1").expect("detail");
    driver.settle();
    let _ = driver.click("Tone info").expect("info tab");
    driver.settle();
    driver
}

/// Puts one card on the page. A build with no service cannot get a card in another way.
fn with_a_card(mut driver: UiDriver) -> UiDriver {
    let backend = MockBackend::from_scenario(&scenario("main-detail"));
    let mut tone: Tone = backend.chain_state(0).chain.left[0]
        .tone
        .clone()
        .expect("tone");
    tone.title = "Card Tone".to_owned();
    let page = Page {
        data: vec![tone],
        page: 1,
        page_size: 12,
        total: 1,
        total_pages: 1,
    };
    let store = driver
        .chrome_mut()
        .browser_mut()
        .expect("browser")
        .store_mut();
    let generation = store.begin_query();
    assert!(store.complete_query(generation, page, false));
    driver.settle();
    driver
}

#[test]
fn the_account_menu_has_only_settings_and_a_shorter_panel_with_no_service() {
    let mut driver = with_service("main-mono");
    let _ = driver.click("Account").expect("account button");
    assert!(driver.is_visible("Sign in"));
    let with_a_row = account_panel(&driver);

    let mut driver = no_service("main-mono");
    let _ = driver.click("Account").expect("account button");
    assert!(driver.is_visible("Settings"));
    assert!(!driver.is_visible("Logout"));
    assert_no_invitation(&driver);
    let panel = account_panel(&driver);
    assert_eq!(panel.height(), with_a_row.height() - ACCOUNT_ROW);
    assert_eq!(panel.width(), with_a_row.width());
    assert_eq!(panel.min, with_a_row.min, "the panel stays in its place");
}

#[test]
fn the_tone_page_has_no_prompt_with_no_service() {
    let driver = on_the_browser(with_service("main-empty"));
    assert!(driver.is_visible(TONE_PROMPT));

    let driver = on_the_browser(no_service("main-empty"));
    assert!(driver.is_visible("Close tone browser"));
    assert_no_invitation(&driver);
}

#[test]
fn the_info_tab_has_no_prompt_with_no_service() {
    let driver = on_the_info_tab(with_service("main-detail"));
    assert!(driver.is_visible("Log In"));

    let driver = on_the_info_tab(no_service("main-detail"));
    assert!(driver.is_visible("Tone info"));
    assert_no_invitation(&driver);
}

#[test]
fn a_tone_card_loads_its_tone_with_no_service() {
    let mut driver = with_a_card(on_the_browser(with_service("main-empty")));
    let _ = driver.click("Card Tone").expect("card");
    driver.settle();
    let browser = driver.chrome().browser().expect("the browser stays");
    assert!(browser.sign_in_page(), "a service asks for a sign-in");

    let backend = MockBackend::from_scenario(&scenario("main-empty"));
    let chrome = ChromeController::new(Arc::new(backend.clone()), Vec::new(), false, true);
    let mut driver = with_a_card(on_the_browser(UiDriver::new(chrome, backend.clone())));
    let _ = driver.click("Card Tone").expect("card");
    driver.settle();

    assert!(driver.chrome().browser().is_none(), "the browser closes");
    assert_eq!(driver.chrome().load_flow_phase(), LoadPhase::Ready);
    assert!(
        backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .any(|block| block.tone.as_ref().is_some_and(|t| t.title == "Card Tone")),
        "the chain holds the tone"
    );
    assert_no_invitation(&driver);
}

#[test]
fn a_gear_chip_starts_no_query_with_no_service() {
    let mut driver = on_the_browser(no_service("main-empty"));

    let _ = driver.click("Pedal").expect("gear chip");
    for _ in 0..4 {
        driver.settle();
        let store = driver.chrome().browser().expect("browser").store();
        assert_eq!(store.status(), CatalogStatus::Idle, "no worker can answer");
        assert_eq!(store.query().gear, "pedal", "the chip keeps its state");
    }
    assert_no_invitation(&driver);

    let _ = driver.click("Pedal").expect("gear chip");
    driver.settle();
    let store = driver.chrome().browser().expect("browser").store();
    assert_eq!(store.status(), CatalogStatus::Idle);
    assert_eq!(store.query().gear, "");
}

#[test]
fn a_sign_in_call_with_no_service_names_no_environment_variable() {
    let mut driver = no_service("main-mono");

    let error = driver.chrome_mut().sign_in().expect_err("no flow");
    assert!(!error.to_string().contains("RONIE_"), "{error}");
    let start = driver
        .chrome_mut()
        .start_device_sign_in()
        .expect_err("no flow");
    assert!(!start.to_string().contains("RONIE_"), "{start}");
    assert!(
        !driver
            .chrome()
            .toast()
            .is_some_and(|toast| toast.contains("RONIE_"))
    );
}

#[test]
fn a_local_file_stays_available_with_no_service() {
    let mut driver = no_service("main-mono");
    let _ = driver
        .secondary_click("'02 Vox AC30/6 Top Boost")
        .expect("tile");
    driver.settle();
    assert!(driver.is_visible("Load File"));
    assert!(driver.is_visible("Load Folder"));
}

#[test]
fn the_scenarios_of_a_prompt_keep_the_prompt_through_a_flow() {
    for id in [
        "browser-signed-out",
        "browser-signed-out-gear",
        "browser-signed-out-empty",
        "browser-signed-out-footer",
    ] {
        let driver = reach_scenario(&scenario(id)).expect("scenario");
        assert!(driver.is_visible(TONE_PROMPT), "{id}");
    }
    let info = reach_scenario(&scenario("main-detail-info-signed-out")).expect("scenario");
    assert!(info.is_visible("Log In"));

    let account = scenario("chrome-account-signed-out");
    let backend = MockBackend::from_scenario(&account);
    let driven = drive_scenario(&account, backend)
        .expect("drive")
        .expect("driver");
    assert!(
        driven.is_visible("Sign in"),
        "{:?}",
        driven.visible_labels()
    );
}

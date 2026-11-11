//! A build with no key: the sign-in controls open the key page, a saved key starts the
//! service with no restart, and a removed key brings the page back. These tests use recorded
//! responses and keep the key in memory, so no test contacts a service or writes a key.

use std::sync::{Arc, Mutex};

use ronie_core::{
    OAuthConfig, OriginPolicy, RecordedTransport, SettingsStore, UI_PREFERENCES_SETTINGS,
};
use ronie_ui::{
    backend::MockBackend,
    default::{ChromeController, SessionStatus},
    driver::UiDriver,
    services::{
        community::CommunityService,
        key_setup::{CommunityParts, KeySetup, KeySetupError},
    },
    testbed::load_scenarios,
};
use tempfile::TempDir;
use url::Url;

const GOOD_KEY: &str = "t3k_pub_test_fake";
const NOT_A_KEY: &str = "not-a-key-value";
const ORIGIN: &str = "https://www.fixtures.invalid";

/// What a test reads back from its key setup.
#[derive(Default)]
struct Saved {
    key: Option<String>,
    saves: usize,
}

/// A key setup that keeps the key in memory and starts a service over recorded responses.
struct MemoryKeySetup {
    saved: Arc<Mutex<Saved>>,
    directory: Arc<TempDir>,
}

impl MemoryKeySetup {
    fn service(&self) -> CommunityParts {
        let origin = Url::parse("https://api.example").expect("origin");
        let key = self.saved.lock().expect("saved").key.clone();
        CommunityService::new(
            OAuthConfig::new(origin.clone(), key.unwrap_or_default()),
            OriginPolicy::new(origin, []),
            RecordedTransport::new(Vec::new()),
            SettingsStore::standalone(self.directory.path(), UI_PREFERENCES_SETTINGS),
        )
        .into()
    }
}

impl KeySetup for MemoryKeySetup {
    fn origin(&self) -> String {
        ORIGIN.to_owned()
    }

    fn has_saved_key(&self) -> bool {
        self.saved.lock().expect("saved").key.is_some()
    }

    fn save(&mut self, key: &str) -> Result<Option<CommunityParts>, KeySetupError> {
        {
            let mut saved = self.saved.lock().expect("saved");
            saved.key = Some(key.to_owned());
            saved.saves += 1;
        }
        Ok(Some(self.service()))
    }

    fn remove(&mut self) -> Result<Option<CommunityParts>, KeySetupError> {
        self.saved.lock().expect("saved").key = None;
        Ok(None)
    }
}

fn backend() -> MockBackend {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-mono")
        .expect("scenario");
    MockBackend::from_scenario(&scenario)
}

/// A standalone controller with no service and a key setup, and the state of that setup.
fn no_key() -> (UiDriver, Arc<Mutex<Saved>>) {
    let saved = Arc::new(Mutex::new(Saved::default()));
    let backend = backend();
    let mut chrome = ChromeController::new(Arc::new(backend.clone()), Vec::new(), false, true);
    chrome.set_standalone(true);
    chrome.use_key_setup(Box::new(MemoryKeySetup {
        saved: saved.clone(),
        directory: Arc::new(tempfile::tempdir().expect("temporary settings")),
    }));
    (UiDriver::new(chrome, backend), saved)
}

fn open_key_page(driver: &mut UiDriver) {
    let _ = driver.click("Account").expect("account button");
    let _ = driver.click("Sign in").expect("sign-in row");
    assert!(driver.chrome().key_page().is_some(), "the key page opens");
}

fn assert_key_page(driver: &UiDriver) {
    for label in [
        "Back",
        "Open TONE3000",
        "TONE3000 key",
        "Save key",
        "Not now",
    ] {
        assert!(
            driver.is_visible(label),
            "{label:?} in {:?}",
            driver.visible_labels()
        );
    }
    assert!(driver.chrome().auth_active(), "the page replaces the chain");
}

fn assert_no_label_holds(driver: &UiDriver, text: &str) {
    for label in driver.visible_labels() {
        assert!(!label.contains(text), "{label:?}");
    }
    assert!(!driver.shows_text(text));
}

#[test]
fn with_no_key_the_account_menu_offers_a_sign_in_that_opens_the_key_page() {
    let (mut driver, saved) = no_key();
    assert_eq!(driver.chrome().session(), SessionStatus::NoKey);

    open_key_page(&mut driver);

    assert_key_page(&driver);
    assert_eq!(saved.lock().expect("saved").saves, 0);
}

#[test]
fn with_no_key_the_tone_browser_prompt_opens_the_key_page() {
    let (mut driver, _) = no_key();
    let _ = driver.click_first("Add Tone").expect("open the browser");
    driver.settle();

    let _ = driver
        .click("Sign in or create free account")
        .expect("browser prompt");

    assert!(driver.chrome().key_page().is_some());
    assert_key_page(&driver);
}

#[test]
fn a_value_that_is_not_a_key_shows_an_error_and_saves_nothing() {
    let (mut driver, saved) = no_key();
    open_key_page(&mut driver);

    let _ = driver.fill("TONE3000 key", NOT_A_KEY).expect("key field");
    let _ = driver.click("Save key").expect("save");

    let page = driver.chrome().key_page().expect("the page stays");
    let error = page.error().expect("inline error");
    assert!(error.contains("t3k_pub_"), "{error}");
    assert!(
        !error.contains(NOT_A_KEY),
        "the error never holds the value"
    );
    assert_eq!(saved.lock().expect("saved").saves, 0, "nothing is saved");
    assert_eq!(driver.chrome().session(), SessionStatus::NoKey);
    assert_no_label_holds(&driver, NOT_A_KEY);
}

#[test]
fn a_key_saves_and_the_sign_in_page_follows_with_no_restart() {
    let (mut driver, saved) = no_key();
    open_key_page(&mut driver);

    // A pasted key can carry spaces at its ends.
    let _ = driver
        .fill("TONE3000 key", &format!(" {GOOD_KEY} "))
        .expect("key field");
    assert_no_label_holds(&driver, GOOD_KEY);
    let _ = driver.click("Save key").expect("save");

    assert_eq!(saved.lock().expect("saved").key.as_deref(), Some(GOOD_KEY));
    let chrome = driver.chrome();
    assert!(chrome.key_page().is_none(), "the key page closes");
    assert!(chrome.key_saved());
    assert!(chrome.community().is_some(), "the service started");
    assert_eq!(chrome.session(), SessionStatus::SignedOut);
    assert!(
        chrome.auth_active(),
        "the sign-in page replaces the key page"
    );
    assert!(
        driver.is_visible("Sign in"),
        "{:?}",
        driver.visible_labels()
    );
    assert_no_label_holds(&driver, GOOD_KEY);
}

#[test]
fn enter_in_the_key_field_saves_the_key() {
    let (mut driver, saved) = no_key();
    open_key_page(&mut driver);
    let _ = driver.fill("TONE3000 key", GOOD_KEY).expect("key field");

    let _ = driver.press_key(eframe::egui::Key::Enter);

    assert_eq!(saved.lock().expect("saved").key.as_deref(), Some(GOOD_KEY));
    assert!(driver.chrome().key_page().is_none());
}

#[test]
fn open_tone3000_opens_the_origin_in_the_system_browser() {
    let (mut driver, _) = no_key();
    open_key_page(&mut driver);

    let opened = driver.click("Open TONE3000").expect("open").opened_urls;

    assert_eq!(opened, vec![ORIGIN.to_owned()]);
    assert!(driver.chrome().key_page().is_some(), "the page stays open");
}

#[test]
fn not_now_and_back_leave_the_page_and_save_nothing() {
    for control in ["Not now", "Back"] {
        let (mut driver, saved) = no_key();
        open_key_page(&mut driver);
        let _ = driver.fill("TONE3000 key", GOOD_KEY).expect("key field");

        let _ = driver.click(control).expect("leave");

        assert!(driver.chrome().key_page().is_none(), "{control}");
        assert!(!driver.chrome().auth_active(), "{control}");
        assert_eq!(saved.lock().expect("saved").saves, 0, "{control}");
        open_key_page(&mut driver);
        assert!(
            driver
                .chrome()
                .key_page()
                .is_some_and(|page| page.error().is_none()),
            "{control}: a new page starts empty"
        );
    }
}

#[test]
fn the_settings_page_shows_a_saved_key_and_remove_brings_the_key_page_back() {
    let (mut driver, saved) = no_key();
    driver.chrome_mut().open_system_settings();
    driver.settle();
    assert!(!driver.is_visible("Remove key"), "no key, so no section");
    driver
        .chrome_mut()
        .settings_mut()
        .expect("settings")
        .close();
    driver.settle();

    open_key_page(&mut driver);
    let _ = driver.fill("TONE3000 key", GOOD_KEY).expect("key field");
    let _ = driver.click("Save key").expect("save");
    driver.chrome_mut().go_to_main();
    driver.chrome_mut().open_system_settings();
    driver
        .chrome_mut()
        .settings_mut()
        .expect("settings")
        .scroll_to_heading("Community");
    driver.settle();
    assert!(
        driver.is_visible("Remove key"),
        "{:?}",
        driver.visible_labels()
    );
    assert_no_label_holds(&driver, GOOD_KEY);

    let _ = driver.click("Remove key").expect("remove");

    assert_eq!(saved.lock().expect("saved").key, None);
    assert!(!driver.is_visible("Remove key"));
    let chrome = driver.chrome_mut();
    assert!(!chrome.key_saved());
    assert!(chrome.community().is_none(), "the service stopped");
    assert_eq!(chrome.session(), SessionStatus::NoKey);
    chrome.settings_mut().expect("settings").close();
    driver.settle();
    open_key_page(&mut driver);
    assert_key_page(&driver);
}

#[test]
fn a_removed_key_signs_out_first() {
    let (mut driver, _) = no_key();
    open_key_page(&mut driver);
    let _ = driver.fill("TONE3000 key", GOOD_KEY).expect("key field");
    let _ = driver.click("Save key").expect("save");

    driver.chrome_mut().remove_key().expect("remove");

    let chrome = driver.chrome();
    assert!(!chrome.session().signed_in());
    assert!(!chrome.auth_active(), "no sign-in page stays");
    assert_eq!(chrome.session(), SessionStatus::NoKey);
}

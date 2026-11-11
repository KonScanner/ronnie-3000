//! The daily release check in the chrome and its switch in the system settings.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use ronie_core::SettingsStore;
use ronie_ui::{
    backend::{MockBackend, UiAction, UiBackend},
    default::{ChromeController, system_settings::UPDATE_CHECK_LABEL},
    driver::UiDriver,
    services::{
        ReleaseAnswer, ReleaseFeed,
        release_check::{CHECK_PREFERENCE, RELEASE_PAGE_PREFIX},
    },
    testbed::{UiScenario, load_scenarios},
};
use serde_json::{Value, json};

const NOW_MS: i64 = 1_800_000_000_000;
const HOUR_MS: i64 = 60 * 60 * 1000;

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn backend() -> Arc<MockBackend> {
    Arc::new(MockBackend::from_scenario(&scenario("settings-system")))
}

fn payload(version: &str) -> Value {
    json!({
        "version": version,
        "message_html": format!("Ronnie 3000 {version} is available."),
        "url": format!("{RELEASE_PAGE_PREFIX}tag/v{version}"),
    })
}

/// A feed that counts its calls and gives one fixed answer.
struct FakeFeed {
    calls: AtomicUsize,
    answer: ReleaseAnswer,
}

impl FakeFeed {
    fn new(answer: ReleaseAnswer) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            answer,
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ReleaseFeed for FakeFeed {
    fn fetch(&self) -> ReleaseAnswer {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answer.clone()
    }
}

fn chrome_with(
    backend: &Arc<MockBackend>,
    feed: Arc<FakeFeed>,
    settings: Option<SettingsStore>,
) -> ChromeController {
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    if let Some(settings) = &settings {
        chrome.use_settings(settings.clone());
    }
    chrome.use_release_feed(feed, settings);
    chrome
}

/// Polls until the feed answered and the chrome took the answer.
fn poll_until_answered(chrome: &mut ChromeController, feed: &FakeFeed, now_ms: i64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        chrome.poll_updates(now_ms);
        if feed.calls() > 0 {
            // The thread can still hold the answer, so take it with a few more polls.
            for _ in 0..50 {
                chrome.poll_updates(now_ms);
                std::thread::sleep(Duration::from_millis(2));
            }
            return;
        }
        assert!(Instant::now() < deadline, "no request");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn set_checks(backend: &MockBackend, enabled: bool) {
    backend
        .dispatch(UiAction::SetPreference {
            key: CHECK_PREFERENCE.to_owned(),
            value: enabled,
        })
        .expect("preference");
}

fn settings_with_state(state: &Value) -> (tempfile::TempDir, SettingsStore) {
    let directory = tempfile::tempdir().expect("settings root");
    let settings = SettingsStore::standalone(directory.path(), "ui-preferences.settings");
    settings
        .merge_key("t3k.updateCheck", &state.to_string())
        .expect("stored state");
    (directory, settings)
}

#[test]
fn an_unset_preference_checks_and_shows_a_newer_release() {
    let backend = backend();
    assert_eq!(backend.preference(CHECK_PREFERENCE), None);
    let feed = FakeFeed::new(ReleaseAnswer::Newer(payload("9.0.0")));
    let mut chrome = chrome_with(&backend, feed.clone(), None);
    assert!(chrome.update_checks_enabled());

    poll_until_answered(&mut chrome, &feed, NOW_MS);

    assert_eq!(feed.calls(), 1);
    let notice = chrome.updates().notice().expect("notice");
    assert_eq!(notice.version, "9.0.0");
    assert!(notice.url.starts_with(RELEASE_PAGE_PREFIX));
}

#[test]
fn the_setting_off_sends_no_request_and_shows_no_stored_notice() {
    let backend = backend();
    set_checks(&backend, false);
    let (_directory, settings) = settings_with_state(&json!({
        "lastCheckMs": NOW_MS - 3 * 24 * HOUR_MS,
        "payload": payload("9.0.0"),
    }));
    let feed = FakeFeed::new(ReleaseAnswer::Newer(payload("9.0.0")));
    let mut chrome = chrome_with(&backend, feed.clone(), Some(settings));

    for hour in 0..72 {
        chrome.poll_updates(NOW_MS + hour * HOUR_MS);
    }
    std::thread::sleep(Duration::from_millis(30));
    chrome.poll_updates(NOW_MS + 72 * HOUR_MS);

    assert_eq!(feed.calls(), 0);
    assert!(chrome.updates().notice().is_none());
}

#[test]
fn a_stored_payload_shows_at_start_with_no_request() {
    let backend = backend();
    let (_directory, settings) = settings_with_state(&json!({
        "lastCheckMs": NOW_MS - HOUR_MS,
        "payload": payload("9.0.0"),
    }));
    let feed = FakeFeed::new(ReleaseAnswer::Current);
    let mut chrome = chrome_with(&backend, feed.clone(), Some(settings));

    chrome.poll_updates(NOW_MS);
    std::thread::sleep(Duration::from_millis(30));
    chrome.poll_updates(NOW_MS);

    assert_eq!(feed.calls(), 0);
    assert_eq!(
        chrome
            .updates()
            .notice()
            .map(|notice| notice.version.as_str()),
        Some("9.0.0")
    );
}

#[test]
fn a_snoozed_stored_payload_stays_hidden_after_a_restart() {
    let backend = backend();
    let (_directory, settings) = settings_with_state(&json!({
        "lastCheckMs": NOW_MS - HOUR_MS,
        "payload": payload("9.0.0"),
    }));
    let feed = FakeFeed::new(ReleaseAnswer::Current);
    let mut chrome = chrome_with(&backend, feed.clone(), Some(settings.clone()));
    chrome.poll_updates(NOW_MS);
    let _ = chrome
        .handle_update_action(ronie_ui::default::UpdateAction::Remind(7), NOW_MS)
        .expect("snooze");
    assert!(chrome.updates().notice().is_none());

    let mut restarted = chrome_with(&backend, feed.clone(), Some(settings));
    restarted.poll_updates(NOW_MS + HOUR_MS);

    assert!(restarted.updates().notice().is_none());
    assert!(restarted.updates().update().is_some());
    assert_eq!(feed.calls(), 0);
}

#[test]
fn a_failed_check_shows_nothing_and_does_not_panic() {
    let backend = backend();
    let feed = FakeFeed::new(ReleaseAnswer::Failed("offline".to_owned()));
    let mut chrome = chrome_with(&backend, feed.clone(), None);

    poll_until_answered(&mut chrome, &feed, NOW_MS);

    assert_eq!(feed.calls(), 1);
    assert!(chrome.updates().notice().is_none());
    assert!(chrome.visible_toast().is_none());
}

#[test]
fn turning_the_setting_off_hides_the_notice_and_on_shows_it_again() {
    let backend = backend();
    let (_directory, settings) = settings_with_state(&json!({
        "lastCheckMs": NOW_MS - HOUR_MS,
        "payload": payload("9.0.0"),
    }));
    let feed = FakeFeed::new(ReleaseAnswer::Current);
    let mut chrome = chrome_with(&backend, feed.clone(), Some(settings));
    chrome.poll_updates(NOW_MS);
    assert!(chrome.updates().notice().is_some());

    set_checks(&backend, false);
    chrome.poll_updates(NOW_MS);
    assert!(chrome.updates().notice().is_none());

    set_checks(&backend, true);
    chrome.poll_updates(NOW_MS);
    assert!(chrome.updates().notice().is_some());
    assert_eq!(feed.calls(), 0);
}

#[test]
fn a_chrome_with_no_feed_never_checks() {
    let backend = backend();
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);

    chrome.poll_updates(NOW_MS);

    assert!(chrome.updates().notice().is_none());
}

#[test]
fn the_system_settings_switch_turns_the_check_off() {
    let backend = backend();
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.open_system_settings();
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    assert!(
        driver.is_visible(UPDATE_CHECK_LABEL),
        "{:?}",
        driver.visible_labels()
    );

    let _ = driver.click(UPDATE_CHECK_LABEL).expect("switch");
    driver.settle();

    assert_eq!(backend.preference(CHECK_PREFERENCE), Some(false));
    let _ = driver.click(UPDATE_CHECK_LABEL).expect("switch");
    driver.settle();
    assert_eq!(backend.preference(CHECK_PREFERENCE), Some(true));
}

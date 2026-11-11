//! Offline gate, retry backoff, update notices, and cached-user tests.

use ronie_ui::default::{ConnectionGate, ConnectionProblem, UpdateCheck};

#[test]
fn offline_gate_queues_action_and_retries_after_backoff() {
    let mut gate = ConnectionGate::new(false);
    assert!(!gate.require_connection("browse"));
    assert_eq!(gate.problem(), Some(ConnectionProblem::Offline));
    assert_eq!(gate.pending_action(), Some("browse"));
    assert!(!gate.retry(1_000));
    gate.set_online(true);
    assert!(!gate.retry(1_000));
    assert!(gate.retry(2_000));
    assert_eq!(gate.problem(), None);
    assert_eq!(gate.pending_action(), None);
}

#[test]
fn insecure_connection_requires_confirmation_and_recovers() {
    let mut gate = ConnectionGate::new(true);
    gate.record_probe(Some(false), 100);
    assert_eq!(gate.problem(), None);
    gate.record_probe(Some(false), 200);
    assert_eq!(gate.problem(), Some(ConnectionProblem::Insecure));
    gate.record_probe(Some(true), 300);
    assert_eq!(gate.problem(), None);
    gate.record_probe(None, 400);
    assert_eq!(gate.problem(), None);
}

#[test]
fn dismiss_clears_pending_action_and_problem() {
    let mut gate = ConnectionGate::new(false);
    let _ = gate.require_connection("browse");
    gate.dismiss();
    assert_eq!(gate.pending_action(), None);
    assert_eq!(gate.problem(), None);
}

#[test]
fn update_versions_snooze_and_daily_memory_work() {
    assert!(UpdateCheck::compare_versions("1.2.0", "1.1.9") > 0);
    assert_eq!(UpdateCheck::compare_versions("v2.0.0-beta", "2.0.0"), 0);
    let mut check = UpdateCheck::new("1.0.0");
    let payload = serde_json::json!({
        "version": "1.1.0",
        "message_html": "Bug fixes",
        "url": "https://api.example/update"
    });
    check.apply_payload(payload.clone(), 1_000);
    assert_eq!(check.notice().expect("notice").version, "1.1.0");
    check.remind_later(1, 2_000).expect("snooze");
    assert!(check.notice().is_none());
    assert_eq!(check.snooze_until(), 2_000 + 86_400_000);
    check.apply_payload(payload, 3_000);
    assert!(check.notice().is_none());
}

#[test]
fn invalid_update_payloads_and_old_versions_stay_silent() {
    let mut check = UpdateCheck::new("1.0.0");
    check.apply_payload(serde_json::json!({"version": "0.9.0", "message_html": "Old", "url": "https://api.example/update"}), 0);
    assert!(check.notice().is_none());
    check.apply_payload(
        serde_json::json!({"version": "2.0.0", "message_html": "Bad", "url": "file:///tmp/update"}),
        0,
    );
    assert!(check.notice().is_none());
}

fn chrome() -> ronie_ui::default::ChromeController {
    let scenario =
        ronie_ui::testbed::load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "main-empty")
            .expect("main-empty");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario));
    ronie_ui::default::ChromeController::new(backend, Vec::new(), false, true)
}

fn update_payload() -> serde_json::Value {
    serde_json::json!({
        "version": "9.1.0",
        "message_html": "Bug fixes",
        "url": "https://api.example/update"
    })
}

#[test]
fn chrome_retry_releases_the_queued_action_after_backoff() {
    let mut chrome = chrome();
    chrome.set_online(false);
    assert!(!chrome.require_connection("browse"));
    chrome.set_online(true);
    assert_eq!(chrome.retry_connection(1_000), None);
    assert_eq!(chrome.retry_connection(2_000).as_deref(), Some("browse"));
    assert_eq!(chrome.connection().problem(), None);
    chrome.set_online(false);
    assert!(!chrome.require_connection("browse"));
    chrome.handle_connection_action(ronie_ui::default::ConnectionAction::Dismiss, 3_000);
    assert_eq!(chrome.connection().pending_action(), None);
}

#[test]
fn chrome_update_download_stays_open_and_remind_later_snoozes() {
    let mut chrome = chrome();
    chrome.apply_update_payload(update_payload(), 1_000);
    let opened = chrome
        .handle_update_action(
            ronie_ui::default::UpdateAction::Download("https://api.example/update".to_owned()),
            2_000,
        )
        .expect("download");
    assert_eq!(opened.as_deref(), Some("https://api.example/update"));
    assert!(chrome.updates().notice().is_some());
    chrome
        .handle_update_action(ronie_ui::default::UpdateAction::Remind(1), 2_000)
        .expect("snooze");
    assert!(chrome.updates().notice().is_none());
    assert_eq!(chrome.updates().snooze_until(), 2_000 + 86_400_000);
}

#[test]
fn update_snooze_survives_a_new_checker_and_expires() {
    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let mut check = UpdateCheck::with_settings("1.0.0", settings.clone());
    check.remind_later(1, 1_000).expect("persist snooze");

    let properties = settings.read().expect("preferences");
    assert!(properties.get("t3k.updateNotice").is_some());
    assert!(properties.get("reference.updateNotice").is_none());
    let mut restored = UpdateCheck::with_settings("1.0.0", settings);
    restored.apply_payload(update_payload(), 2_000);
    assert!(restored.notice().is_none());
    assert_eq!(restored.update().expect("update").version, "9.1.0");
    restored.apply_payload(update_payload(), 1_000 + 86_400_000);
    assert!(restored.notice().is_some());
}

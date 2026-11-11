//! Default header, account, hints, overlays, and backend-effect tests.

use std::sync::Arc;

use ronie_ui::{
    backend::{MockBackend, UiBackend},
    default::{ChromeController, DismissReason, Overlay, overlays::OverlayController},
    testbed::{UiScenario, load_scenarios},
};

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn controller(id: &str) -> (ChromeController, Arc<MockBackend>) {
    let scenario = scenario(id);
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let controller = ChromeController::new(backend.clone(), Vec::new(), false, true);
    (controller, backend)
}

#[test]
fn header_chain_tuner_and_history_actions_reach_the_backend() {
    let (mut chrome, backend) = controller("chrome-account-signed-out");
    chrome.set_stereo(true).expect("stereo");
    chrome.set_tuner(true);
    chrome.cycle_input_mode().expect("input mode");
    chrome.undo().expect("undo");
    chrome.redo().expect("redo");
    let state = backend.chain_state(0);
    assert!(state.chain.stereo_enabled);
    assert!(state.can_undo);
    assert!(chrome.tuner_visible());
    assert!(state.revision >= 5);
}

#[test]
fn account_menu_rejects_unconfigured_sign_in_and_restores_focus() {
    let (mut chrome, _backend) = controller("chrome-account-signed-out");
    chrome.open_account();
    assert_eq!(chrome.overlay(), Overlay::Account);
    assert!(chrome.sign_in().is_err());
    assert!(!chrome.session().signed_in());
    chrome.open_account();
    chrome.dismiss(DismissReason::Outside);
    assert_eq!(chrome.overlay(), Overlay::None);
    assert_eq!(
        chrome.focus().focused().map(String::as_str),
        Some("account")
    );
    chrome.open_account();
    chrome.sign_out().expect("sign out");
    assert!(!chrome.session().signed_in());
    chrome.open_account();
    chrome.dismiss(DismissReason::Escape);
    assert_eq!(chrome.overlay(), Overlay::None);
}

#[test]
fn going_to_the_main_screen_closes_an_open_overlay_and_restores_its_anchor() {
    let (mut chrome, _) = controller("main-mono");
    chrome.open_account();
    assert_eq!(chrome.overlay(), Overlay::Account);

    chrome.go_to_main();

    assert_eq!(chrome.overlay(), Overlay::None);
    assert_eq!(
        chrome.focus().focused().map(String::as_str),
        Some("account")
    );
}

#[test]
fn overlay_focus_moves_into_the_surface_and_returns_to_its_anchor_with_the_reason() {
    let mut overlays = OverlayController::default();
    for reason in [
        DismissReason::Outside,
        DismissReason::Escape,
        DismissReason::Action,
    ] {
        overlays.open(Overlay::SavePreset, "save");
        assert_ne!(overlays.focus().focused().map(String::as_str), Some("save"));
        overlays.dismiss(reason);
        assert_eq!(overlays.overlay(), Overlay::None);
        assert_eq!(overlays.focus().focused().map(String::as_str), Some("save"));
        assert_eq!(overlays.last_dismissal(), Some(reason));
    }
    overlays.dismiss(DismissReason::Outside);
    assert_eq!(
        overlays.last_dismissal(),
        Some(DismissReason::Action),
        "a dismissal with nothing open changes nothing"
    );
}

#[test]
fn hints_and_toast_have_accessible_announcements() {
    assert_eq!(
        ronie_ui::default::hints::HintController::new(true).current(),
        ""
    );
    let (mut chrome, _backend) = controller("chrome-hints-off");
    chrome.set_hints(false);
    assert!(!chrome.hints_enabled());
    chrome.announce("Preset Saved");
    assert_eq!(chrome.drain_announcements(), vec!["Preset Saved"]);
}

#[test]
fn edit_revisions_coalesce_for_four_hundred_ms() {
    let (mut chrome, _backend) = controller("chrome-account-signed-out");
    assert_eq!(chrome.coalesce_edit(0, 1), 1);
    assert_eq!(chrome.coalesce_edit(300, 2), 1);
    assert_eq!(chrome.coalesce_edit(400, 3), 3);
    assert_eq!(chrome.coalesce_edit(450, 4), 3);
}

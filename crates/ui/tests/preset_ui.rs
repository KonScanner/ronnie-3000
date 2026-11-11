//! Preset save, browse, rename, reorder, delete, PC, and error-state tests.

use std::sync::Arc;

use ronie_core::PresetInfo;
use ronie_core::params::{ParameterId, ParameterValue};
use ronie_ui::{
    backend::{
        ChainViewState, GesturePhase, MeterSnapshot, MockBackend, UiAction, UiBackend, UiError,
        UiReceipt,
    },
    default::{ChromeController, Overlay},
    testbed::{UiScenario, load_scenarios},
};

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn presets(scenario: &UiScenario) -> Vec<PresetInfo> {
    scenario.state["presets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|preset| PresetInfo {
            id: preset["id"].as_str().unwrap_or_default().to_owned(),
            name: preset["name"].as_str().unwrap_or_default().to_owned(),
            factory: preset["factory"].as_bool().unwrap_or(false),
        })
        .collect()
}

fn controller(id: &str) -> (ChromeController, Arc<MockBackend>) {
    let scenario = scenario(id);
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let controller = ChromeController::new(backend.clone(), presets(&scenario), false, true);
    (controller, backend)
}

#[test]
fn save_updates_the_active_preset_and_shows_a_toast() {
    let (mut chrome, backend) = controller("chrome-toast-saved");
    chrome.open_save();
    chrome.set_draft("  Stadium Lead  ");
    chrome.save().expect("save");
    assert_eq!(chrome.active_preset_name(), Some("Stadium Lead"));
    assert_eq!(chrome.toast(), Some("Preset Saved"));
    assert_eq!(chrome.overlay(), Overlay::None);
    assert_eq!(
        backend
            .chain_state(0)
            .session
            .active_preset
            .as_ref()
            .map(|preset| preset.name.clone()),
        Some("Stadium Lead".to_owned())
    );
    assert_eq!(chrome.drain_announcements(), vec!["Preset Saved"]);
}

#[test]
fn same_name_save_updates_instead_of_duplicating() {
    let (mut chrome, _backend) = controller("chrome-preset-save");
    let initial = chrome.presets().len();
    chrome.open_save();
    chrome.set_draft("My Lead Tone");
    chrome.save().expect("same-name update");
    assert_eq!(chrome.presets().len(), initial);
    assert!(
        chrome
            .presets()
            .iter()
            .any(|preset| preset.name == "My Lead Tone")
    );
}

#[test]
fn browse_load_rename_reorder_and_delete_use_backend_actions() {
    let (mut chrome, backend) = controller("chrome-preset-browse");
    chrome.open_presets();
    assert_eq!(chrome.overlay(), Overlay::PresetBrowser);
    chrome.load_preset("user-1").expect("load");
    assert_eq!(
        backend
            .chain_state(0)
            .session
            .active_preset
            .as_ref()
            .map(|preset| preset.id.clone()),
        Some("user-1".to_owned())
    );
    chrome
        .rename_preset("user-1", "Renamed Lead")
        .expect("rename");
    assert!(
        chrome
            .presets()
            .iter()
            .any(|preset| preset.name == "Renamed Lead")
    );
    chrome.move_preset("user-1", 1).expect("move");
    chrome.delete_preset("user-1").expect("delete");
    assert!(!chrome.presets().iter().any(|preset| preset.id == "user-1"));
}

#[test]
fn previous_next_and_pc_badges_follow_the_visible_preset_list() {
    let (mut chrome, _backend) = controller("chrome-preset-browse");
    chrome.set_show_pc(true);
    assert_eq!(chrome.pc_badge(0).as_deref(), Some("001"));
    chrome.set_show_pc(false);
    assert_eq!(chrome.pc_badge(0), None);
    chrome.next_preset().expect("next");
    assert_eq!(chrome.active_preset_name(), Some("Clean Platform"));
    chrome.previous_preset().expect("previous");
    assert_eq!(chrome.active_preset_name(), Some("Crunch Rhythm"));

    let mut scenario = scenario("chrome-preset-browse");
    scenario.state["chain"]
        .as_object_mut()
        .expect("chain object")
        .remove("preset");
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let mut chrome = ChromeController::new(backend, presets(&scenario), false, true);
    chrome.next_preset().expect("next without active");
    assert_eq!(chrome.active_preset_name(), Some("Crunch Rhythm"));
    chrome.previous_preset().expect("previous without active");
    assert_eq!(chrome.active_preset_name(), Some("Church Sunday"));
}

#[test]
fn empty_and_failed_saves_keep_the_editor_open_with_an_error() {
    let (mut chrome, _backend) = controller("chrome-preset-save");
    chrome.open_save();
    chrome.set_draft("   ");
    assert!(chrome.save().is_err());
    assert_eq!(chrome.overlay(), Overlay::SavePreset);
    assert!(chrome.save_error().is_some());

    let scenario = scenario("chrome-preset-save");
    let backend = Arc::new(FailingSaveBackend {
        inner: MockBackend::from_scenario(&scenario),
    });
    let mut chrome = ChromeController::new(backend, presets(&scenario), false, true);
    chrome.open_save();
    chrome.set_draft("Valid Name");
    assert!(chrome.save().is_err());
    assert_eq!(chrome.overlay(), Overlay::SavePreset);
    assert_eq!(chrome.toast(), None);
}

struct FailingSaveBackend {
    inner: MockBackend,
}

impl UiBackend for FailingSaveBackend {
    fn chain_state(&self, known_revision: u64) -> ChainViewState {
        self.inner.chain_state(known_revision)
    }

    fn meter_levels(&self) -> MeterSnapshot {
        self.inner.meter_levels()
    }

    fn dispatch(&self, action: UiAction) -> Result<UiReceipt, UiError> {
        if matches!(action, UiAction::SavePreset { .. }) {
            return Err(UiError::Control("read-only preset store".to_owned()));
        }
        self.inner.dispatch(action)
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

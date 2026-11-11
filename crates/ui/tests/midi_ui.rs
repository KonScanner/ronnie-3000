//! MIDI mapping, learning, draft, picker, and hardware-input workflows.

use std::sync::Arc;

use ronie_core::{MidiMapping, MidiSource};
use ronie_ui::{
    backend::{DeviceAction, MidiAction, MockBackend, UiAction},
    default::{MidiMapController, SettingsController, SettingsTab},
    testbed::{UiScenario, load_scenarios},
};

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn backend(id: &str) -> Arc<MockBackend> {
    Arc::new(MockBackend::from_scenario(&scenario(id)))
}

#[test]
fn midi_scenarios_expose_mappings_learning_and_target_picker_state() {
    let empty = MidiMapController::new(backend("settings-midi-empty"));
    assert_eq!(empty.snapshot().channel, 0);
    assert!(empty.snapshot().mappings.is_empty());
    assert!(empty.snapshot().learn_target_id.is_empty());

    let learning = MidiMapController::new(backend("settings-midi-learning"));
    assert_eq!(learning.snapshot().channel, 1);
    assert_eq!(learning.snapshot().learn_target_id, "outputLevel");
    assert_eq!(learning.snapshot().mappings.len(), 3);
    assert_eq!(
        learning.source_label(&learning.snapshot().mappings[1]),
        "Note C4"
    );
    assert_eq!(
        learning.behavior_label(&learning.snapshot().mappings[2]),
        "Trigger"
    );
    assert!(
        !learning
            .available_targets()
            .iter()
            .any(|target| target.id == "inputLevel")
    );

    let mut picker = MidiMapController::new(backend("settings-midi-picker"));
    assert!(!picker.picker_open());
    picker.open_picker();
    assert!(picker.picker_open());
    assert!(picker.available_targets().len() > 10);
}

#[test]
fn midi_rows_resolve_display_names_and_live_block_context() {
    let midi = MidiMapController::new(backend("settings-midi-learning"));
    assert_eq!(midi.target_name("inputLevel"), "Input Gain");
    assert_eq!(midi.target_context("outputLevel"), "Global");
    assert_eq!(
        midi.target_context("block1Power"),
        "Chain · Green Russian Big Muff + BD-2"
    );
}

#[test]
fn cc_and_note_learning_update_session_mappings_through_typed_actions() {
    let backend = backend("settings-midi-empty");
    let mut midi = MidiMapController::new(backend.clone());
    midi.start_learn("outputLevel").expect("arm CC");
    midi.learn_cc(74).expect("CC");
    assert_eq!(
        midi.snapshot().mappings.last(),
        Some(&MidiMapping {
            target_id: "outputLevel".to_owned(),
            source: MidiSource::Cc,
            number: 74,
        })
    );
    midi.start_learn("block1Power").expect("arm note");
    midi.learn_note(60).expect("note");
    assert_eq!(
        midi.snapshot().mappings.last(),
        Some(&MidiMapping {
            target_id: "block1Power".to_owned(),
            source: MidiSource::Note,
            number: 60,
        })
    );
    assert!(
        backend
            .actions()
            .contains(&UiAction::Midi(MidiAction::SetMapping {
                target_id: "outputLevel".to_owned(),
                source: MidiSource::Cc,
                number: 74,
            }))
    );
    assert!(
        backend
            .actions()
            .contains(&UiAction::Midi(MidiAction::SetMapping {
                target_id: "block1Power".to_owned(),
                source: MidiSource::Note,
                number: 60,
            }))
    );
    assert!(midi.snapshot().learn_target_id.is_empty());
}

#[test]
fn typed_cc_draft_survives_rebuilds_and_rejects_out_of_range_values() {
    let backend = backend("settings-midi-empty");
    let mut first = MidiMapController::new(backend.clone());
    first.start_learn("outputLevel").expect("arm");
    first.edit_draft("1x2").expect("draft");
    assert_eq!(first.draft().as_deref(), Some("12"));
    drop(first);

    let mut rebuilt = MidiMapController::new(backend.clone());
    assert_eq!(rebuilt.draft().as_deref(), Some("12"));
    rebuilt.commit_draft().expect("commit");
    assert_eq!(rebuilt.snapshot().mappings[0].number, 12);
    rebuilt.start_learn("presetNext").expect("next");
    rebuilt.edit_draft("128").expect("draft");
    rebuilt.commit_draft().expect("out of range");
    assert!(
        rebuilt
            .snapshot()
            .mappings
            .iter()
            .all(|mapping| mapping.target_id != "presetNext")
    );
    assert_eq!(rebuilt.draft().as_deref(), Some("128"));
}

#[test]
fn target_picker_filters_existing_mappings_and_selects_a_new_target() {
    let backend = backend("settings-midi-learning");
    let mut midi = MidiMapController::new(backend);
    assert!(midi.available_targets().iter().all(|target| {
        !matches!(
            target.id.as_str(),
            "inputLevel" | "block1Power" | "presetNext"
        )
    }));
    assert!(
        midi.available_targets()
            .iter()
            .all(|target| !target.id.starts_with("rightBlock"))
    );
    midi.open_picker();
    midi.select_target("presetPrevious").expect("target");
    assert!(!picker_open(&midi));
    assert_eq!(midi.snapshot().learn_target_id, "presetPrevious");
}

fn picker_open(midi: &MidiMapController) -> bool {
    midi.picker_open()
}

#[test]
fn channel_filter_and_mapping_removal_validate_and_persist() {
    let backend = backend("settings-midi-learning");
    let mut midi = MidiMapController::new(backend.clone());
    midi.set_channel(16).expect("channel");
    assert_eq!(midi.snapshot().channel, 16);
    assert!(midi.set_channel(17).is_err());
    midi.remove_mapping("inputLevel").expect("remove");
    assert!(
        midi.snapshot()
            .mappings
            .iter()
            .all(|mapping| mapping.target_id != "inputLevel")
    );
    assert!(
        backend
            .actions()
            .contains(&UiAction::Midi(MidiAction::RemoveMapping(
                "inputLevel".to_owned()
            )))
    );
}

#[test]
fn midi_input_enablement_uses_the_device_boundary() {
    let backend = backend("settings-system");
    let settings = SettingsController::new(backend.clone(), true, SettingsTab::System);
    settings
        .system()
        .set_midi_enabled("midi-2", true)
        .expect("enable");
    assert!(
        settings
            .system()
            .device()
            .expect("device")
            .midi_inputs
            .iter()
            .any(|input| { input.id == "midi-2" && input.enabled })
    );
    assert!(
        backend
            .actions()
            .contains(&UiAction::Device(DeviceAction::SetMidiInputEnabled {
                id: "midi-2".to_owned(),
                enabled: true,
            }))
    );
}

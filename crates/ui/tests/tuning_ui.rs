//! The tuning picker of the pitch deck, the pitch knob caption, and the tuner line of a shift.

use std::sync::Arc;

use eframe::egui;
use ronie_core::{
    OPTIMIZE_PREFERENCE,
    params::{ParameterId, ParameterValue},
};
use ronie_ui::{
    backend::{GesturePhase, MockBackend, UiAction, UiBackend},
    default::{ChromeController, TunerState, tuning},
    driver::UiDriver,
    native_scenario::reach_scenario,
    testbed::{UiScenario, load_scenarios},
};

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .unwrap_or_else(|| panic!("no scenario {id}"))
}

/// The real Default UI of one scenario on its mock.
fn driver(id: &str) -> (UiDriver, MockBackend) {
    let backend = MockBackend::from_scenario(&scenario(id));
    let chrome = ChromeController::new(Arc::new(backend.clone()), Vec::new(), false, true);
    let mut driver = UiDriver::new(chrome, backend.clone());
    driver.settle();
    (driver, backend)
}

fn value(backend: &MockBackend, id: &'static str) -> f32 {
    backend.parameter(ParameterId(id)).expect("parameter").0
}

fn set(backend: &MockBackend, id: &'static str, next: f32) {
    backend
        .set_parameter(ParameterId(id), ParameterValue(next), GesturePhase::Change)
        .expect("parameter write");
}

/// Opens the pitch deck with a right-click on the pitch knob.
fn open_deck(driver: &mut UiDriver) {
    let pitch = driver.label_starting_with("Pitch:").expect("pitch knob");
    let _ = driver.secondary_click(&pitch).expect("open the deck");
}

/// Opens one picker list of the pitch deck and clicks one row of it.
fn pick(driver: &mut UiDriver, picker: &str, row: &str) {
    let label = driver
        .label_starting_with(picker)
        .unwrap_or_else(|| panic!("no {picker} in {:?}", driver.visible_labels()));
    let _ = driver.click(&label).expect("open the list");
    let _ = driver.click(row).expect("pick the row");
    driver.settle();
}

/// The pitch window choices: 20 ms, 30 ms, and 40 ms.
const MS_20: f32 = 0.0;
const MS_30: f32 = 1.0;
const MS_40: f32 = 2.0;

/// The writes of the pitch parameters in the action log, with their gesture phases.
fn pitch_writes(backend: &MockBackend) -> Vec<(&'static str, GesturePhase)> {
    backend
        .actions()
        .into_iter()
        .filter_map(|action| match action {
            UiAction::SetParameter { id, gesture, .. } if id.0.starts_with("pitch") => {
                Some((id.0, gesture))
            }
            _ => None,
        })
        .collect()
}

fn set_optimize(backend: &MockBackend, enabled: bool) {
    backend
        .dispatch(UiAction::SetPreference {
            key: OPTIMIZE_PREFERENCE.to_owned(),
            value: enabled,
        })
        .expect("preference");
}

fn center(driver: &UiDriver, prefix: &str) -> egui::Pos2 {
    driver
        .controls()
        .iter()
        .find(|control| control.label.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} in {:?}", driver.visible_labels()))
        .rect
        .center()
}

fn button(driver: &mut UiDriver, pos: egui::Pos2, pressed: bool) {
    driver.play(vec![egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }]);
}

/// Presses on a control and moves up by ten points in each of `steps` frames. The button
/// stays down.
fn press_and_drag_up(driver: &mut UiDriver, prefix: &str, steps: usize) -> egui::Pos2 {
    let mut pos = center(driver, prefix);
    driver.play(vec![egui::Event::PointerMoved(pos)]);
    button(driver, pos, true);
    for _ in 0..steps {
        pos.y -= 10.0;
        driver.play(vec![egui::Event::PointerMoved(pos)]);
    }
    pos
}

/// Opens the type-in field of the pitch knob with a double-click, types `text`, and presses
/// Enter.
fn type_pitch(driver: &mut UiDriver, text: &str) {
    let pitch = center(driver, "Pitch:");
    for _ in 0..2 {
        driver.play(vec![egui::Event::PointerMoved(pitch)]);
        button(driver, pitch, true);
        button(driver, pitch, false);
    }
    driver.settle();
    assert!(
        driver.is_visible("Pitch value"),
        "{:?}",
        driver.visible_labels()
    );
    driver.play(vec![egui::Event::Text(text.to_owned())]);
    let _ = driver.press_key(egui::Key::Enter);
}

#[test]
fn optimize_is_on_with_no_stored_value_and_a_stored_off_turns_it_off() {
    let (mut driver, backend) = driver("chrome-pitch-deck");
    assert_eq!(backend.preference(OPTIMIZE_PREFERENCE), None);
    assert!(tuning::optimize(&backend));
    open_deck(&mut driver);
    assert!(driver.is_visible("Optimize settings"));

    set_optimize(&backend, false);
    assert!(!tuning::optimize(&backend));
    set_optimize(&backend, true);
    assert!(tuning::optimize(&backend));
}

#[test]
fn a_click_on_optimize_turns_it_off_and_on_and_on_applies_the_policy() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    open_deck(&mut driver);
    assert_eq!(value(&backend, "pitchWindow"), MS_30);

    let _ = driver.click("Optimize settings").expect("OPTIMIZE");
    assert_eq!(backend.preference(OPTIMIZE_PREFERENCE), Some(false));
    assert_eq!(value(&backend, "pitchWindow"), MS_30, "off changes nothing");

    let _ = driver.click("Optimize settings").expect("OPTIMIZE");
    assert_eq!(backend.preference(OPTIMIZE_PREFERENCE), Some(true));
    assert_eq!(value(&backend, "pitchWindow"), MS_20, "a guitar uses 20 ms");
    assert_eq!(
        value(&backend, "pitchEnabled"),
        1.0,
        "D Standard keeps its shift"
    );
}

#[test]
fn with_optimize_on_a_picked_bass_uses_the_40_ms_window_as_one_undo_step() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    assert_eq!(value(&backend, "pitchWindow"), MS_30);
    open_deck(&mut driver);

    pick(&mut driver, "Instrument:", "Bass 4-string");

    assert_eq!(value(&backend, "pitchWindow"), MS_40);
    assert_eq!(value(&backend, "pitchEnabled"), 1.0);
    assert_eq!(value(&backend, "pitchSemitones"), -2.0, "the shift stays");
    let writes = pitch_writes(&backend);
    assert_eq!(
        &writes[writes.len() - 3..],
        &[
            ("pitchWindow", GesturePhase::Begin),
            ("pitchWindow", GesturePhase::Change),
            ("pitchWindow", GesturePhase::End),
        ]
    );
    backend.dispatch(UiAction::Undo).expect("undo");
    assert_eq!(value(&backend, "pitchWindow"), MS_30);
    assert_eq!(
        backend
            .text_preference(tuning::INSTRUMENT_PREFERENCE)
            .as_deref(),
        Some("bass4"),
        "the instrument is a machine setting, not an undo step"
    );
}

#[test]
fn with_optimize_off_a_picked_bass_keeps_the_window() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    set_optimize(&backend, false);
    open_deck(&mut driver);
    let writes = pitch_writes(&backend).len();

    pick(&mut driver, "Instrument:", "Bass 4-string");

    assert_eq!(value(&backend, "pitchWindow"), MS_30);
    assert_eq!(pitch_writes(&backend).len(), writes, "no parameter changes");
    assert!(driver.is_visible("Buffer: 30 ms"));
}

#[test]
fn with_optimize_on_the_pitch_knob_back_to_0_turns_the_pitch_shift_off() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    assert_eq!(value(&backend, "pitchEnabled"), 1.0);

    type_pitch(&mut driver, "0");

    assert_eq!(value(&backend, "pitchSemitones"), 0.0);
    assert_eq!(value(&backend, "pitchEnabled"), 0.0, "no shift, no delay");
    assert_eq!(value(&backend, "pitchWindow"), MS_20);
    assert_eq!(tuning::caption(&backend), "Pitch");
}

#[test]
fn optimize_follows_the_pitch_knob_at_the_end_of_a_drag_only() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    let before = pitch_writes(&backend).len();

    let pos = press_and_drag_up(&mut driver, "Pitch:", 4);
    let during = pitch_writes(&backend);
    assert!(
        during[before..]
            .iter()
            .all(|(id, _)| *id == "pitchSemitones"),
        "no policy write during the drag: {:?}",
        &during[before..]
    );
    assert_ne!(
        value(&backend, "pitchSemitones"),
        -2.0,
        "the drag moved the knob"
    );
    assert_eq!(value(&backend, "pitchWindow"), MS_30);

    button(&mut driver, pos, false);
    driver.settle();

    assert_eq!(value(&backend, "pitchWindow"), MS_20);
    let after = pitch_writes(&backend);
    let policy = after[during.len()..]
        .iter()
        .filter(|(id, _)| *id == "pitchWindow")
        .count();
    assert_eq!(
        policy,
        3,
        "one gesture after the release: {:?}",
        &after[during.len()..]
    );
}

#[test]
fn with_optimize_on_step_turns_the_pitch_shift_off_for_a_shift_that_rounds_to_0() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    set(&backend, "pitchStep", 0.0);
    set(&backend, "pitchSemitones", 0.4);
    open_deck(&mut driver);
    assert_eq!(value(&backend, "pitchEnabled"), 1.0);

    let _ = driver.click("STEP").expect("STEP");

    assert_eq!(value(&backend, "pitchStep"), 1.0);
    assert_eq!(value(&backend, "pitchEnabled"), 0.0);
}

#[test]
fn a_drag_of_the_buffer_knob_turns_optimize_off_and_keeps_the_window() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    open_deck(&mut driver);
    assert_eq!(value(&backend, "pitchWindow"), MS_30);

    let pos = press_and_drag_up(&mut driver, "Buffer:", 8);
    button(&mut driver, pos, false);
    driver.settle();

    assert_eq!(value(&backend, "pitchWindow"), MS_40);
    assert_eq!(backend.preference(OPTIMIZE_PREFERENCE), Some(false));
    // The policy no longer takes the window back.
    pick(&mut driver, "Tuning:", "C# Standard (-3)");
    assert_eq!(value(&backend, "pitchSemitones"), -3.0);
    assert_eq!(value(&backend, "pitchWindow"), MS_40);
    assert!(driver.is_visible("Buffer: 40 ms"));
}

#[test]
fn a_picked_tuning_sets_the_shift_step_and_power_as_one_undo_step() {
    let (mut driver, backend) = driver("chrome-pitch-deck");
    // The knob stays on the bottom bar while the pitch shift is off.
    backend
        .dispatch(UiAction::SetPreference {
            key: "t3k.showPitchControl".to_owned(),
            value: true,
        })
        .expect("preference");
    set(&backend, "pitchStep", 0.0);
    set(&backend, "pitchEnabled", 0.0);
    driver.settle();
    let before = backend.chain_state(0);
    open_deck(&mut driver);
    assert!(driver.is_visible("Tuning: E Standard"));
    assert!(driver.is_visible("Instrument: Guitar 6-string"));

    pick(&mut driver, "Tuning:", "D Standard (-2)");

    assert_eq!(value(&backend, "pitchSemitones"), -2.0);
    assert_eq!(value(&backend, "pitchStep"), 1.0);
    assert_eq!(value(&backend, "pitchEnabled"), 1.0);
    assert!(driver.is_visible("Tuning: D Standard (-2)"));
    assert!(driver.shows_text("Strings: E→D  A→G  D→C  G→F  B→A  e→d"));
    assert!(driver.is_visible("Pitch: -2 st"), "the knob keeps its name");
    let phases = backend
        .actions()
        .into_iter()
        .filter_map(|action| match action {
            UiAction::SetParameter { id, gesture, .. } if id.0.starts_with("pitch") => {
                Some((id.0, gesture))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    // OPTIMIZE is on with no stored value, so the window of a guitar joins the pick.
    assert_eq!(value(&backend, "pitchWindow"), MS_20);
    assert_eq!(
        &phases[phases.len() - 6..],
        &[
            ("pitchSemitones", GesturePhase::Begin),
            ("pitchSemitones", GesturePhase::Change),
            ("pitchStep", GesturePhase::Change),
            ("pitchEnabled", GesturePhase::Change),
            ("pitchWindow", GesturePhase::Change),
            ("pitchSemitones", GesturePhase::End),
        ]
    );

    // One undo takes the whole pick back.
    backend.dispatch(UiAction::Undo).expect("undo");
    assert_eq!(value(&backend, "pitchSemitones"), 0.0);
    assert_eq!(value(&backend, "pitchStep"), 0.0);
    assert_eq!(value(&backend, "pitchEnabled"), 0.0);
    assert_eq!(value(&backend, "pitchWindow"), MS_30);
    assert_eq!(backend.chain_state(0).can_undo, before.can_undo);
}

#[test]
fn the_tuning_with_no_shift_turns_the_pitch_shift_off() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    assert_eq!(value(&backend, "pitchSemitones"), -2.0);
    open_deck(&mut driver);

    pick(&mut driver, "Tuning:", "E Standard");

    assert_eq!(value(&backend, "pitchSemitones"), 0.0);
    assert_eq!(value(&backend, "pitchEnabled"), 0.0);
    assert_eq!(value(&backend, "pitchStep"), 1.0);
    backend.dispatch(UiAction::Undo).expect("undo");
    assert_eq!(value(&backend, "pitchSemitones"), -2.0);
    assert_eq!(value(&backend, "pitchEnabled"), 1.0);
}

#[test]
fn a_picked_instrument_is_a_machine_preference_and_renames_the_tunings() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    let can_undo = backend.chain_state(0).can_undo;
    open_deck(&mut driver);

    pick(&mut driver, "Instrument:", "Bass 5-string");

    assert_eq!(
        backend
            .text_preference(tuning::INSTRUMENT_PREFERENCE)
            .as_deref(),
        Some("bass5")
    );
    assert_eq!(backend.chain_state(0).can_undo, can_undo);
    assert_eq!(value(&backend, "pitchSemitones"), -2.0, "the shift stays");
    assert!(driver.is_visible("Instrument: Bass 5-string"));
    assert!(driver.is_visible("Tuning: A Standard (-2)"));
    assert!(driver.shows_text("Strings: B→A  E→D  A→G  D→C  G→F"));
    assert_eq!(tuning::caption(&backend), "A Std");
}

#[test]
fn the_caption_names_a_listed_tuning_shows_a_custom_shift_and_reads_pitch_when_off() {
    let backend = MockBackend::from_scenario(&scenario("chrome-pitch-caption"));
    assert_eq!(tuning::caption(&backend), "D Std");
    assert_eq!(
        tuning::current_tuning(&backend).map(|tuning| tuning.name()),
        Some("D Standard".to_owned())
    );

    set(&backend, "pitchSemitones", -7.0);
    assert_eq!(tuning::caption(&backend), "-7 st");
    assert_eq!(tuning::current_tuning(&backend), None);

    set(&backend, "pitchStep", 0.0);
    set(&backend, "pitchSemitones", 1.5);
    assert_eq!(tuning::caption(&backend), "+1.5 st");
    // STEP rounds the shift that sounds, and the caption shows that shift.
    set(&backend, "pitchStep", 1.0);
    assert_eq!(tuning::caption(&backend), "+2 st");

    set(&backend, "pitchSemitones", 0.0);
    assert_eq!(tuning::caption(&backend), "Pitch");
    set(&backend, "pitchSemitones", -2.0);
    set(&backend, "pitchEnabled", 0.0);
    assert_eq!(tuning::caption(&backend), "Pitch");
}

#[test]
fn a_click_on_the_other_picker_opens_its_list_under_it() {
    let (mut driver, _backend) = driver("chrome-pitch-tuning");
    open_deck(&mut driver);
    let _ = driver
        .click("Instrument: Guitar 6-string")
        .expect("open the instruments");
    assert!(driver.is_visible("Bass 5-string"));

    let _ = driver
        .click("Tuning: D Standard (-2)")
        .expect("open the tunings");
    driver.settle();

    assert!(!driver.is_visible("Bass 5-string"));
    let rect = |label: &str| {
        driver
            .controls()
            .iter()
            .find(|control| control.label == label)
            .unwrap_or_else(|| panic!("no {label}"))
            .rect
    };
    let picker = rect("Tuning: D Standard (-2)");
    let row = rect("E Standard");
    assert!(
        row.left() >= picker.left() && row.left() <= picker.left() + 10.0,
        "the list starts under its picker: {row:?} {picker:?}"
    );
}

#[test]
fn a_custom_shift_reads_custom_in_the_picker() {
    let (mut driver, backend) = driver("chrome-pitch-tuning");
    set(&backend, "pitchSemitones", -7.0);
    open_deck(&mut driver);
    assert!(driver.is_visible("Tuning: Custom"));
    assert!(driver.is_visible("Pitch: -7 st"));
}

#[test]
fn the_tuner_names_the_note_that_the_shift_makes() {
    let state = TunerState {
        has_signal: true,
        note: "E".to_owned(),
        octave: 2,
        cents: 0.0,
        frequency_hz: 82.41,
    };
    assert_eq!(state.sounds_as(-2.0).as_deref(), Some("Sounds as D"));
    assert_eq!(state.sounds_as(-5.0).as_deref(), Some("Sounds as B"));
    assert_eq!(state.sounds_as(0.0), None);
    assert_eq!(TunerState::default().sounds_as(-2.0), None);

    let driver = reach_scenario(&scenario("main-tuner-shifted")).expect("tuner scenario");
    assert!(driver.shows_text("Sounds as D"));
    assert!(
        driver
            .chrome()
            .tuner()
            .state()
            .accessibility_text()
            .starts_with("E2"),
        "the big note stays the real string"
    );

    let unshifted = reach_scenario(&scenario("main-tuner-intune")).expect("tuner scenario");
    assert!(!unshifted.shows_text("Sounds as A"), "no shift, no line");
}

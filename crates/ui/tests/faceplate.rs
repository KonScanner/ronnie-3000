//! Faceplate widgets, mode-dependent controls, decks, and meter tests.

use std::sync::Arc;

use eframe::egui;
use ronie_core::params::{ParameterId, ParameterValue};
use ronie_ui::{
    backend::{GesturePhase, MockBackend, UiAction, UiBackend},
    default::{ChromeController, DeckKind, DeckModel, Faceplate, MeasurementState},
    driver::UiDriver,
    testbed::load_scenarios,
    widgets::{Knob, KnobSpec, MeterStore},
};

fn backend() -> Arc<MockBackend> {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "chrome-gate-deck")
        .expect("scenario");
    Arc::new(MockBackend::from_scenario(&scenario))
}

#[test]
fn knob_uses_reference_gain_mapping_and_typed_input() {
    let backend = backend();
    let knob = Knob::new(backend, "inputLevel", "Input").expect("input knob");
    assert_eq!(knob.format(), "0.0 dB");
    let spec = KnobSpec::parameter(ParameterId("inputLevel")).expect("catalog");
    assert_eq!(spec.format(1.0), "+24.0 dB");
    assert_eq!(spec.parse("-12"), Some(0.25));
}

#[test]
fn the_knob_and_the_tile_gain_show_one_text_for_one_value() {
    use ronie_ui::default::tile::format_gain_db;

    let spec = KnobSpec::parameter(ParameterId("inputLevel")).expect("catalog");
    for (typed, normalized, text) in [
        ("6", 0.625, "+6.0 dB"),
        ("-0.5", 0.5 - 0.5 / 48.0, "-0.5 dB"),
        ("0", 0.5, "0.0 dB"),
    ] {
        let value = spec.parse(typed).expect("typed dB");
        assert_eq!(spec.format(value), text);
        assert_eq!(KnobSpec::block_gain().format(normalized), text);
        assert_eq!(format_gain_db(normalized), text);
    }
}

#[test]
fn pitch_typed_values_match_reference() {
    let spec = KnobSpec::parameter(ParameterId("pitchSemitones")).expect("catalog");
    let value = spec.parse("3").expect("typed semitones");
    assert_eq!(spec.position(value), 0.5625);
    assert_eq!(spec.format(value), "+3 st");
}

#[test]
fn meter_latches_clip_clears_groups_and_limits_repaints() {
    let mut meters = MeterStore::default();
    meters.update_main("input", -70.0);
    assert_eq!(meters.level("input"), -60.0);
    meters.update_main("input", 0.25);
    assert!(meters.clipped("input"));
    assert!(meters.clipped("input:l"));
    assert!(meters.clipped("input:r"));
    meters.update_main("input", -60.0);
    assert!(meters.clipped("input"));
    meters.clear_clip("input");
    assert!(!meters.clipped("input"));
    assert!(!meters.clipped("input:l"));
    assert!(!meters.clipped("input:r"));
}

#[test]
fn cpu_uses_the_reference_ema_and_publish_interval() {
    let mut meters = MeterStore::default();
    meters.apply_cpu(1.0, 0);
    assert_eq!(meters.cpu_percent(), 100.0);
    meters.apply_cpu(0.0, 100);
    assert_eq!(meters.cpu_percent(), 100.0);
    meters.apply_cpu(0.0, 400);
    assert_eq!(meters.cpu_percent(), 72.3);
}

#[test]
fn faceplate_and_decks_expose_mode_dependent_parameters() {
    let backend = backend();
    let faceplate = Faceplate::new(backend.clone());
    let visible = faceplate.visible_controls(false, false);
    assert!(visible.contains(&"spreadOffset"));
    assert!(!visible.contains(&"alignOffset"));
    let visible = faceplate.visible_controls(true, true);
    assert!(visible.contains(&"alignOffset"));
    assert!(!visible.contains(&"spreadOffset"));

    let mut deck = DeckModel::new(backend, DeckKind::AutoBalance);
    assert_eq!(deck.state(), MeasurementState::Idle);
    deck.start();
    assert_eq!(deck.state(), MeasurementState::Running);
    deck.cancel();
    assert_eq!(deck.state(), MeasurementState::Idle);
}

#[test]
fn input_mode_menu_dispatches_the_selected_routing_mode() {
    let scenarios =
        load_scenarios(include_str!("../../../fixtures/ui-scenarios.json")).expect("scenarios");
    let scenario = scenarios
        .iter()
        .find(|scenario| scenario.id == "chrome-input-mode")
        .expect("input mode scenario");
    let backend = Arc::new(MockBackend::from_scenario(scenario));
    let mut faceplate = Faceplate::new(backend.clone());
    faceplate.open_input_mode();
    let context = eframe::egui::Context::default();
    context.enable_accesskit();
    ronie_ui::fonts::Fonts::install(&context);
    let input = eframe::egui::RawInput {
        screen_rect: Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(1024.0, 578.0),
        )),
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| {
        eframe::egui::Panel::bottom("faceplate")
            .exact_size(108.0)
            .show(ui, |ui| faceplate.show(ui));
    });
    output.textures_delta.clear();
    let mut output = context.run_ui(eframe::egui::RawInput::default(), |ui| {
        eframe::egui::Panel::bottom("faceplate")
            .exact_size(108.0)
            .show(ui, |ui| faceplate.show(ui));
    });
    output.textures_delta.clear();
    let point = eframe::egui::pos2(112.0, 405.0);
    let input = eframe::egui::RawInput {
        events: vec![eframe::egui::Event::PointerButton {
            pos: point,
            button: eframe::egui::PointerButton::Primary,
            pressed: true,
            modifiers: eframe::egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| {
        eframe::egui::Panel::bottom("faceplate")
            .exact_size(108.0)
            .show(ui, |ui| faceplate.show(ui));
    });
    output.textures_delta.clear();
    let input = eframe::egui::RawInput {
        events: vec![eframe::egui::Event::PointerButton {
            pos: point,
            button: eframe::egui::PointerButton::Primary,
            pressed: false,
            modifiers: eframe::egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| {
        eframe::egui::Panel::bottom("faceplate")
            .exact_size(108.0)
            .show(ui, |ui| faceplate.show(ui));
    });
    output.textures_delta.clear();
    assert_eq!(
        ronie_ui::backend::UiBackend::chain_state(backend.as_ref(), 0)
            .session
            .input_mode,
        ronie_core::InputMode::DualMono
    );
}

#[test]
fn advanced_decks_bind_only_their_advanced_controls() {
    for (kind, parameters) in [
        (
            DeckKind::Gate,
            &["gateRelease", "gateHold", "gateRange"][..],
        ),
        (
            DeckKind::Pitch,
            &["pitchStep", "pitchTonality", "pitchWindow"][..],
        ),
        (
            DeckKind::Spread,
            &[
                "spreadWobble",
                "spreadWobbleEnabled",
                "spreadCrossover",
                "spreadCrossoverEnabled",
                "spreadDiffuseEnabled",
            ][..],
        ),
        (
            DeckKind::Align,
            &[
                "alignWobble",
                "alignWobbleEnabled",
                "alignCrossover",
                "alignCrossoverEnabled",
                "alignDiffuseEnabled",
            ][..],
        ),
    ] {
        let deck = DeckModel::new(backend(), kind);
        assert_eq!(deck.parameters(), parameters);
    }
}

#[test]
fn scenario_sliders_toggles_and_meters_reach_the_ui() {
    let scenarios =
        load_scenarios(include_str!("../../../fixtures/ui-scenarios.json")).expect("scenarios");
    let spread = scenarios
        .iter()
        .find(|scenario| scenario.id == "chrome-spread-on")
        .expect("spread scenario");
    let backend = Arc::new(MockBackend::from_scenario(spread));
    assert_eq!(
        Knob::new(backend.clone(), "toneMid", "Middle")
            .expect("middle knob")
            .normalized(),
        0.55
    );
    assert_eq!(
        Knob::new(backend.clone(), "gateThreshold", "Gate")
            .expect("gate knob")
            .normalized(),
        0.35
    );
    assert_eq!(
        ronie_ui::backend::UiBackend::parameter(
            backend.as_ref(),
            ronie_core::params::ParameterId("spreadEnabled")
        )
        .expect("spread toggle")
        .0,
        1.0
    );

    let hot = scenarios
        .iter()
        .find(|scenario| scenario.id == "main-meters-hot")
        .expect("hot meters scenario");
    let backend = MockBackend::from_scenario(hot);
    let meters = ronie_ui::backend::UiBackend::meter_levels(&backend);
    assert_eq!(meters.input_db, [0.0, -2.0]);
    assert_eq!(meters.output_db, [-3.0, -4.0]);
    assert_eq!(meters.cpu, 0.22);
}

/// The phases of the parameter writes of one parameter, in the order of the backend log.
fn phases(backend: &MockBackend, parameter: &str) -> Vec<GesturePhase> {
    backend
        .actions()
        .into_iter()
        .filter_map(|action| match action {
            UiAction::SetParameter { id, gesture, .. } if id.0 == parameter => Some(gesture),
            _ => None,
        })
        .collect()
}

/// Whether the backend has an open gesture. Only an open gesture can take a cancel.
fn gesture_open(backend: &MockBackend) -> bool {
    backend
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.5),
            GesturePhase::Cancel,
        )
        .is_ok()
}

fn value(backend: &MockBackend, parameter: &'static str) -> f32 {
    backend
        .parameter(ParameterId(parameter))
        .expect("catalog parameter")
        .0
}

/// The real Default UI of the `main-mono` scenario on its mock, with the pitch knob shown.
fn faceplate_driver() -> (UiDriver, MockBackend) {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-mono")
        .expect("scenario");
    let backend = MockBackend::from_scenario(&scenario);
    show_pitch(&backend, true);
    let chrome = ChromeController::new(Arc::new(backend.clone()), Vec::new(), false, true);
    let mut driver = UiDriver::new(chrome, backend.clone());
    driver.settle();
    (driver, backend)
}

fn show_pitch(backend: &MockBackend, shown: bool) {
    backend
        .dispatch(UiAction::SetPreference {
            key: "t3k.showPitchControl".to_owned(),
            value: shown,
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
fn type_pitch(driver: &mut UiDriver, backend: &MockBackend, step: bool, text: &str) {
    backend
        .set_parameter(
            ParameterId("pitchStep"),
            ParameterValue(if step { 1.0 } else { 0.0 }),
            GesturePhase::End,
        )
        .expect("STEP");
    driver.settle();
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
    // The field selects its text when it opens, so the typed text replaces it.
    driver.play(vec![egui::Event::Text(text.to_owned())]);
    let _ = driver.press_key(egui::Key::Enter);
}

#[test]
fn with_step_on_a_typed_pitch_is_a_whole_semitone() {
    let (mut driver, backend) = faceplate_driver();
    type_pitch(&mut driver, &backend, true, "2.4");

    assert_eq!(value(&backend, "pitchSemitones"), 2.0);
    assert_eq!(
        driver.label_starting_with("Pitch:").as_deref(),
        Some("Pitch: +2 st")
    );
}

#[test]
fn with_step_off_a_typed_pitch_keeps_its_decimals() {
    let (mut driver, backend) = faceplate_driver();
    type_pitch(&mut driver, &backend, false, "2.4");

    assert!(
        (value(&backend, "pitchSemitones") - 2.4).abs() < 1.0e-5,
        "{}",
        value(&backend, "pitchSemitones")
    );
    assert_eq!(
        driver.label_starting_with("Pitch:").as_deref(),
        Some("Pitch: +2.4 st")
    );
}

#[test]
fn a_dropped_knob_ends_its_drag_through_the_real_egui_path() {
    let backend = backend();
    let mut knob = Knob::new(backend.clone(), "inputLevel", "Input").expect("knob");
    let context = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(48.0, 75.0));
    let frame = |knob: &mut Knob, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            let _ = knob.show_in_rect(ui, rect, 48.0);
        });
        output.textures_delta.clear();
    };
    let mut pos = rect.center();
    frame(&mut knob, vec![egui::Event::PointerMoved(pos)]);
    frame(
        &mut knob,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    for _ in 0..2 {
        pos.y -= 10.0;
        frame(&mut knob, vec![egui::Event::PointerMoved(pos)]);
    }
    assert_eq!(
        phases(&backend, "inputLevel"),
        [GesturePhase::Begin, GesturePhase::Change]
    );

    drop(knob);

    assert_eq!(
        phases(&backend, "inputLevel"),
        [GesturePhase::Begin, GesturePhase::Change, GesturePhase::End]
    );
    assert!(!gesture_open(&backend));
}

#[test]
fn a_faceplate_knob_that_hides_during_its_drag_ends_the_drag_once() {
    let (mut driver, backend) = faceplate_driver();
    let mut pos = press_and_drag_up(&mut driver, "Pitch:", 3);
    assert_eq!(phases(&backend, "pitchSemitones")[0], GesturePhase::Begin);

    show_pitch(&backend, false);
    for _ in 0..3 {
        pos.y -= 10.0;
        driver.play(vec![egui::Event::PointerMoved(pos)]);
    }
    button(&mut driver, pos, false);
    show_pitch(&backend, true);
    driver.settle();

    let phases = phases(&backend, "pitchSemitones");
    assert_eq!(
        phases
            .iter()
            .filter(|phase| **phase == GesturePhase::End)
            .count(),
        1,
        "{phases:?}"
    );
    assert_eq!(phases.last(), Some(&GesturePhase::End), "{phases:?}");
    assert!(!gesture_open(&backend));
}

#[test]
fn a_faceplate_knob_drag_continues_when_a_knob_before_it_shows() {
    let (mut driver, backend) = faceplate_driver();
    show_pitch(&backend, false);
    driver.settle();
    let start = value(&backend, "outputLevel");

    let mut pos = press_and_drag_up(&mut driver, "Output:", 2);
    show_pitch(&backend, true);
    for _ in 0..2 {
        pos.y -= 10.0;
        driver.play(vec![egui::Event::PointerMoved(pos)]);
    }
    button(&mut driver, pos, false);
    driver.settle();

    let moved = f64::from(value(&backend, "outputLevel") - start);
    let expected = 40.0 * ronie_ui::widgets::knob::DRAG_PER_POINT;
    assert!(
        (moved - expected).abs() < 1.0e-3,
        "the drag stayed on the output knob: {moved} and not {expected}"
    );
    assert!(
        phases(&backend, "pitchSemitones").is_empty(),
        "the pitch knob took no drag"
    );
    assert_eq!(
        phases(&backend, "outputLevel"),
        [
            GesturePhase::Begin,
            GesturePhase::Change,
            GesturePhase::Change,
            GesturePhase::Change,
            GesturePhase::End
        ]
    );
}

#[test]
fn escape_during_a_deck_knob_drag_cancels_the_drag_and_keeps_the_deck() {
    let (mut driver, backend) = faceplate_driver();
    let gate = driver.label_starting_with("Gate:").expect("gate knob");
    let _ = driver.secondary_click(&gate).expect("open the deck");
    let start = value(&backend, "gateHold");
    let pos = press_and_drag_up(&mut driver, "Hold:", 3);
    assert_ne!(
        value(&backend, "gateHold"),
        start,
        "the drag moved the value"
    );

    let _ = driver.press_key(egui::Key::Escape);
    button(&mut driver, pos, false);
    driver.settle();

    assert_eq!(value(&backend, "gateHold"), start);
    assert!(
        driver.label_starting_with("Hold:").is_some(),
        "one Escape cancels the drag only: {:?}",
        driver.visible_labels()
    );
    let _ = driver.press_key(egui::Key::Escape);
    assert_eq!(
        driver.label_starting_with("Hold:"),
        None,
        "the second Escape closes the deck"
    );
}

#[test]
fn escape_in_a_deck_knob_field_closes_the_field_and_keeps_the_deck() {
    let (mut driver, _backend) = faceplate_driver();
    let gate = driver.label_starting_with("Gate:").expect("gate knob");
    let _ = driver.secondary_click(&gate).expect("open the deck");
    let hold = center(&driver, "Hold:");
    // egui counts a third click within 0.3 s of the right-click as a triple click, so the
    // double-click waits for 20 frames of 1/60 s.
    for _ in 0..20 {
        driver.frame();
    }
    for _ in 0..2 {
        driver.play(vec![egui::Event::PointerMoved(hold)]);
        button(&mut driver, hold, true);
        button(&mut driver, hold, false);
    }
    driver.settle();
    assert!(
        driver.is_visible("Hold value"),
        "{:?}",
        driver.visible_labels()
    );

    let _ = driver.press_key(egui::Key::Escape);

    assert!(!driver.is_visible("Hold value"), "the field closed");
    assert!(
        driver.label_starting_with("Hold:").is_some(),
        "one Escape closes the field only: {:?}",
        driver.visible_labels()
    );
    let _ = driver.press_key(egui::Key::Escape);
    assert_eq!(driver.label_starting_with("Hold:"), None);
}

#[test]
fn a_drag_on_the_buffer_choice_of_the_pitch_deck_moves_it() {
    let (mut driver, backend) = faceplate_driver();
    let pitch = driver.label_starting_with("Pitch:").expect("pitch knob");
    let _ = driver.secondary_click(&pitch).expect("open the deck");
    let start = value(&backend, "pitchWindow");

    let pos = press_and_drag_up(&mut driver, "Buffer:", 8);
    button(&mut driver, pos, false);
    driver.settle();

    assert_eq!(value(&backend, "pitchWindow"), start + 1.0);
    assert!(!gesture_open(&backend));
}

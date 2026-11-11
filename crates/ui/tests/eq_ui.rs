//! EQ math, band editing, keyboard, gesture, and spectrum visibility tests.

use std::sync::Arc;

use eframe::egui;
use ronie_core::{EqBandType, params::ParameterId};
use ronie_ui::{
    backend::{GesturePhase, MockBackend, UiAction, UiBackend},
    default::{
        BlockView, EqEditor, EqField, EqViewMode, format_freq, freq_to_norm, gain_to_y,
        norm_to_freq, parse_decimal, parse_freq, slider_gain_at, type_default_q, type_label,
        y_to_gain,
    },
    driver::UiDriver,
    native_scenario::driver_for_scenario,
    testbed::{UiScenario, load_scenarios},
};

fn scenario() -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-detail-eq-sliders")
        .expect("scenario")
}

fn editor() -> (EqEditor, Arc<MockBackend>) {
    let backend = Arc::new(MockBackend::from_scenario(&scenario()));
    (
        EqEditor::new(backend.clone(), "blk-1").expect("editor"),
        backend,
    )
}

/// Drags one band fully to the left, past its neighbour's limit.
fn drag_fully_left(editor: &mut EqEditor, index: usize) {
    editor.begin_band_drag(index, 500.0, 100.0);
    editor.drag_band_to(-100_000.0, 100.0, false);
    editor.end_drag();
}

#[test]
fn dragging_a_band_after_its_neighbours_are_at_their_limit_does_not_panic() {
    let (mut editor, _) = editor();
    drag_fully_left(&mut editor, 1);
    drag_fully_left(&mut editor, 0);

    let (mut editor, _) = self::editor();
    for index in [1, 2, 3, 4, 5, 3, 2, 4] {
        drag_fully_left(&mut editor, index);
    }
}

proptest::proptest! {
    #[test]
    fn any_drag_order_keeps_every_band_range_valid(
        drags in proptest::collection::vec((0_usize..6, -3000.0_f64..3000.0), 1..40),
    ) {
        let (mut editor, _) = editor();
        for (index, target_x) in drags {
            editor.begin_band_drag(index, 500.0, 100.0);
            editor.drag_band_to(target_x, 100.0, false);
            editor.end_drag();
            for band in 0..6 {
                let (low, high) = editor.freq_range(band);
                proptest::prop_assert!(low <= high, "band {band}: {low} > {high}");
            }
        }
    }
}

#[test]
fn axes_formats_and_typed_values_match_reference_units() {
    assert_eq!(freq_to_norm(20.0), 0.0);
    assert_eq!(freq_to_norm(20_000.0), 1.0);
    assert!((freq_to_norm(1000.0) - 0.5663233347786729).abs() < 1.0e-9);
    assert!((norm_to_freq(0.5) - 632.4555).abs() < 0.001);
    assert!((y_to_gain(gain_to_y(6.0)) - 6.0).abs() < 1.0e-9);
    assert_eq!(format_freq(251.0), "251 Hz");
    assert_eq!(format_freq(1600.0), "1.60k");
    assert_eq!(format_freq(12_500.0), "12.5k");
    assert_eq!(parse_freq("1,2k"), Some(1200.0));
    assert_eq!(parse_freq("800 Hz"), Some(800.0));
    assert_eq!(parse_freq("not-a-frequency"), None);
    assert_eq!(parse_decimal("-12,5"), Some(-12.5));
    assert_eq!(type_label(EqBandType::Lowcut), "Low Cut");
    assert_eq!(type_default_q(EqBandType::Lowshelf), Some(0.71));
    assert_eq!(type_default_q(EqBandType::Bell), None);
}

#[test]
fn response_curve_uses_processing_coefficients() {
    let (mut editor, _backend) = editor();
    editor.select_band(1);
    editor.commit_gain("6").expect("gain");
    let response = editor.response_db(&[150.0], 48_000.0);
    assert!(
        (response[0] - 6.0).abs() < 0.05,
        "response was {}",
        response[0]
    );
}

#[test]
fn typed_frequency_is_clamped_between_neighbours() {
    let (mut editor, backend) = editor();
    editor.select_band(0);
    editor.commit_freq("1000").expect("frequency");
    assert_eq!(editor.params().bands[0].freq_hz, 147.0);
    editor.select_band(1);
    editor.commit_freq("20").expect("frequency");
    assert_eq!(editor.params().bands[1].freq_hz, 149.94);
    let block = &backend.chain_state(0).chain.left[0];
    assert_eq!(block.params.eq.bands[0].freq_hz, 147.0);
    assert_eq!(block.params.eq.bands[1].freq_hz, 149.94);
}

#[test]
fn graph_drag_changes_frequency_gain_and_cut_q_with_fine_control() {
    let (mut editor, _backend) = editor();
    editor.select_band(1);
    editor.begin_band_drag(1, 0.5, gain_to_y(0.0));
    editor.drag_band_to(0.6, gain_to_y(0.0) - 10.0, false);
    assert!((editor.params().bands[1].gain_db - 1.204819).abs() < 0.001);
    assert!(editor.params().bands[1].freq_hz > 150.0);
    editor.end_drag();

    editor.begin_band_drag(0, 0.0, gain_to_y(0.0));
    editor.drag_band_to(0.0, gain_to_y(0.0) - 10.0, true);
    assert!((editor.params().bands[0].q - 0.71 * 0.025_f64.exp()).abs() < 0.001);
    assert_eq!(editor.params().bands[0].gain_db, 0.0);
    editor.end_drag();
}

#[test]
fn sliders_map_absolute_gain_and_relative_fine_drag() {
    assert_eq!(slider_gain_at(20.0, 20.0, 200.0), 15.0);
    assert_eq!(slider_gain_at(220.0, 20.0, 200.0), -15.0);
    assert_eq!(slider_gain_at(120.0, 20.0, 200.0), 0.0);
    let (mut editor, _backend) = editor();
    editor.select_band(1);
    editor.set_slider_gain(1, 120.0, 20.0, 200.0);
    assert_eq!(editor.params().bands[1].gain_db, 0.0);
    editor.nudge_slider(1, -20.0, 200.0, true);
    assert!((editor.params().bands[1].gain_db - 0.375).abs() < 0.001);
}

#[test]
fn type_changes_reset_only_the_shape_default_and_band_reset_preserves_frequency() {
    let (mut editor, _backend) = editor();
    assert_eq!(
        EqEditor::type_options(0),
        vec![EqBandType::Lowcut, EqBandType::Lowshelf]
    );
    assert_eq!(EqEditor::type_options(1), vec![EqBandType::Bell]);
    assert_eq!(
        EqEditor::type_options(5),
        vec![EqBandType::Highcut, EqBandType::Highshelf]
    );
    editor.select_band(0);
    editor.commit_freq("80").expect("frequency");
    editor.commit_q("2").expect("q");
    editor.change_type(EqBandType::Lowshelf);
    assert_eq!(editor.params().bands[0].kind, EqBandType::Lowshelf);
    assert_eq!(editor.params().bands[0].q, 0.71);
    editor.commit_gain("9").expect("gain");
    editor.reset_band(0);
    assert_eq!(editor.params().bands[0].gain_db, 0.0);
    assert_eq!(editor.params().bands[0].freq_hz, 80.0);
    editor.change_type(EqBandType::Lowcut);
    editor.commit_q("3").expect("q");
    editor.reset_band(0);
    assert_eq!(editor.params().bands[0].q, 0.71);
}

#[test]
fn keyboard_steps_cover_frequency_gain_and_q_at_both_sensitivities() {
    let (mut editor, _backend) = editor();
    editor.select_band(1);
    editor.keyboard_step(EqField::Gain, 1, false);
    assert_eq!(editor.params().bands[1].gain_db, 0.5);
    editor.keyboard_step(EqField::Gain, 1, true);
    assert_eq!(editor.params().bands[1].gain_db, 0.5625);
    editor.keyboard_step(EqField::Frequency, 1, false);
    assert!((editor.params().bands[1].freq_hz - 151.5).abs() < 0.001);
    editor.keyboard_step(EqField::Q, 1, false);
    assert!((editor.params().bands[1].q - 1.05).abs() < 0.001);
}

#[test]
fn enable_pre_reset_and_view_controls_preserve_expected_state() {
    let (mut editor, _backend) = editor();
    editor.set_enabled(false);
    assert!(!editor.params().enabled);
    editor.set_pre(true);
    assert!(editor.params().pre);
    editor.set_view(EqViewMode::Curve);
    assert_eq!(editor.view(), EqViewMode::Curve);
    editor.select_band(1);
    editor.commit_gain("12").expect("gain");
    editor.reset();
    assert_eq!(editor.params().bands[1].gain_db, 0.0);
    assert!(editor.params().pre);
    assert!(!editor.params().enabled);
}

#[test]
fn drag_emits_one_begin_change_and_end_gesture_family() {
    let (mut editor, backend) = editor();
    editor.begin_band_drag(1, 0.5, gain_to_y(0.0));
    editor.drag_band_to(0.55, gain_to_y(0.0), false);
    editor.drag_band_to(0.6, gain_to_y(0.0), false);
    editor.end_drag();
    let phases: Vec<GesturePhase> = backend
        .actions()
        .into_iter()
        .filter_map(|action| match action {
            UiAction::SetEq { gesture, .. } => Some(gesture),
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        vec![GesturePhase::Begin, GesturePhase::Change, GesturePhase::End]
    );
}

#[test]
fn native_resync_is_ignored_mid_drag_and_accepted_after_drop() {
    let (mut editor, _backend) = editor();
    editor.begin_band_drag(1, 0.5, gain_to_y(0.0));
    editor.drag_band_to(0.6, gain_to_y(0.0), false);
    let expected = editor.params().clone();
    let mut stale = expected.clone();
    stale.bands[1].freq_hz = 9999.0;
    editor.sync_params(stale.clone());
    assert_eq!(editor.params().bands[1].freq_hz, expected.bands[1].freq_hz);
    editor.end_drag();
    editor.sync_params(stale);
    assert_eq!(editor.params().bands[1].freq_hz, 9999.0);
}

#[test]
fn spectrum_is_enabled_only_while_the_eq_editor_is_visible() {
    let (mut editor, _backend) = editor();
    assert!(!editor.spectrum_enabled());
    editor.set_spectrum_visible(true);
    assert!(editor.spectrum_enabled());
    editor.set_spectrum_bins([-20.0; 64]);
    assert_eq!(editor.spectrum_bins()[0], -20.0);
    editor.set_view(EqViewMode::Curve);
    assert!(editor.spectrum_enabled());
    editor.set_spectrum_visible(false);
    assert!(!editor.spectrum_enabled());
    assert!(editor.spectrum_bins().iter().all(|bin| *bin == -100.0));
}

#[test]
fn eq_does_not_touch_unrelated_host_parameters() {
    let (mut editor, backend) = editor();
    let before = backend
        .parameter(ParameterId("inputLevel"))
        .expect("input")
        .0;
    editor.select_band(1);
    editor.commit_gain("5").expect("gain");
    assert_eq!(
        backend
            .parameter(ParameterId("inputLevel"))
            .expect("input")
            .0,
        before
    );
}

/// The real Default UI on the EQ page of the detail of `blk-2`, in the given EQ view.
fn eq_page(view: EqViewMode) -> (UiDriver, MockBackend) {
    let scenario = scenario();
    let backend = MockBackend::from_scenario(&scenario);
    let mut driver = driver_for_scenario(&scenario, backend.clone()).expect("driver");
    let detail = driver.chrome_mut().detail_mut().expect("detail");
    detail.set_view(BlockView::Eq);
    detail.eq_mut().set_view(view);
    driver.settle();
    (driver, backend)
}

/// Undoes each undo step of the backend, and returns how many it held.
fn undo_all(backend: &MockBackend) -> usize {
    let mut steps = 0;
    while backend.dispatch(UiAction::Undo).is_ok() {
        steps += 1;
    }
    steps
}

/// The undo steps of the scenario before any edit.
fn scenario_steps() -> usize {
    undo_all(&MockBackend::from_scenario(&scenario()))
}

fn band(backend: &MockBackend, index: usize) -> ronie_core::EqBand {
    backend
        .chain_state(0)
        .chain
        .block(&ronie_core::BlockId("blk-2".to_owned()))
        .expect("detail block")
        .params
        .eq
        .bands[index]
}

fn eq_phases(backend: &MockBackend) -> Vec<GesturePhase> {
    backend
        .actions()
        .into_iter()
        .filter_map(|action| match action {
            UiAction::SetEq { gesture, .. } => Some(gesture),
            _ => None,
        })
        .collect()
}

fn control_center(driver: &UiDriver, label: &str) -> egui::Pos2 {
    driver
        .controls()
        .iter()
        .find(|control| control.label == label)
        .unwrap_or_else(|| panic!("no {label} in {:?}", driver.visible_labels()))
        .rect
        .center()
}

fn pointer_button(driver: &mut UiDriver, pos: egui::Pos2, pressed: bool) {
    driver.play(vec![egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }]);
}

/// Presses at `offset` from the centre of the control `label`, and moves by `step` in each of
/// `frames` frames. The button stays down.
fn press_and_move(
    driver: &mut UiDriver,
    label: &str,
    offset: egui::Vec2,
    step: egui::Vec2,
    frames: usize,
) -> egui::Pos2 {
    let mut pos = control_center(driver, label) + offset;
    driver.play(vec![egui::Event::PointerMoved(pos)]);
    pointer_button(driver, pos, true);
    for _ in 0..frames {
        pos += step;
        driver.play(vec![egui::Event::PointerMoved(pos)]);
    }
    pos
}

#[test]
fn a_fader_drag_over_twenty_frames_is_one_undo_step() {
    let (mut driver, backend) = eq_page(EqViewMode::Sliders);
    let start = band(&backend, 1).gain_db;

    let pos = press_and_move(
        &mut driver,
        "Band 2 gain",
        egui::Vec2::ZERO,
        egui::vec2(0.0, -2.0),
        20,
    );
    pointer_button(&mut driver, pos, false);
    driver.settle();

    let moved = band(&backend, 1).gain_db;
    assert!(moved > start, "the drag raised the gain: {moved}");
    let phases = eq_phases(&backend);
    assert_eq!(phases.first(), Some(&GesturePhase::Begin), "{phases:?}");
    assert_eq!(phases.last(), Some(&GesturePhase::End), "{phases:?}");
    assert_eq!(
        phases
            .iter()
            .filter(|phase| **phase == GesturePhase::End)
            .count(),
        1,
        "{phases:?}"
    );
    assert_eq!(undo_all(&backend), scenario_steps() + 1);
    assert_eq!(band(&backend, 1).gain_db, start);
}

#[test]
fn a_fader_follows_the_motion_from_the_press_point_and_does_not_jump() {
    let (mut driver, backend) = eq_page(EqViewMode::Sliders);
    let start = band(&backend, 1).gain_db;
    // A press ten points above the centre of the thumb, inside its hit area, and a motion of
    // ten points down. The pointer ends on the centre of the thumb, where a fader that jumps to
    // the pointer shows the start gain.
    let end = press_and_move(
        &mut driver,
        "Band 2 gain",
        egui::vec2(0.0, -10.0),
        egui::vec2(0.0, 2.0),
        5,
    );
    let moved = band(&backend, 1).gain_db - start;
    let most = 10.0 / 225.0 * 30.0;
    assert!(
        moved < 0.0 && -moved <= most + 1.0e-9,
        "the gain follows the motion down from the press point: {moved}"
    );
    pointer_button(&mut driver, end, false);
    driver.settle();
}

#[test]
fn escape_during_a_fader_drag_restores_the_gain_and_keeps_no_undo_step() {
    let (mut driver, backend) = eq_page(EqViewMode::Sliders);
    let start = band(&backend, 1).gain_db;

    let pos = press_and_move(
        &mut driver,
        "Band 2 gain",
        egui::Vec2::ZERO,
        egui::vec2(0.0, -3.0),
        10,
    );
    assert_ne!(band(&backend, 1).gain_db, start, "the drag moved the fader");
    let _ = driver.press_key(egui::Key::Escape);

    assert_eq!(band(&backend, 1).gain_db, start);
    assert_eq!(eq_phases(&backend).last(), Some(&GesturePhase::Cancel));
    assert!(
        driver.chrome().detail().is_some(),
        "the Escape cancels the drag only"
    );
    pointer_button(&mut driver, pos, false);
    driver.settle();
    assert_eq!(band(&backend, 1).gain_db, start);
    assert_eq!(undo_all(&backend), scenario_steps());
}

#[test]
fn a_field_drag_is_one_undo_step() {
    let (mut driver, backend) = eq_page(EqViewMode::Curve);
    let _ = driver
        .click("Band 2 frequency and gain")
        .expect("select band 2");
    let actions = backend.actions().len();
    let start = band(&backend, 1).gain_db;

    let pos = press_and_move(
        &mut driver,
        "Gain",
        egui::Vec2::ZERO,
        egui::vec2(3.0, 0.0),
        12,
    );
    pointer_button(&mut driver, pos, false);
    driver.settle();

    assert_ne!(
        band(&backend, 1).gain_db,
        start,
        "the drag changed the gain"
    );
    let phases: Vec<GesturePhase> = backend.actions()[actions..]
        .iter()
        .filter_map(|action| match action {
            UiAction::SetEq { gesture, .. } => Some(*gesture),
            _ => None,
        })
        .collect();
    assert_eq!(phases.first(), Some(&GesturePhase::Begin), "{phases:?}");
    assert_eq!(phases.last(), Some(&GesturePhase::End), "{phases:?}");
    assert_eq!(undo_all(&backend), scenario_steps() + 1);
    assert_eq!(band(&backend, 1).gain_db, start);
}

/// Types a number into the Q field of band 2, and returns the Q from before it.
fn type_q(driver: &mut UiDriver, backend: &MockBackend, text: &str) -> f64 {
    let _ = driver
        .click("Band 2 frequency and gain")
        .expect("select band 2");
    let start = band(backend, 1).q;
    let _ = driver.fill("Q", text).expect("Q field");
    start
}

#[test]
fn a_typed_field_value_is_one_undo_step() {
    let (mut driver, backend) = eq_page(EqViewMode::Curve);
    let start = type_q(&mut driver, &backend, "2.5");
    let _ = driver.press_key(egui::Key::Enter);

    assert_eq!(band(&backend, 1).q, 2.5);
    assert_ne!(band(&backend, 1).q, start);
    assert_eq!(undo_all(&backend), scenario_steps() + 1);
    assert_eq!(band(&backend, 1).q, start);
}

#[test]
fn escape_in_a_field_restores_the_value_and_keeps_no_undo_step() {
    let (mut driver, backend) = eq_page(EqViewMode::Curve);
    let start = type_q(&mut driver, &backend, "2.5");
    assert_eq!(
        band(&backend, 1).q,
        2.5,
        "the field updates while it is edited"
    );
    let _ = driver.press_key(egui::Key::Escape);

    assert_eq!(band(&backend, 1).q, start);
    assert_eq!(eq_phases(&backend).last(), Some(&GesturePhase::Cancel));
    assert!(driver.chrome().detail().is_some());
    assert_eq!(undo_all(&backend), scenario_steps());
}

#[test]
fn a_dot_drag_that_the_page_hides_ends_before_the_next_edit() {
    let (mut driver, backend) = eq_page(EqViewMode::Curve);
    let start = band(&backend, 1);

    let mut pos = press_and_move(
        &mut driver,
        "Band 2 frequency and gain",
        egui::Vec2::ZERO,
        egui::vec2(4.0, -4.0),
        3,
    );
    assert_eq!(eq_phases(&backend).first(), Some(&GesturePhase::Begin));
    driver
        .chrome_mut()
        .detail_mut()
        .expect("detail")
        .set_view(BlockView::Tone);
    for _ in 0..3 {
        pos += egui::vec2(4.0, -4.0);
        driver.play(vec![egui::Event::PointerMoved(pos)]);
    }
    pointer_button(&mut driver, pos, false);
    driver.settle();
    assert_eq!(eq_phases(&backend).last(), Some(&GesturePhase::End));
    assert_ne!(band(&backend, 1), start, "the drag kept its change");

    backend
        .dispatch(UiAction::SetParameter {
            id: ParameterId("inputLevel"),
            value: ronie_core::params::ParameterValue(0.25),
            gesture: GesturePhase::End,
        })
        .expect("another edit");

    assert_eq!(undo_all(&backend), scenario_steps() + 2);
    assert_eq!(band(&backend, 1), start);
}

#[test]
fn escape_during_a_dot_drag_restores_the_band_and_keeps_no_undo_step() {
    let (mut driver, backend) = eq_page(EqViewMode::Curve);
    let start = band(&backend, 1);

    let pos = press_and_move(
        &mut driver,
        "Band 2 frequency and gain",
        egui::Vec2::ZERO,
        egui::vec2(4.0, -4.0),
        4,
    );
    assert_ne!(band(&backend, 1), start, "the drag moved the band");
    let _ = driver.press_key(egui::Key::Escape);
    pointer_button(&mut driver, pos, false);
    driver.settle();

    assert_eq!(band(&backend, 1), start);
    assert_eq!(eq_phases(&backend).last(), Some(&GesturePhase::Cancel));
    assert!(driver.chrome().detail().is_some());
    assert_eq!(undo_all(&backend), scenario_steps());
}

#[test]
fn a_dropped_detail_ends_its_eq_drag() {
    let (mut driver, backend) = eq_page(EqViewMode::Sliders);
    let _ = press_and_move(
        &mut driver,
        "Band 2 gain",
        egui::Vec2::ZERO,
        egui::vec2(0.0, -3.0),
        3,
    );
    assert_eq!(eq_phases(&backend).first(), Some(&GesturePhase::Begin));

    drop(driver);

    assert_eq!(eq_phases(&backend).last(), Some(&GesturePhase::End));
}

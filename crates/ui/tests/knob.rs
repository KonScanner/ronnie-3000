//! The knob module through its interface: one knob in a headless egui context, real input
//! events, and a value that each edit changes the same way that a backend does.

use eframe::egui;
use ronie_core::params::{ParameterCatalog, ParameterId, ParameterSpec};
use ronie_ui::{
    backend::GesturePhase,
    widgets::{
        KnobEdit, KnobSpec,
        knob::{DRAG_PER_POINT, FINE, show},
    },
};

const RECT: egui::Rect =
    egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(148.0, 175.0));

/// One knob and the value that a backend would hold for it.
///
/// The rig stores each edit the way that the session editor stores it. A catalog parameter
/// becomes an `f32`, stays in its range, and rounds to its choice or to its interval. A block
/// setting, such as a block gain, is an `f64` and stays exact.
struct Rig {
    context: egui::Context,
    spec: KnobSpec,
    /// The catalog parameter of the knob, or `None` for a block setting.
    parameter: Option<&'static ParameterSpec>,
    value: f64,
    /// The value at the start of the open gesture, which a cancel restores.
    start: Option<f64>,
    edits: Vec<KnobEdit>,
    name: String,
    /// The numeric value of the knob in the accessibility tree.
    numeric: Option<f64>,
    /// Whether the frames show the knob.
    visible: bool,
    /// Whether the `Ui` of the knob is enabled.
    enabled: bool,
    /// The clip rect of the `Ui` of the knob, or `None` for the whole screen.
    clip: Option<egui::Rect>,
    /// Whether an Escape press was left in the input after the knob of the last frame.
    escape_left: bool,
    /// Whether the knob had the keyboard focus after the last frame.
    focused: bool,
}

impl Rig {
    fn new(spec: KnobSpec, value: f64) -> Self {
        Self::with_parameter(spec, None, value)
    }

    /// A rig for one catalog parameter, which rounds each edit like the backend.
    fn parameter(id: &'static str, value: f64) -> Self {
        let spec = KnobSpec::parameter(ParameterId(id)).expect("catalog");
        let parameter = ParameterCatalog::v1()
            .iter()
            .find(|parameter| parameter.id.0 == id);
        Self::with_parameter(spec, parameter, value)
    }

    fn with_parameter(
        spec: KnobSpec,
        parameter: Option<&'static ParameterSpec>,
        value: f64,
    ) -> Self {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut rig = Self {
            context,
            spec,
            parameter,
            value,
            start: None,
            edits: Vec::new(),
            name: String::new(),
            numeric: None,
            visible: true,
            enabled: true,
            clip: None,
            escape_left: false,
            focused: false,
        };
        rig.frame(Vec::new());
        rig
    }

    /// The value that the backend keeps for an edit.
    fn stored(&self, value: f64) -> f64 {
        let Some(parameter) = self.parameter else {
            return value;
        };
        let mut next = (value as f32).clamp(parameter.min, parameter.max);
        if !parameter.choices.is_empty() {
            next = next.round();
        } else if let Some(interval) = parameter.interval.filter(|interval| *interval > 0.0) {
            next = (next / interval).round() * interval;
        }
        f64::from(next)
    }

    fn id() -> egui::Id {
        egui::Id::unique("knob")
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            events,
            ..Default::default()
        };
        let (spec, value, visible) = (self.spec.clone(), self.value, self.visible);
        let (enabled, clip) = (self.enabled, self.clip);
        let mut edit = None;
        let mut escape_left = false;
        let mut output = self.context.run_ui(input, |ui| {
            if visible {
                let mut knob_ui = ui.new_child(egui::UiBuilder::new());
                if !enabled {
                    knob_ui.disable();
                }
                if let Some(clip) = clip {
                    knob_ui.set_clip_rect(clip);
                }
                edit = show(&mut knob_ui, Self::id(), RECT, &spec, "Gain", value).edit;
            }
            escape_left = ui.input(|input| input.key_pressed(egui::Key::Escape));
        });
        output.textures_delta.clear();
        self.escape_left = escape_left;
        self.focused = self.context.memory(|memory| memory.has_focus(Self::id()));
        if let Some(edit) = edit {
            assert!(edit.value.is_finite(), "{edit:?}");
            self.edits.push(edit);
            match edit.phase {
                GesturePhase::Begin => {
                    self.start = Some(self.value);
                    self.value = self.stored(edit.value);
                }
                GesturePhase::Change => self.value = self.stored(edit.value),
                GesturePhase::End => {
                    self.start = None;
                    self.value = self.stored(edit.value);
                }
                GesturePhase::Cancel => {
                    self.value = self.start.take().expect("an open gesture");
                }
            }
        }
        if let Some(update) = output.platform_output.accesskit_update.take()
            && let Some(node) = update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label().is_some_and(|label| label.starts_with("Gain:")))
        {
            self.name = node.label().unwrap_or_default().to_owned();
            self.numeric = node.numeric_value();
        }
    }

    fn modifiers(&mut self, modifiers: egui::Modifiers) {
        self.frame(vec![egui::Event::ModifiersChanged(modifiers)]);
    }

    fn button(&mut self, pos: egui::Pos2, pressed: bool) {
        self.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }

    fn click(&mut self) {
        let center = RECT.center();
        self.frame(vec![egui::Event::PointerMoved(center)]);
        self.button(center, true);
        self.button(center, false);
    }

    fn double_click(&mut self) {
        self.click();
        let center = RECT.center();
        self.button(center, true);
        self.button(center, false);
        self.frame(Vec::new());
    }

    /// Presses on the knob and moves up by `step` points in each of `count` frames.
    fn drag_up(&mut self, step: f32, count: usize) -> egui::Pos2 {
        let mut pos = RECT.center();
        self.frame(vec![egui::Event::PointerMoved(pos)]);
        self.button(pos, true);
        for _ in 0..count {
            pos.y -= step;
            self.frame(vec![egui::Event::PointerMoved(pos)]);
        }
        pos
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.frame(vec![event(true)]);
        self.frame(vec![event(false)]);
    }

    fn focus(&mut self) {
        self.context
            .memory_mut(|memory| memory.request_focus(Self::id()));
        self.frame(Vec::new());
        self.frame(Vec::new());
    }

    fn type_text(&mut self, text: &str) {
        self.frame(vec![egui::Event::Text(text.to_owned())]);
    }

    fn phases(&self) -> Vec<GesturePhase> {
        self.edits.iter().map(|edit| edit.phase).collect()
    }
}

/// Whether two values are equal to the precision of an `f32`, which a parameter has.
fn close(left: f64, right: f64) -> bool {
    (left - right).abs() < 1.0e-6 * right.abs().max(1.0)
}

#[test]
fn a_drag_is_one_begin_then_a_change_for_each_move_then_one_end() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    let end = rig.drag_up(10.0, 3);
    rig.button(end, false);

    assert_eq!(
        rig.phases(),
        [
            GesturePhase::Begin,
            GesturePhase::Change,
            GesturePhase::Change,
            GesturePhase::End
        ]
    );
    assert!(
        close(rig.edits[0].value, 0.5 + 10.0 * DRAG_PER_POINT),
        "the Begin carries the first move: {:?}",
        rig.edits
    );
    assert!(close(rig.value, 0.5 + 30.0 * DRAG_PER_POINT));
}

#[test]
fn a_still_pointer_during_a_drag_sends_no_edit() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    let pos = rig.drag_up(10.0, 1);
    for _ in 0..3 {
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
    }
    rig.button(pos, false);
    assert_eq!(rig.phases(), [GesturePhase::Begin, GesturePhase::End]);
}

#[test]
fn shift_makes_a_drag_eight_times_finer() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.modifiers(egui::Modifiers::SHIFT);
    let end = rig.drag_up(10.0, 2);
    rig.button(end, false);
    assert!(
        close(rig.value, 0.5 + 20.0 * DRAG_PER_POINT / FINE),
        "{}",
        rig.value
    );
}

#[test]
fn escape_during_a_drag_cancels_it_and_the_rest_of_the_drag_sends_nothing() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    let mut pos = rig.drag_up(10.0, 2);
    rig.key(egui::Key::Escape, egui::Modifiers::NONE);
    pos.y -= 20.0;
    rig.frame(vec![egui::Event::PointerMoved(pos)]);
    rig.button(pos, false);

    assert_eq!(
        rig.phases(),
        [
            GesturePhase::Begin,
            GesturePhase::Change,
            GesturePhase::Cancel
        ]
    );
    assert_eq!(rig.edits[2].value, 0.5, "the cancel carries the start");
    assert_eq!(rig.value, 0.5);
}

#[test]
fn alt_click_resets_in_one_edit_and_a_reset_value_needs_no_edit() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.75);
    rig.modifiers(egui::Modifiers::ALT);
    rig.click();
    assert_eq!(
        rig.edits,
        [KnobEdit {
            value: 0.5,
            phase: GesturePhase::End
        }]
    );
    rig.click();
    assert_eq!(rig.edits.len(), 1, "no edit that keeps the value");
}

#[test]
fn a_parameter_knob_resets_to_its_catalog_default() {
    let mut rig = Rig::parameter("gateThreshold", -40.0);
    rig.modifiers(egui::Modifiers::ALT);
    rig.click();
    assert_eq!(rig.value, -80.0);
}

#[test]
fn a_caller_reset_value_replaces_the_default() {
    let mut rig = Rig::new(KnobSpec::percent(1.0).with_reset(0.25), 0.5);
    rig.modifiers(egui::Modifiers::ALT);
    rig.click();
    assert_eq!(rig.value, 0.25);
}

#[test]
fn a_plain_click_changes_nothing() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.75);
    rig.click();
    assert!(rig.edits.is_empty(), "{:?}", rig.edits);
}

#[test]
fn arrow_keys_step_the_focused_knob_and_each_press_is_one_edit() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.key(egui::Key::ArrowUp, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "a knob without focus takes no key");

    rig.focus();
    rig.key(egui::Key::ArrowUp, egui::Modifiers::NONE);
    rig.key(egui::Key::ArrowRight, egui::Modifiers::NONE);
    rig.key(egui::Key::ArrowLeft, egui::Modifiers::NONE);
    rig.key(egui::Key::ArrowDown, egui::Modifiers::SHIFT);

    assert_eq!(rig.phases(), [GesturePhase::End; 4]);
    let db = 1.0 / 48.0;
    let values: Vec<f64> = rig.edits.iter().map(|edit| edit.value).collect();
    for (value, expected) in
        values
            .iter()
            .zip([0.5 + db, 0.5 + 2.0 * db, 0.5 + db, 0.5 + db - db / FINE])
    {
        assert!(close(*value, expected), "{values:?}");
    }
    assert_eq!(rig.name, "Gain: +0.9 dB");
}

#[test]
fn home_and_end_go_to_the_ends_and_a_key_at_an_end_sends_nothing() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.focus();
    rig.key(egui::Key::End, egui::Modifiers::NONE);
    assert_eq!(rig.value, 1.0);
    rig.key(egui::Key::ArrowUp, egui::Modifiers::NONE);
    assert_eq!(rig.edits.len(), 1, "no step past the end");
    rig.key(egui::Key::Home, egui::Modifiers::NONE);
    assert_eq!(rig.value, 0.0);
}

#[test]
fn the_default_key_step_is_one_hundredth_of_the_range() {
    let mut rig = Rig::parameter("toneMid", 5.0);
    rig.focus();
    rig.key(egui::Key::ArrowUp, egui::Modifiers::NONE);
    assert!(close(rig.value, 5.1), "{}", rig.value);
}

#[test]
fn a_knob_with_a_limited_travel_stays_inside_it() {
    let mut rig = Rig::parameter("chainPanLeft", 0.5);
    rig.spec = rig.spec.clone().with_travel(0.0, 0.5);
    rig.focus();
    rig.key(egui::Key::ArrowUp, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "{:?}", rig.edits);
    rig.key(egui::Key::End, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "End is the end of the travel");
    let end = rig.drag_up(-30.0, 1);
    rig.button(end, false);
    assert!(rig.value < 0.5);
}

#[test]
fn a_double_click_opens_type_in_and_enter_commits_one_edit() {
    let mut rig = Rig::parameter("inputLevel", 0.5);
    rig.double_click();
    assert!(rig.edits.is_empty(), "opening the field is no edit");

    rig.type_text("-12");
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);

    assert_eq!(
        rig.edits,
        [KnobEdit {
            value: 0.25,
            phase: GesturePhase::End
        }]
    );
    assert_eq!(rig.name, "Gain: -12.0 dB");
    rig.key(egui::Key::ArrowUp, egui::Modifiers::NONE);
    assert_eq!(rig.edits.len(), 1, "the field closed");
}

#[test]
fn escape_closes_type_in_with_no_edit() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.double_click();
    rig.type_text("6");
    rig.key(egui::Key::Escape, egui::Modifiers::NONE);
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "{:?}", rig.edits);
    assert_eq!(rig.value, 0.5);
}

#[test]
fn a_typed_text_that_is_no_number_changes_nothing() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.double_click();
    rig.type_text("loud");
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "{:?}", rig.edits);
}

#[test]
fn the_accessible_name_is_the_label_and_the_value_text() {
    let rig = Rig::new(KnobSpec::block_gain(), 0.625);
    assert_eq!(rig.name, "Gain: +6.0 dB");
}

#[test]
fn typed_values_use_the_unit_that_the_knob_shows() {
    let spec = |id| KnobSpec::parameter(ParameterId(id)).expect("catalog");
    assert_eq!(spec("pitchSemitones").parse("3"), Some(3.0));
    assert_eq!(spec("pitchSemitones").format(3.0), "+3 st");
    let near =
        |value: Option<f64>, expected: f64| value.is_some_and(|value| close(value, expected));
    assert!(near(spec("gateRelease").parse("120 ms"), 120.0));
    assert!(near(spec("pitchTonality").parse("5"), 5000.0));
    assert_eq!(spec("pitchTonality").parse("off"), Some(20_000.0));
    assert_eq!(spec("pitchWindow").parse("40"), Some(2.0));
    assert_eq!(spec("pitchWindow").parse("50"), None, "no such choice");
    assert_eq!(spec("spreadOffset").parse("12 L"), Some(0.25));
    assert_eq!(spec("spreadOffset").parse("-12"), Some(0.25));
    assert_eq!(spec("spreadOffset").parse("12 R"), Some(0.75));
    assert_eq!(spec("inputLevel").parse("+48"), Some(1.0), "the range end");
}

#[test]
fn the_crossover_shows_the_frequency_of_the_engine() {
    let spec = KnobSpec::parameter(ParameterId("spreadCrossover")).expect("catalog");
    assert_eq!(spec.format(0.0), "32 Hz");
    assert_eq!(spec.format(0.5), "130 Hz");
    assert_eq!(spec.format(1.0), "520 Hz");
    let typed = spec.parse("130 Hz").expect("number");
    assert!(close(typed, 0.5), "{typed}");
}

#[test]
fn every_catalog_parameter_shows_a_number_and_reads_its_own_text_back() {
    for parameter in ParameterCatalog::v1() {
        let spec = KnobSpec::parameter(parameter.id).expect("catalog");
        let (min, max) = (f64::from(parameter.min), f64::from(parameter.max));
        for value in [min, f64::from(parameter.default), (min + max) / 2.0, max] {
            let text = spec.format(value);
            let position = spec.position(value);
            assert!(
                position.is_finite() && (0.0..=1.0).contains(&position),
                "{}: position {position}",
                parameter.id.0
            );
            for bad in ["NaN", "nan", "inf"] {
                assert!(
                    !text.contains(bad),
                    "{} at {value} shows {text:?}",
                    parameter.id.0
                );
            }
            let read = spec
                .parse(&text)
                .unwrap_or_else(|| panic!("{} cannot read {text:?}", parameter.id.0));
            assert_eq!(spec.format(read), text, "{} at {value}", parameter.id.0);
        }
    }
}

#[test]
fn only_a_knob_with_a_neutral_centre_is_bipolar() {
    let bipolar = |id| {
        KnobSpec::parameter(ParameterId(id))
            .expect("catalog")
            .bipolar()
    };
    for id in [
        "pitchSemitones",
        "spreadOffset",
        "alignOffset",
        "outputBalance",
    ] {
        assert!(bipolar(id), "{id}");
    }
    for id in ["inputLevel", "outputLevel", "toneBass", "gateThreshold"] {
        assert!(!bipolar(id), "{id}");
    }
    assert!(!KnobSpec::block_gain().bipolar());
}

fn escape() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn a_drag_on_a_choice_parameter_moves_it_over_several_frames() {
    // Four choices, so one choice is a third of the range, or 55.6 points of drag. Each frame
    // moves 10 points, and the backend rounds each frame back to a choice.
    let mut rig = Rig::parameter("pitchWindow", 1.0);
    let end = rig.drag_up(10.0, 8);
    rig.button(end, false);
    assert_eq!(rig.value, 2.0, "{:?}", rig.edits);
    assert_eq!(rig.phases().first(), Some(&GesturePhase::Begin));
    assert_eq!(rig.phases().last(), Some(&GesturePhase::End));
}

#[test]
fn a_choice_knob_shows_and_sends_only_choices_during_a_drag() {
    // One choice is 55.6 points of drag. The drag moves a little more than half of one choice.
    let mut rig = Rig::parameter("pitchWindow", 1.0);
    let mut pos = rig.drag_up(5.0, 1);
    for _ in 0..6 {
        pos.y -= 5.0;
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
        let numeric = rig.numeric.expect("accessible value");
        assert_eq!(
            numeric,
            numeric.round(),
            "the face shows a choice: {numeric}"
        );
    }
    assert!(
        rig.edits
            .iter()
            .all(|edit| edit.value == edit.value.round()),
        "each edit is a choice: {:?}",
        rig.edits
    );
    assert_eq!(rig.numeric, Some(2.0), "the nearest choice");
    assert_eq!(rig.name, format!("Gain: {}", rig.spec.format(2.0)));
    rig.button(pos, false);
    assert_eq!(rig.value, 2.0);
}

#[test]
fn a_knob_with_an_interval_holds_only_its_multiples() {
    let pitch = KnobSpec::parameter(ParameterId("pitchSemitones")).expect("catalog");
    assert!(close(pitch.parse("2.4").expect("typed pitch"), 2.4));
    let whole = pitch.with_interval(1.0);
    assert_eq!(whole.parse("2.4"), Some(2.0));
    assert_eq!(whole.parse("-2.6"), Some(-3.0));
    assert_eq!(whole.format(2.0), "+2 st");

    let mut rig = Rig::parameter("pitchSemitones", 0.0);
    rig.spec = rig.spec.clone().with_interval(1.0);
    let mut pos = rig.drag_up(5.0, 1);
    for _ in 0..10 {
        pos.y -= 5.0;
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
    }
    rig.button(pos, false);
    assert!(rig.value > 0.0, "the drag raised the pitch");
    assert!(
        rig.edits
            .iter()
            .all(|edit| edit.value == edit.value.round()),
        "each edit is a whole semitone: {:?}",
        rig.edits
    );
}

#[test]
fn a_slow_shift_drag_keeps_its_motion_when_the_backend_rounds_each_frame() {
    let mut rig = Rig::parameter("inputLevel", 0.5);
    rig.modifiers(egui::Modifiers::SHIFT);
    let mut pos = rig.drag_up(10.0, 1);
    // Each of these frames moves less than half of the interval of the parameter.
    for _ in 0..40 {
        pos.y -= 0.05;
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
    }
    rig.button(pos, false);
    let expected = 0.5 + 12.0 * DRAG_PER_POINT / FINE;
    assert!(
        (rig.value - expected).abs() < 0.0002,
        "{} and not {expected}",
        rig.value
    );
}

#[test]
fn a_knob_that_a_frame_hides_forgets_its_drag_and_sends_no_late_end() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    let mut pos = rig.drag_up(10.0, 2);
    rig.visible = false;
    pos.y -= 10.0;
    rig.frame(vec![egui::Event::PointerMoved(pos)]);
    rig.visible = true;
    for _ in 0..2 {
        pos.y -= 10.0;
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
    }
    rig.button(pos, false);
    rig.frame(Vec::new());

    assert_eq!(
        rig.phases(),
        [GesturePhase::Begin, GesturePhase::Change],
        "the owner closes the gesture, and the knob sends nothing more for the old drag"
    );
    let end = rig.drag_up(10.0, 1);
    rig.button(end, false);
    assert_eq!(
        rig.phases()[2..],
        [GesturePhase::Begin, GesturePhase::End],
        "a new drag works"
    );
}

#[test]
fn escape_during_a_drag_is_used_by_the_cancel() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    let _ = rig.drag_up(10.0, 2);
    rig.frame(vec![escape()]);
    assert_eq!(rig.phases().last(), Some(&GesturePhase::Cancel));
    assert!(
        !rig.escape_left,
        "a popover under the knob must not see the press"
    );
}

#[test]
fn escape_in_the_field_is_used_by_the_field() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.double_click();
    // A new field takes the focus in the frame after its first frame.
    rig.frame(Vec::new());
    rig.frame(vec![escape()]);
    assert!(
        !rig.escape_left,
        "a popover under the knob must not see the press"
    );
    rig.type_text("6");
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "the field closed: {:?}", rig.edits);
}

#[test]
fn a_field_that_a_frame_hides_closes_and_the_knob_takes_a_drag() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.double_click();
    rig.visible = false;
    rig.frame(Vec::new());
    rig.visible = true;
    rig.frame(Vec::new());

    rig.type_text("6");
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "the field closed: {:?}", rig.edits);
    let end = rig.drag_up(10.0, 1);
    rig.button(end, false);
    assert_eq!(rig.phases(), [GesturePhase::Begin, GesturePhase::End]);
}

#[test]
fn a_field_that_loses_the_focus_to_another_control_closes() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.double_click();
    rig.frame(Vec::new());
    rig.context
        .memory_mut(|memory| memory.request_focus(egui::Id::unique("another control")));
    rig.frame(Vec::new());
    rig.frame(Vec::new());

    rig.type_text("6");
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "{:?}", rig.edits);
    let end = rig.drag_up(10.0, 1);
    rig.button(end, false);
    assert_eq!(rig.phases(), [GesturePhase::Begin, GesturePhase::End]);
}

#[test]
fn a_click_gives_the_focus_and_the_arrow_keys_then_step_the_knob() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.click();
    assert!(rig.focused);
    rig.key(egui::Key::ArrowRight, egui::Modifiers::NONE);
    assert_eq!(
        rig.edits,
        [KnobEdit {
            value: 0.5 + 1.0 / 48.0,
            phase: GesturePhase::End
        }]
    );
}

#[test]
fn a_drag_gives_the_focus() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    let end = rig.drag_up(10.0, 1);
    rig.button(end, false);
    assert!(rig.focused);
}

#[test]
fn one_arrow_press_on_the_pitch_is_one_semitone() {
    let mut rig = Rig::parameter("pitchSemitones", 0.0);
    rig.focus();
    rig.key(egui::Key::ArrowRight, egui::Modifiers::NONE);
    assert!(close(rig.value, 1.0), "{}", rig.value);
    assert_eq!(rig.name, "Gain: +1 st");
    rig.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
    assert!(close(rig.value, 1.0 + 1.0 / FINE), "{}", rig.value);
}

#[test]
fn a_value_that_is_not_finite_shows_the_reset_and_sends_no_edit() {
    let mut rig = Rig::new(KnobSpec::block_gain(), f64::NAN);
    for _ in 0..3 {
        rig.frame(Vec::new());
    }
    assert!(rig.edits.is_empty(), "{:?}", rig.edits);
    assert_eq!(rig.name, "Gain: 0.0 dB");
    rig.modifiers(egui::Modifiers::ALT);
    rig.click();
    assert!(rig.edits.is_empty(), "the reset value shows already");
    rig.modifiers(egui::Modifiers::NONE);
    rig.focus();
    rig.key(egui::Key::ArrowUp, egui::Modifiers::NONE);
    assert_eq!(rig.edits.len(), 1);
    assert!(close(rig.value, 0.5 + 1.0 / 48.0), "{}", rig.value);
}

#[test]
fn a_builder_value_that_is_not_finite_changes_nothing() {
    let spec = KnobSpec::block_gain();
    assert_eq!(spec.clone().with_travel(f64::NAN, 0.5), spec);
    assert_eq!(spec.clone().with_travel(0.0, f64::INFINITY), spec);
    assert_eq!(spec.clone().with_key_step(f64::NAN), spec);
    assert_eq!(spec.clone().with_reset(f64::NAN), spec);
}

#[test]
fn the_accessible_value_is_the_number_of_the_shown_value() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.625);
    assert_eq!(rig.numeric, Some(0.625));
    rig.focus();
    rig.key(egui::Key::End, egui::Modifiers::NONE);
    assert_eq!(rig.numeric, Some(1.0));
}

#[test]
fn a_disabled_knob_closes_its_open_field_and_commits_nothing() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.double_click();
    rig.frame(Vec::new());
    rig.type_text("6");

    rig.enabled = false;
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "{:?}", rig.edits);

    rig.enabled = true;
    rig.type_text("6");
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "the field closed: {:?}", rig.edits);
    assert_eq!(rig.value, 0.5);
}

#[test]
fn a_field_of_a_knob_that_the_view_clips_closes_with_no_edit() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.double_click();
    rig.frame(Vec::new());
    rig.type_text("6");

    // The view scrolls, and its clip rect now cuts the right side of the knob.
    rig.clip = Some(egui::Rect::from_min_max(
        egui::Pos2::ZERO,
        egui::pos2(RECT.center().x, 300.0),
    ));
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "{:?}", rig.edits);

    rig.clip = None;
    rig.type_text("6");
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(rig.edits.is_empty(), "the field closed: {:?}", rig.edits);
    let end = rig.drag_up(10.0, 1);
    rig.button(end, false);
    assert_eq!(rig.phases(), [GesturePhase::Begin, GesturePhase::End]);
}

#[test]
fn an_open_field_inside_the_view_still_commits() {
    let mut rig = Rig::new(KnobSpec::block_gain(), 0.5);
    rig.clip = Some(RECT.expand(40.0));
    rig.double_click();
    rig.type_text("6");
    rig.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(
        rig.edits,
        [KnobEdit {
            value: 0.625,
            phase: GesturePhase::End
        }]
    );
}

#[test]
fn a_blend_knob_shows_the_share_of_each_source_and_reads_the_share_of_a() {
    let spec = KnobSpec::blend();
    assert_eq!(spec.format(0.5), "50 / 50");
    assert_eq!(spec.format(0.35), "65 / 35");
    assert_eq!(spec.format(0.0), "100 / 0");
    for (text, blend) in [("65", 0.35), ("80 / 20", 0.2)] {
        let read = spec.parse(text).expect("a number");
        assert!((read - blend).abs() < 1e-9, "{text} read as {read}");
    }
}

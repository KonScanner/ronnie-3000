//! The one knob of the UI: value mapping, gestures, type-in, and the accessible name.
//!
//! A caller describes the range of a knob with a [`KnobSpec`], gives the value that the backend
//! holds, and calls [`show`] once in each frame. The knob reads the pointer and the keys, and
//! gives back at most one [`KnobEdit`] in each frame. The caller sends that edit to the backend
//! and paints the face at [`KnobOutput::shown`]. Each knob follows the same rules:
//!
//! - A vertical drag moves the value. Shift makes the drag [`FINE`] times slower. The knob adds
//!   up the motion of the whole drag, so a backend that rounds the value loses no motion.
//! - A double-click opens a small field. Enter commits the typed value, and Escape closes it.
//! - An Alt-click sets the reset value.
//! - A click or a drag gives the knob the keyboard focus. With the focus, Up and Right raise the
//!   value, and Down and Left lower it. Shift makes the step [`FINE`] times smaller. Home and End
//!   go to the ends of the range.
//! - Escape during a drag sends [`GesturePhase::Cancel`], and the backend restores the value
//!   from before the drag.
//! - The knob uses an Escape that cancels a drag or closes the field, so that the same press does
//!   not also close a popover or a page.
//!
//! The phases make the undo steps. A drag is `Begin`, then `Change` for each frame that moves the
//! value, then `End`, and the backend keeps it as one undo step. The `Begin` carries the value of
//! its frame, so the first motion is not lost. A reset, a key press, and a typed value are each
//! one `End` with no gesture open, and so each one is one undo step. The knob sends no edit that
//! keeps the value, so a still pointer or a key at the end of the range makes no undo step.
//!
//! A view can stop showing a knob during its drag. The knob then forgets the drag and sends no
//! more edits for it, and the owner of the knob must close the gesture on the backend itself.
//! `OpenDrag` keeps that record for an owner in this crate.

use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use ronie_core::params::{ParameterCatalog, ParameterId, ParameterSpec, ParameterValue};

use crate::backend::{GesturePhase, UiAction, UiBackend, UiError};

/// The change of the value for one point of vertical drag, as a fraction of the range.
pub const DRAG_PER_POINT: f64 = 0.006;
/// How many times slower a Shift drag is, and how many times smaller a Shift key step is.
pub const FINE: f64 = 8.0;
/// The default key step, as a fraction of the range.
pub const DEFAULT_KEY_STEP: f64 = 0.01;
/// The range of a block gain knob on each side of 0 dB.
pub const GAIN_RANGE_DB: f64 = 24.0;
/// The range of the output balance knob on each side of 0 dB.
const BALANCE_RANGE_DB: f64 = 12.0;
/// The size of the type-in field.
const FIELD_SIZE: Vec2 = Vec2::new(72.0, 22.0);
/// The pitch shift parameter, in semitones.
const PITCH_SEMITONES: &str = "pitchSemitones";
/// The parameter that makes the pitch shift use whole semitones.
const PITCH_STEP: &str = "pitchStep";

/// How the position of a knob shows as text, and how a typed number becomes a position.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Scale {
    Percent,
    /// A gain in decibels, ±24 dB with 0 dB at the centre.
    GainDb,
    /// A balance in decibels, ±12 dB with 0 dB at the centre.
    BalanceDb,
    Tone,
    GateDb,
    Semitones,
    SidedMs,
    /// The real value of the parameter with a unit and a number of decimals.
    Real(&'static str, usize),
    Tonality,
    Buffer,
    Crossover,
    Pan,
}

impl Scale {
    fn for_id(id: &str) -> Self {
        match id {
            "inputLevel" | "outputLevel" => Self::GainDb,
            "outputBalance" => Self::BalanceDb,
            "toneBass" | "toneMid" | "toneTreble" => Self::Tone,
            "gateThreshold" => Self::GateDb,
            "pitchSemitones" => Self::Semitones,
            "spreadOffset" | "alignOffset" => Self::SidedMs,
            "pitchTonality" => Self::Tonality,
            "pitchWindow" => Self::Buffer,
            "spreadCrossover" | "alignCrossover" => Self::Crossover,
            "gateRelease" | "gateHold" => Self::Real("ms", 0),
            "gateRange" => Self::Real("dB", 0),
            "chainPanLeft" | "chainPanRight" => Self::Pan,
            _ => Self::Percent,
        }
    }

    /// Whether the arc of the face starts at the centre.
    ///
    /// A knob with a centre that means "none", such as a balance or a side offset, is bipolar.
    /// A gain knob is not, because its arc shows the level from the bottom of the range.
    fn bipolar(self) -> bool {
        matches!(self, Self::BalanceDb | Self::Semitones | Self::SidedMs)
    }

    /// The range in decibels on each side of the centre of a decibel scale.
    fn db_range(self) -> f64 {
        if self == Self::BalanceDb {
            BALANCE_RANGE_DB
        } else {
            GAIN_RANGE_DB
        }
    }
}

/// The range of the value that the backend stores.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Stored {
    /// A block setting from 0 to 1, where the stored value is the position.
    Unit,
    /// A catalog parameter. The catalog owns the rule between the position and the value, so
    /// the knob and the host use one rule.
    Catalog(&'static ParameterSpec),
}

impl Stored {
    fn position(self, value: f64) -> f64 {
        match self {
            Self::Unit if value.is_finite() => value.clamp(0.0, 1.0),
            Self::Unit => 0.0,
            Self::Catalog(spec) => f64::from(spec.position_of(value as f32)),
        }
    }

    fn value(self, position: f64) -> f64 {
        match self {
            Self::Unit => position.clamp(0.0, 1.0),
            Self::Catalog(spec) => f64::from(spec.value_at(position as f32)),
        }
    }
}

/// The description of one knob: its range, its text, its reset value, and its key step.
#[derive(Debug, Clone, PartialEq)]
pub struct KnobSpec {
    scale: Scale,
    stored: Stored,
    choices: &'static [&'static str],
    reset: f64,
    key_step: f64,
    travel: (f64, f64),
    /// The interval of the stored values, when the knob holds only multiples of it.
    interval: Option<f64>,
}

impl KnobSpec {
    /// The knob of one catalog parameter. The reset value is the catalog default.
    ///
    /// Returns `None` for an ID that is not in the catalog.
    #[must_use]
    pub fn parameter(id: ParameterId) -> Option<Self> {
        ParameterCatalog::v1()
            .iter()
            .find(|spec| spec.id == id)
            .map(Self::from_catalog)
    }

    fn from_catalog(spec: &'static ParameterSpec) -> Self {
        let choices = spec.choices;
        let scale = Scale::for_id(spec.id.0);
        let (min, max) = (f64::from(spec.min), f64::from(spec.max));
        Self {
            scale,
            stored: Stored::Catalog(spec),
            choices,
            reset: f64::from(spec.default),
            key_step: if choices.len() > 1 {
                // One step of a choice parameter is one choice, because a smaller step rounds
                // back.
                1.0 / (choices.len() - 1) as f64
            } else if scale == Scale::Semitones && max > min {
                // One step of a pitch is one semitone, the unit that the knob shows.
                1.0 / (max - min)
            } else {
                DEFAULT_KEY_STEP
            },
            travel: (0.0, 1.0),
            interval: None,
        }
    }

    /// A gain knob of a block: a value from 0 to 1 that shows ±24 dB, with 0 dB at 0.5.
    ///
    /// The reset value is 0 dB, and one key step is 1 dB.
    #[must_use]
    pub fn block_gain() -> Self {
        Self {
            scale: Scale::GainDb,
            stored: Stored::Unit,
            choices: &[],
            reset: 0.5,
            key_step: 1.0 / (2.0 * GAIN_RANGE_DB),
            travel: (0.0, 1.0),
            interval: None,
        }
    }

    /// A knob for a value from 0 to 1 that shows as a percentage.
    #[must_use]
    pub fn percent(reset: f64) -> Self {
        Self {
            scale: Scale::Percent,
            stored: Stored::Unit,
            choices: &[],
            reset,
            key_step: DEFAULT_KEY_STEP,
            travel: (0.0, 1.0),
            interval: None,
        }
    }

    /// The same knob with another reset value, in the units that the backend stores.
    ///
    /// A value that is not finite changes nothing, and the knob keeps its reset value.
    #[must_use]
    pub fn with_reset(mut self, reset: f64) -> Self {
        if reset.is_finite() {
            self.reset = reset;
        }
        self
    }

    /// The same knob with another key step, as a fraction of the range.
    ///
    /// A step that is not finite changes nothing, and the knob keeps its key step.
    #[must_use]
    pub fn with_key_step(mut self, step: f64) -> Self {
        if step.is_finite() {
            self.key_step = step;
        }
        self
    }

    /// The same knob with its travel limited to a part of the range, as fractions of the range.
    ///
    /// A pan knob of one lane uses one half of the range. An end that is not finite changes
    /// nothing, and the knob keeps its travel.
    #[must_use]
    pub fn with_travel(mut self, low: f64, high: f64) -> Self {
        if low.is_finite() && high.is_finite() {
            let low = low.clamp(0.0, 1.0);
            self.travel = (low, high.clamp(low, 1.0));
        }
        self
    }

    /// The same knob, which holds only multiples of `interval` in the units that the backend
    /// stores. A drag, a key, and a typed value each give the nearest multiple.
    ///
    /// The pitch knob with STEP on uses an interval of one semitone. An interval that is not
    /// finite or not above zero changes nothing.
    #[must_use]
    pub fn with_interval(mut self, interval: f64) -> Self {
        if interval.is_finite() && interval > 0.0 {
            self.interval = Some(interval);
        }
        self
    }

    /// The position of a stored value, from 0 at the start of the range to 1 at its end.
    #[must_use]
    pub fn position(&self, value: f64) -> f64 {
        self.stored.position(value)
    }

    /// Whether the arc of the face starts at the centre of the range.
    #[must_use]
    pub fn bipolar(&self) -> bool {
        self.scale.bipolar()
    }

    /// The text of a stored value, for example "+6.0 dB", "50%", or "120 Hz".
    #[must_use]
    pub fn format(&self, value: f64) -> String {
        let position = self.position(value);
        let real = self.stored.value(position);
        match self.scale {
            Scale::Percent => format!("{:.0}%", position * 100.0),
            Scale::GainDb | Scale::BalanceDb => {
                format_db((position - 0.5) * 2.0 * self.scale.db_range())
            }
            Scale::Tone => format!("{:.1}", position * 10.0),
            Scale::GateDb => format!("{:.0} dB", -100.0 + position * 100.0),
            Scale::Semitones => ronie_core::tuning::format_semitones(-24.0 + position * 48.0),
            Scale::SidedMs => sided(
                (position - 0.5) * 48.0,
                |magnitude| format!("{magnitude:.1} ms"),
                "0 ms",
            ),
            Scale::Real(unit, decimals) => format!("{real:.decimals$} {unit}"),
            Scale::Tonality if position >= 1.0 - 1.0e-6 => "Off".to_owned(),
            Scale::Tonality => format!("{:.1} kHz", real / 1000.0),
            Scale::Buffer => self
                .choices
                .get(real.round().max(0.0) as usize)
                .map_or_else(|| format!("{real:.0}"), |choice| (*choice).to_owned()),
            Scale::Crossover => format!("{:.0} Hz", crossover_hz(position)),
            Scale::Pan => sided(
                (position - 0.5) * 200.0,
                |magnitude| format!("{magnitude:.0}%"),
                "C",
            ),
        }
    }

    /// The stored value of a typed text, or `None` when the text is not a value of this knob.
    ///
    /// The number is in the unit that the knob shows, and the unit itself is optional. A side
    /// knob takes a sign or an "L" or "R" after the number. A value outside the travel goes to
    /// the nearest end.
    #[must_use]
    pub fn parse(&self, text: &str) -> Option<f64> {
        let text = text.trim();
        if self.scale == Scale::Tonality && text.eq_ignore_ascii_case("off") {
            return Some(self.limit(1.0));
        }
        if self.scale == Scale::Pan && text.eq_ignore_ascii_case("c") {
            return Some(self.limit(0.5));
        }
        let (number, rest) = leading_number(text)?;
        let side = if rest.to_ascii_uppercase().contains('L') {
            -number.abs()
        } else if rest.to_ascii_uppercase().contains('R') {
            number.abs()
        } else {
            number
        };
        let position = match self.scale {
            Scale::Percent => number / 100.0,
            Scale::GainDb | Scale::BalanceDb => number / (2.0 * self.scale.db_range()) + 0.5,
            Scale::Tone => number / 10.0,
            Scale::GateDb => (number + 100.0) / 100.0,
            Scale::Semitones => (number + 24.0) / 48.0,
            Scale::SidedMs => side / 48.0 + 0.5,
            Scale::Pan => side / 200.0 + 0.5,
            Scale::Real(..) => self.stored.position(number),
            Scale::Tonality => self.stored.position(number * 1000.0),
            Scale::Buffer => {
                let index = self.choices.iter().position(|choice| {
                    leading_number(choice).is_some_and(|(value, _)| value == number)
                })?;
                self.stored.position(index as f64)
            }
            Scale::Crossover => {
                let (low, high) = (crossover_hz(0.0), crossover_hz(1.0));
                (number / low).ln() / (high / low).ln()
            }
        };
        position
            .is_finite()
            .then(|| self.limit(position.clamp(0.0, 1.0)))
    }

    /// The text that the type-in field starts with: the number of the value, with no unit.
    fn edit_text(&self, value: f64) -> String {
        let text = self.format(value);
        match self.scale {
            Scale::Tonality if text == "Off" => text,
            Scale::SidedMs | Scale::Pan => {
                let number = leading_number(&text).map_or(0.0, |(number, _)| number);
                let signed = if text.ends_with('L') { -number } else { number };
                format!("{signed}")
            }
            _ => leading_number(&text)
                .map(|(_, rest)| text[..text.len() - rest.len()].trim().to_owned())
                .unwrap_or(text),
        }
    }

    /// The stored value at a position, held inside the travel.
    ///
    /// A choice knob gives the nearest choice, and a knob with an interval gives the nearest
    /// multiple of it. So the face, the accessible value, and the stored value agree during a
    /// drag, and the backend does not round the value again.
    fn limit(&self, position: f64) -> f64 {
        let value = self
            .stored
            .value(position.clamp(self.travel.0, self.travel.1));
        if self.choices.len() > 1 {
            value.round()
        } else if let Some(interval) = self.interval {
            (value / interval).round() * interval
        } else {
            value
        }
    }

    /// The stored value after a move of `delta` positions from `value`, held inside the travel.
    ///
    /// No move keeps the value exactly, so no key sends no edit.
    fn moved(&self, value: f64, delta: f64) -> f64 {
        if delta == 0.0 {
            return value;
        }
        self.limit(self.position(value) + delta)
    }
}

/// The text of a signed value with a side: "2.0 ms L", "C", "50% R".
fn sided(value: f64, magnitude: impl Fn(f64) -> String, centre: &str) -> String {
    let text = magnitude(value.abs());
    if magnitude(0.0) == text {
        centre.to_owned()
    } else if value < 0.0 {
        format!("{text} L")
    } else {
        format!("{text} R")
    }
}

/// The crossover frequency that the engine uses for a position.
fn crossover_hz(position: f64) -> f64 {
    f64::from(ronie_dsp::image::deck_crossover_hz(position as f32))
}

/// The number at the start of a text, and the rest of the text.
fn leading_number(text: &str) -> Option<(f64, &str)> {
    let text = text.trim();
    let end = text
        .char_indices()
        .find(|(index, character)| {
            !(character.is_ascii_digit()
                || *character == '.'
                || *character == ','
                || (*index == 0 && matches!(character, '+' | '-' | '−')))
        })
        .map_or(text.len(), |(index, _)| index);
    let number = text[..end].replace(',', ".").replace('−', "-");
    number
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .map(|number| (number, &text[end..]))
}

/// The text of a gain in decibels, for example "+6.0 dB", "-0.5 dB", or "0.0 dB".
///
/// A value that is not zero after the rounding to one decimal has a sign. Every view that shows
/// a gain uses this text, so one value always reads the same.
#[must_use]
pub fn format_db(db: f64) -> String {
    let rounded = (db * 10.0).round() / 10.0;
    if rounded == 0.0 {
        "0.0 dB".to_owned()
    } else {
        format!("{rounded:+.1} dB")
    }
}

/// One change that a knob asks for in one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnobEdit {
    /// The new value, in the units that the backend stores.
    ///
    /// For [`GesturePhase::Cancel`] this is the value from before the drag, which the backend
    /// restores.
    pub value: f64,
    /// The phase of the gesture that the edit belongs to.
    pub phase: GesturePhase,
}

/// What one frame of a knob gives back.
#[derive(Debug)]
pub struct KnobOutput {
    /// The response of the knob area.
    pub response: egui::Response,
    /// The change that the caller sends to the backend, if any.
    pub edit: Option<KnobEdit>,
    /// The value to paint in this frame: the edit value, or else the given value.
    pub shown: f64,
}

/// What a knob keeps between frames.
#[derive(Debug, Clone, Default)]
struct Memory {
    /// The last pass that showed the knob.
    last_pass: u64,
    /// The open drag.
    drag: Option<DragState>,
    /// The open type-in field.
    editor: Option<Editor>,
}

/// One open drag.
#[derive(Debug, Clone, Copy)]
struct DragState {
    /// The value at the start of the drag, which a cancel restores.
    start: f64,
    /// The motion of the whole drag, as a fraction of the range.
    ///
    /// Each frame adds its motion here and does not start from the value of the backend. A
    /// backend rounds the value, for example to one choice, so a start from it can lose the
    /// motion of each frame.
    travel: f64,
    /// The value of the last edit of this drag.
    sent: f64,
}

impl DragState {
    /// Adds the motion of one frame, and gives the value of the whole travel.
    ///
    /// The travel stays inside the travel of the knob, so the knob turns back at once when the
    /// pointer turns back past an end. No travel gives the start value exactly.
    fn moved(&mut self, spec: &KnobSpec, motion: f64) -> f64 {
        let start = spec.position(self.start);
        self.travel = (start + self.travel + motion).clamp(spec.travel.0, spec.travel.1) - start;
        if self.travel == 0.0 {
            self.start
        } else {
            spec.limit(start + self.travel)
        }
    }
}

/// One open type-in field.
#[derive(Debug, Clone)]
struct Editor {
    /// The typed text.
    text: String,
    /// The pass that opened the field.
    opened_pass: u64,
    /// Whether the field had the keyboard focus.
    ///
    /// A new floating area is not interactive in its first frame, so the field asks for the
    /// focus until it has it, for [`FOCUS_PASSES`] passes at most.
    focused: bool,
}

/// How many passes a new type-in field asks for the focus. A field that gets no focus in that
/// time closes, so that it does not take the focus from each other control.
const FOCUS_PASSES: u64 = 3;

/// Shows one knob in `rect` and reads its input for this frame.
///
/// `id` must be the same in each frame, and one pass must not show two knobs with one `id`.
/// `label` is the name of the knob, and the accessible name is "<label>: <value text>". `value`
/// is the value that the backend holds now, in the units that the backend stores. A value that
/// is not finite shows as the reset value, and the knob never sends a value that is not finite.
/// The type-in field floats over the bottom of `rect`, where a knob shows its label. It follows
/// the clip rect and the layer transform of `ui`.
///
/// A knob that a pass does not show forgets its drag and its type-in field, and sends no edit for
/// them. The owner of the knob must close an open gesture itself, for example with `OpenDrag`.
/// A disabled `ui`, or a `ui` that does not show all of `rect`, closes the type-in field with no
/// edit.
pub fn show(
    ui: &mut egui::Ui,
    id: egui::Id,
    rect: egui::Rect,
    spec: &KnobSpec,
    label: &str,
    value: f64,
) -> KnobOutput {
    let value = if value.is_finite() { value } else { spec.reset };
    let pass = ui.ctx().cumulative_pass_nr();
    #[cfg(debug_assertions)]
    check_unique(ui.ctx(), id, pass);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut memory = ui
        .ctx()
        .data_mut(|data| data.get_temp::<Memory>(id))
        .unwrap_or_default();
    if memory.last_pass + 1 < pass {
        // A pass did not show the knob, so its drag and its field ended with no input to read.
        memory.drag = None;
        memory.editor = None;
    }
    memory.last_pass = pass;
    if !ui.is_enabled() || !ui.clip_rect().contains_rect(rect) {
        // A disabled knob commits no typed value. A field of a knob that the view clips would
        // float over the edge of the view.
        memory.editor = None;
    }
    let edit = if memory.editor.is_some() {
        type_in(ui, id, rect, spec, label, value, &mut memory)
    } else {
        pointer_and_keys(ui, &response, spec, value, &mut memory)
    }
    .filter(|edit| edit.value.is_finite());
    ui.ctx().data_mut(|data| data.insert_temp(id, memory));
    let shown = edit.map_or(value, |edit| edit.value);
    let name = format!("{label}: {}", spec.format(shown));
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::Slider, true, &name);
        info.value = Some(shown);
        info
    });
    KnobOutput {
        response,
        edit,
        shown,
    }
}

/// Stops a debug build when one pass shows two knobs with one ID, because the two would share
/// one drag and one type-in field.
#[cfg(debug_assertions)]
fn check_unique(context: &egui::Context, id: egui::Id, pass: u64) {
    #[derive(Debug, Clone, Default)]
    struct Shown {
        pass: u64,
        ids: Vec<egui::Id>,
    }
    let unique = context.data_mut(|data| {
        let shown = data.get_temp_mut_or_default::<Shown>(egui::Id::unique("knob-ids-of-the-pass"));
        if shown.pass != pass {
            shown.pass = pass;
            shown.ids.clear();
        }
        let unique = !shown.ids.contains(&id);
        shown.ids.push(id);
        unique
    });
    debug_assert!(unique, "one pass shows two knobs with the ID {id:?}");
}

/// The drag, the reset, the opening of the type-in field, and the keys.
fn pointer_and_keys(
    ui: &egui::Ui,
    response: &egui::Response,
    spec: &KnobSpec,
    value: f64,
    memory: &mut Memory,
) -> Option<KnobEdit> {
    let modifiers = ui.input(|input| input.modifiers);
    let speed = DRAG_PER_POINT / if modifiers.shift { FINE } else { 1.0 };
    let motion = -f64::from(response.drag_delta().y) * speed;
    if response.clicked() || response.drag_started() {
        // The arrow keys then step the knob that the user touched.
        response.request_focus();
    }
    if response.drag_started() {
        let mut drag = DragState {
            start: value,
            travel: 0.0,
            sent: value,
        };
        let next = drag.moved(spec, motion);
        drag.sent = next;
        memory.drag = Some(drag);
        return Some(KnobEdit {
            value: next,
            phase: GesturePhase::Begin,
        });
    }
    if let Some(drag) = &mut memory.drag {
        if response.dragged() {
            let next = drag.moved(spec, motion);
            if next == drag.sent {
                return None;
            }
            drag.sent = next;
            return Some(KnobEdit {
                value: next,
                phase: GesturePhase::Change,
            });
        }
        let start = drag.start;
        memory.drag = None;
        // egui ends a drag when Escape is pressed, and then the drag is a cancel. The cancel uses
        // the press, so that it does not also close the popover or the page of the knob.
        return Some(if take_escape(ui) {
            KnobEdit {
                value: start,
                phase: GesturePhase::Cancel,
            }
        } else {
            KnobEdit {
                value,
                phase: GesturePhase::End,
            }
        });
    }
    if response.clicked() && modifiers.alt {
        return (spec.reset != value).then_some(KnobEdit {
            value: spec.reset,
            phase: GesturePhase::End,
        });
    }
    if response.double_clicked() {
        memory.editor = Some(Editor {
            text: spec.edit_text(value),
            opened_pass: memory.last_pass,
            focused: false,
        });
        return None;
    }
    key_step(ui, response, spec, value)
}

/// Uses an Escape press of this frame, and returns whether the frame had one.
pub(crate) fn take_escape(ui: &egui::Ui) -> bool {
    ui.ctx().input_mut(|input| {
        let modifiers = input.modifiers;
        input.consume_key(modifiers, egui::Key::Escape)
    })
}

/// The edit of the arrow keys, Home, and End on the focused knob.
fn key_step(
    ui: &egui::Ui,
    response: &egui::Response,
    spec: &KnobSpec,
    value: f64,
) -> Option<KnobEdit> {
    if !response.has_focus() {
        return None;
    }
    // The arrow keys set the value, so they must not move the focus to another control.
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            },
        );
    });
    // Each press has its own modifiers, so a Shift press is a fine step in a frame of presses.
    let (steps, home, end) = ui.input(|input| {
        let mut steps = 0.0;
        for event in &input.events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                let direction = match key {
                    egui::Key::ArrowUp | egui::Key::ArrowRight => 1.0,
                    egui::Key::ArrowDown | egui::Key::ArrowLeft => -1.0,
                    _ => continue,
                };
                steps += direction / if modifiers.shift { FINE } else { 1.0 };
            }
        }
        (
            steps,
            input.key_pressed(egui::Key::Home),
            input.key_pressed(egui::Key::End),
        )
    });
    let next = if end {
        spec.limit(1.0)
    } else if home {
        spec.limit(0.0)
    } else {
        spec.moved(value, steps * spec.key_step)
    };
    (next != value).then_some(KnobEdit {
        value: next,
        phase: GesturePhase::End,
    })
}

/// The floating type-in field. Enter commits a valid number, and Escape or a click elsewhere
/// closes the field with no change.
///
/// The field also closes when the focus goes to another control by any other path, and when it
/// gets no focus in [`FOCUS_PASSES`] passes. A field with no focus cannot take keys, and the knob
/// under it would take no input.
fn type_in(
    ui: &egui::Ui,
    id: egui::Id,
    rect: egui::Rect,
    spec: &KnobSpec,
    label: &str,
    value: f64,
    memory: &mut Memory,
) -> Option<KnobEdit> {
    let mut editor = memory.editor.take()?;
    let field_id = id.with("type-in");
    if !editor.focused {
        if memory.last_pass > editor.opened_pass + FOCUS_PASSES {
            return None;
        }
        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::select_all_str(&editor.text)));
        state.store(ui.ctx(), field_id);
        ui.memory_mut(|memory| memory.request_focus(field_id));
    }
    let text = &mut editor.text;
    let width = FIELD_SIZE.x.max(rect.width());
    let field = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.bottom() - FIELD_SIZE.y / 2.0),
        egui::vec2(width, FIELD_SIZE.y),
    );
    // The field stays inside the view of the knob, and moves and scales with the layer of the
    // knob, as a popover does.
    let clip = ui.clip_rect();
    let to_global = ui
        .ctx()
        .layer_transform_to_global(ui.layer_id())
        .unwrap_or(egui::emath::TSTransform::IDENTITY);
    let area = egui::Area::new(id.with("type-in-area"))
        .order(egui::Order::Foreground)
        .fixed_pos(field.min);
    ui.ctx().set_transform_layer(area.layer(), to_global);
    let response = area
        .show(ui.ctx(), |ui| {
            ui.set_clip_rect(clip);
            ui.set_min_size(field.size());
            ui.painter().rect(
                field,
                4.0,
                crate::theme::palette::SURFACE_RAISED,
                Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                egui::StrokeKind::Inside,
            );
            let response = ui.put(
                field,
                egui::TextEdit::singleline(text)
                    .id(field_id)
                    .frame(egui::Frame::NONE)
                    .font(egui::FontId::proportional(13.0))
                    .align(egui::Align2::CENTER_CENTER)
                    .desired_width(width - 8.0),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, format!("{label} value"))
            });
            response
        })
        .inner;
    if response.lost_focus() {
        // The field uses the Escape that closes it, so that the press does not also close the
        // popover or the page of the knob.
        if take_escape(ui) || !ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            return None;
        }
        let next = spec.parse(&editor.text)?;
        return (next != value).then_some(KnobEdit {
            value: next,
            phase: GesturePhase::End,
        });
    }
    let has_focus = response.has_focus();
    if editor.focused && !has_focus {
        return None;
    }
    editor.focused = has_focus;
    memory.editor = Some(editor);
    None
}

/// The drag gesture that one owner of knobs keeps open on the backend, and the last pass that
/// showed the knob of that drag.
///
/// A view can stop showing a knob during its drag. [`show`] then forgets the drag and sends no
/// `End`. The owner must close the gesture, or later edits join it and one undo takes them back
/// together. The owner gives each frame of its knobs to [`Self::track`], and at the end of its
/// own frame closes the gesture that [`Self::take_hidden`] gives.
#[derive(Debug, Clone)]
pub(crate) struct OpenDrag<K> {
    open: Option<(K, u64)>,
}

impl<K> Default for OpenDrag<K> {
    fn default() -> Self {
        Self { open: None }
    }
}

/// What [`OpenDrag::track`] found for one frame of one knob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tracked<K> {
    /// A gesture that the owner must close before it sends the edit of this frame: a drag whose
    /// knob a pass did not show, or the drag of another knob when this edit begins a new one.
    pub(crate) stale: Option<K>,
    /// Whether the drag of this knob had an open gesture before this edit.
    pub(crate) was_open: bool,
}

impl<K: Clone + PartialEq> OpenDrag<K> {
    /// Records one frame of the knob `key` in `pass`, with the edit of that frame.
    pub(crate) fn track(&mut self, key: &K, pass: u64, edit: Option<&KnobEdit>) -> Tracked<K> {
        self.track_phase(key, pass, edit.map(|edit| edit.phase))
    }

    /// Records one frame of the control `key` in `pass`, with the phase of the edit of that
    /// frame. A control that is not a knob, such as an EQ fader, uses this.
    pub(crate) fn track_phase(
        &mut self,
        key: &K,
        pass: u64,
        phase: Option<GesturePhase>,
    ) -> Tracked<K> {
        let begins = phase == Some(GesturePhase::Begin);
        let stale = self
            .open
            .take_if(|(open, shown)| *shown + 1 < pass || (begins && open != key))
            .map(|(open, _)| open);
        let was_open = match &mut self.open {
            Some((open, shown)) if open == key => {
                *shown = pass;
                true
            }
            _ => false,
        };
        match phase {
            Some(GesturePhase::Begin) => self.open = Some((key.clone(), pass)),
            Some(GesturePhase::End | GesturePhase::Cancel) if was_open => self.open = None,
            _ => {}
        }
        Tracked { stale, was_open }
    }

    /// Takes the open gesture when `pass` did not show its knob. The owner must close it.
    pub(crate) fn take_hidden(&mut self, pass: u64) -> Option<K> {
        self.open
            .take_if(|(_, shown)| *shown != pass)
            .map(|(open, _)| open)
    }

    /// Takes the open gesture, for an owner that goes away. The owner must close it.
    pub(crate) fn take(&mut self) -> Option<K> {
        self.open.take().map(|(open, _)| open)
    }
}

/// One knob attached to a catalog parameter of the backend, for the faceplate and the decks.
///
/// It reads the value from the backend in each frame and sends each edit of [`show`] as
/// [`UiAction::SetParameter`]. Its egui ID comes from its parameter, so a knob that shows or
/// hides before it does not change it.
pub struct Knob {
    backend: Arc<dyn UiBackend>,
    id: ParameterId,
    spec: KnobSpec,
    label: String,
    /// The text under the face in place of the label, when the owner gives one.
    caption: Option<String>,
    /// The drag that has an open gesture on the backend.
    drag: OpenDrag<()>,
    /// The phase of the edit that the latest frame of the knob sent, or `None` for no edit.
    last_phase: Option<GesturePhase>,
}

impl Drop for Knob {
    /// A view can drop a knob during a drag, and the gesture must not stay open.
    fn drop(&mut self) {
        if self.drag.take().is_some() {
            self.end_gesture();
        }
    }
}

impl Knob {
    /// Attaches to one catalog parameter.
    ///
    /// # Errors
    /// Returns an unknown-parameter error when the ID is outside the catalog.
    pub fn new(
        backend: Arc<dyn UiBackend>,
        id: &'static str,
        label: &str,
    ) -> Result<Self, UiError> {
        let id = ParameterId(id);
        let spec = KnobSpec::parameter(id).ok_or(UiError::UnknownParameter(id))?;
        Ok(Self {
            backend,
            id,
            spec,
            label: label.to_owned(),
            caption: None,
            drag: OpenDrag::default(),
            last_phase: None,
        })
    }

    /// Sets the text under the face. `None` shows the label.
    ///
    /// The caption changes only the text under the face. The accessible name stays
    /// "<label>: <value text>".
    pub fn set_caption(&mut self, caption: Option<String>) {
        self.caption = caption;
    }

    /// The phase of the edit that the latest frame of the knob sent, or `None` for no edit.
    ///
    /// An owner reads this after [`Self::show_in_rect`], for example to act once at the end of a
    /// gesture and not on each frame of a drag.
    #[must_use]
    pub fn last_phase(&self) -> Option<GesturePhase> {
        self.last_phase
    }

    /// Closes the open gesture on the backend with the value that it holds.
    fn end_gesture(&self) {
        let _ = self.backend.dispatch(UiAction::SetParameter {
            id: self.id,
            value: ParameterValue(self.value() as f32),
            gesture: GesturePhase::End,
        });
    }

    /// Closes the gesture of a drag when `pass` did not show this knob.
    ///
    /// An owner that can stop showing a knob calls this at the end of each of its frames.
    pub(crate) fn end_hidden_drag(&mut self, pass: u64) {
        if self.drag.take_hidden(pass).is_some() {
            self.end_gesture();
        }
    }

    /// The description of the knob for the edits of this frame.
    ///
    /// With STEP on, the engine and the caption use whole semitones. The pitch knob then holds
    /// whole semitones too, so that the value that the knob shows is the value that plays.
    fn edit_spec(&self) -> std::borrow::Cow<'_, KnobSpec> {
        let whole_semitones = self.id == ParameterId(PITCH_SEMITONES)
            && self
                .backend
                .parameter(ParameterId(PITCH_STEP))
                .is_some_and(|step| step.0 >= 0.5);
        if whole_semitones {
            std::borrow::Cow::Owned(self.spec.clone().with_interval(1.0))
        } else {
            std::borrow::Cow::Borrowed(&self.spec)
        }
    }

    /// The value that the backend holds, or the reset value when it holds none.
    fn value(&self) -> f64 {
        self.backend
            .parameter(self.id)
            .map_or(self.spec.reset, |value| f64::from(value.0))
    }

    /// Current position, from 0 to 1.
    #[must_use]
    pub fn normalized(&self) -> f32 {
        self.spec.position(self.value()) as f32
    }

    /// Current text of the value.
    #[must_use]
    pub fn format(&self) -> String {
        self.spec.format(self.value())
    }

    /// Shows the knob in a design-space column: a face of `size` at the top, and the label under
    /// it.
    pub fn show_in_rect(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        size: f32,
    ) -> egui::Response {
        // The cursor still moves, so that the layout of the other widgets stays the same.
        let _ = ui.advance_cursor_after_rect(rect);
        let id = ui.scope_id().with(("knob", self.id.0));
        let output = show(ui, id, rect, &self.edit_spec(), &self.label, self.value());
        let pass = ui.ctx().cumulative_pass_nr();
        if self
            .drag
            .track(&(), pass, output.edit.as_ref())
            .stale
            .is_some()
        {
            self.end_gesture();
        }
        self.last_phase = output.edit.as_ref().map(|edit| edit.phase);
        if let Some(edit) = output.edit {
            let _ = self.backend.dispatch(UiAction::SetParameter {
                id: self.id,
                value: ParameterValue(edit.value as f32),
                gesture: edit.phase,
            });
        }
        let face = egui::Rect::from_min_size(
            egui::pos2(rect.center().x - size / 2.0, rect.top()),
            egui::Vec2::splat(size),
        );
        paint_knob(
            ui.painter(),
            face,
            self.spec.position(output.shown) as f32,
            self.spec.bipolar(),
        );
        ui.painter().text(
            Pos2::new(rect.center().x, rect.top() + size + 8.0 + 8.5),
            egui::Align2::CENTER_CENTER,
            self.caption.as_deref().unwrap_or(&self.label),
            egui::FontId::proportional(14.0),
            crate::theme::palette::TEXT_MUTED,
        );
        output.response
    }
}

pub(crate) fn paint_knob(painter: &egui::Painter, rect: egui::Rect, value: f32, bipolar: bool) {
    paint_knob_sweep(
        painter,
        rect,
        value,
        if bipolar {
            KnobSweep::Bipolar
        } else {
            KnobSweep::Full
        },
    );
}

pub(crate) fn paint_pan_knob(painter: &egui::Painter, rect: egui::Rect, value: f32, left: bool) {
    paint_knob_sweep(
        painter,
        rect,
        value,
        if left {
            KnobSweep::PanLeft
        } else {
            KnobSweep::PanRight
        },
    );
}

#[derive(Clone, Copy)]
enum KnobSweep {
    Full,
    Bipolar,
    PanLeft,
    PanRight,
}

fn paint_knob_sweep(painter: &egui::Painter, rect: egui::Rect, value: f32, variant: KnobSweep) {
    let size = rect.width();
    let scale = size / 200.0;
    let center = rect.center();
    painter.circle_filled(center, 100.0 * scale, crate::theme::palette::SURFACE_RAISED);
    painter.circle_filled(center, 98.4 * scale, Color32::BLACK);

    let value = value.clamp(0.0, 1.0);
    let angle = match variant {
        KnobSweep::PanLeft => value.clamp(0.0, 0.5) * 270.0 - 135.0,
        KnobSweep::PanRight => (value - 0.5).clamp(0.0, 0.5) * 270.0,
        KnobSweep::Full | KnobSweep::Bipolar => value * 270.0 - 135.0,
    };
    let arc_from: f32 = if matches!(variant, KnobSweep::Full) {
        -135.0
    } else {
        0.0
    };
    let from = arc_from.min(angle);
    let to = arc_from.max(angle);
    if to - from >= 0.25 {
        let steps = ((to - from).abs() / 4.0).ceil() as usize;
        let mut points = Vec::with_capacity(steps + 1);
        for index in 0..=steps {
            let degrees = from + (to - from) * index as f32 / steps.max(1) as f32;
            points.push(point_on_knob(center, 91.56 * scale, degrees));
        }
        painter.add(egui::epaint::PathShape::line(
            points,
            Stroke::new(10.12 * scale, crate::theme::palette::FLAME),
        ));
    }

    painter.circle_filled(center, 86.4 * scale, crate::theme::palette::SURFACE_RAISED);
    let primary = size > 36.0;
    let fills = if primary {
        [
            (
                crate::theme::palette::KNOB_METAL,
                Color32::from_rgb(61, 61, 61),
            ),
            (
                crate::theme::palette::KNOB_METAL,
                Color32::from_rgb(36, 36, 36),
            ),
            (
                Color32::from_rgb(151, 151, 151),
                Color32::from_rgb(35, 35, 35),
            ),
        ]
    } else {
        [
            (
                Color32::from_rgb(163, 163, 163),
                Color32::from_rgb(163, 163, 163),
            ),
            (crate::theme::palette::KNOB_METAL, Color32::BLACK),
            (Color32::from_rgb(80, 80, 80), Color32::BLACK),
        ]
    };
    for (radius, (top, bottom)) in [84.9, 82.75, 79.05].into_iter().zip(fills) {
        gradient_disc(painter, center, radius * scale, top, bottom);
    }

    let pointer = point_on_knob(center, 51.0 * scale, angle);
    gradient_disc(
        painter,
        pointer,
        14.4 * scale,
        Color32::from_rgb(4, 4, 4),
        crate::theme::palette::KNOB_METAL,
    );
    painter.circle_filled(pointer, 10.25 * scale, Color32::BLACK);
}

fn point_on_knob(center: Pos2, radius: f32, degrees: f32) -> Pos2 {
    let radians = degrees.to_radians();
    center + Vec2::new(radians.sin() * radius, -radians.cos() * radius)
}

fn gradient_disc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    top: Color32,
    bottom: Color32,
) {
    const SEGMENTS: usize = 48;
    let mut mesh = egui::epaint::Mesh::default();
    mesh.vertices.push(egui::epaint::Vertex {
        pos: center,
        uv: Pos2::ZERO,
        color: mix_color(top, bottom, 0.5),
    });
    for index in 0..=SEGMENTS {
        let angle = index as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let y = -angle.cos();
        mesh.vertices.push(egui::epaint::Vertex {
            pos: center + Vec2::new(angle.sin() * radius, y * radius),
            uv: Pos2::ZERO,
            color: mix_color(top, bottom, (y + 1.0) * 0.5),
        });
    }
    for index in 0..SEGMENTS {
        mesh.indices
            .extend_from_slice(&[0, index as u32 + 1, index as u32 + 2]);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn mix_color(top: Color32, bottom: Color32, fraction: f32) -> Color32 {
    let mix =
        |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * fraction).round() as u8;
    Color32::from_rgb(
        mix(top.r(), bottom.r()),
        mix(top.g(), bottom.g()),
        mix(top.b(), bottom.b()),
    )
}

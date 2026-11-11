//! Chromatic tuner state and native takeover view.

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2};
use ronie_dsp::analysis::TunerReading;

use crate::{
    accessibility::{AccessibleNode, AccessibleRole},
    theme::palette,
};

const HOLD_MS: i64 = 900;
const MIN_CONFIDENCE: f64 = 0.5;
const CENTS_SMOOTHING: f64 = 0.3;
const IN_TUNE_CENTS: f64 = 5.0;
const MAX_CENTS: f64 = 50.0;
const BARS_PER_SIDE: usize = 6;

/// Nearest chromatic pitch and signed cents offset.
#[derive(Debug, Clone, PartialEq)]
pub struct Pitch {
    /// Note letter with an optional sharp accidental.
    pub name: String,
    /// Scientific pitch octave.
    pub octave: i32,
    /// Signed offset from the nearest semitone.
    pub cents: f64,
}

/// Converts a positive frequency to the nearest chromatic pitch.
#[must_use]
pub fn pitch_from_frequency(frequency_hz: f64) -> Pitch {
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 {
        return Pitch {
            name: String::new(),
            octave: 0,
            cents: 0.0,
        };
    }
    const NAMES: [&str; 12] = [
        "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
    ];
    let midi = 69.0 + 12.0 * (frequency_hz / 440.0).log2();
    let nearest = midi.round() as i32;
    Pitch {
        name: NAMES[nearest.rem_euclid(12) as usize].to_owned(),
        octave: nearest.div_euclid(12) - 1,
        cents: (midi - f64::from(nearest)) * 100.0,
    }
}

/// Number of bars lit on one side for an absolute cents deflection.
#[must_use]
pub fn lit_count(abs_cents: f64) -> usize {
    let abs_cents = abs_cents.abs();
    if abs_cents <= IN_TUNE_CENTS {
        return 1;
    }
    let position = ((abs_cents - IN_TUNE_CENTS) / (MAX_CENTS - IN_TUNE_CENTS)).min(1.0);
    (1 + (position * 5.0 + 0.5).floor() as usize).min(BARS_PER_SIDE)
}

/// Coarse tuner status used by drawing and accessibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunerStatus {
    /// No reliable input pitch.
    Idle,
    /// Within five cents of the nearest note.
    InTune,
    /// More than five cents below the nearest note.
    Flat,
    /// More than five cents above the nearest note.
    Sharp,
}

/// Current smoothed tuner display state.
#[derive(Debug, Clone, PartialEq)]
pub struct TunerState {
    /// Whether a note is currently locked or held.
    pub has_signal: bool,
    /// Last detected note name.
    pub note: String,
    /// Last detected note octave.
    pub octave: i32,
    /// Smoothed signed cents offset.
    pub cents: f64,
    /// Last detected frequency.
    pub frequency_hz: f64,
}

impl Default for TunerState {
    fn default() -> Self {
        Self {
            has_signal: false,
            note: String::new(),
            octave: 0,
            cents: 0.0,
            frequency_hz: 0.0,
        }
    }
}

impl TunerState {
    /// Current coarse status.
    #[must_use]
    pub fn status(&self) -> TunerStatus {
        if !self.has_signal {
            TunerStatus::Idle
        } else if self.cents.abs() <= IN_TUNE_CENTS {
            TunerStatus::InTune
        } else if self.cents < 0.0 {
            TunerStatus::Flat
        } else {
            TunerStatus::Sharp
        }
    }

    /// Lit bars on the flat and sharp sides.
    #[must_use]
    pub fn lit_bars(&self) -> (usize, usize) {
        match self.status() {
            TunerStatus::Idle => (0, 0),
            TunerStatus::InTune => (1, 1),
            TunerStatus::Flat => (lit_count(self.cents), 0),
            TunerStatus::Sharp => (0, lit_count(self.cents)),
        }
    }

    /// The line under the frequency while a pitch shift moves the signal, for example
    /// "Sounds as D", or `None` with no signal or no shift.
    ///
    /// `shift` is the shift that the player hears, in semitones.
    #[must_use]
    pub fn sounds_as(&self, shift: f32) -> Option<String> {
        if !self.has_signal {
            return None;
        }
        super::tuning::sounds_as(self.frequency_hz, shift).map(|note| format!("Sounds as {note}"))
    }

    /// Human-readable status for screen readers.
    #[must_use]
    pub fn accessibility_text(&self) -> String {
        if !self.has_signal {
            return "No pitch detected".to_owned();
        }
        let prefix = format!(
            "{}{}, {:.1} Hz, ",
            self.note, self.octave, self.frequency_hz
        );
        match self.status() {
            TunerStatus::InTune => format!("{prefix}in tune"),
            TunerStatus::Flat => format!("{prefix}{} cents flat", self.cents.abs().round() as i32),
            TunerStatus::Sharp => format!("{prefix}{} cents sharp", self.cents.round() as i32),
            TunerStatus::Idle => prefix,
        }
    }

    /// Accessible static-text node for the takeover.
    #[must_use]
    pub fn accessible_node(&self) -> AccessibleNode {
        AccessibleNode::new(
            "tuner-status".to_owned(),
            AccessibleRole::StaticText,
            "Tuner",
        )
        .with_value(self.accessibility_text())
    }
}

/// Polling state with light cents smoothing and a 900 ms signal hold.
#[derive(Debug, Default)]
pub struct TunerFeed {
    state: TunerState,
    hold_until_ms: i64,
}

impl TunerFeed {
    /// Applies one raw-input reading and returns whether visible state changed.
    pub fn update(&mut self, reading: TunerReading, now_ms: i64) -> bool {
        let mut next = self.state.clone();
        if reading.frequency_hz > 0.0 && reading.confidence > MIN_CONFIDENCE {
            let pitch = pitch_from_frequency(reading.frequency_hz);
            next.cents = self.state.cents * (1.0 - CENTS_SMOOTHING) + pitch.cents * CENTS_SMOOTHING;
            next.note = pitch.name;
            next.octave = pitch.octave;
            next.frequency_hz = reading.frequency_hz;
            next.has_signal = true;
            self.hold_until_ms = now_ms.saturating_add(HOLD_MS);
        } else if self.state.has_signal && now_ms >= self.hold_until_ms {
            next.has_signal = false;
        }
        let changed = next != self.state;
        self.state = next;
        changed
    }

    /// Current display state.
    #[must_use]
    pub const fn state(&self) -> &TunerState {
        &self.state
    }

    /// Clears the held reading when the takeover closes.
    pub fn reset(&mut self) {
        self.state = TunerState::default();
        self.hold_until_ms = 0;
    }
}

/// Native tuner takeover with tapered bars and directional triangles.
#[derive(Debug, Default)]
pub struct TunerView;

impl TunerView {
    /// Draws the takeover and returns whether the user requested close.
    ///
    /// `shift` is the pitch shift that the player hears, in semitones. The tuner reads the
    /// signal before the shift, so the big note stays the real string, and a line under it
    /// names the note that the shift makes.
    pub fn show(&mut self, ui: &mut egui::Ui, state: &TunerState, shift: f32) -> bool {
        let close = super::popover::escape_reaches_surface(ui.ctx());
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, Color32::BLACK);
        let close_response = ui.allocate_rect(
            Rect::from_min_size(
                Pos2::new(area.right() - 48.0, area.top() + 16.0),
                Vec2::splat(28.0),
            ),
            egui::Sense::click(),
        );
        ui.painter().text(
            close_response.rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            FontId::proportional(20.0),
            Color32::WHITE,
        );
        close_response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close tuner"));
        let (left_lit, right_lit) = state.lit_bars();
        let status = state.status();
        let colour = tune_colour(state);
        let row_width = 944.0;
        let column_height = 288.0;
        let origin = Pos2::new(
            area.center().x - row_width / 2.0,
            area.center().y - column_height / 2.0,
        );
        paint_bars(ui, origin, left_lit, right_lit, status);
        let centre_x = area.center().x;
        if matches!(status, TunerStatus::InTune | TunerStatus::Sharp) {
            paint_triangle(ui, centre_x, origin.y, true, colour);
        }
        if matches!(status, TunerStatus::InTune | TunerStatus::Flat) {
            paint_triangle(
                ui,
                centre_x,
                origin.y + 53.0 + 36.0 + 110.0 + 36.0,
                false,
                colour,
            );
        }
        if state.has_signal {
            let note_y = origin.y + 53.0 + 36.0;
            let letter = state
                .note
                .chars()
                .next()
                .map_or_else(String::new, String::from);
            let accidental = state.note.get(letter.len()..).unwrap_or_default();
            let letter_rect =
                Rect::from_min_size(Pos2::new(centre_x - 60.0, note_y), Vec2::new(120.0, 110.0));
            ui.painter().text(
                letter_rect.center(),
                egui::Align2::CENTER_CENTER,
                letter,
                FontId::new(110.0, egui::FontFamily::Name("arimo-bold".into())),
                colour,
            );
            if !accidental.is_empty() {
                ui.painter().text(
                    Pos2::new(letter_rect.right() - 26.5, note_y + 13.0),
                    egui::Align2::LEFT_TOP,
                    accidental,
                    FontId::new(38.5, egui::FontFamily::Name("arimo-bold".into())),
                    colour,
                );
            }
            let cents = state.cents.round() as i32;
            let sign = if cents >= 0 { "+" } else { "" };
            ui.painter().text(
                Pos2::new(centre_x, note_y + 104.0),
                egui::Align2::CENTER_TOP,
                format!("{:.1} Hz {sign}{cents}¢", state.frequency_hz),
                FontId::monospace(13.0),
                palette::CHROME,
            );
            if let Some(text) = state.sounds_as(shift) {
                let sounds_rect = Rect::from_center_size(
                    Pos2::new(centre_x, note_y + 132.0),
                    Vec2::new(200.0, 18.0),
                );
                ui.painter().text(
                    sounds_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &text,
                    FontId::proportional(13.0),
                    palette::TEXT_MUTED,
                );
                ui.allocate_rect(sounds_rect, egui::Sense::hover())
                    .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
            }
            let status = ui.allocate_rect(letter_rect, egui::Sense::hover());
            status.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Label, true, state.accessibility_text())
            });
        }
        close || close_response.clicked()
    }
}

/// Bars from the center that stay orange before the far bars turn red.
const NEAR_BARS: usize = 3;

/// Colour of one lit bar: green only for a perfect note, then orange, then red.
fn bar_colour(index: usize, status: TunerStatus) -> Color32 {
    if status == TunerStatus::InTune {
        palette::IN_TUNE
    } else if index < NEAR_BARS {
        palette::TUNE_NEAR
    } else {
        palette::TUNE_FAR
    }
}

/// Colour of the note and the arrows, which follows the outermost lit bar.
fn tune_colour(state: &TunerState) -> Color32 {
    let (left, right) = state.lit_bars();
    let lit = left.max(right);
    if lit == 0 {
        palette::CHROME
    } else {
        bar_colour(lit - 1, state.status())
    }
}

fn paint_bars(ui: &egui::Ui, origin: Pos2, left_lit: usize, right_lit: usize, status: TunerStatus) {
    for side in [false, true] {
        for slot in 0..BARS_PER_SIDE {
            let index = if side { slot } else { BARS_PER_SIDE - 1 - slot };
            let x = origin.x + if side { 564.0 } else { 0.0 } + slot as f32 * 66.0;
            let y = origin.y + 53.5;
            let lit = if side {
                index < right_lit
            } else {
                index < left_lit
            };
            let colour = if lit {
                bar_colour(index, status)
            } else {
                palette::COOLED
            };
            let (outer, inner) = if side { (x + 50.0, x) } else { (x, x + 50.0) };
            ui.painter().add(Shape::convex_polygon(
                vec![
                    Pos2::new(outer, y),
                    Pos2::new(inner, y + 30.0),
                    Pos2::new(inner, y + 151.0),
                    Pos2::new(outer, y + 181.0),
                ],
                colour,
                Stroke::NONE,
            ));
        }
    }
}

fn paint_triangle(ui: &egui::Ui, centre_x: f32, y: f32, down: bool, colour: Color32) {
    let (top, bottom) = if down { (y + 53.0, y) } else { (y, y + 53.0) };
    ui.painter().add(Shape::convex_polygon(
        vec![
            Pos2::new(centre_x - 30.5, bottom),
            Pos2::new(centre_x + 30.5, bottom),
            Pos2::new(centre_x, top),
        ],
        colour,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(cents: f64) -> TunerState {
        TunerState {
            has_signal: true,
            cents,
            ..TunerState::default()
        }
    }

    #[test]
    fn perfect_tune_is_green_then_orange_then_red() {
        assert_eq!(tune_colour(&state(0.0)), palette::IN_TUNE);
        assert_eq!(tune_colour(&state(4.9)), palette::IN_TUNE);
        assert_eq!(tune_colour(&state(-8.0)), palette::TUNE_NEAR);
        assert_eq!(tune_colour(&state(12.0)), palette::TUNE_NEAR);
        assert_eq!(tune_colour(&state(-50.0)), palette::TUNE_FAR);
        assert_eq!(tune_colour(&state(50.0)), palette::TUNE_FAR);
    }

    #[test]
    fn only_a_perfect_note_lights_a_green_bar() {
        assert_eq!(bar_colour(0, TunerStatus::InTune), palette::IN_TUNE);
        assert_eq!(bar_colour(0, TunerStatus::Flat), palette::TUNE_NEAR);
        assert_eq!(bar_colour(2, TunerStatus::Sharp), palette::TUNE_NEAR);
        assert_eq!(bar_colour(3, TunerStatus::Sharp), palette::TUNE_FAR);
        assert_eq!(bar_colour(5, TunerStatus::Flat), palette::TUNE_FAR);
    }
}

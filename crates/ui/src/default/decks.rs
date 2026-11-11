//! Advanced gate, pitch, spread, align, and auto-balance decks.

use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke};
use ronie_core::{
    Instrument, Tuning,
    params::{ParameterId, ParameterValue},
};

use super::{
    popover::{Placement, Popover, Surface, Trigger},
    tuning,
};
use crate::{
    backend::{GesturePhase, UiBackend},
    widgets::Knob,
};

/// The height of the instrument and tuning pickers of the pitch deck.
const PICKER_HEIGHT: f32 = 28.0;
/// The width of the instrument picker.
const INSTRUMENT_WIDTH: f32 = 100.0;
/// The width of the tuning picker.
const TUNING_WIDTH: f32 = 170.0;
/// The distance from the top of the pitch deck content to its STEP and knob row.
const PITCH_KNOB_ROW: f32 = 68.0;
/// The height of one row of a picker list.
const MENU_ROW: f32 = 36.0;
/// The width of the OPTIMIZE toggle of the pitch deck.
const OPTIMIZE_WIDTH: f32 = 60.0;
/// The accessible name of the OPTIMIZE toggle.
const OPTIMIZE_NAME: &str = "Optimize settings";
/// The pitch window parameter, which the Buffer knob of the pitch deck sets.
const PITCH_WINDOW: &str = "pitchWindow";

/// The open list of the pitch deck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PitchMenu {
    Instrument,
    Tuning,
}

/// One row of a picker list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PitchPick {
    Instrument(Instrument),
    Tuning(Tuning),
}

/// Advanced-deck and detail navigation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckKind {
    /// Noise gate deck.
    Gate,
    /// Pitch-shift deck.
    Pitch,
    /// Spread deck.
    Spread,
    /// Alignment deck.
    Align,
    /// Block metadata.
    BlockInfo,
    /// Automatic stereo balance measurement.
    AutoBalance,
}

/// Automatic measurement state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementState {
    /// No active measurement.
    #[default]
    Idle,
    /// Measurement running.
    Running,
    /// Measurement completed.
    Complete,
    /// Measurement failed.
    Failed,
}

/// One advanced deck and its parameter attachments.
pub struct DeckModel {
    backend: Arc<dyn UiBackend>,
    kind: DeckKind,
    state: MeasurementState,
    knobs: Vec<Knob>,
    /// The open picker list of the pitch deck, and the picker that opened it.
    menu: Option<(PitchMenu, Trigger)>,
}

impl DeckModel {
    /// Creates a deck with the parameters appropriate to its kind.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>, kind: DeckKind) -> Self {
        let knobs = continuous_parameters(kind)
            .iter()
            .filter_map(|(id, label)| Knob::new(backend.clone(), id, label).ok())
            .collect();
        Self {
            backend,
            kind,
            state: MeasurementState::Idle,
            knobs,
            menu: None,
        }
    }

    /// The size of the floating panel of this deck.
    #[must_use]
    pub fn size(&self) -> egui::Vec2 {
        match self.kind {
            DeckKind::Pitch => egui::vec2(366.0, 156.0),
            DeckKind::Spread | DeckKind::Align => egui::vec2(316.0, 85.0),
            DeckKind::Gate | DeckKind::BlockInfo | DeckKind::AutoBalance => egui::vec2(262.0, 85.0),
        }
    }

    /// Deck kind.
    #[must_use]
    pub fn kind(&self) -> DeckKind {
        self.kind
    }

    /// Parameter IDs exposed by this deck.
    #[must_use]
    pub fn parameters(&self) -> &'static [&'static str] {
        parameters(self.kind)
    }

    /// Current measurement state.
    #[must_use]
    pub fn state(&self) -> MeasurementState {
        self.state
    }

    /// Starts automatic balance measurement.
    pub fn start(&mut self) {
        self.state = MeasurementState::Running;
    }

    /// Cancels the active measurement.
    pub fn cancel(&mut self) {
        self.state = MeasurementState::Idle;
    }

    /// Closes the gesture of each knob drag when `pass` did not show that knob.
    pub(crate) fn end_hidden_drags(&mut self, pass: u64) {
        for knob in &mut self.knobs {
            knob.end_hidden_drag(pass);
        }
    }

    /// Renders the deck into its exact floating-panel rectangle.
    pub fn show_in_rect(&mut self, ui: &mut egui::Ui, rect: Rect) {
        match self.kind {
            DeckKind::AutoBalance | DeckKind::BlockInfo => {}
            DeckKind::Gate => self.show_knob_row(ui, rect.min + egui::vec2(16.0, 14.0)),
            DeckKind::Pitch => self.show_pitch(ui, rect),
            DeckKind::Spread | DeckKind::Align => self.show_image(ui, rect),
        }
    }

    /// Shows the row of knobs that starts at `origin`. The pitch deck starts its row with the
    /// STEP and OPTIMIZE toggles.
    fn show_knob_row(&mut self, ui: &mut egui::Ui, origin: Pos2) {
        let pitch = self.kind == DeckKind::Pitch;
        if pitch {
            let backend = self.backend.as_ref();
            let center = egui::pos2(origin.x + 32.0, origin.y + 28.5);
            if text_toggle(ui, &self.backend, center, "pitchStep", "STEP") {
                let _ = tuning::apply_optimize(backend);
            }
            let center = egui::pos2(origin.x + 82.0 + 32.0, origin.y + 28.5);
            let on = tuning::optimize(backend);
            if toggle(ui, center, OPTIMIZE_WIDTH, "OPTIMIZE", OPTIMIZE_NAME, on).clicked() {
                let _ = tuning::set_optimize(backend, !on);
            }
            if let Some(delay) = backend
                .device_settings()
                .and_then(|device| device.banner.delay_ms)
            {
                delay_readout(ui, egui::pos2(center.x, origin.y + 50.0), delay);
            }
        }
        let window = pitch.then(|| self.backend.parameter(ParameterId(PITCH_WINDOW)));
        let first = if pitch { 2 } else { 0 };
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            let column = first + index;
            let column_rect = Rect::from_min_size(
                egui::pos2(origin.x + column as f32 * 82.0, origin.y),
                egui::vec2(64.0, 63.0),
            );
            knob.show_in_rect(ui, column_rect, 36.0);
        }
        // A change of the Buffer knob by hand turns OPTIMIZE off, so that the policy does not
        // take the window back.
        let backend = self.backend.as_ref();
        if window.is_some_and(|window| window != backend.parameter(ParameterId(PITCH_WINDOW)))
            && tuning::optimize(backend)
        {
            let _ = tuning::set_optimize(backend, false);
        }
    }

    /// Shows the instrument and tuning pickers, the string strip, and the knob row.
    fn show_pitch(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let content = Rect::from_min_max(
            rect.min + egui::vec2(16.0, 14.0),
            rect.max - egui::vec2(16.0, 8.0),
        );
        let backend = Arc::clone(&self.backend);
        let instrument = tuning::instrument(backend.as_ref());
        let shift = tuning::shift(backend.as_ref());
        let current = instrument.tuning(shift);
        let open = self.menu.map(|(menu, _)| menu);

        let instrument_rect =
            Rect::from_min_size(content.min, egui::vec2(INSTRUMENT_WIDTH, PICKER_HEIGHT));
        let instrument_response = picker(
            ui,
            instrument_rect,
            instrument.short_name(),
            &format!("Instrument: {}", instrument.name()),
            open == Some(PitchMenu::Instrument),
        );
        ui.painter().text(
            egui::pos2(instrument_rect.right() + 12.0, instrument_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Tuning",
            egui::FontId::proportional(13.0),
            crate::theme::palette::TEXT_MUTED,
        );
        let tuning_rect = Rect::from_min_size(
            egui::pos2(content.right() - TUNING_WIDTH, content.top()),
            egui::vec2(TUNING_WIDTH, PICKER_HEIGHT),
        );
        let tuning_text = current.map_or_else(|| "Custom".to_owned(), Tuning::label);
        let tuning_response = picker(
            ui,
            tuning_rect,
            &tuning_text,
            &format!("Tuning: {tuning_text}"),
            open == Some(PitchMenu::Tuning),
        );
        for (response, menu) in [
            (&instrument_response, PitchMenu::Instrument),
            (&tuning_response, PitchMenu::Tuning),
        ] {
            if response.clicked() {
                self.menu = (open != Some(menu)).then(|| (menu, Trigger::from(response)));
            }
        }

        let strip = instrument.string_strip(shift);
        let strip_rect = Rect::from_min_size(
            egui::pos2(content.left(), instrument_rect.bottom() + 10.0),
            egui::vec2(content.width(), 18.0),
        );
        ui.painter().with_clip_rect(strip_rect).text(
            strip_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            &strip,
            egui::FontId::proportional(13.0),
            crate::theme::palette::TEXT_SECONDARY,
        );
        ui.interact(
            strip_rect,
            ui.scope_id().with("string-strip"),
            Sense::hover(),
        )
        .widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Label, true, format!("Strings: {strip}"))
        });
        let rule_y = content.top() + PITCH_KNOB_ROW - 10.0;
        ui.painter().line_segment(
            [
                egui::pos2(content.left(), rule_y),
                egui::pos2(content.right(), rule_y),
            ],
            Stroke::new(1.0, crate::theme::palette::HAIRLINE),
        );

        self.show_knob_row(
            ui,
            egui::pos2(content.left(), content.top() + PITCH_KNOB_ROW),
        );
        // A click of this frame can open the other list, so the anchor follows the list now.
        let anchor = match self.menu {
            Some((PitchMenu::Instrument, _)) => instrument_rect,
            _ => tuning_rect,
        };
        self.show_pitch_menu(ui, anchor, instrument, current);
    }

    /// Shows the open picker list above or under its picker, and applies a picked row.
    fn show_pitch_menu(
        &mut self,
        ui: &egui::Ui,
        anchor: Rect,
        instrument: Instrument,
        current: Option<Tuning>,
    ) {
        let Some((menu, trigger)) = self.menu else {
            return;
        };
        let options: Vec<(String, bool, PitchPick)> = match menu {
            PitchMenu::Instrument => Instrument::ALL
                .into_iter()
                .map(|option| {
                    (
                        option.name().to_owned(),
                        option == instrument,
                        PitchPick::Instrument(option),
                    )
                })
                .collect(),
            PitchMenu::Tuning => instrument
                .tunings()
                .map(|option| {
                    (
                        option.label(),
                        Some(option) == current,
                        PitchPick::Tuning(option),
                    )
                })
                .collect(),
        };
        let width = options
            .iter()
            .map(|(label, _, _)| (text_width(ui, label, 14.0) + 24.0 + 24.0 + 19.0).ceil())
            .fold(anchor.width(), f32::max);
        let height = 18.0 + options.len() as f32 * MENU_ROW;
        let screen = ui.ctx().content_rect();
        let placement = if anchor.bottom() + 6.0 + height <= screen.bottom() - 8.0 {
            Placement::Below { anchor, gap: 6.0 }
        } else {
            Placement::Above { anchor, gap: 6.0 }
        };
        let shown = Popover::new(
            ui.scope_id().with(("pitch-menu", menu)),
            placement,
            egui::vec2(width, height),
            Surface::hairline(14.0),
        )
        .trigger(trigger)
        .show(ui, |ui, panel| {
            let mut pick = None;
            for (index, (label, selected, option)) in options.into_iter().enumerate() {
                let row = Rect::from_min_size(
                    egui::pos2(
                        panel.left() + 9.0,
                        panel.top() + 9.0 + index as f32 * MENU_ROW,
                    ),
                    egui::vec2(panel.width() - 18.0, MENU_ROW),
                );
                let response = ui.interact(
                    row,
                    ui.scope_id().with(("pitch-option", menu, index)),
                    Sense::click(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone())
                });
                let painter = ui.painter_at(row);
                if response.hovered() {
                    painter.rect_filled(row, 8.0, crate::theme::palette::HOVER_FILL);
                }
                if selected {
                    paint_check(&painter, egui::pos2(row.left() + 20.0, row.center().y));
                }
                painter.text(
                    egui::pos2(row.left() + 36.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    &label,
                    egui::FontId::proportional(14.0),
                    if selected {
                        Color32::WHITE
                    } else {
                        crate::theme::palette::TEXT_SECONDARY
                    },
                );
                if response.clicked() {
                    pick = Some(option);
                }
            }
            pick
        });
        match shown.pick {
            Some(PitchPick::Instrument(option)) => {
                let _ = tuning::select_instrument(self.backend.as_ref(), option);
            }
            Some(PitchPick::Tuning(option)) => {
                let _ = tuning::select_tuning(self.backend.as_ref(), option);
            }
            None => {}
        }
        if shown.dismissed.is_some() {
            self.menu = None;
        }
    }

    fn show_image(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let (wobble_enabled, crossover_enabled, diffuse) = if self.kind == DeckKind::Spread {
            (
                "spreadWobbleEnabled",
                "spreadCrossoverEnabled",
                "spreadDiffuseEnabled",
            )
        } else {
            (
                "alignWobbleEnabled",
                "alignCrossoverEnabled",
                "alignDiffuseEnabled",
            )
        };
        let content = Rect::from_min_max(
            rect.min + egui::vec2(17.0, 15.0),
            rect.max - egui::vec2(17.0, 9.0),
        );
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            let column_rect = Rect::from_min_size(
                egui::pos2(content.left() + index as f32 * 100.0, content.top()),
                egui::vec2(82.0, 63.0),
            );
            knob.show_in_rect(ui, column_rect, 36.0);
            let enabled_id = [wobble_enabled, crossover_enabled][index];
            if !enabled(&self.backend, enabled_id) {
                ui.painter().rect_filled(
                    column_rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(20, 20, 22, 140),
                );
            }
            power_button(
                ui,
                &self.backend,
                egui::pos2(column_rect.center().x + 24.0, content.top() + 8.0),
                enabled_id,
                if index == 0 {
                    "Wobble power"
                } else {
                    "Crossover power"
                },
            );
        }
        power_button(
            ui,
            &self.backend,
            egui::pos2(content.left() + 200.0 + 31.0, content.top() + 8.0),
            diffuse,
            "Diffuse",
        );
        ui.painter().text(
            egui::pos2(
                content.left() + 200.0 + 41.0,
                content.top() + 36.0 + 8.0 + 8.5,
            ),
            egui::Align2::CENTER_CENTER,
            "Diffuse",
            egui::FontId::proportional(14.0),
            crate::theme::palette::TEXT_MUTED,
        );
        let correlation = self.backend.meter_levels().correlation;
        let color = if correlation < 0.0 {
            crate::theme::palette::FIRE
        } else if correlation < 0.5 {
            crate::theme::palette::BLAZE
        } else {
            Color32::from_rgba_unmultiplied(235, 235, 245, 102)
        };
        ui.painter().circle_filled(
            egui::pos2(rect.right() - 12.0, rect.top() + 12.0),
            3.0,
            color,
        );
    }
}

fn continuous_parameters(kind: DeckKind) -> &'static [(&'static str, &'static str)] {
    match kind {
        DeckKind::Gate => &[
            ("gateRelease", "Release"),
            ("gateHold", "Hold"),
            ("gateRange", "Range"),
        ],
        DeckKind::Pitch => &[("pitchTonality", "Tonality"), ("pitchWindow", "Buffer")],
        DeckKind::Spread => &[("spreadWobble", "Wobble"), ("spreadCrossover", "Crossover")],
        DeckKind::Align => &[("alignWobble", "Wobble"), ("alignCrossover", "Crossover")],
        DeckKind::BlockInfo | DeckKind::AutoBalance => &[],
    }
}

fn parameters(kind: DeckKind) -> &'static [&'static str] {
    match kind {
        DeckKind::Gate => &["gateRelease", "gateHold", "gateRange"],
        DeckKind::Pitch => &["pitchStep", "pitchTonality", "pitchWindow"],
        DeckKind::Spread => &[
            "spreadWobble",
            "spreadWobbleEnabled",
            "spreadCrossover",
            "spreadCrossoverEnabled",
            "spreadDiffuseEnabled",
        ],
        DeckKind::Align => &[
            "alignWobble",
            "alignWobbleEnabled",
            "alignCrossover",
            "alignCrossoverEnabled",
            "alignDiffuseEnabled",
        ],
        DeckKind::BlockInfo | DeckKind::AutoBalance => &[],
    }
}

fn enabled(backend: &Arc<dyn UiBackend>, id: &'static str) -> bool {
    backend
        .parameter(ParameterId(id))
        .is_some_and(|value| value.0 > 0.5)
}

fn power_button(
    ui: &mut egui::Ui,
    backend: &Arc<dyn UiBackend>,
    position: Pos2,
    id: &'static str,
    label: &'static str,
) {
    let rect = Rect::from_min_size(position, egui::Vec2::splat(20.0));
    let on = enabled(backend, id);
    let response = ui.interact(rect, ui.scope_id().with((id, "power")), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let color = if on {
        Color32::WHITE
    } else {
        crate::theme::palette::TEXT_MUTED
    };
    if !on {
        ui.painter()
            .rect_filled(rect, 2.0, crate::theme::palette::TINT_FILL);
    }
    let center = rect.center();
    let stroke = Stroke::new(1.4, color);
    let mut points = Vec::new();
    for index in 0..=24 {
        let angle = (135.0 + index as f32 / 24.0 * 270.0).to_radians();
        points.push(center + egui::vec2(angle.cos() * 6.0, angle.sin() * 6.0));
    }
    ui.painter()
        .add(egui::epaint::PathShape::line(points, stroke));
    ui.painter().line_segment(
        [center - egui::vec2(0.0, 8.0), center - egui::vec2(0.0, 1.0)],
        stroke,
    );
    if response.clicked() {
        let _ = backend.set_parameter(
            ParameterId(id),
            ParameterValue(f32::from(!on)),
            GesturePhase::Change,
        );
    }
}

/// A picker in the look of the filter chips of the browser: a rounded edge, the value, and a
/// chevron.
fn picker(
    ui: &mut egui::Ui,
    rect: Rect,
    text: &str,
    accessible_name: &str,
    open: bool,
) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("picker", rect.left() as i32)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, accessible_name.to_owned())
    });
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        rect.height() / 2.0,
        Stroke::new(
            1.0,
            if open || response.hovered() {
                Color32::WHITE
            } else {
                crate::theme::palette::HAIRLINE
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter
        .with_clip_rect(rect.shrink(4.0).with_max_x(rect.right() - 30.0))
        .text(
            rect.left_center() + egui::vec2(14.0, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
    let trailing = Rect::from_center_size(
        rect.right_center() - egui::vec2(18.0, 0.0),
        egui::Vec2::splat(16.0),
    );
    let point = |x: f32, y: f32| trailing.min + egui::vec2(x * 16.0 / 24.0, y * 16.0 / 24.0);
    painter.add(egui::epaint::PathShape::line(
        vec![point(6.0, 9.0), point(12.0, 15.0), point(18.0, 9.0)],
        Stroke::new(1.0, crate::theme::palette::TEXT_MUTED),
    ));
    response
}

/// The check mark of the picked row of a picker list, in the look of the browser filters.
fn paint_check(painter: &egui::Painter, center: Pos2) {
    let check = Rect::from_center_size(center, egui::Vec2::splat(16.0));
    let point = |x: f32, y: f32| check.min + egui::vec2(x * 16.0 / 24.0, y * 16.0 / 24.0);
    painter.add(egui::epaint::PathShape::line(
        vec![point(20.0, 6.0), point(9.0, 17.0), point(4.0, 12.0)],
        Stroke::new(16.0 * 2.0 / 24.0, Color32::WHITE),
    ));
}

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                Color32::WHITE,
            )
            .size()
            .x
    })
}

/// A toggle of one parameter, with its text in the toggle. Returns whether a click changed it.
fn text_toggle(
    ui: &mut egui::Ui,
    backend: &Arc<dyn UiBackend>,
    center: Pos2,
    id: &'static str,
    label: &'static str,
) -> bool {
    let on = enabled(backend, id);
    let clicked = toggle(ui, center, 38.0, label, label, on).clicked();
    if clicked {
        let _ = backend.set_parameter(
            ParameterId(id),
            ParameterValue(f32::from(!on)),
            GesturePhase::Change,
        );
    }
    clicked
}

/// The estimated delay of the running stream under the OPTIMIZE toggle, for example "~14 ms".
fn delay_readout(ui: &mut egui::Ui, center: Pos2, milliseconds: f64) {
    let rect = Rect::from_center_size(center, egui::vec2(OPTIMIZE_WIDTH, 14.0));
    ui.interact(rect, ui.scope_id().with("delay-readout"), Sense::hover())
        .widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Label,
                true,
                format!("Delay: about {milliseconds:.0} ms"),
            )
        });
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        format!("~{milliseconds:.0} ms"),
        egui::FontId::proportional(11.0),
        crate::theme::palette::TEXT_MUTED,
    );
}

/// A toggle with its text in it, `width` wide, that shows `on`. The owner applies a click.
fn toggle(
    ui: &mut egui::Ui,
    center: Pos2,
    width: f32,
    label: &'static str,
    accessible_name: &'static str,
    on: bool,
) -> egui::Response {
    let rect = Rect::from_center_size(center, egui::vec2(width, 20.0));
    let response = ui.interact(rect, ui.scope_id().with(accessible_name), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, accessible_name));
    if on {
        ui.painter()
            .rect_filled(rect, 2.0, crate::theme::palette::BLAZE);
    }
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        if on {
            Color32::BLACK
        } else {
            crate::theme::palette::TEXT_MUTED
        },
    );
    response
}

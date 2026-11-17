//! Reference Default faceplate and mode-dependent parameter controls.

use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke};
use ronie_core::params::{ParameterId, ParameterValue};

use crate::{
    backend::{GesturePhase, HostControl, UiAction, UiBackend},
    widgets::Knob,
};

use super::{
    decks::{DeckKind, DeckModel},
    popover::{Placement, Popover, Surface, Trigger},
};

struct Control {
    id: &'static str,
    label: &'static str,
    deck: Option<DeckKind>,
}

const CONTROLS: &[Control] = &[
    Control {
        id: "inputLevel",
        label: "Input",
        deck: None,
    },
    Control {
        id: "gateThreshold",
        label: "Gate",
        deck: Some(DeckKind::Gate),
    },
    Control {
        id: "pitchSemitones",
        label: "Pitch",
        deck: Some(DeckKind::Pitch),
    },
    Control {
        id: "toneBass",
        label: "Bass",
        deck: None,
    },
    Control {
        id: "toneMid",
        label: "Middle",
        deck: None,
    },
    Control {
        id: "toneTreble",
        label: "Treble",
        deck: None,
    },
    Control {
        id: "spreadOffset",
        label: "Offset",
        deck: Some(DeckKind::Spread),
    },
    Control {
        id: "alignOffset",
        label: "Offset",
        deck: Some(DeckKind::Align),
    },
    Control {
        id: "outputBalance",
        label: "Bal",
        deck: None,
    },
    Control {
        id: "outputLevel",
        label: "Output",
        deck: None,
    },
];

/// Width of the Mono/Stereo lanes switch.
const LANES_WIDTH: f32 = 64.0;
const PRIMARY: f32 = 48.0;
const SECONDARY: f32 = 36.0;
const ICON: f32 = 20.0;
const GROUP_GAP: f32 = 10.0;
const TONE_GAP: f32 = 24.0;
const KNOB_HEIGHT_PRIMARY: f32 = 75.0;
const KNOB_HEIGHT_SECONDARY: f32 = 63.0;

/// Persistent faceplate bindings and optional advanced deck.
pub struct Faceplate {
    backend: Arc<dyn UiBackend>,
    /// Whether the Mono/Stereo lanes switch shows after Input.
    lanes: bool,
    /// A lanes change the user asked for, for the controller to apply.
    lanes_request: Option<bool>,
    knobs: Vec<(Control, Knob)>,
    active_deck: Option<DeckModel>,
    deck_anchor: Option<Pos2>,
    deck_trigger: Option<Trigger>,
    input_mode_open: bool,
}

impl Faceplate {
    /// Creates all faceplate parameter bindings.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>) -> Self {
        let knobs = CONTROLS
            .iter()
            .filter_map(|control| {
                Some((
                    Control {
                        id: control.id,
                        label: control.label,
                        deck: control.deck,
                    },
                    Knob::new(backend.clone(), control.id, control.label).ok()?,
                ))
            })
            .collect();
        Self {
            backend,
            knobs,
            active_deck: None,
            deck_anchor: None,
            deck_trigger: None,
            input_mode_open: false,
            lanes: false,
            lanes_request: None,
        }
    }

    /// Parameter IDs visible in the current stereo mode.
    #[must_use]
    pub fn visible_controls(&self, stereo_enabled: bool, _dual_mono: bool) -> Vec<&'static str> {
        self.knobs
            .iter()
            .filter(|(control, _)| {
                self.effect_visible(control.id)
                    && if control.id == "spreadOffset" {
                        !stereo_enabled
                    } else if control.id == "alignOffset" {
                        stereo_enabled
                    } else {
                        true
                    }
            })
            .map(|(control, _)| control.id)
            .collect()
    }

    fn effect_visible(&self, id: &str) -> bool {
        match id {
            "gateThreshold" => {
                self.backend
                    .preference("t3k.showGateControl")
                    .unwrap_or(true)
                    || self.enabled("gateEnabled")
            }
            "pitchSemitones" => {
                self.backend
                    .preference("t3k.showPitchControl")
                    .unwrap_or(true)
                    || self.enabled("pitchEnabled")
            }
            _ => true,
        }
    }

    fn enabled(&self, id: &'static str) -> bool {
        self.backend
            .parameter(ParameterId(id))
            .is_some_and(|value| value.0 > 0.5)
    }

    /// Opens the input-routing chooser above its trigger.
    pub fn open_input_mode(&mut self) {
        self.input_mode_open = true;
    }

    /// Whether the automatic balance measurement runs.
    #[must_use]
    pub fn measuring(&self) -> bool {
        self.active_deck.as_ref().is_some_and(|deck| {
            deck.kind() == DeckKind::AutoBalance
                && deck.state() == super::decks::MeasurementState::Running
        })
    }

    /// Opens one advanced deck through the same model used by view actions.
    pub fn open_deck(&mut self, kind: DeckKind) {
        let mut deck = DeckModel::new(self.backend.clone(), kind);
        if kind == DeckKind::AutoBalance {
            deck.start();
        }
        self.active_deck = Some(deck);
        self.deck_anchor = None;
        self.deck_trigger = None;
    }

    fn knob(
        &mut self,
        ui: &mut egui::Ui,
        id: &'static str,
        rect: Rect,
        size: f32,
        deck: Option<DeckKind>,
    ) -> Option<Trigger> {
        let (control, knob) = self
            .knobs
            .iter_mut()
            .find(|(control, _)| control.id == id)?;
        let response = knob.show_in_rect(ui, rect, size);
        if response.secondary_clicked()
            && let Some(kind) = control.deck.or(deck)
        {
            self.active_deck = Some(DeckModel::new(self.backend.clone(), kind));
            self.deck_anchor = None;
            self.deck_trigger = None;
        }
        Some(Trigger::from(&response))
    }

    fn set_enabled(&self, id: &'static str, enabled: bool) {
        let _ = self.backend.set_parameter(
            ParameterId(id),
            ParameterValue(f32::from(enabled)),
            GesturePhase::Change,
        );
    }

    /// Renders visible knobs and any advanced deck.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let bounds = ui.available_rect_before_wrap();
        let painter = ui.painter_at(bounds);
        let look = crate::theme::Look::get(ui.ctx());
        painter.rect_filled(bounds, 0.0, look.panel);
        painter.line_segment(
            [bounds.left_top(), bounds.right_top()],
            Stroke::new(1.0, look.edge),
        );
        let content = Rect::from_min_max(
            bounds.min + egui::vec2(30.0, 16.0),
            bounds.max - egui::vec2(30.0, 16.0),
        );
        let baseline = content.bottom();
        let knob_y = |size: f32| baseline + 2.0 - (size + 27.0);
        let chrome_y = baseline - ICON - 30.0;
        let chain = self.backend.chain_state(0).chain;
        let stereo = chain.stereo_enabled;
        let gate = self.effect_visible("gateThreshold");
        let pitch = self.effect_visible("pitchSemitones");
        let input_mode = chain.stereo_input;
        let input_w = PRIMARY
            + if input_mode { GROUP_GAP + 38.0 } else { 0.0 }
            + if self.lanes {
                GROUP_GAP + LANES_WIDTH
            } else {
                0.0
            };
        let effects_w = if gate { 66.0 } else { 0.0 }
            + if pitch { 66.0 } else { 0.0 }
            + if gate && pitch { TONE_GAP } else { 0.0 };
        let tone_w = 3.0 * PRIMARY + 2.0 * TONE_GAP + GROUP_GAP + ICON;
        let image_w = 148.0;
        let output_w = ICON + GROUP_GAP + SECONDARY + GROUP_GAP + PRIMARY;
        let peers = 4 + usize::from(effects_w > 0.0);
        let gap = (content.width() - (input_w + effects_w + tone_w + image_w + output_w))
            / (peers - 1) as f32;
        let mut x = content.left();
        let primary_rect = |x: f32| {
            Rect::from_min_size(
                egui::pos2(x.round(), knob_y(PRIMARY).round()),
                egui::vec2(PRIMARY, KNOB_HEIGHT_PRIMARY),
            )
        };
        let secondary_rect = |x: f32| {
            Rect::from_min_size(
                egui::pos2(x.round(), knob_y(SECONDARY).round()),
                egui::vec2(SECONDARY, KNOB_HEIGHT_SECONDARY),
            )
        };

        let _ = self.knob(ui, "inputLevel", primary_rect(x), PRIMARY, None);
        if input_mode {
            let mode_rect = Rect::from_min_size(
                egui::pos2(x + PRIMARY + GROUP_GAP, chrome_y),
                egui::vec2(38.0, ICON),
            );
            let response = ui.interact(mode_rect, ui.scope_id().with("input-mode"), Sense::click());
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Input mode"));
            let routing_mode = self.backend.chain_state(0).session.input_mode;
            if routing_mode != ronie_core::InputMode::Stereo {
                painter.rect_filled(mode_rect, 2.0, crate::theme::palette::PANEL_EDGE);
            }
            match routing_mode {
                ronie_core::InputMode::Left | ronie_core::InputMode::Right => {
                    painter.text(
                        egui::pos2(mode_rect.left() + 10.0, mode_rect.center().y),
                        egui::Align2::CENTER_CENTER,
                        if routing_mode == ronie_core::InputMode::Left {
                            "L"
                        } else {
                            "R"
                        },
                        egui::FontId::new(11.0, egui::FontFamily::Name("roboto-mono-bold".into())),
                        Color32::WHITE,
                    );
                }
                ronie_core::InputMode::Stereo | ronie_core::InputMode::DualMono => {
                    painter.circle_stroke(
                        mode_rect.center() + egui::vec2(-9.0, 0.0),
                        4.0,
                        Stroke::new(1.2, Color32::WHITE),
                    );
                    painter.circle_stroke(
                        mode_rect.center() + egui::vec2(-2.0, 0.0),
                        4.0,
                        Stroke::new(1.2, Color32::WHITE),
                    );
                }
            }
            let chevron = mode_rect.center() + egui::vec2(11.0, 0.0);
            let stroke = Stroke::new(1.5, crate::theme::palette::TEXT_MUTED);
            painter.line_segment(
                [
                    chevron + egui::vec2(-4.0, -2.0),
                    chevron + egui::vec2(0.0, 2.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    chevron + egui::vec2(0.0, 2.0),
                    chevron + egui::vec2(4.0, -2.0),
                ],
                stroke,
            );
            if response.clicked() {
                self.input_mode_open = !self.input_mode_open;
            }
            self.show_input_mode_menu(ui, &response, stereo, chain.branch.is_some());
        }
        if self.lanes {
            let lanes_x = x + input_w - LANES_WIDTH;
            self.show_lanes(
                ui,
                &painter,
                lanes_x,
                content.top(),
                content.bottom(),
                stereo,
            );
        }
        x += input_w + gap;

        if gate {
            let rect = secondary_rect(x);
            let knob = self.knob(ui, "gateThreshold", rect, SECONDARY, Some(DeckKind::Gate));
            if !self.enabled("gateEnabled") {
                painter.rect_filled(rect, 0.0, crate::theme::Look::get(painter.ctx()).veil);
            }
            power_button(
                ui,
                &self.backend,
                egui::pos2(x + SECONDARY + GROUP_GAP, chrome_y),
                "gateEnabled",
                "Gate power",
            );
            if self
                .active_deck
                .as_ref()
                .is_some_and(|deck| deck.kind() == DeckKind::Gate)
            {
                self.deck_anchor = Some(rect.left_top());
                self.deck_trigger = knob;
            }
            x += 66.0 + if pitch { TONE_GAP } else { 0.0 };
        }
        if pitch {
            let rect = secondary_rect(x);
            let caption = super::tuning::caption(self.backend.as_ref());
            if let Some((_, knob)) = self
                .knobs
                .iter_mut()
                .find(|(control, _)| control.id == "pitchSemitones")
            {
                knob.set_caption(Some(caption));
            }
            let knob = self.knob(ui, "pitchSemitones", rect, SECONDARY, Some(DeckKind::Pitch));
            // OPTIMIZE follows the shift at the end of a gesture, not on each frame of a drag.
            let ended = self.knobs.iter().any(|(control, knob)| {
                control.id == "pitchSemitones" && knob.last_phase() == Some(GesturePhase::End)
            });
            if ended {
                let _ = super::tuning::apply_optimize(self.backend.as_ref());
            }
            if !self.enabled("pitchEnabled") {
                painter.rect_filled(rect, 0.0, crate::theme::Look::get(painter.ctx()).veil);
            }
            power_button(
                ui,
                &self.backend,
                egui::pos2(x + SECONDARY + GROUP_GAP, chrome_y),
                "pitchEnabled",
                "Pitch power",
            );
            if self
                .active_deck
                .as_ref()
                .is_some_and(|deck| deck.kind() == DeckKind::Pitch)
            {
                self.deck_anchor = Some(rect.left_top());
                self.deck_trigger = knob;
            }
        }
        if effects_w > 0.0 {
            x += 66.0 * f32::from(pitch) + gap;
        }

        for (offset, id) in [
            (0.0, "toneBass"),
            (PRIMARY + TONE_GAP, "toneMid"),
            (2.0 * (PRIMARY + TONE_GAP), "toneTreble"),
        ] {
            let _ = self.knob(ui, id, primary_rect(x + offset), PRIMARY, None);
        }
        if !self.enabled("toneEqEnabled") {
            painter.rect_filled(
                Rect::from_min_size(
                    egui::pos2(x, knob_y(PRIMARY)),
                    egui::vec2(3.0 * PRIMARY + 2.0 * TONE_GAP, KNOB_HEIGHT_PRIMARY),
                ),
                0.0,
                crate::theme::Look::get(painter.ctx()).veil,
            );
        }
        power_button(
            ui,
            &self.backend,
            egui::pos2(x + 3.0 * PRIMARY + 2.0 * TONE_GAP + GROUP_GAP, chrome_y),
            "toneEqEnabled",
            "Tone EQ power",
        );
        x += tone_w + gap;

        let image_kind = if stereo {
            DeckKind::Align
        } else {
            DeckKind::Spread
        };
        let image_id = if stereo {
            "alignOffset"
        } else {
            "spreadOffset"
        };
        let image_enabled = self.enabled(if stereo {
            "alignEnabled"
        } else {
            "spreadEnabled"
        });
        let advert_rect = Rect::from_min_size(
            egui::pos2(x, baseline - 40.0 - PRIMARY / 2.0),
            egui::vec2(image_w, PRIMARY),
        );
        if image_enabled {
            let row_w = ICON + GROUP_GAP + PRIMARY + GROUP_GAP + ICON;
            let row_x = x + (image_w - row_w) / 2.0;
            let offset_rect = primary_rect(row_x + ICON + GROUP_GAP);
            let knob = self.knob(ui, image_id, offset_rect, PRIMARY, Some(image_kind));
            power_button(
                ui,
                &self.backend,
                egui::pos2(row_x + ICON + GROUP_GAP + PRIMARY + GROUP_GAP, chrome_y),
                if stereo {
                    "alignEnabled"
                } else {
                    "spreadEnabled"
                },
                "Stereo image power",
            );
            if self
                .active_deck
                .as_ref()
                .is_some_and(|deck| deck.kind() == image_kind)
            {
                self.deck_anchor = Some(offset_rect.left_top());
                self.deck_trigger = knob;
            }
        } else {
            let response = ui.interact(
                advert_rect,
                ui.scope_id().with("image-advert"),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Button,
                    true,
                    if stereo {
                        "Enable Align"
                    } else {
                        "Enable Spread"
                    },
                )
            });
            draw_advert(
                &painter,
                advert_rect,
                if stereo { "ALIGN" } else { "SPREAD" },
                stereo,
            );
            if response.clicked() {
                self.set_enabled(
                    if stereo {
                        "alignEnabled"
                    } else {
                        "spreadEnabled"
                    },
                    true,
                );
            }
            if response.secondary_clicked() {
                self.active_deck = Some(DeckModel::new(self.backend.clone(), image_kind));
            }
            if self
                .active_deck
                .as_ref()
                .is_some_and(|deck| deck.kind() == image_kind)
            {
                self.deck_anchor = Some(advert_rect.right_top());
                self.deck_trigger = Some(Trigger::from(&response));
            }
        }
        x += image_w + gap;

        let balance_active = stereo
            || chain.dual_mono_active
            || (self.enabled("spreadEnabled") && chain.stereo_output);
        if (stereo || chain.dual_mono_active) && self.backend.supports(HostControl::AutoBalance) {
            let rect = Rect::from_min_size(egui::pos2(x, chrome_y), egui::Vec2::splat(ICON));
            let running = self.measuring();
            let response = ui.interact(rect, ui.scope_id().with("auto-balance"), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Auto balance")
            });
            let accent = if running {
                crate::theme::palette::BLAZE
            } else {
                crate::theme::palette::HAIRLINE
            };
            if running {
                painter.rect_filled(rect, 2.0, accent);
            }
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, accent),
                egui::StrokeKind::Inside,
            );
            let icon_color = if running {
                Color32::BLACK
            } else {
                crate::theme::Look::get(painter.ctx()).text_muted
            };
            let stroke = Stroke::new(7.0 / 6.0, icon_color);
            for y in [-7.0 / 3.0, 7.0 / 3.0] {
                painter.line_segment(
                    [
                        rect.center() + egui::vec2(-14.0 / 3.0, y),
                        rect.center() + egui::vec2(14.0 / 3.0, y),
                    ],
                    stroke,
                );
            }
            if response.clicked() {
                if running {
                    if let Some(deck) = &mut self.active_deck {
                        deck.cancel();
                    }
                } else {
                    self.open_deck(DeckKind::AutoBalance);
                }
            }
        }
        if balance_active {
            let _ = self.knob(
                ui,
                "outputBalance",
                secondary_rect(x + ICON + GROUP_GAP),
                SECONDARY,
                None,
            );
        }
        let _ = self.knob(
            ui,
            "outputLevel",
            primary_rect(x + ICON + GROUP_GAP + SECONDARY + GROUP_GAP),
            PRIMARY,
            None,
        );

        // Decks float on a dark surface, so they keep the dark colours in any layout.
        let look = crate::theme::Look::get(ui.ctx());
        crate::theme::Look::install(ui.ctx(), crate::theme::Look::reference());
        self.show_deck(ui, bounds.top());
        crate::theme::Look::install(ui.ctx(), look);
        self.close_hidden_drags(ui.ctx().cumulative_pass_nr());
    }

    /// Closes the gesture of each knob drag when `pass` did not show that knob.
    ///
    /// The faceplate calls this at the end of its own frame. The page switch also calls it in a
    /// pass that does not show the faceplate, so that a page that covers the faceplate closes
    /// the drag at once.
    pub(crate) fn close_hidden_drags(&mut self, pass: u64) {
        for (_, knob) in &mut self.knobs {
            knob.end_hidden_drag(pass);
        }
        if let Some(deck) = &mut self.active_deck {
            deck.end_hidden_drags(pass);
        }
    }

    fn show_input_mode_menu(
        &mut self,
        ui: &mut egui::Ui,
        trigger: &egui::Response,
        stereo_chains: bool,
        branched: bool,
    ) {
        if !self.input_mode_open {
            return;
        }
        let current = self.backend.chain_state(0).session.input_mode;
        let options = [
            (ronie_core::InputMode::Stereo, "Stereo SUM (L+R)"),
            (ronie_core::InputMode::DualMono, "Stereo Dual Mono (L&R)"),
            (ronie_core::InputMode::Left, "Left"),
            (ronie_core::InputMode::Right, "Right"),
        ]
        .into_iter()
        .filter(|(mode, _)| {
            !(branched
                && matches!(
                    mode,
                    ronie_core::InputMode::Stereo | ronie_core::InputMode::DualMono
                ))
                && !(stereo_chains && *mode == ronie_core::InputMode::DualMono)
        })
        .map(|(mode, label)| {
            (
                mode,
                if stereo_chains && mode == ronie_core::InputMode::Stereo {
                    "Stereo"
                } else {
                    label
                },
            )
        })
        .collect::<Vec<_>>();
        let height = 43.0 + options.len() as f32 * 33.0;
        let shown = Popover::new(
            ui.scope_id().with("input-mode-menu"),
            Placement::Above {
                anchor: trigger.rect,
                gap: 14.0,
            },
            egui::vec2(224.0, height),
            Surface::hairline(14.0),
        )
        .trigger(Trigger::from(trigger))
        .show(ui, |ui, panel| {
            let painter = ui.painter_at(panel);
            let mut pick = None;
            painter.text(
                egui::pos2(panel.left() + 20.0, panel.top() + 12.0),
                egui::Align2::LEFT_TOP,
                "Input Mode",
                egui::FontId::new(11.0, egui::FontFamily::Name("arimo-bold".into())),
                Color32::from_rgb(117, 117, 122),
            );
            for (index, (mode, label)) in options.into_iter().enumerate() {
                let row = Rect::from_min_size(
                    egui::pos2(panel.left() + 8.0, panel.top() + 33.0 + index as f32 * 33.0),
                    egui::vec2(panel.width() - 16.0, 33.0),
                );
                let response = ui.interact(
                    row,
                    ui.scope_id().with(("input-mode-option", label)),
                    Sense::click(),
                );
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                if response.hovered() {
                    painter.rect_filled(row, 8.0, crate::theme::palette::PANEL_EDGE);
                }
                let colour = if mode == current {
                    Color32::WHITE
                } else {
                    crate::theme::palette::TEXT_MUTED
                };
                draw_input_mode_glyph(
                    &painter,
                    mode,
                    Rect::from_center_size(
                        egui::pos2(row.left() + 22.0, row.center().y),
                        egui::vec2(20.0, 20.0),
                    ),
                    colour,
                );
                painter.text(
                    egui::pos2(row.left() + 44.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(13.0),
                    colour,
                );
                if response.clicked() {
                    pick = Some(mode);
                }
            }
            pick
        });
        if let Some(mode) = shown.pick {
            let _ = self.backend.dispatch(UiAction::SetInputMode(mode));
        }
        if shown.dismissed.is_some() {
            self.input_mode_open = false;
        }
    }

    /// Shows the open deck above its knob. `bar_top` is the top edge of the bottom bar.
    /// Shows the Mono/Stereo lanes switch after Input.
    pub fn set_show_lanes(&mut self, shown: bool) {
        self.lanes = shown;
    }

    /// The lanes change the user asked for since the last call: `Some(true)` for stereo.
    pub fn take_lanes_request(&mut self) -> Option<bool> {
        self.lanes_request.take()
    }

    fn show_lanes(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        x: f32,
        top: f32,
        bottom: f32,
        stereo: bool,
    ) {
        let look = crate::theme::Look::get(ui.ctx());
        let center_x = x + LANES_WIDTH / 2.0;
        painter.text(
            egui::pos2(center_x, top + 7.0),
            egui::Align2::CENTER_CENTER,
            "LANES",
            egui::FontId::new(11.0, egui::FontFamily::Name("arimo-bold".into())),
            look.text,
        );
        let track = Rect::from_min_size(egui::pos2(x + 4.0, top + 22.0), egui::vec2(18.0, 40.0));
        painter.rect(
            track,
            9.0,
            look.edge,
            Stroke::new(1.0, look.edge),
            egui::StrokeKind::Inside,
        );
        let dot_y = if stereo {
            track.top() + 9.0
        } else {
            track.bottom() - 9.0
        };
        painter.circle_filled(egui::pos2(track.center().x, dot_y), 6.0, look.text);
        let rows = [
            ("Stereo", "Stereo mode", true, track.top() + 9.0),
            ("Mono", "Mono mode", false, track.bottom() - 9.0),
        ];
        for (text, label, wants_stereo, y) in rows {
            let row =
                Rect::from_min_max(egui::pos2(x, y - 9.0), egui::pos2(x + LANES_WIDTH, y + 9.0));
            let response = ui.interact(row, ui.scope_id().with(label), Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
            painter.text(
                egui::pos2(track.right() + 8.0, y),
                egui::Align2::LEFT_CENTER,
                text,
                egui::FontId::proportional(11.0),
                if wants_stereo == stereo {
                    look.text
                } else {
                    look.text_muted
                },
            );
            if response.clicked() && wants_stereo != stereo {
                self.lanes_request = Some(wants_stereo);
            }
        }
        let _ = bottom;
    }

    fn show_deck(&mut self, ui: &mut egui::Ui, bar_top: f32) {
        let Some(kind) = self.active_deck.as_ref().map(DeckModel::kind) else {
            return;
        };
        if kind == DeckKind::AutoBalance {
            return;
        }
        let image = matches!(kind, DeckKind::Spread | DeckKind::Align);
        let size = self
            .active_deck
            .as_ref()
            .map_or(egui::vec2(262.0, 85.0), DeckModel::size);
        let anchor = self.deck_anchor.unwrap_or_else(|| egui::pos2(30.0, 470.0));
        let right_anchored = image
            && !self.enabled(if kind == DeckKind::Align {
                "alignEnabled"
            } else {
                "spreadEnabled"
            });
        let position = egui::pos2(
            if right_anchored {
                anchor.x - size.x
            } else {
                anchor.x
            },
            if kind == DeckKind::Pitch {
                // The pitch deck is taller than the others, and it stays clear of the bottom bar.
                (anchor.y - size.y - 6.0).min(bar_top - size.y - 8.0)
            } else {
                anchor.y - size.y - 6.0
            },
        );
        let Some(deck) = &mut self.active_deck else {
            return;
        };
        let shown = Popover::new(
            ui.scope_id().with("advanced-deck"),
            Placement::At(position),
            size,
            Surface::Frame(
                egui::Frame::NONE
                    .fill(crate::theme::palette::PANEL)
                    .stroke(Stroke::new(1.0, crate::theme::palette::HAIRLINE))
                    .corner_radius(14.0),
            ),
        )
        .trigger(self.deck_trigger)
        .show(ui, |ui, rect| {
            ui.set_min_size(size);
            ui.set_clip_rect(rect);
            deck.show_in_rect(ui, rect);
            None::<()>
        });
        if shown.dismissed.is_some() {
            self.active_deck = None;
        }
    }
}

fn draw_input_mode_glyph(
    painter: &egui::Painter,
    mode: ronie_core::InputMode,
    rect: Rect,
    colour: Color32,
) {
    if matches!(
        mode,
        ronie_core::InputMode::Stereo | ronie_core::InputMode::DualMono
    ) {
        painter.circle_stroke(
            rect.center() + egui::vec2(-4.0, 0.0),
            4.0,
            Stroke::new(1.2, colour),
        );
        painter.circle_stroke(
            rect.center() + egui::vec2(3.0, 0.0),
            4.0,
            Stroke::new(1.2, colour),
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if mode == ronie_core::InputMode::Left {
                "L"
            } else {
                "R"
            },
            egui::FontId::new(11.0, egui::FontFamily::Name("roboto-mono-bold".into())),
            colour,
        );
    }
}

fn power_button(
    ui: &mut egui::Ui,
    backend: &Arc<dyn UiBackend>,
    position: Pos2,
    id: &'static str,
    label: &'static str,
) {
    let rect = Rect::from_min_size(position, egui::Vec2::splat(ICON));
    let enabled = backend
        .parameter(ParameterId(id))
        .is_some_and(|value| value.0 > 0.5);
    let response = ui.interact(rect, ui.scope_id().with((id, "power")), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    draw_power(ui.painter(), rect, enabled);
    if response.clicked() {
        let _ = backend.set_parameter(
            ParameterId(id),
            ParameterValue(f32::from(!enabled)),
            GesturePhase::Change,
        );
    }
}

fn draw_power(painter: &egui::Painter, rect: Rect, enabled: bool) {
    let color = if enabled {
        crate::theme::Look::get(painter.ctx()).text
    } else {
        crate::theme::Look::get(painter.ctx()).text_muted
    };
    let stroke = Stroke::new(1.2, color);
    let center = rect.center();
    let mut points = Vec::new();
    for index in 0..=32 {
        let degrees = -40.0 + index as f32 / 32.0 * 260.0;
        let radians = degrees.to_radians();
        points.push(center + egui::vec2(radians.cos() * 5.25, radians.sin() * 5.25));
    }
    painter.add(egui::epaint::PathShape::line(points, stroke));
    painter.line_segment([center - egui::vec2(0.0, 5.83), center], stroke);
}

fn draw_advert(painter: &egui::Painter, rect: Rect, label: &'static str, inward: bool) {
    painter.rect_stroke(
        rect,
        24.0,
        Stroke::new(1.0, crate::theme::Look::get(painter.ctx()).text),
        egui::StrokeKind::Inside,
    );
    let font = egui::FontId::proportional(12.0);
    let text_width = painter.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
    });
    let mut x = rect.center().x - (56.0 + text_width) / 2.0;
    for points_right in [inward, !inward] {
        let center = egui::pos2(x + 10.0, rect.center().y);
        let point = |px: f32, py: f32| {
            let px = if points_right { px } else { 24.0 - px };
            center + egui::vec2((px - 12.0) * 0.7, (py - 5.0) * 0.7)
        };
        painter.add(egui::epaint::PathShape::line(
            vec![
                point(2.0, 2.0),
                point(20.5, 5.0),
                point(2.0, 8.0),
                point(2.0, 2.0),
            ],
            Stroke::new(0.805, crate::theme::Look::get(painter.ctx()).text),
        ));
        if points_right == inward {
            x += 28.0 + text_width;
            painter.text(
                egui::pos2(x - text_width / 2.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                label,
                font.clone(),
                crate::theme::Look::get(painter.ctx()).text,
            );
            x += 8.0;
        }
    }
}

//! Model stepper, active-row dropdown paging, and generation-safe catalog state.

use std::{
    ops::Range,
    sync::atomic::{AtomicU64, Ordering},
};

use eframe::egui::{self, Color32, Rect, Sense};
use ronie_core::{ModelId, ToneId};

use super::popover::{Placement, Popover, Surface, Trigger};
use crate::services::session::SessionStatus;

/// Source of fetch generations. All pickers share it, so that a late page of a closed picker
/// cannot match the generation of the next picker of the same tone.
static NEXT_FETCH_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_fetch_generation() -> u64 {
    NEXT_FETCH_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// One selectable model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    /// Stable model identity.
    pub id: ModelId,
    /// Display name.
    pub name: String,
}

/// Model picker with at most five visible rows and generation-safe list updates.
pub struct ModelSelect {
    local: bool,
    options: Vec<ModelOption>,
    active: Option<ModelId>,
    /// Whether a choice only becomes active after its bytes arrive.
    deferred: bool,
    /// Choice that waits for its bytes. The picker shows it, and the chain does not hold it yet.
    target: Option<ModelId>,
    total: usize,
    loading: bool,
    open: bool,
    generation: u64,
    fetch_tone: Option<ToneId>,
}

impl ModelSelect {
    /// Creates a picker with local or catalog policy.
    #[must_use]
    pub fn new(local: bool) -> Self {
        Self {
            local,
            options: Vec::new(),
            active: None,
            deferred: false,
            target: None,
            total: 0,
            loading: false,
            open: false,
            generation: 0,
            fetch_tone: None,
        }
    }

    /// Replaces options and their catalog-wide total.
    pub fn set_options(&mut self, options: Vec<ModelOption>, total: usize) {
        self.options = options;
        self.total = total;
    }

    /// Current options.
    #[must_use]
    pub fn options(&self) -> &[ModelOption] {
        &self.options
    }

    /// Sets the active model without changing the option list.
    pub fn set_active(&mut self, id: ModelId) {
        self.active = Some(id);
    }

    /// Active model.
    #[must_use]
    pub const fn active(&self) -> Option<ModelId> {
        self.active
    }

    /// Whether catalog switching is unavailable while signed out.
    #[must_use]
    pub const fn disabled(&self, session: SessionStatus) -> bool {
        !self.local && !session.signed_in()
    }

    /// Whether a previous or next model exists.
    #[must_use]
    pub fn can_step(&self, delta: isize, session: SessionStatus) -> bool {
        if self.disabled(session) {
            return false;
        }
        let Some(index) = self.current_index() else {
            return false;
        };
        let next = index as isize + delta;
        next >= 0 && next < self.options.len() as isize
    }

    /// Steps to an adjacent model without opening the dropdown.
    pub fn step(&mut self, delta: isize, session: SessionStatus) -> Option<ModelId> {
        if !self.can_step(delta, session) {
            return None;
        }
        let index = self.current_index()? as isize + delta;
        let option = self.options.get(index as usize)?.id;
        self.choose(option);
        Some(option)
    }

    /// Selects a model from the dropdown and closes it.
    pub fn select(&mut self, id: ModelId, session: SessionStatus) -> Option<ModelId> {
        if self.disabled(session) || !self.options.iter().any(|option| option.id == id) {
            return None;
        }
        self.choose(id);
        self.close();
        Some(id)
    }

    /// Range containing at most five rows, scrolled to the active model.
    #[must_use]
    pub fn visible_range(&self) -> Range<usize> {
        let end = self.options.len().min(5);
        let active = self.current_index().unwrap_or(0);
        let start = active
            .saturating_sub(4)
            .min(self.options.len().saturating_sub(5));
        start..start + end.min(self.options.len() - start)
    }

    /// Opens the dropdown unless disabled.
    pub fn open(&mut self, session: SessionStatus) {
        if !self.disabled(session) {
            self.open = true;
        }
    }

    /// Closes the dropdown.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Whether the dropdown is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Starts a model-page generation and marks it loading.
    pub fn begin_fetch(&mut self, tone_id: ToneId) -> u64 {
        self.generation = next_fetch_generation();
        self.fetch_tone = Some(tone_id);
        self.loading = true;
        self.generation
    }

    /// Applies only the newest page for the same tone.
    pub fn complete_fetch(
        &mut self,
        generation: u64,
        tone_id: ToneId,
        options: Vec<ModelOption>,
        total: usize,
    ) -> bool {
        if generation != self.generation || self.fetch_tone != Some(tone_id) {
            return false;
        }
        self.set_options(options, total);
        self.loading = false;
        true
    }

    /// Finishes a failed fetch while preserving the stored active model and previous options.
    pub fn fail_fetch(&mut self, generation: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        true
    }

    /// Stops the fetch in flight, so that its page cannot complete.
    pub fn cancel_fetch(&mut self) {
        self.generation = next_fetch_generation();
        self.loading = false;
    }

    /// Makes a choice wait for its bytes. A choice of the active model needs no wait.
    pub fn set_deferred(&mut self, deferred: bool) {
        self.deferred = deferred;
    }

    /// Choice that waits for its bytes.
    #[must_use]
    pub const fn target(&self) -> Option<ModelId> {
        self.target
    }

    /// Forgets the choice that waits, and the picker shows the active model again.
    pub fn clear_target(&mut self) {
        self.target = None;
    }

    /// Whether a catalog fetch is in flight.
    #[must_use]
    pub const fn loading(&self) -> bool {
        self.loading
    }

    /// Renders the picker and returns a model selected by the user.
    pub fn show(&mut self, ui: &mut egui::Ui, session: SessionStatus) -> Option<ModelId> {
        let width = ui.available_width().max(180.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 36.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let content_color = if self.disabled(session) {
            Color32::from_white_alpha(96)
        } else {
            Color32::WHITE
        };
        painter.rect_filled(
            rect,
            8.0,
            if self.disabled(session) {
                Color32::from_rgb(19, 19, 20)
            } else {
                crate::theme::palette::CONTROL_FILL
            },
        );
        let prev = Rect::from_min_size(rect.min + egui::vec2(12.0, 8.0), egui::Vec2::splat(20.0));
        let next = Rect::from_min_size(
            egui::pos2(rect.right() - 123.0, rect.top() + 8.0),
            egui::Vec2::splat(20.0),
        );
        let tally = Rect::from_min_size(
            egui::pos2(rect.right() - 82.0, rect.top()),
            egui::vec2(70.0, 36.0),
        );
        painter.line_segment(
            [
                egui::pos2(rect.right() - 93.0, rect.top() + 8.0),
                egui::pos2(rect.right() - 93.0, rect.bottom() - 8.0),
            ],
            egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
        );
        let mut selected = None;
        let previous = ui
            .add_enabled_ui(self.can_step(-1, session), |ui| {
                ui.interact(prev, ui.scope_id().with("model-previous"), Sense::click())
            })
            .inner;
        previous.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                self.can_step(-1, session),
                "Previous model",
            )
        });
        if previous.clicked() {
            selected = self.step(-1, session);
        }
        let next_response = ui
            .add_enabled_ui(self.can_step(1, session), |ui| {
                ui.interact(next, ui.scope_id().with("model-next"), Sense::click())
            })
            .inner;
        next_response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, self.can_step(1, session), "Next model")
        });
        if next_response.clicked() {
            selected = self.step(1, session);
        }
        draw_model_chevron(
            &painter,
            prev,
            false,
            if self.can_step(-1, session) {
                Color32::WHITE
            } else {
                Color32::from_white_alpha(115)
            },
        );
        draw_model_chevron(
            &painter,
            next,
            true,
            if self.can_step(1, session) {
                Color32::WHITE
            } else {
                Color32::from_white_alpha(115)
            },
        );
        let index = self.current_index().map_or(0, |index| index + 1);
        let tally_text = format!("{index}/{}", self.total);
        let tally_font = egui::FontId::proportional(13.0);
        let tally_color = content_color;
        let text_width = ui.ctx().fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(tally_text.clone(), tally_font.clone(), tally_color)
                .size()
                .x
        });
        let content_width = 20.0 + text_width;
        let content_x = tally.center().x - content_width / 2.0;
        draw_model_folder(&painter, egui::pos2(content_x + 7.0, tally.center().y));
        painter.text(
            egui::pos2(content_x + 20.0, tally.center().y),
            egui::Align2::LEFT_CENTER,
            tally_text,
            tally_font,
            tally_color,
        );
        let trigger = Rect::from_min_max(
            prev.right_top() + egui::vec2(10.0, 0.0),
            next.left_bottom() - egui::vec2(10.0, 0.0),
        );
        let trigger_response = ui
            .add_enabled_ui(!self.disabled(session), |ui| {
                ui.interact(trigger, ui.scope_id().with("model-select"), Sense::click())
            })
            .inner;
        trigger_response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, !self.disabled(session), "Select model")
        });
        if trigger_response.clicked() {
            if self.open {
                self.close();
            } else {
                self.open(session);
            }
        }
        let name = self
            .current_index()
            .and_then(|index| self.options.get(index))
            .map_or("Select models...", |option| option.name.as_str());
        painter.with_clip_rect(trigger).text(
            trigger.center(),
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(14.0),
            content_color,
        );
        if self.open {
            selected =
                selected.or_else(|| self.show_dropdown(ui, rect, trigger_response.id, session));
        }
        selected
    }

    fn show_dropdown(
        &mut self,
        ui: &mut egui::Ui,
        anchor: Rect,
        focus: egui::Id,
        session: SessionStatus,
    ) -> Option<ModelId> {
        let rows = self.visible_range();
        let loading_rows = usize::from(self.loading);
        let row_count = rows.len();
        let height = (row_count * 41 + row_count.saturating_sub(1) + loading_rows * 28) as f32;
        let options = self.options.clone();
        let active = self.shown();
        let shown = Popover::new(
            ui.scope_id().with("model-select-list"),
            Placement::Above { anchor, gap: 4.0 },
            egui::vec2(anchor.width(), height),
            Surface::Panel {
                fill: crate::theme::palette::LIST_FILL,
                radius: 8.0,
                edge: None,
            },
        )
        .trigger(Trigger::new(anchor, Some(focus)))
        .show(ui, |ui, list_rect| {
            ui.set_clip_rect(list_rect);
            let painter = ui.painter_at(list_rect);
            let mut selected = None;
            for (row, index) in rows.enumerate() {
                let Some(option) = options.get(index) else {
                    continue;
                };
                let row_rect = Rect::from_min_size(
                    list_rect.min + egui::vec2(0.0, row as f32 * 42.0),
                    egui::vec2(list_rect.width(), 41.0),
                );
                let response = ui.interact(
                    row_rect,
                    ui.scope_id().with(("model-option", option.id.0)),
                    Sense::click(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, option.name.clone())
                });
                if active == Some(option.id) || response.hovered() {
                    painter.rect_filled(row_rect, 0.0, Color32::from_white_alpha(25));
                }
                painter.text(
                    row_rect.left_center() + egui::vec2(16.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &option.name,
                    egui::FontId::proportional(14.0),
                    Color32::WHITE,
                );
                if row + 1 < row_count {
                    let y = row_rect.bottom();
                    painter.line_segment(
                        [
                            egui::pos2(list_rect.left(), y),
                            egui::pos2(list_rect.right(), y),
                        ],
                        egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                    );
                }
                if response.clicked() {
                    selected = self.select(option.id, session);
                }
            }
            if self.loading {
                painter.text(
                    list_rect.left_bottom() + egui::vec2(16.0, -14.0),
                    egui::Align2::LEFT_CENTER,
                    "Loading…",
                    egui::FontId::proportional(14.0),
                    Color32::WHITE,
                );
            }
            selected
        });
        if shown.dismissed.is_some() {
            self.close();
        }
        shown.pick
    }

    fn choose(&mut self, id: ModelId) {
        if self.deferred {
            self.target = (self.active != Some(id)).then_some(id);
        } else {
            self.active = Some(id);
        }
    }

    /// The model that the picker shows.
    fn shown(&self) -> Option<ModelId> {
        self.target.or(self.active)
    }

    fn current_index(&self) -> Option<usize> {
        let shown = self.shown();
        self.options
            .iter()
            .position(|option| Some(option.id) == shown)
    }
}

fn draw_model_chevron(painter: &egui::Painter, rect: Rect, right: bool, color: Color32) {
    let point = |x: f32, y: f32| rect.min + egui::vec2(x * 20.0 / 24.0, y * 20.0 / 24.0);
    let points = if right {
        vec![point(9.0, 18.0), point(15.0, 12.0), point(9.0, 6.0)]
    } else {
        vec![point(15.0, 18.0), point(9.0, 12.0), point(15.0, 6.0)]
    };
    painter.add(egui::epaint::PathShape::line(
        points,
        egui::Stroke::new(5.0 / 3.0, color),
    ));
}

fn draw_model_folder(painter: &egui::Painter, center: egui::Pos2) {
    let color = Color32::from_white_alpha(153);
    let point =
        |x: f32, y: f32| center + egui::vec2((x - 12.0) * 14.0 / 24.0, (y - 12.0) * 14.0 / 24.0);
    let stroke = egui::Stroke::new(7.0 / 6.0, color);
    painter.add(egui::epaint::PathShape::line(
        vec![
            point(20.0, 20.0),
            point(22.0, 18.0),
            point(22.0, 8.0),
            point(20.0, 6.0),
            point(12.1, 6.0),
            point(10.41, 5.1),
            point(9.6, 3.9),
            point(7.93, 3.0),
            point(4.0, 3.0),
            point(2.0, 5.0),
            point(2.0, 18.0),
            point(4.0, 20.0),
            point(20.0, 20.0),
        ],
        stroke,
    ));
    painter.line_segment([point(2.0, 10.0), point(22.0, 10.0)], stroke);
}

//! Tone metadata expansion, signed-out prompts, and public links.

use eframe::egui;
use ronie_core::Tone;

use crate::services::session::SessionStatus;

/// Prompt shown instead of metadata sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoPrompt {
    /// Authentication is required.
    SignIn,
    /// A foreground metadata request failed.
    Retry,
}

/// Action selected in the metadata panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoAction {
    /// Start sign-in.
    SignIn,
    /// Retry the metadata request.
    Retry,
    /// Open the public tone page.
    OpenUrl(String),
}

/// Computed metadata layout independent of the renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoLayout {
    /// Total preferred height.
    pub height: i32,
    /// Visible description line count after clamping.
    pub description_lines: usize,
    /// Whether MORE/LESS is needed.
    pub show_more: bool,
    /// Clean makes and models.
    pub makes: Vec<String>,
    /// Clean, deduplicated tags.
    pub tags: Vec<String>,
    /// Public tone page URL.
    pub page_url: String,
    /// Optional sign-in or retry prompt.
    pub prompt: Option<InfoPrompt>,
}

/// Metadata state with a three-line description clamp.
#[derive(Default)]
pub struct BlockInfoPanel {
    error: Option<String>,
    tone: Option<Tone>,
    page_url: String,
    desc_expanded: bool,
}

impl BlockInfoPanel {
    /// Sets or clears a foreground metadata error.
    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    /// Replaces the tone and resets expansion when its description changes.
    pub fn set_tone(&mut self, tone: Option<Tone>) {
        let old_description = self.tone.as_ref().and_then(description);
        let new_description = tone.as_ref().and_then(description);
        if old_description != new_description {
            self.desc_expanded = false;
        }
        self.tone = tone;
    }

    /// Sets the public page URL.
    pub fn set_page_url(&mut self, url: impl Into<String>) {
        self.page_url = url.into();
    }

    /// Toggles MORE/LESS.
    pub fn toggle_more(&mut self) {
        self.desc_expanded = !self.desc_expanded;
    }

    /// Computes all visible metadata state at one width.
    #[must_use]
    pub fn layout(&self, width: i32, session: SessionStatus) -> InfoLayout {
        let prompt = if session.can_sign_in() {
            Some(InfoPrompt::SignIn)
        } else {
            self.error.as_ref().map(|_| InfoPrompt::Retry)
        };
        let makes = self
            .tone
            .as_ref()
            .map(|tone| clean_strings(tone.raw.get("makes")))
            .unwrap_or_default();
        let mut tags = self
            .tone
            .as_ref()
            .map(|tone| clean_strings(tone.raw.get("tags")))
            .unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();
        tags.retain(|tag| seen.insert(tag.clone()));
        let lines = self
            .tone
            .as_ref()
            .and_then(description)
            .map(|text| wrap_text(&text, width))
            .unwrap_or_default();
        let description_lines = if self.desc_expanded {
            lines.len()
        } else {
            lines.len().min(3)
        };
        let mut height = 25;
        if prompt.is_some() {
            height += 88;
        } else {
            if description_lines > 0 {
                height += 24 + description_lines as i32 * 20 + 8;
            }
            if self.desc_expanded || lines.len() > 3 {
                height += 44;
            }
            if !makes.is_empty() {
                height += 24 + makes.len() as i32 * 20;
            }
            if !tags.is_empty() {
                height += 24 + 34;
            }
        }
        if !self.page_url.is_empty() {
            height += 61;
        }
        InfoLayout {
            height,
            description_lines,
            show_more: lines.len() > 3,
            makes,
            tags,
            page_url: self.page_url.clone(),
            prompt,
        }
    }

    /// Renders metadata and returns at most one selected action.
    pub fn show(&mut self, ui: &mut egui::Ui, session: SessionStatus) -> Option<InfoAction> {
        let root = ui.available_rect_before_wrap();
        let width = root.width().max(1.0);
        let layout = self.layout(width as i32, session);
        let painter = ui.painter_at(root);
        let mut action = None;
        let mut y = root.top();
        painter.line_segment(
            [egui::pos2(root.left(), y), egui::pos2(root.right(), y)],
            egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
        );
        y += 25.0;

        if let Some(prompt) = layout.prompt {
            let copy = if prompt == InfoPrompt::SignIn {
                "Sign in with TONE3000 to see tone details."
            } else {
                "Failed to load tone details."
            };
            let galley = info_flow_aligned(
                ui,
                copy,
                14.0,
                width,
                egui::Color32::WHITE,
                egui::Align::Center,
            );
            let text_height = galley.rect.height();
            ui.painter().galley(
                egui::pos2(root.center().x, y + 8.0),
                galley,
                egui::Color32::WHITE,
            );
            let label = if prompt == InfoPrompt::SignIn {
                "Log In"
            } else {
                "Try again"
            };
            let button_y = y + 8.0 + text_height + 16.0;
            let button_width =
                info_text_width(ui, label, egui::FontId::proportional(14.0)).round() + 40.0;
            let button = filled_info_button(
                ui,
                egui::Rect::from_min_size(
                    egui::pos2(root.center().x - button_width / 2.0, button_y),
                    egui::vec2(button_width, 36.0),
                ),
                label,
            );
            if button.clicked() {
                action = Some(if prompt == InfoPrompt::SignIn {
                    InfoAction::SignIn
                } else {
                    InfoAction::Retry
                });
            }
            y = button_y + 68.0;
        } else {
            if let Some(text) = self.tone.as_ref().and_then(description) {
                info_title(&painter, root.left(), y, "Description");
                y += 27.6;
                let galley = info_flow(ui, &text, 14.0, width, crate::theme::palette::TEXT_MUTED);
                let line_count = galley.rows.len();
                let visible_lines = if self.desc_expanded {
                    line_count
                } else {
                    line_count.min(3)
                };
                let clip = egui::Rect::from_min_size(
                    egui::pos2(root.left(), y),
                    egui::vec2(width, visible_lines as f32 * 19.6),
                );
                ui.painter().with_clip_rect(clip).galley(
                    egui::pos2(root.left(), y),
                    galley,
                    crate::theme::palette::TEXT_MUTED,
                );
                y += visible_lines as f32 * 19.6;
                if line_count > 3 {
                    y += 8.0;
                    let label = if self.desc_expanded { "LESS" } else { "MORE" };
                    let text_width = info_text_width(ui, label, egui::FontId::monospace(14.0));
                    let more = egui::Rect::from_min_size(
                        egui::pos2(root.left(), y),
                        egui::vec2(text_width + 24.0, 19.6),
                    );
                    let response = ui.interact(
                        more,
                        ui.scope_id().with("tone-description-more"),
                        egui::Sense::click(),
                    );
                    response
                        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                    painter.text(
                        more.left_center(),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::monospace(14.0),
                        egui::Color32::WHITE,
                    );
                    info_chevron(
                        &painter,
                        egui::Rect::from_center_size(
                            egui::pos2(more.right() - 8.0, more.center().y),
                            egui::Vec2::splat(16.0),
                        ),
                        self.desc_expanded,
                    );
                    if response.clicked() {
                        self.toggle_more();
                    }
                    y += 19.6;
                }
                y += 24.0;
            }
            if !layout.makes.is_empty() {
                info_title(&painter, root.left(), y, "Makes & Models");
                y += 27.6;
                for make in &layout.makes {
                    let galley =
                        info_flow(ui, make, 14.0, width, crate::theme::palette::TEXT_MUTED);
                    let height = galley.rect.height();
                    ui.painter().galley(
                        egui::pos2(root.left(), y),
                        galley,
                        crate::theme::palette::TEXT_MUTED,
                    );
                    y += height + 4.0;
                }
                y -= 4.0;
                y += 24.0;
            }
            if !layout.tags.is_empty() {
                info_title(&painter, root.left(), y, "Tags");
                y += 27.6;
                let mut x = root.left();
                for tag in &layout.tags {
                    let chip_width =
                        info_text_width(ui, tag, egui::FontId::proportional(12.0)).round() + 34.0;
                    if x > root.left() && x + chip_width > root.right() {
                        x = root.left();
                        y += 43.0;
                    }
                    let chip =
                        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(chip_width, 35.0));
                    painter.rect_stroke(
                        chip,
                        17.5,
                        egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        chip.center(),
                        egui::Align2::CENTER_CENTER,
                        tag,
                        egui::FontId::proportional(12.0),
                        egui::Color32::WHITE,
                    );
                    x += chip_width + 8.0;
                }
                y += 59.0;
            }
        }
        if !layout.page_url.is_empty() {
            painter.line_segment(
                [egui::pos2(root.left(), y), egui::pos2(root.right(), y)],
                egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
            );
            y += 25.0;
            if view_info_button(
                ui,
                egui::Rect::from_min_size(egui::pos2(root.left(), y), egui::vec2(158.0, 34.0)),
            )
            .clicked()
            {
                action = Some(InfoAction::OpenUrl(layout.page_url));
            }
        }
        action
    }
}

fn info_flow(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    width: f32,
    color: egui::Color32,
) -> std::sync::Arc<egui::Galley> {
    info_flow_aligned(ui, text, size, width, color, egui::Align::Min)
}

fn info_flow_aligned(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    width: f32,
    color: egui::Color32,
    align: egui::Align,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        egui::FontId::proportional(size),
        color,
        width,
    );
    job.halign = align;
    for section in &mut job.sections {
        section.format.line_height = Some(size * 1.4);
    }
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

fn info_text_width(ui: &egui::Ui, text: &str, font: egui::FontId) -> f32 {
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE)
            .size()
            .x
    })
}

fn info_title(painter: &egui::Painter, x: f32, y: f32, title: &str) {
    painter.text(
        egui::pos2(x, y + 9.8),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::new(14.0, egui::FontFamily::Name("arimo-bold".into())),
        egui::Color32::WHITE,
    );
}

fn info_chevron(painter: &egui::Painter, rect: egui::Rect, up: bool) {
    let point = |x: f32, y: f32| rect.min + egui::vec2(x * 16.0 / 24.0, y * 16.0 / 24.0);
    let points = if up {
        vec![point(6.0, 15.0), point(12.0, 9.0), point(18.0, 15.0)]
    } else {
        vec![point(6.0, 9.0), point(12.0, 15.0), point(18.0, 9.0)]
    };
    painter.add(egui::epaint::PathShape::line(
        points,
        egui::Stroke::new(4.0 / 3.0, egui::Color32::WHITE),
    ));
}

fn filled_info_button(ui: &mut egui::Ui, rect: egui::Rect, label: &str) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(label), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.to_owned()));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 18.0, egui::Color32::WHITE);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(14.0),
        egui::Color32::BLACK,
    );
    response
}

fn view_info_button(ui: &mut egui::Ui, rect: egui::Rect) -> egui::Response {
    let label = "View on TONE3000";
    let response = ui.interact(rect, ui.scope_id().with(label), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(1.0, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 24.0, rect.center().y),
        egui::Vec2::splat(16.0),
    );
    let point = |x: f32, y: f32| icon.min + egui::vec2(x * 16.0 / 24.0, y * 16.0 / 24.0);
    let stroke = egui::Stroke::new(4.0 / 3.0, egui::Color32::WHITE);
    painter.add(egui::epaint::PathShape::line(
        vec![point(18.0, 13.0), point(18.0, 6.0), point(13.0, 6.0)],
        stroke,
    ));
    painter.add(egui::epaint::PathShape::line(
        vec![point(6.0, 13.0), point(6.0, 19.0), point(11.0, 19.0)],
        stroke,
    ));
    painter.add(egui::epaint::PathShape::line(
        vec![point(15.0, 3.0), point(21.0, 3.0), point(21.0, 9.0)],
        stroke,
    ));
    painter.line_segment([point(10.0, 14.0), point(21.0, 3.0)], stroke);
    painter.text(
        egui::pos2(rect.left() + 40.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );
    response
}

fn description(tone: &Tone) -> Option<String> {
    tone.raw
        .get("description")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn clean_strings(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value
                .as_str()
                .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
        })
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .collect()
}

fn wrap_text(text: &str, width: i32) -> Vec<String> {
    let limit = (width.max(1) / 7).max(1) as usize;
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let extra = usize::from(!current.is_empty()) + word.len();
        if !current.is_empty() && current.len() + extra > limit {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

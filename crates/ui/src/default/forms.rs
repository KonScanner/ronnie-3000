//! Fractional settings-form layout and shared form styling.

use eframe::egui;

/// Section spacing in design pixels.
pub const SECTION_GAP: f32 = 48.0;
/// Control spacing in design pixels.
pub const CONTROL_GAP: f32 = 16.0;
/// Outlined field height in design pixels.
pub const FIELD_HEIGHT: f32 = 42.0;
/// Maximum settings column width in design pixels.
pub const COLUMN_WIDTH: f32 = 480.0;

/// Left origin and width of the centered settings column.
#[must_use]
pub fn column_bounds(available_width: f32) -> (f32, f32) {
    let width = available_width.clamp(0.0, COLUMN_WIDTH);
    (
        ((available_width - width) / 2.0).max(0.0) + 24.0,
        (width - 48.0).max(0.0),
    )
}

/// Fractional layout box before absolute pixel snapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormRect {
    /// Horizontal origin.
    pub x: f32,
    /// Vertical origin.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl FormRect {
    /// Creates a fractional box.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Snaps edges in absolute coordinates and preserves the lost top fraction.
    #[must_use]
    pub fn snap(self, parent_phase: f32) -> SnappedFormRect {
        let top = (parent_phase + self.y).round() as i32;
        let bottom = (parent_phase + self.y + self.height).round() as i32;
        SnappedFormRect {
            x: self.x.round() as i32,
            y: top,
            width: self.width.round() as i32,
            height: bottom - top,
            phase: parent_phase + self.y - top as f32,
        }
    }
}

/// Pixel-snapped box and the fractional phase inherited by children.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnappedFormRect {
    /// Horizontal origin in pixels.
    pub x: i32,
    /// Vertical origin in pixels.
    pub y: i32,
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
    /// Fraction lost when the top edge was snapped.
    pub phase: f32,
}

/// Height of one stack with gaps only between visible entries.
#[must_use]
pub fn stack_height(heights: &[Option<f32>], gap: f32, trailing: f32) -> f32 {
    let mut total = 0.0;
    let mut first = true;
    for height in heights.iter().flatten() {
        if !first {
            total += gap;
        }
        total += height;
        first = false;
    }
    total + trailing
}

const MUTED: egui::Color32 = crate::theme::palette::TEXT_MUTED;
const SUBTLE: egui::Color32 = crate::theme::palette::TEXT_SUBTLE;
const BORDER: egui::Color32 = crate::theme::palette::PANEL_EDGE;
const LINK: egui::Color32 = crate::theme::palette::BLAZE;

/// Draws a settings section label with consistent typography.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    let target =
        ui.data(|data| data.get_temp::<String>(egui::Id::unique("settings-scroll-target")));
    let response = ui.label(egui::RichText::new(text).color(egui::Color32::WHITE).font(
        egui::FontId::new(15.0, egui::FontFamily::Name("arimo-bold".into())),
    ));
    if target.as_deref() == Some(text) {
        ui.data_mut(|data| {
            data.insert_temp(
                egui::Id::unique("settings-scroll-heading-top"),
                response.rect.top(),
            );
        });
    }
}

/// Draws a field heading, its help copy, and the gap before its control.
pub fn field_heading(ui: &mut egui::Ui, label: &str, description: &str) {
    heading(ui, label);
    ui.add_space(8.0);
    help(ui, description);
    ui.add_space(CONTROL_GAP);
}

/// Draws a wrapped settings help paragraph with consistent typography.
pub fn help(ui: &mut egui::Ui, text: &str) {
    let _ = help_with_line_height(ui, text, 20.3);
}

fn help_with_line_height(ui: &mut egui::Ui, text: &str, line_height: f32) -> egui::Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(14.0)
                .line_height(Some(line_height))
                .color(MUTED),
        )
        .wrap(),
    )
}

/// Draws an information icon and a wrapped white tip.
pub fn tip(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.circle_stroke(
            rect.center(),
            9.0,
            egui::Stroke::new(1.5, egui::Color32::WHITE),
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "i",
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .size(14.0)
                    .line_height(Some(20.3))
                    .color(egui::Color32::WHITE),
            )
            .wrap(),
        );
    });
}

/// Draws the calibration tip with an inline gauge icon.
pub fn gauge_tip(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
        let icon_painter = ui.painter_at(icon_rect);
        icon_painter.circle_stroke(
            icon_rect.center(),
            9.0,
            egui::Stroke::new(1.5, egui::Color32::WHITE),
        );
        icon_painter.text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            "i",
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(
                    egui::RichText::new("Captures that include calibration data show a ")
                        .size(14.0)
                        .line_height(Some(20.3))
                        .color(egui::Color32::WHITE),
                );
                let (gauge, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, 20.3), egui::Sense::hover());
                let painter = ui.painter_at(gauge);
                let center = gauge.center();
                painter.circle_stroke(center, 6.0, egui::Stroke::new(1.3, egui::Color32::WHITE));
                painter.line_segment(
                    [center, center + egui::vec2(3.5, -4.0)],
                    egui::Stroke::new(1.3, egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(" icon on their")
                        .size(14.0)
                        .line_height(Some(20.3))
                        .color(egui::Color32::WHITE),
                );
            });
            ui.label(
                egui::RichText::new("block and it’s enabled by default.")
                    .size(14.0)
                    .line_height(Some(20.3))
                    .color(egui::Color32::WHITE),
            );
        });
    });
}

/// Draws a full-width outlined select field.
pub fn select<R>(
    ui: &mut egui::Ui,
    id: &str,
    selected: impl Into<egui::WidgetText>,
    muted: bool,
    options: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    select_field(ui, id, selected, muted, options)
}

/// Draws the 42 px system-settings variant of the outlined select field.
pub(crate) fn select_field<R>(
    ui: &mut egui::Ui,
    id: &str,
    selected: impl Into<egui::WidgetText>,
    muted: bool,
    options: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let selected = selected.into();
    let selected_text = selected.text().to_owned();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, FIELD_HEIGHT), egui::Sense::hover());
    let response = ui.interact(rect, ui.make_persistent_id(id), egui::Sense::click());
    let popup_id = egui::Popup::default_response_id(&response);
    paint_select_button(
        ui,
        rect,
        &selected_text,
        muted,
        egui::Popup::is_id_open(ui.ctx(), popup_id),
    );
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::ComboBox, true, id.to_owned());
        info.current_text_value = Some(selected_text.clone());
        info
    });
    let inner = egui::Popup::menu(&response)
        .width(width)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            ui.set_min_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(264.0)
                .show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    options(ui)
                })
                .inner
        })
        .map(|response| response.inner);
    egui::InnerResponse { inner, response }
}

/// Draws a full-width outlined select trigger with explicit open state.
pub fn select_button(
    ui: &mut egui::Ui,
    selected: &str,
    accessible_label: &str,
    muted: bool,
    open: bool,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, FIELD_HEIGHT), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ComboBox, true, accessible_label.to_owned())
    });
    paint_select_button(ui, rect, selected, muted, open);
    response
}

fn paint_select_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    selected: &str,
    muted: bool,
    open: bool,
) {
    let rect = rect.translate(egui::vec2(0.0, 0.5));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, egui::Color32::BLACK);
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    let color = if muted { MUTED } else { egui::Color32::WHITE };
    let font = egui::FontId::proportional(14.0);
    // A long device name stops before the chevron.
    let mut job = egui::text::LayoutJob::simple_singleline(selected.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: rect.width() - 53.0,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    let line_height = crate::fonts::normal_line_height(14.0) as f32;
    let baseline = rect.top()
        + (rect.height() - line_height) / 2.0
        + crate::fonts::css_baseline(14.0, line_height);
    let galley_baseline = galley
        .rows
        .first()
        .and_then(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
        .unwrap_or(0.0);
    painter.galley(
        egui::pos2(rect.left() + 17.0, (baseline - galley_baseline).round()),
        galley,
        color,
    );
    chevron(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(rect.right() - 24.0, rect.center().y),
            egui::vec2(16.0, 16.0),
        ),
        open,
    );
}

fn chevron(ui: &egui::Ui, rect: egui::Rect, open: bool) {
    let painter = ui.painter_at(rect);
    let stroke = egui::Stroke::new(1.5, MUTED);
    let center = rect.center();
    let direction = if open { -1.0 } else { 1.0 };
    painter.line_segment(
        [
            egui::pos2(center.x - 5.0, center.y - 2.0 * direction),
            egui::pos2(center.x, center.y + 3.0 * direction),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(center.x, center.y + 3.0 * direction),
            egui::pos2(center.x + 5.0, center.y - 2.0 * direction),
        ],
        stroke,
    );
}

/// Draws a borderless blue text action.
pub fn text_action(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let response = ui.add(
        egui::Label::new(egui::RichText::new(label).size(14.0).color(LINK))
            .sense(egui::Sense::click()),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.to_owned()));
    response
}

/// Draws a full-width white-outlined call-to-action button.
pub fn cta(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let width = ui.available_width();
    ui.add_sized(
        [width, FIELD_HEIGHT],
        egui::Button::new(egui::RichText::new(label).size(15.0))
            .fill(egui::Color32::BLACK)
            .stroke(egui::Stroke::new(1.0, egui::Color32::WHITE))
            .corner_radius(10.0),
    )
}

/// Draws one labelled radio option with help text.
pub fn radio_option(ui: &mut egui::Ui, label: &str, description: &str, selected: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
        response.rect = rect.translate(egui::vec2(0.0, 2.0));
        let rect = response.rect;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::RadioButton, true, label.to_owned())
        });
        clicked |= response.clicked();
        let painter = ui.painter_at(rect);
        painter.circle_stroke(
            rect.center(),
            8.0,
            egui::Stroke::new(
                2.0,
                if selected {
                    egui::Color32::WHITE
                } else {
                    SUBTLE
                },
            ),
        );
        if selected {
            painter.circle_filled(rect.center(), 4.0, egui::Color32::WHITE);
        }
        let label_response = ui.add(
            egui::Label::new(egui::RichText::new(label).size(14.0)).sense(egui::Sense::click()),
        );
        label_response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::RadioButton, true, label.to_owned())
        });
        clicked |= label_response.clicked();
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(30.0);
        help(ui, description);
    });
    clicked
}

/// Independent vertical offsets within one toggle row.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ToggleOffsets {
    /// Label and toggle offset, canceled after the heading.
    pub heading: f32,
    /// Description offset, canceled after the help text.
    pub description: f32,
    /// Additional toggle-only offset.
    pub toggle: f32,
}

/// Draws one label, help text, and a Boolean switch.
pub fn toggle_row(ui: &mut egui::Ui, label: &str, description: &str, value: &mut bool) -> bool {
    toggle_row_with_line_height(ui, label, description, value, 20.3)
}

/// Draws a Boolean switch with an explicit description gap.
pub(crate) fn toggle_row_with_gap(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    value: &mut bool,
    description_gap: f32,
) -> bool {
    toggle_row_layout(
        ui,
        label,
        description,
        value,
        20.3,
        description_gap,
        ToggleOffsets::default(),
    )
}

/// Draws a Boolean switch with independent vertical offsets.
pub(crate) fn toggle_row_with_offsets(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    value: &mut bool,
    line_height: f32,
    offsets: ToggleOffsets,
) -> bool {
    toggle_row_layout(ui, label, description, value, line_height, 1.0, offsets)
}

/// Draws a Boolean switch with an explicit help line height.
pub fn toggle_row_with_line_height(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    value: &mut bool,
    line_height: f32,
) -> bool {
    toggle_row_layout(
        ui,
        label,
        description,
        value,
        line_height,
        1.0,
        ToggleOffsets::default(),
    )
}

fn toggle_row_layout(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    value: &mut bool,
    line_height: f32,
    description_gap: f32,
    offsets: ToggleOffsets,
) -> bool {
    let mut changed = false;
    ui.add_space(offsets.heading);
    ui.horizontal(|ui| {
        let response = ui.label(egui::RichText::new(label).color(egui::Color32::WHITE).font(
            egui::FontId::new(15.0, egui::FontFamily::Name("arimo-bold".into())),
        ));
        let target =
            ui.data(|data| data.get_temp::<String>(egui::Id::unique("settings-scroll-target")));
        if target.as_deref() == Some(label) {
            ui.data_mut(|data| {
                data.insert_temp(
                    egui::Id::unique("settings-scroll-heading-top"),
                    response.rect.top() - offsets.heading,
                );
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(40.0, 24.0), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::CheckBox, true, label.to_owned())
            });
            if response.clicked() {
                *value = !*value;
                changed = true;
            }
            let rect = rect.translate(egui::vec2(0.0, -0.5 + offsets.toggle));
            let painter = ui.painter_at(rect);
            painter.rect_filled(
                rect,
                12.0,
                if *value {
                    crate::theme::palette::FLAME
                } else {
                    egui::Color32::from_rgb(113, 113, 122)
                },
            );
            let radius = 12.0;
            let span = |y: f32| {
                let dy = y - rect.center().y;
                let dx = (radius * radius - dy * dy).max(0.0).sqrt();
                (rect.left() + radius - dx, rect.right() - radius + dx)
            };
            for index in 0..7 {
                let y = rect.top() + index as f32;
                let (left, right) = span(y);
                let alpha = (33.0 * (1.0 - index as f32 / 7.0)).round() as u8;
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(left, y), egui::pos2(right, y + 1.0)),
                    0.0,
                    egui::Color32::from_black_alpha(alpha),
                );
            }
            for index in 0..3 {
                let y = rect.bottom() - 3.0 + index as f32;
                let (left, right) = span(y);
                let alpha = (10.0 * (index + 1) as f32 / 3.0).round() as u8;
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(left, y), egui::pos2(right, y + 1.0)),
                    0.0,
                    egui::Color32::from_black_alpha(alpha),
                );
            }
            let knob_x = if *value {
                rect.right() - 20.0
            } else {
                rect.left() + 4.0
            };
            let knob_center = egui::pos2(knob_x + 8.0, rect.center().y);
            painter.circle_filled(
                knob_center + egui::vec2(0.0, 1.0),
                9.0,
                egui::Color32::from_black_alpha(25),
            );
            painter.circle_filled(
                knob_center + egui::vec2(0.0, 1.0),
                8.0,
                egui::Color32::from_black_alpha(31),
            );
            painter.circle_filled(knob_center, 8.0, egui::Color32::WHITE);
        });
    });
    ui.add_space(-offsets.heading);
    ui.add_space(description_gap + offsets.description);
    let _ = help_with_line_height(ui, description, line_height);
    ui.add_space(-offsets.description);
    changed
}

/// Draws an outlined field around one control.
pub fn field(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), FIELD_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, egui::Color32::BLACK);
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    let content = rect.shrink2(egui::vec2(17.0, 12.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
    ));
    add(&mut child);
}

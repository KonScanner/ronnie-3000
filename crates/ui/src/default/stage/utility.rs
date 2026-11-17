//! Stage utility bar: tools and global switches along the bottom edge.

use eframe::egui::{self, Align2, Rect, Stroke};

use crate::backend::ChainViewState;
use crate::theme::Look;

use super::super::{
    chain::GallerySelection, header::ChromeController, overlays::Overlay, popover::Trigger,
};

const TEXT_SIZE: f32 = 11.0;
const PADDING: f32 = 12.0;
/// OPTIMIZE text while the low-latency mode is on.
const OPTIMIZE_ON: egui::Color32 = egui::Color32::from_rgb(0x3d, 0xd6, 0x8c);

/// Draws the utility bar into the current panel.
pub(super) fn show_utility(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &ChainViewState,
) {
    let look = Look::get(ui.ctx());
    let bounds = ui.max_rect();
    if let Some(hairline) = look.utility_hairline {
        ui.painter().hline(
            bounds.x_range(),
            bounds.top() + 0.5,
            Stroke::new(1.0, hairline),
        );
    }
    let font = egui::FontId::new(TEXT_SIZE, egui::FontFamily::Name("arimo-bold".into()));
    let mut x = bounds.left() + 10.0;
    let tuner = chrome.tuner_visible();
    let optimize = chrome.optimize();

    let items: [(&str, &'static str, bool); 5] = [
        ("TUNER", "Tuner", tuner),
        ("MIDI", "MIDI", false),
        ("OPTIMIZE", "Optimize", optimize),
        ("TONE3000", "TONE3000", false),
        ("SETTINGS", "Settings", false),
    ];
    let mut clicked = None;
    for (index, (text, label, active)) in items.into_iter().enumerate() {
        if index > 0 {
            divider(ui, x, bounds);
        }
        let (response, width) = item(ui, x, bounds, text, label, active, &font, look);
        if response.clicked() {
            clicked = Some(label);
        }
        x += width;
    }

    // Right side: the account, the light or dark switch, then CPU.
    let toggle_width = appearance_width(ui, &font);
    let toggle_x = bounds.right() - 96.0 - toggle_width;
    divider(ui, bounds.right() - 96.0, bounds);
    divider(ui, toggle_x, bounds);
    if appearance_toggle(ui, toggle_x, toggle_width, bounds, &font, look).clicked() {
        clicked = Some("Dark mode");
    }
    let account_text = chrome
        .account_name()
        .map_or_else(|| "SIGN IN".to_owned(), |name| name.to_uppercase());
    let account_width = text_width(ui, &account_text, &font) + 2.0 * PADDING;
    let account_x = toggle_x - account_width;
    divider(ui, account_x, bounds);
    let (account, _) = item(
        ui,
        account_x,
        bounds,
        &account_text,
        "Account",
        chrome.overlay() == Overlay::Account,
        &font,
        look,
    );
    if account.clicked() {
        clicked = Some("Account");
    }
    if chrome.overlay() == Overlay::Account {
        chrome.set_overlay_trigger(Some(Trigger::from(&account)));
    }

    ui.painter().text(
        egui::pos2(bounds.right() - 16.0, bounds.center().y),
        Align2::RIGHT_CENTER,
        format!("CPU {:.1}%", chrome.cpu_percent()),
        egui::FontId::monospace(TEXT_SIZE),
        egui::Color32::from_rgb(0xbd, 0xbd, 0xbd),
    );

    match clicked {
        Some("Tuner") => chrome.set_tuner(!tuner),
        Some("MIDI" | "Settings") => chrome.open_settings(),
        Some("Optimize") => {
            let _ = chrome.set_optimize(!optimize);
        }
        Some("TONE3000") => {
            let selected = match chrome.gallery().selection() {
                GallerySelection::BlockDetail(id) => Some(id.clone()),
                _ => None,
            };
            if let Some(id) = super::header::lane_insert(state, selected.as_ref()) {
                chrome.gallery_mut().activate(&id);
            }
        }
        Some("Account") => chrome.open_account(),
        Some("Dark mode") => {
            if let Err(error) = chrome.toggle_appearance(ui.ctx()) {
                chrome.show_toast(
                    format!("The dark mode choice was not saved: {error}"),
                    crate::services::auth::now_ms(),
                );
            }
        }
        _ => {}
    }
}

fn divider(ui: &egui::Ui, x: f32, bounds: Rect) {
    ui.painter().vline(
        x,
        (bounds.center().y - 6.0)..=(bounds.center().y + 6.0),
        Stroke::new(1.0, egui::Color32::from_rgb(0x4a, 0x4a, 0x4a)),
    );
}

fn text_width(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    })
}

/// One text button of the bar; returns its response and width.
#[allow(clippy::too_many_arguments)]
fn item(
    ui: &mut egui::Ui,
    x: f32,
    bounds: Rect,
    text: &str,
    label: &'static str,
    active: bool,
    font: &egui::FontId,
    look: Look,
) -> (egui::Response, f32) {
    let width = text_width(ui, text, font) + 2.0 * PADDING;
    let rect = Rect::from_min_size(
        egui::pos2(x, bounds.top()),
        egui::vec2(width, bounds.height()),
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with(("stage-utility", label)),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, super::add_menu::HOVER);
    }
    let color = match (active, label) {
        (true, "Optimize") => OPTIMIZE_ON,
        (true, _) => look.accent,
        (false, _) => look.utility_text,
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font.clone(),
        color,
    );
    (response, width)
}

/// Side of the sun or moon icon of the light or dark switch, and the gap before its word.
const APPEARANCE_ICON: f32 = 13.0;
const APPEARANCE_GAP: f32 = 6.0;

/// The width of the light or dark switch, the same for both of its words.
fn appearance_width(ui: &egui::Ui, font: &egui::FontId) -> f32 {
    let word = text_width(ui, "DARK", font).max(text_width(ui, "LIGHT", font));
    APPEARANCE_ICON + APPEARANCE_GAP + word + 2.0 * PADDING
}

/// The switch between light and dark: a moon and DARK while light, a sun and LIGHT while dark.
fn appearance_toggle(
    ui: &mut egui::Ui,
    x: f32,
    width: f32,
    bounds: Rect,
    font: &egui::FontId,
    look: Look,
) -> egui::Response {
    let rect = Rect::from_min_size(
        egui::pos2(x, bounds.top()),
        egui::vec2(width, bounds.height()),
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with(("stage-utility", "Dark mode")),
        egui::Sense::click(),
    );
    let dark = look.is_dark();
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, dark, "Dark mode"));
    let response = response.on_hover_text(if dark {
        "Switch to light"
    } else {
        "Switch to dark"
    });
    if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, super::add_menu::HOVER);
    }
    let color = look.utility_text;
    let word = if dark { "LIGHT" } else { "DARK" };
    let content = APPEARANCE_ICON + APPEARANCE_GAP + text_width(ui, word, font);
    let left = rect.center().x - content / 2.0;
    let icon = Rect::from_min_size(
        egui::pos2(left, rect.center().y - APPEARANCE_ICON / 2.0),
        egui::Vec2::splat(APPEARANCE_ICON),
    );
    if dark {
        draw_sun(ui.painter(), icon, color);
    } else {
        draw_moon(ui.painter(), icon, color);
    }
    ui.painter().text(
        egui::pos2(icon.right() + APPEARANCE_GAP, rect.center().y),
        Align2::LEFT_CENTER,
        word,
        font.clone(),
        color,
    );
    response
}

/// Points of an arc of the 24-unit icon drawing, in degrees with y down.
fn arc_points(
    at: impl Fn(f32, f32) -> egui::Pos2,
    center: (f32, f32),
    radius: f32,
    from: f32,
    to: f32,
) -> Vec<egui::Pos2> {
    let steps = 24;
    (0..=steps)
        .map(|step| {
            let degrees = from + (to - from) * step as f32 / steps as f32;
            let radians = degrees.to_radians();
            at(
                center.0 + radius * radians.cos(),
                center.1 + radius * radians.sin(),
            )
        })
        .collect()
}

/// The crescent of the design's moon: a long arc round the lower left, closed by a shorter
/// arc back to its start.
fn draw_moon(painter: &egui::Painter, rect: Rect, color: egui::Color32) {
    let scale = rect.width() / 24.0;
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y) * scale;
    let mut points = arc_points(at, (12.64, 11.36), 8.0, 23.1, 246.9);
    points.extend(
        arc_points(at, (14.75, 9.25), 7.42, 225.0, 45.0)
            .into_iter()
            .skip(1),
    );
    painter.add(egui::epaint::PathShape::closed_line(
        points,
        Stroke::new(2.2 * scale, color),
    ));
}

/// The design's sun: a disc outline of radius 4 and eight short rays.
fn draw_sun(painter: &egui::Painter, rect: Rect, color: egui::Color32) {
    let scale = rect.width() / 24.0;
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y) * scale;
    let stroke = Stroke::new(2.2 * scale, color);
    painter.circle_stroke(at(12.0, 12.0), 4.0 * scale, stroke);
    let rays = [
        ((12.0, 2.5), (12.0, 4.5)),
        ((12.0, 19.5), (12.0, 21.5)),
        ((2.5, 12.0), (4.5, 12.0)),
        ((19.5, 12.0), (21.5, 12.0)),
        ((5.3, 5.3), (6.7, 6.7)),
        ((17.3, 17.3), (18.7, 18.7)),
        ((5.3, 18.7), (6.7, 17.3)),
        ((17.3, 6.7), (18.7, 5.3)),
    ];
    for ((x0, y0), (x1, y1)) in rays {
        painter.line_segment([at(x0, y0), at(x1, y1)], stroke);
    }
}

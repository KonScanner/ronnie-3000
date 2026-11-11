//! Update notice view with download and snooze actions.

use eframe::egui;

use crate::services::update::UpdateInfo;

/// Action selected in the update notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAction {
    /// Open the update page in the system browser.
    Download(String),
    /// Snooze for this many days.
    Remind(i64),
}

/// Renders an update notice without an embedded webview.
pub fn show(ui: &mut egui::Ui, info: &UpdateInfo) -> Option<UpdateAction> {
    let mut action = None;
    let root = ui.ctx().viewport_rect();
    let card = egui::Rect::from_center_size(root.center(), egui::vec2(420.0, 203.0));
    let painter = ui.painter_at(card);
    painter.rect_filled(card, 16.0, crate::theme::palette::PANEL);
    painter.rect_stroke(
        card,
        16.0,
        egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(card.center().x, card.top() + 33.0),
        egui::Align2::CENTER_CENTER,
        format!("Update available: v{}", info.version),
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );

    let close = egui::Rect::from_center_size(
        egui::pos2(card.right() - 21.0, card.top() + 21.0),
        egui::Vec2::splat(16.0),
    );
    let close_response = ui.interact(
        close,
        ui.scope_id().with("close-update"),
        egui::Sense::click(),
    );
    close_response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close update notice"));
    let close_stroke = egui::Stroke::new(1.5, crate::theme::palette::TEXT_MUTED);
    for (start, end) in [
        (egui::vec2(-4.0, -4.0), egui::vec2(4.0, 4.0)),
        (egui::vec2(-4.0, 4.0), egui::vec2(4.0, -4.0)),
    ] {
        painter.line_segment([close.center() + start, close.center() + end], close_stroke);
    }
    if close_response.clicked() {
        action = Some(UpdateAction::Remind(1));
    }

    let muted = crate::theme::palette::TEXT_SECONDARY;
    let mut message = egui::text::LayoutJob::simple(
        message_text(&info.message_html),
        egui::FontId::proportional(13.0),
        muted,
        340.0,
    );
    message.halign = egui::Align::Center;
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(message));
    ui.painter().galley(
        egui::pos2(card.center().x, card.top() + 57.0),
        galley,
        muted,
    );

    let download = egui::Rect::from_center_size(
        egui::pos2(card.center().x, card.top() + 130.0),
        egui::vec2(148.0, 36.0),
    );
    let download_response = ui.interact(
        download,
        ui.scope_id().with("download-update"),
        egui::Sense::click(),
    );
    download_response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Download update"));
    painter.rect_filled(download, 18.0, egui::Color32::WHITE);
    painter.text(
        download.center(),
        egui::Align2::CENTER_CENTER,
        "Download update",
        egui::FontId::proportional(14.0),
        egui::Color32::BLACK,
    );
    if download_response.clicked() {
        action = Some(UpdateAction::Download(info.url.clone()));
    }

    let remind_top = card.top() + 164.0;
    let label = "Remind me in";
    let labels = [(1_i64, "1 day"), (7_i64, "7 days"), (30_i64, "30 days")];
    let label_width = text_width(ui, label, 12.0);
    let row_width = label_width
        + labels
            .iter()
            .map(|(_, text)| 10.0 + text_width(ui, text, 12.0))
            .sum::<f32>();
    let mut x = card.center().x - row_width / 2.0;
    painter.text(
        egui::pos2(x, remind_top + 7.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        muted,
    );
    x += label_width;
    for (days, text) in labels {
        x += 10.0;
        let width = text_width(ui, text, 12.0);
        let rect = egui::Rect::from_min_size(egui::pos2(x, remind_top), egui::vec2(width, 14.0));
        if remind_link(ui, rect, text).clicked() {
            action = Some(UpdateAction::Remind(days));
        }
        x += width;
    }
    action
}

fn remind_link(ui: &mut egui::Ui, rect: egui::Rect, label: &str) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("remind-update", label)),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Remind me in {label}"))
    });
    let painter = ui.painter_at(rect);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(12.0),
        egui::Color32::WHITE,
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1.0, egui::Color32::WHITE),
    );
    response
}

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
            )
            .size()
            .x
    })
}

fn message_text(html: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    let mut separated = false;
    for character in html.chars() {
        match character {
            '<' => {
                in_tag = true;
                separated = true;
            }
            '>' => in_tag = false,
            _ if !in_tag => {
                if separated && !output.is_empty() && !output.ends_with(char::is_whitespace) {
                    output.push(' ');
                }
                separated = false;
                output.push(character);
            }
            _ => {}
        }
    }
    output
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn update_message_is_plain_readable_text() {
        assert_eq!(
            super::message_text("<p>New in 1.5.0: EQ &amp; stereo.<br>Lower CPU.</p>"),
            "New in 1.5.0: EQ & stereo. Lower CPU."
        );
    }
}

//! Offline and secure-connection recovery modal.

use eframe::egui;

use crate::services::connectivity::{ConnectionGate, ConnectionProblem};

/// Action selected in the connection modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionAction {
    /// Retry the queued action or secure probe.
    Retry,
    /// Dismiss the warning.
    Dismiss,
}

/// Renders a connection problem with retry and dismiss controls.
pub fn show(ui: &mut egui::Ui, gate: &mut ConnectionGate) -> Option<ConnectionAction> {
    let problem = gate.problem()?;
    let mut action = None;
    let root = ui.available_rect_before_wrap();
    let center_x = root.center().x;
    let stroke = egui::Stroke::new(1.8, egui::Color32::WHITE);
    let icon = egui::Rect::from_center_size(egui::pos2(center_x, 250.0), egui::Vec2::splat(28.0));
    let painter = ui.painter_at(icon);
    match problem {
        ConnectionProblem::Offline => {
            let point = |x: f32, y: f32| icon.min + egui::vec2(x * 28.0 / 24.0, y * 28.0 / 24.0);
            let wifi_stroke =
                egui::Stroke::new(28.0 * 2.0 / 24.0, egui::Color32::from_white_alpha(230));
            for points in [
                vec![
                    point(8.5, 16.429),
                    point(9.3, 15.5),
                    point(10.5, 15.0),
                    point(12.0, 14.9),
                    point(13.5, 15.0),
                    point(14.7, 15.5),
                    point(15.5, 16.429),
                ],
                vec![
                    point(5.0, 12.859),
                    point(6.0, 11.9),
                    point(7.2, 11.1),
                    point(8.6, 10.5),
                    point(10.17, 10.169),
                ],
                vec![
                    point(19.0, 12.859),
                    point(18.4, 12.2),
                    point(17.7, 11.7),
                    point(16.993, 11.336),
                ],
                vec![
                    point(2.0, 8.82),
                    point(3.0, 7.7),
                    point(4.2, 6.9),
                    point(6.177, 6.177),
                ],
                vec![
                    point(22.0, 8.82),
                    point(19.0, 6.1),
                    point(15.5, 4.9),
                    point(10.712, 5.056),
                ],
                vec![point(2.0, 2.0), point(22.0, 22.0)],
            ] {
                painter.add(egui::epaint::PathShape::line(points, wifi_stroke));
            }
            painter.circle_filled(
                point(12.0, 20.0),
                28.0 / 24.0,
                egui::Color32::from_white_alpha(230),
            );
        }
        ConnectionProblem::Insecure => {
            painter.add(egui::epaint::PathShape::line(
                vec![
                    icon.center_top(),
                    icon.right_top() + egui::vec2(0.0, 6.0),
                    icon.center_bottom(),
                    icon.left_top() + egui::vec2(0.0, 6.0),
                    icon.center_top(),
                ],
                stroke,
            ));
        }
    }
    let copy = match problem {
        ConnectionProblem::Offline => {
            "No internet connection. Connect to browse and load tones from TONE3000."
        }
        ConnectionProblem::Insecure => {
            "Couldn't make a secure connection. Check the date, time, firewall, VPN, or security software."
        }
    };
    let color = egui::Color32::from_white_alpha(242);
    let mut job = egui::text::LayoutJob::simple(
        copy.to_owned(),
        egui::FontId::proportional(14.0),
        color,
        400.0,
    );
    job.halign = egui::Align::Center;
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter()
        .galley(egui::pos2(center_x, 286.0), galley, color);
    let retry =
        egui::Rect::from_center_size(egui::pos2(center_x - 46.0, 348.0), egui::vec2(98.0, 36.0));
    let dismiss =
        egui::Rect::from_center_size(egui::pos2(center_x + 55.0, 348.0), egui::vec2(80.0, 36.0));
    if pill(ui, retry, "Try again", true).clicked() {
        action = Some(ConnectionAction::Retry);
    }
    if pill(ui, dismiss, "Dismiss", false).clicked() {
        action = Some(ConnectionAction::Dismiss);
    }
    action
}

fn pill(ui: &mut egui::Ui, rect: egui::Rect, label: &str, filled: bool) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("connection-pill", label)),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.to_owned()));
    let painter = ui.painter_at(rect);
    if filled {
        painter.rect_filled(rect, rect.height() / 2.0, egui::Color32::WHITE);
    } else {
        painter.rect_stroke(
            rect,
            rect.height() / 2.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(150)),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(if filled { 14.0 } else { 13.0 }),
        if filled {
            egui::Color32::BLACK
        } else {
            egui::Color32::WHITE
        },
    );
    response
}

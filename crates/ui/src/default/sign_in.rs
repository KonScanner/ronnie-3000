//! System-browser and device sign-in views without an embedded webview.

use eframe::egui;
use ronie_core::{HttpTransport, OAuthError};

use crate::services::{
    auth::AuthService,
    session::{AuthFlowAdapter, AuthPhase, AuthSnapshot},
};

/// Action selected in the sign-in view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInAction {
    /// Start device authorization.
    StartDevice,
    /// Retry the current flow.
    Retry,
    /// Close the flow.
    Close,
}

/// Phase-based sign-in view.
pub struct SignInView<T: HttpTransport + Send + Sync + 'static> {
    service: AuthService<T>,
}

impl<T: HttpTransport + Send + Sync + 'static> SignInView<T> {
    /// Wraps one authentication service.
    #[must_use]
    pub fn new(service: AuthService<T>) -> Self {
        Self { service }
    }

    /// Authentication service for external async orchestration.
    pub fn service_mut(&mut self) -> &mut AuthService<T> {
        &mut self.service
    }

    /// Renders the current phase and returns at most one action.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<SignInAction> {
        let snapshot = self.service.snapshot();
        let mut action = None;
        ui.vertical_centered(|ui| {
            ui.heading(match snapshot.phase {
                AuthPhase::DeviceWaiting | AuthPhase::DeviceRequesting => "Sign in on your phone",
                AuthPhase::Returning => "Finishing sign-in",
                AuthPhase::Error => "Sign-in failed",
                _ => "Sign in with your browser",
            });
            match snapshot.phase {
                AuthPhase::BrowserWaiting => {
                    ui.label("Browser didn't open? Copy link to sign in, then return here.");
                    if ui
                        .button(if snapshot.copied {
                            "Copied"
                        } else {
                            "Copy Link"
                        })
                        .clicked()
                        && let Some(url) = self.service.copy_link()
                    {
                        ui.ctx().copy_text(url);
                    }
                    if ui.button("Sign in on a different device").clicked() {
                        action = Some(SignInAction::StartDevice);
                    }
                }
                AuthPhase::DeviceRequesting => {
                    ui.label("Getting a code for your phone");
                }
                AuthPhase::DeviceWaiting => {
                    ui.label("Scan the QR code to sign in, then return here.");
                    if let Some(uri) = snapshot.verification_uri {
                        ui.label(uri);
                    }
                    if let Some(code) = snapshot.user_code {
                        ui.label(egui::RichText::new(code).size(24.0).strong());
                    }
                }
                AuthPhase::Returning => {
                    ui.label("Finishing sign-in");
                }
                AuthPhase::Error | AuthPhase::DeviceFailed => {
                    ui.label(
                        snapshot
                            .error
                            .unwrap_or_else(|| "Sign-in failed.".to_owned()),
                    );
                    if ui.button("Try again").clicked() {
                        action = Some(SignInAction::Retry);
                    }
                    if ui.button("Dismiss").clicked() {
                        action = Some(SignInAction::Close);
                    }
                }
                AuthPhase::Authenticated => {
                    ui.label("Sign-in complete");
                }
                AuthPhase::Idle => {
                    if ui.button("Sign in").clicked() {
                        action = Some(SignInAction::Retry);
                    }
                }
            }
            if snapshot.phase != AuthPhase::Idle
                && snapshot.phase != AuthPhase::Authenticated
                && ui.button("Back").clicked()
            {
                action = Some(SignInAction::Close);
            }
        });
        action
    }
}

/// Converts an error into a retryable view action when appropriate.
#[must_use]
pub fn error_action(error: &OAuthError) -> SignInAction {
    match error {
        OAuthError::Cancelled => SignInAction::Close,
        _ => SignInAction::Retry,
    }
}

/// Renders a transport-erased auth snapshot and copy-link action.
pub fn show_snapshot(
    ui: &mut egui::Ui,
    snapshot: AuthSnapshot,
    copy_link: impl FnMut() -> Option<String>,
) -> Option<SignInAction> {
    let mut action = None;
    if back_button(ui).clicked() {
        action = Some(SignInAction::Close);
    }
    let page_action = match snapshot.phase {
        AuthPhase::DeviceWaiting => {
            show_device_cards(ui, &snapshot);
            None
        }
        AuthPhase::BrowserWaiting => show_browser_wait(ui, &snapshot, copy_link),
        AuthPhase::DeviceFailed => show_device_failed(ui, &snapshot, copy_link),
        AuthPhase::Error => show_error(ui, &snapshot),
        AuthPhase::DeviceRequesting => {
            show_progress(
                ui,
                "Sign in on your phone",
                Some("Getting a code for your phone"),
            );
            None
        }
        AuthPhase::Returning => {
            show_progress(ui, "Finishing sign-in", None);
            None
        }
        AuthPhase::Authenticated => {
            show_progress(ui, "Sign-in complete", None);
            None
        }
        AuthPhase::Idle => show_idle(ui),
    };
    // The back arrow is on every page, so a page without an action must not clear its click.
    action.or(page_action)
}

/// The back arrow at the top left of a page that replaces the signal chain.
pub(super) fn back_button(ui: &mut egui::Ui) -> egui::Response {
    let root = ui.available_rect_before_wrap();
    let back =
        egui::Rect::from_min_size(root.min + egui::vec2(24.0, 24.0), egui::Vec2::splat(24.0));
    let back_response = ui.interact(
        back,
        ui.scope_id().with("sign-in-back"),
        egui::Sense::click(),
    );
    back_response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Back".to_owned()));
    let painter = ui.painter_at(back);
    let center = back.center();
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
    painter.line_segment(
        [
            center + egui::vec2(4.0, -6.0),
            center + egui::vec2(-3.0, 0.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center + egui::vec2(-3.0, 0.0),
            center + egui::vec2(4.0, 6.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center + egui::vec2(-3.0, 0.0),
            center + egui::vec2(9.0, 0.0),
        ],
        stroke,
    );
    back_response
}

/// Waiting page: three dots, one title, and an optional detail line.
fn show_progress(ui: &mut egui::Ui, title: &str, detail: Option<&str>) {
    let root = ui.available_rect_before_wrap();
    let painter = ui.painter();
    for x in [-12.0, 0.0, 12.0] {
        painter.circle_filled(
            egui::pos2(root.center().x + x, root.top() + 193.0),
            4.0,
            crate::theme::palette::TEXT_MUTED,
        );
    }
    centered_text(ui, root, 227.0, title, 16.0, true);
    if let Some(detail) = detail {
        centered_text(ui, root, 263.0, detail, 14.0, false);
    }
}

fn show_idle(ui: &mut egui::Ui) -> Option<SignInAction> {
    let root = ui.available_rect_before_wrap();
    let sign_in = action_button(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(root.center().x, root.top() + 236.0),
            egui::vec2(96.0, 36.0),
        ),
        "Sign in",
        true,
    );
    sign_in.clicked().then_some(SignInAction::Retry)
}

fn show_browser_wait(
    ui: &mut egui::Ui,
    snapshot: &AuthSnapshot,
    mut copy_link: impl FnMut() -> Option<String>,
) -> Option<SignInAction> {
    let root = ui.available_rect_before_wrap();
    let copy = snapshot
        .browser_problem
        .as_deref()
        .unwrap_or("Your browser didn't open? Copy link to sign in, then return here.");
    let galley = centered_flow(ui, copy, 14.0, 400.0);
    let half_extra_line = galley.rows.len().saturating_sub(1) as f32 * 9.8;
    let painter = ui.painter();
    for x in [-12.0, 0.0, 12.0] {
        painter.circle_filled(
            egui::pos2(root.center().x + x, root.top() + 193.0 - half_extra_line),
            4.0,
            crate::theme::palette::TEXT_MUTED,
        );
    }
    centered_text(
        ui,
        root,
        227.0 - half_extra_line,
        "Sign in with your browser",
        16.0,
        true,
    );
    ui.painter().galley(
        egui::pos2(root.center().x, root.top() + 263.5 - half_extra_line),
        galley,
        egui::Color32::WHITE,
    );
    let copy = copy_button(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(root.center().x, root.top() + 315.0 + half_extra_line),
            egui::vec2(112.0, 31.0),
        ),
        if snapshot.copied {
            "Copied"
        } else {
            "Copy Link"
        },
        false,
    );
    if copy.clicked()
        && let Some(url) = copy_link()
    {
        ui.ctx().copy_text(url);
    }
    centered_text(
        ui,
        root,
        365.0 + half_extra_line,
        "Sign in on a different device",
        14.0,
        false,
    );
    let other = ui.interact(
        egui::Rect::from_center_size(
            egui::pos2(root.center().x, root.top() + 373.0 + half_extra_line),
            egui::vec2(200.0, 24.0),
        ),
        ui.scope_id().with("other-device"),
        egui::Sense::click(),
    );
    other.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, "Sign in on a different device")
    });
    other.clicked().then_some(SignInAction::StartDevice)
}

fn show_device_failed(
    ui: &mut egui::Ui,
    snapshot: &AuthSnapshot,
    mut copy_link: impl FnMut() -> Option<String>,
) -> Option<SignInAction> {
    let root = ui.available_rect_before_wrap();
    centered_text(
        ui,
        root,
        236.0,
        snapshot.error.as_deref().unwrap_or("That code expired."),
        14.0,
        false,
    );
    // A failed code request has no link, so a copy control would do nothing.
    let has_link = snapshot.verification_uri.is_some();
    let new_code = action_button(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(
                root.center().x - if has_link { 62.0 } else { 0.0 },
                root.top() + 298.0,
            ),
            egui::vec2(100.0, 36.0),
        ),
        "New code",
        true,
    );
    if has_link {
        let copy = copy_button(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(root.center().x + 56.0, root.top() + 298.0),
                egui::vec2(112.0, 31.0),
            ),
            if snapshot.copied {
                "Copied"
            } else {
                "Copy Link"
            },
            false,
        );
        if copy.clicked()
            && let Some(url) = copy_link()
        {
            ui.ctx().copy_text(url);
        }
    }
    new_code.clicked().then_some(SignInAction::StartDevice)
}

fn show_error(ui: &mut egui::Ui, snapshot: &AuthSnapshot) -> Option<SignInAction> {
    let root = ui.available_rect_before_wrap();
    centered_text(ui, root, 200.0, "Sign-in failed", 16.0, true);
    centered_text(
        ui,
        root,
        236.0,
        snapshot
            .error
            .as_deref()
            .unwrap_or("Something went wrong. Try again."),
        14.0,
        false,
    );
    let retry = action_button(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(root.center().x - 46.0, root.top() + 308.0),
            egui::vec2(96.0, 36.0),
        ),
        "Try again",
        true,
    );
    let dismiss = action_button(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(root.center().x + 64.0, root.top() + 308.0),
            egui::vec2(96.0, 36.0),
        ),
        "Dismiss",
        false,
    );
    if retry.clicked() {
        return Some(SignInAction::Retry);
    }
    dismiss.clicked().then_some(SignInAction::Close)
}

pub(super) fn centered_text(
    ui: &mut egui::Ui,
    root: egui::Rect,
    top: f32,
    text: &str,
    size: f32,
    bold: bool,
) {
    ui.painter().text(
        egui::pos2(root.center().x, root.top() + top),
        egui::Align2::CENTER_TOP,
        text,
        egui::FontId::new(
            size,
            if bold {
                egui::FontFamily::Name("arimo-bold".into())
            } else {
                egui::FontFamily::Proportional
            },
        ),
        egui::Color32::WHITE,
    );
}

pub(super) fn centered_flow(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        egui::FontId::proportional(size),
        egui::Color32::WHITE,
        width,
    );
    job.halign = egui::Align::Center;
    for section in &mut job.sections {
        section.format.line_height = Some(size * 1.4);
    }
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

pub(super) fn action_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    label: &str,
    primary: bool,
) -> egui::Response {
    named_action_button(ui, rect, label, label, primary)
}

/// An action button whose accessible name is not the text that it shows.
pub(super) fn named_action_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    label: &str,
    name: &str,
    primary: bool,
) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(label), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.to_owned()));
    ui.painter().rect_filled(
        rect,
        18.0,
        if primary {
            egui::Color32::WHITE
        } else {
            egui::Color32::BLACK
        },
    );
    ui.painter().rect_stroke(
        rect,
        18.0,
        egui::Stroke::new(1.0, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if primary {
            egui::Color32::BLACK
        } else {
            egui::Color32::WHITE
        },
    );
    response
}

fn copy_button(ui: &mut egui::Ui, rect: egui::Rect, label: &str, primary: bool) -> egui::Response {
    let color = if primary {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    };
    let font = egui::FontId::proportional(13.0);
    let text_width = ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(label.to_owned(), font.clone(), color)
            .size()
            .x
    });
    let rect = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(text_width.round() + 56.0, rect.height()),
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with(("copy-pill", label)),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.to_owned()));
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        rect.height() / 2.0,
        if primary {
            egui::Color32::WHITE
        } else {
            egui::Color32::BLACK
        },
    );
    painter.rect_stroke(
        rect,
        rect.height() / 2.0,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );

    let content_width = text_width + 22.0;
    let content_x = rect.center().x - content_width / 2.0;
    let icon_origin = egui::pos2(content_x, rect.center().y - 7.0);
    let point = |x: f32, y: f32| icon_origin + egui::vec2(x * 14.0 / 24.0, y * 14.0 / 24.0);
    let stroke = egui::Stroke::new(7.0 / 6.0, color);
    painter.add(egui::epaint::PathShape::line(
        vec![
            point(4.0, 16.0),
            point(2.9, 16.0),
            point(2.0, 14.0),
            point(2.0, 4.0),
            point(2.9, 2.0),
            point(4.0, 2.0),
            point(14.0, 2.0),
            point(15.1, 2.0),
            point(16.0, 4.0),
        ],
        stroke,
    ));
    painter.rect_stroke(
        egui::Rect::from_min_max(point(8.0, 8.0), point(22.0, 22.0)),
        7.0 / 6.0,
        stroke,
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(content_x + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        color,
    );
    response
}

fn show_device_cards(ui: &mut egui::Ui, snapshot: &AuthSnapshot) {
    let root = ui.available_rect_before_wrap();
    let title = egui::Rect::from_center_size(
        egui::pos2(root.center().x, root.top() + 124.0),
        egui::vec2(240.0, 24.0),
    );
    let painter = ui.painter_at(title);
    let phone = egui::Rect::from_center_size(
        egui::pos2(title.left() + 12.0, title.center().y),
        egui::vec2(14.0, 22.0),
    );
    painter.rect_stroke(
        phone,
        2.0,
        egui::Stroke::new(1.5, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(title.left() + 34.0, title.center().y),
        egui::Align2::LEFT_CENTER,
        "Sign in on your phone",
        egui::FontId::new(16.0, egui::FontFamily::Name("arimo-bold".into())),
        egui::Color32::WHITE,
    );
    let left = egui::Rect::from_min_size(
        egui::pos2(root.center().x - 269.0, root.top() + 166.0),
        egui::vec2(224.0, 280.0),
    );
    let right = left.translate(egui::vec2(314.0, 0.0));
    for card in [left, right] {
        ui.painter_at(card).rect_stroke(
            card,
            16.0,
            egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
            egui::StrokeKind::Inside,
        );
    }
    let qr_rect =
        egui::Rect::from_min_size(left.min + egui::vec2(16.0, 16.0), egui::Vec2::splat(192.0));
    let uri = snapshot.verification_uri.as_deref().unwrap_or_default();
    let code = snapshot.user_code.as_deref().unwrap_or_default();
    let complete = if uri.contains("user_code=") {
        uri.to_owned()
    } else {
        let separator = if uri.contains('?') { '&' } else { '?' };
        format!("{}{}user_code={code}", uri.trim_end_matches('/'), separator)
    };
    draw_qr(ui, qr_rect, &complete);
    ui.painter_at(left).text(
        egui::pos2(left.center().x, left.top() + 235.0),
        egui::Align2::CENTER_TOP,
        "Scan the QR code to sign in,\nthen return here.",
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );
    let typed_uri = uri
        .split("://")
        .nth(1)
        .unwrap_or(uri)
        .trim_start_matches("www.")
        .split('?')
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    ui.painter_at(right).text(
        egui::pos2(right.center().x, right.top() + 105.0),
        egui::Align2::CENTER_TOP,
        format!("Visit {typed_uri},\nsign in, and enter this code:"),
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );
    ui.painter_at(right).text(
        egui::pos2(right.center().x, right.top() + 168.0),
        egui::Align2::CENTER_CENTER,
        code,
        egui::FontId::new(24.0, egui::FontFamily::Name("arimo-bold".into())),
        egui::Color32::WHITE,
    );
    ui.painter().text(
        egui::pos2(left.right() + 45.0, left.center().y),
        egui::Align2::CENTER_CENTER,
        "OR",
        egui::FontId::new(16.0, egui::FontFamily::Name("arimo-bold".into())),
        egui::Color32::GRAY,
    );
}

fn draw_qr(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, egui::Color32::WHITE);
    let Ok(qr) = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
    else {
        return;
    };
    let count = qr.width();
    let module = (168 / count).max(1);
    let side = module * count;
    let origin = rect.min + egui::vec2((192 - side) as f32 / 2.0, (192 - side) as f32 / 2.0);
    let colors = qr.to_colors();
    for y in 0..count {
        for x in 0..count {
            if colors[y * count + x] == qrcode::Color::Dark {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        origin + egui::vec2((x * module) as f32, (y * module) as f32),
                        egui::Vec2::splat(module as f32),
                    ),
                    0.0,
                    egui::Color32::BLACK,
                );
            }
        }
    }
}

//! Single app-wide toast with timed and pinned presentation.

use eframe::egui::{self, Color32, Id};

use crate::accessibility::Announcer;

const SHOW_MS: i64 = 1800;

/// Toast state with replacement, expiry, and accessible announcements.
#[derive(Debug, Default)]
pub struct ToastController {
    message: Option<String>,
    dismiss_at_ms: Option<i64>,
    announcements: Announcer,
}

impl ToastController {
    /// Shows a message for 1800 ms, replacing any previous toast.
    pub fn show(&mut self, message: impl Into<String>, now_ms: i64) {
        let message = message.into();
        self.announcements.announce(message.clone());
        self.message = Some(message);
        self.dismiss_at_ms = Some(now_ms.saturating_add(SHOW_MS));
    }

    /// Pins a message until the next show or clear.
    pub fn pin(&mut self, message: impl Into<String>, _now_ms: i64) {
        let message = message.into();
        self.announcements.announce(message.clone());
        self.message = Some(message);
        self.dismiss_at_ms = None;
    }

    /// Clears the current message without a follow-up announcement.
    pub fn clear(&mut self) {
        self.message = None;
        self.dismiss_at_ms = None;
    }

    /// Current message without evaluating a clock.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Current message at a known time, expiring exactly at its deadline.
    pub fn message_at(&mut self, now_ms: i64) -> Option<&str> {
        if self
            .dismiss_at_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            self.clear();
        }
        self.message()
    }

    /// Takes queued screen-reader announcements in order.
    #[must_use]
    pub fn drain_announcements(&mut self) -> Vec<String> {
        self.announcements.drain()
    }
}

/// Draws the noninteractive white confirmation pill above the faceplate.
pub fn show(ui: &mut egui::Ui, message: &str, bottom_offset: f32) {
    let font = egui::FontId::new(16.0, egui::FontFamily::Name("arimo-bold".into()));
    let galley = ui
        .ctx()
        .fonts_mut(|fonts| fonts.layout_no_wrap(message.to_owned(), font, Color32::BLACK));
    let size = egui::vec2(galley.size().x + 48.0, 44.0);
    let screen = ui.ctx().viewport_rect();
    let rect = egui::Rect::from_center_size(
        egui::pos2(
            screen.center().x,
            screen.bottom() - bottom_offset - size.y / 2.0,
        ),
        size,
    );
    egui::Area::new(Id::unique("default-toast-pill"))
        .fixed_pos(rect.min)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.painter().rect_filled(rect, 16.0, Color32::WHITE);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                message,
                egui::FontId::new(16.0, egui::FontFamily::Name("arimo-bold".into())),
                Color32::BLACK,
            );
        });
}

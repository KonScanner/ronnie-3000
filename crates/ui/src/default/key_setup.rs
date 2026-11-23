//! The "Connect your TONE3000 account" page. It replaces the signal chain like the sign-in page.
//!
//! The page holds the draft of the key until the user saves it or leaves. The draft never
//! reaches a label, a hint, an announcement, or the accessibility tree.

use eframe::egui;

use super::sign_in::{
    action_button, back_button, centered_flow, centered_text, named_action_button,
};

/// The title of the page.
pub const KEY_PAGE_TITLE: &str = "Connect your TONE3000 account";
/// What the page tells the user.
pub const KEY_PAGE_COPY: &str = "Ronnie downloads tones from TONE3000. To connect, create a \
publishable key on TONE3000 in Settings > API Keys. Paste the key here, then sign in. The key \
stays on this computer.";
/// The accessible name of the key field.
pub const KEY_FIELD_LABEL: &str = "TONE3000 key";
/// The accessible name of the save control. The control shows "Save".
pub const SAVE_KEY_LABEL: &str = "Save key";
/// What the page shows after a save when the service cannot start in this run.
pub const RESTART_TO_SIGN_IN: &str = "Restart Ronnie to sign in.";

/// The state of the open page.
#[derive(Debug, Default)]
pub struct KeyPage {
    draft: String,
    error: Option<String>,
}

impl KeyPage {
    /// The text in the field. Only the controller reads it, to save it.
    pub(crate) fn draft(&self) -> &str {
        &self.draft
    }

    /// Replaces the text in the field and removes an earlier error.
    pub fn set_draft(&mut self, draft: impl Into<String>) {
        self.draft = draft.into();
        self.error = None;
    }

    /// The inline error, when the last save failed.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Shows an inline error.
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }
}

/// What the user did on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPageAction {
    /// Open the service in the system browser.
    OpenSite,
    /// Save the draft.
    Save,
    /// Leave the page and save nothing.
    NotNow,
}

/// Renders the page and returns at most one action.
pub fn show(ui: &mut egui::Ui, page: &mut KeyPage) -> Option<KeyPageAction> {
    let mut action = back_button(ui).clicked().then_some(KeyPageAction::NotNow);
    let root = ui.available_rect_before_wrap();
    let center = root.center().x;
    centered_text(ui, root, 120.0, KEY_PAGE_TITLE, 16.0, true);
    let copy = centered_flow(ui, KEY_PAGE_COPY, 14.0, 440.0);
    let copy_height = copy.size().y;
    ui.painter().galley(
        egui::pos2(center, root.top() + 156.0),
        copy,
        egui::Color32::WHITE,
    );
    let mut y = root.top() + 156.0 + copy_height + 24.0;

    let open = action_button(
        ui,
        egui::Rect::from_min_size(egui::pos2(center - 70.0, y), egui::vec2(140.0, 36.0)),
        "Open TONE3000",
        false,
    );
    if open.clicked() {
        action = Some(KeyPageAction::OpenSite);
    }
    y += 36.0 + 24.0;

    let field = egui::Rect::from_min_size(egui::pos2(center - 180.0, y), egui::vec2(360.0, 36.0));
    let mut draft = page.draft.clone();
    let response = ui.put(
        field,
        egui::TextEdit::singleline(&mut draft)
            .password(true)
            .hint_text("t3k_pub_…")
            .font(egui::FontId::proportional(13.0))
            .margin(egui::Margin {
                left: 10,
                right: 10,
                top: 10,
                bottom: 6,
            }),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::PasswordInput, true, KEY_FIELD_LABEL.to_owned())
    });
    // The edge goes on top of the field, so that the fill of the field does not hide it.
    ui.painter().rect_stroke(
        field,
        6.0,
        egui::Stroke::new(
            1.0,
            if page.error.is_some() {
                crate::theme::palette::FLAME
            } else {
                crate::theme::palette::HAIRLINE
            },
        ),
        egui::StrokeKind::Inside,
    );
    if response.changed() {
        page.set_draft(draft);
    }
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        action = Some(KeyPageAction::Save);
    }
    y += 36.0;

    if let Some(error) = &page.error {
        let galley = ui.ctx().fonts_mut(|fonts| {
            fonts.layout(
                error.clone(),
                egui::FontId::proportional(12.0),
                crate::theme::palette::FLAME,
                360.0,
            )
        });
        let height = galley.size().y;
        ui.painter().galley(
            egui::pos2(field.left(), y + 8.0),
            galley,
            crate::theme::palette::FLAME,
        );
        y += 8.0 + height;
    }
    y += 24.0;

    // The control shows "Save". Its name says what it saves, because "Save" also saves presets.
    let save = named_action_button(
        ui,
        egui::Rect::from_min_size(egui::pos2(center - 104.0, y), egui::vec2(96.0, 36.0)),
        "Save",
        SAVE_KEY_LABEL,
        true,
    );
    if save.clicked() {
        action = Some(KeyPageAction::Save);
    }
    let not_now = action_button(
        ui,
        egui::Rect::from_min_size(egui::pos2(center + 8.0, y), egui::vec2(96.0, 36.0)),
        "Not now",
        false,
    );
    if not_now.clicked() {
        action = Some(KeyPageAction::NotNow);
    }
    action
}

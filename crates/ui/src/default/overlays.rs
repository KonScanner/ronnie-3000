//! Shared overlay, focus-return, and announcement behavior.

use crate::accessibility::FocusManager;

/// Currently open global surface.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// No overlay.
    #[default]
    None,
    /// Account menu.
    Account,
    /// Save-preset editor.
    SavePreset,
    /// Preset browser.
    PresetBrowser,
}

/// Reason an overlay is being dismissed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DismissReason {
    /// Pointer outside the surface.
    Outside,
    /// Escape key.
    Escape,
    /// Accepted action.
    Action,
}

/// Focus and announcement state shared by every overlay.
#[derive(Debug, Default)]
pub struct OverlayController {
    overlay: Overlay,
    anchor: Option<String>,
    last_dismissal: Option<DismissReason>,
    focus: FocusManager,
    announcements: Vec<String>,
}

impl OverlayController {
    /// Current overlay.
    #[must_use]
    pub fn overlay(&self) -> Overlay {
        self.overlay
    }

    /// Current focus owner.
    #[must_use]
    pub fn focus(&self) -> &FocusManager {
        &self.focus
    }

    /// Opens one overlay, moves focus into it, and remembers its anchor for focus return.
    pub fn open(&mut self, overlay: Overlay, anchor: &str) {
        self.overlay = overlay;
        self.anchor = Some(anchor.to_owned());
        self.focus.focus(format!("{anchor}-overlay"));
    }

    /// Dismisses and restores the anchor instead of selecting a default control.
    ///
    /// Nothing changes when no overlay is open, so a late second dismissal keeps its reason.
    pub fn dismiss(&mut self, reason: DismissReason) {
        if self.overlay == Overlay::None {
            return;
        }
        self.overlay = Overlay::None;
        self.last_dismissal = Some(reason);
        if let Some(anchor) = self.anchor.take() {
            self.focus.focus(anchor);
        }
    }

    /// Why the latest overlay closed.
    #[must_use]
    pub const fn last_dismissal(&self) -> Option<DismissReason> {
        self.last_dismissal
    }

    /// Queues an accessible status announcement.
    pub fn announce(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.announcements.last() != Some(&message) {
            self.announcements.push(message);
        }
    }

    /// Takes queued announcements in order.
    #[must_use]
    pub fn drain_announcements(&mut self) -> Vec<String> {
        std::mem::take(&mut self.announcements)
    }
}

//! Accessible metadata, announcements, focus policy, and host transport keys.

use std::collections::VecDeque;

use thiserror::Error;

/// Accessible control roles consumed by platform adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibleRole {
    /// Action button.
    Button,
    /// Continuous value control.
    Slider,
    /// Text editor.
    TextInput,
    /// Menu container.
    Menu,
    /// Menu action.
    MenuItem,
    /// Modal dialog.
    Dialog,
    /// Informational image.
    Image,
    /// Read-only text.
    StaticText,
    /// Binary toggle.
    Checkbox,
}

/// Toolkit-neutral accessible node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessibleNode {
    /// Stable node identifier.
    pub id: String,
    /// Platform role.
    pub role: AccessibleRole,
    /// Accessible name.
    pub name: String,
    /// Optional accessible value or state text.
    pub value: Option<String>,
    /// Whether the control can be used.
    pub enabled: bool,
}

impl AccessibleNode {
    /// Creates an enabled node.
    #[must_use]
    pub fn new(id: String, role: AccessibleRole, name: impl Into<String>) -> Self {
        Self {
            id,
            role,
            name: name.into(),
            value: None,
            enabled: true,
        }
    }

    /// Adds accessible value text.
    #[must_use]
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Checks that interactive users receive a nonempty name and identifier.
    ///
    /// # Errors
    /// Returns [`AccessibilityError::MissingIdentity`] when identity text is empty.
    pub fn validate(&self) -> Result<(), AccessibilityError> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Err(AccessibilityError::MissingIdentity);
        }
        Ok(())
    }
}

/// Accessible metadata validation failure.
#[derive(Debug, Error)]
pub enum AccessibilityError {
    /// A stable ID or accessible name is missing.
    #[error("accessible ID and name are required")]
    MissingIdentity,
}

/// Coalescing announcement queue for status transitions.
#[derive(Debug, Default)]
pub struct Announcer {
    messages: VecDeque<String>,
}

impl Announcer {
    /// Queues a new status message, suppressing only an immediate duplicate.
    pub fn announce(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.messages.back() != Some(&message) {
            self.messages.push_back(message);
        }
    }

    /// Takes all queued messages in insertion order.
    #[must_use]
    pub fn drain(&mut self) -> Vec<String> {
        self.messages.drain(..).collect()
    }
}

/// Host transport keys that must pass through when no control owns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKey {
    /// Play or stop.
    Space,
    /// Enter or activate.
    Enter,
}

/// Browser-like focus state with no automatic default focus.
#[derive(Debug, Default)]
pub struct FocusManager {
    focused: Option<String>,
}

impl FocusManager {
    /// Currently focused control ID.
    #[must_use]
    pub fn focused(&self) -> Option<&String> {
        self.focused.as_ref()
    }

    /// Focuses one explicit control.
    pub fn focus(&mut self, id: String) {
        self.focused = Some(id);
    }

    /// Clears focus without selecting a fallback.
    pub fn clear(&mut self) {
        self.focused = None;
    }

    /// Removes a control and clears focus only when it was focused.
    pub fn remove(&mut self, id: &String) {
        if self.focused.as_ref() == Some(id) {
            self.clear();
        }
    }

    /// Enters or advances the current tab order.
    pub fn tab(&mut self, order: &[String], shift: bool) -> bool {
        if order.is_empty() {
            return false;
        }
        let next = match self
            .focused
            .as_ref()
            .and_then(|focused| order.iter().position(|id| id == focused))
        {
            Some(index) if shift => (index + order.len() - 1) % order.len(),
            Some(index) => (index + 1) % order.len(),
            None if shift => order.len() - 1,
            None => 0,
        };
        self.focused = Some(order[next].clone());
        true
    }

    /// Dismisses the focused control. Returns false when nothing was focused.
    pub fn escape(&mut self) -> bool {
        let had_focus = self.focused.is_some();
        self.clear();
        had_focus
    }

    /// Clears focus for unrelated pointer work, preserving ancestry-related editors.
    pub fn pointer_down(&mut self, related: bool) {
        if !related {
            self.clear();
        }
    }

    /// Returns whether a host transport key should bypass the UI.
    #[must_use]
    pub fn forward_key(&self, _key: HostKey) -> bool {
        self.focused.is_none()
    }
}

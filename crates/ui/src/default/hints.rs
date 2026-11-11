//! Hint strip and transient toast state.

/// Hint visibility and current toast.
#[derive(Debug)]
pub struct HintController {
    enabled: bool,
    current: String,
    toast: Option<String>,
}

impl HintController {
    /// Creates hint state with no control hovered.
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            current: String::new(),
            toast: None,
        }
    }

    /// Whether the hint strip is visible.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Changes hint visibility.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Current hint text.
    #[must_use]
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Changes contextual hint text.
    pub fn set_current(&mut self, text: impl Into<String>) {
        self.current = text.into();
    }

    /// Current transient toast.
    #[must_use]
    pub fn toast(&self) -> Option<&str> {
        self.toast.as_deref()
    }

    /// Shows or clears a toast.
    pub fn set_toast(&mut self, toast: Option<String>) {
        self.toast = toast;
    }
}

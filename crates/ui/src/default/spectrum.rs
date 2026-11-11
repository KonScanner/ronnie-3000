//! Visibility-scoped spectrum DTO shared by both EQ views.

/// Sixty-four log-spaced spectrum bins with an explicit enabled state.
#[derive(Debug, Clone)]
pub struct SpectrumFeed {
    visible: bool,
    bins: [f32; 64],
}

impl Default for SpectrumFeed {
    fn default() -> Self {
        Self {
            visible: false,
            bins: [-100.0; 64],
        }
    }
}

impl SpectrumFeed {
    /// Enables analysis only while the EQ editor is visible.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if !visible {
            self.bins = [-100.0; 64];
        }
    }

    /// Whether the analyzer accepts new samples.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.visible
    }

    /// Publishes a control-thread spectrum snapshot while visible.
    pub fn set_bins(&mut self, bins: [f32; 64]) {
        if self.visible {
            self.bins = bins;
        }
    }

    /// Current bins, or the disabled floor.
    #[must_use]
    pub const fn bins(&self) -> &[f32; 64] {
        &self.bins
    }
}

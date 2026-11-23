//! Shared 30 Hz UI heartbeat with hidden-window low-power behavior.

/// One scheduler for meters, spectra, tuner values, and service polls.
#[derive(Default)]
pub struct UiClock {
    listeners: Vec<Box<dyn FnMut() + Send>>,
    period_ms: u64,
}

impl UiClock {
    /// Adds one poller and activates the 30 Hz schedule.
    pub fn add_listener(&mut self, listener: impl FnMut() + Send + 'static) {
        self.listeners.push(Box::new(listener));
        self.period_ms = 33;
    }

    /// Current scheduler period in milliseconds.
    #[must_use]
    pub fn period_ms(&self) -> u64 {
        if self.period_ms == 0 {
            33
        } else {
            self.period_ms
        }
    }

    /// Restores the active 30 Hz period after visibility returns.
    pub fn wake(&mut self) {
        if !self.listeners.is_empty() {
            self.period_ms = 33;
        }
    }

    /// Advances the scheduler once. Hidden windows change cadence but never poll listeners.
    pub fn tick(&mut self, visible: bool) {
        if self.listeners.is_empty() {
            return;
        }
        self.period_ms = if visible { 33 } else { 250 };
        if visible {
            for listener in &mut self.listeners {
                listener();
            }
        }
    }
}

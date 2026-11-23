/// Automatic chain-energy matching state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoBalanceState {
    /// No measurement is active.
    Idle,
    /// Raw chain energy is being captured.
    Listening,
    /// A matched-dB result is ready to publish once.
    Done,
    /// Too little usable signal arrived before the deadline.
    Timeout,
}

/// Plain result from one auto-balance poll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoBalancePoll {
    /// State after this poll.
    pub state: AutoBalanceState,
    /// Fraction of the two-second gated signal target.
    pub progress: f32,
    /// Matched level difference in dB when `state` is `Done`.
    pub matched_db: Option<f32>,
}

impl AutoBalancePoll {
    /// Balance-knob position for a completed measurement.
    #[must_use]
    pub fn balance(self) -> f32 {
        (0.5 + self.matched_db.unwrap_or(0.0) / 48.0).clamp(0.0, 1.0)
    }
}

/// One-shot chain-energy measurement with no callback allocation.
pub struct AutoBalance {
    sample_rate: f64,
    state: AutoBalanceState,
    sum_left: f64,
    sum_right: f64,
    samples: u64,
    elapsed: u64,
    matched_db: f32,
}

impl AutoBalance {
    /// Creates an idle measurement at the host rate.
    #[must_use]
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            state: AutoBalanceState::Idle,
            sum_left: 0.0,
            sum_right: 0.0,
            samples: 0,
            elapsed: 0,
            matched_db: 0.0,
        }
    }

    /// Starts a fresh measurement.
    pub fn start(&mut self) {
        self.sum_left = 0.0;
        self.sum_right = 0.0;
        self.samples = 0;
        self.elapsed = 0;
        self.matched_db = 0.0;
        self.state = AutoBalanceState::Listening;
    }

    /// Cancels measurement without publishing a result.
    pub fn cancel(&mut self) {
        self.state = AutoBalanceState::Idle;
    }

    /// Current workflow state.
    #[must_use]
    pub const fn state(&self) -> AutoBalanceState {
        self.state
    }

    /// Progress through the two-second gated signal target.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.state != AutoBalanceState::Listening {
            return 0.0;
        }
        (self.samples as f64 / (2.0 * self.sample_rate)).clamp(0.0, 1.0) as f32
    }

    /// Accumulates raw pre-balance/pre-pan chain energy only while listening.
    pub fn process(&mut self, left: &[f32], right: &[f32]) {
        if self.state != AutoBalanceState::Listening {
            return;
        }
        let frames = left.len().min(right.len());
        self.elapsed = self.elapsed.saturating_add(frames as u64);
        let mut sum_left = 0.0;
        let mut sum_right = 0.0;
        for index in 0..frames {
            sum_left += f64::from(left[index]) * f64::from(left[index]);
            sum_right += f64::from(right[index]) * f64::from(right[index]);
        }
        if frames > 0 {
            let block_rms = (sum_left.max(sum_right) / frames as f64).sqrt();
            if block_rms > 3.16e-3 {
                self.sum_left += sum_left;
                self.sum_right += sum_right;
                self.samples = self.samples.saturating_add(frames as u64);
            }
        }

        let needed = (2.0 * self.sample_rate) as u64;
        if self.samples >= needed {
            let energy_left = self.sum_left.max(1.0e-12);
            let energy_right = self.sum_right.max(1.0e-12);
            self.matched_db =
                (10.0 * (energy_left / energy_right).log10()).clamp(-24.0, 24.0) as f32;
            self.state = AutoBalanceState::Done;
        } else if self.elapsed > (15.0 * self.sample_rate) as u64 {
            self.state = AutoBalanceState::Timeout;
        }
    }

    /// Polls and publishes a completed result once, then returns to idle.
    #[must_use]
    pub fn poll(&mut self) -> AutoBalancePoll {
        let state = self.state;
        let result = AutoBalancePoll {
            state,
            progress: self.progress(),
            matched_db: (state == AutoBalanceState::Done).then_some(self.matched_db),
        };
        if matches!(state, AutoBalanceState::Done | AutoBalanceState::Timeout) {
            self.state = AutoBalanceState::Idle;
        }
        result
    }
}

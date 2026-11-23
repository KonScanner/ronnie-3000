use crate::{ChainError, PlanarBuffer};

/// Noise-gate controls in real units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateParams {
    /// Detector level that opens the gate, in dB.
    pub threshold_db: f32,
    /// Closing gain-ramp time constant in milliseconds.
    pub release_ms: f32,
    /// Time below the close threshold before release starts, in milliseconds.
    pub hold_ms: f32,
    /// Maximum attenuation in positive dB.
    pub range_db: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            threshold_db: -80.0,
            release_ms: 50.0,
            hold_ms: 20.0,
            range_db: 80.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateState {
    Closed,
    Open,
    Holding,
}

#[derive(Debug, Clone, Copy, Default)]
struct GateChannel {
    svf_ic1: f32,
    svf_ic2: f32,
    lowpass_state: f32,
    envelope: f32,
    gain: f32,
    hold_counter: i64,
    state: Option<GateState>,
}

/// Zero-latency gate with a band-passed detector, hysteresis, and expansion.
pub struct NoiseGate {
    sample_rate: f64,
    params: GateParams,
    detector_rise_coeff: f32,
    detector_fall_coeff: f32,
    attack_coeff: f32,
    release_coeff: f32,
    hold_samples: i64,
    floor_gain: f32,
    svf_k: f32,
    svf_a1: f32,
    svf_a2: f32,
    svf_a3: f32,
    lowpass_coeff: f32,
    open_threshold: f32,
    close_threshold: f32,
    inv_open_threshold: f32,
    channels: [GateChannel; 2],
}

impl NoiseGate {
    /// Prepares detector coefficients and starts with two closed channels.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64) -> Result<Self, ChainError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(ChainError::InvalidLayout("gate sample rate is invalid"));
        }
        let params = GateParams::default();
        let mut gate = Self {
            sample_rate,
            params,
            detector_rise_coeff: one_pole_coeff(0.2, sample_rate),
            detector_fall_coeff: one_pole_coeff(25.0, sample_rate),
            attack_coeff: one_pole_coeff(0.2, sample_rate),
            release_coeff: 1.0,
            hold_samples: 0,
            floor_gain: 1.0,
            svf_k: std::f32::consts::SQRT_2,
            svf_a1: 0.0,
            svf_a2: 0.0,
            svf_a3: 0.0,
            lowpass_coeff: 0.0,
            open_threshold: 1.0,
            close_threshold: 1.0,
            inv_open_threshold: 1.0,
            channels: [GateChannel::default(), GateChannel::default()],
        };
        let g = (std::f32::consts::PI * (80.0 / sample_rate) as f32).tan();
        gate.svf_a1 = 1.0 / (1.0 + g * (g + gate.svf_k));
        gate.svf_a2 = g * gate.svf_a1;
        gate.svf_a3 = g * gate.svf_a2;
        gate.lowpass_coeff = 1.0 - (-std::f32::consts::TAU * (5000.0 / sample_rate) as f32).exp();
        gate.apply_threshold();
        gate.apply_release();
        gate.apply_hold();
        gate.apply_range();
        gate.reset();
        Ok(gate)
    }

    /// Clears detector and gain history so both channels start closed.
    pub fn reset(&mut self) {
        for channel in &mut self.channels {
            *channel = GateChannel::default();
        }
    }

    /// Applies changed controls without recomputing unchanged coefficients.
    pub fn set_params(&mut self, params: GateParams) {
        if params.threshold_db != self.params.threshold_db {
            self.params.threshold_db = params.threshold_db;
            self.apply_threshold();
        }
        if params.release_ms != self.params.release_ms {
            self.params.release_ms = params.release_ms;
            self.apply_release();
        }
        if params.hold_ms != self.params.hold_ms {
            self.params.hold_ms = params.hold_ms;
            self.apply_hold();
        }
        if params.range_db != self.params.range_db {
            self.params.range_db = params.range_db;
            self.apply_range();
        }
    }

    /// Gates one mono or stereo planar buffer in place without allocating.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for more than two channels.
    pub fn process(&mut self, buffer: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
        let channels = buffer.channels();
        if !matches!(channels, 1 | 2) {
            return Err(ChainError::InvalidLayout(
                "gate supports one or two channels",
            ));
        }
        for channel in 0..channels {
            self.process_channel(channel, buffer.channel_mut(channel)?);
        }
        Ok(())
    }

    fn process_channel(&mut self, index: usize, samples: &mut [f32]) {
        let channel = &mut self.channels[index];
        let state_enum = channel.state.unwrap_or(GateState::Closed);
        channel.state = Some(state_enum);
        for sample in samples {
            let input = *sample;
            let v3 = input - channel.svf_ic2;
            let v1 = self.svf_a1 * channel.svf_ic1 + self.svf_a2 * v3;
            let v2 = channel.svf_ic2 + self.svf_a2 * channel.svf_ic1 + self.svf_a3 * v3;
            channel.svf_ic1 = 2.0 * v1 - channel.svf_ic1;
            channel.svf_ic2 = 2.0 * v2 - channel.svf_ic2;
            let highpassed = input - self.svf_k * v1 - v2;
            channel.lowpass_state += self.lowpass_coeff * (highpassed - channel.lowpass_state);

            let rectified = channel.lowpass_state.abs();
            channel.envelope += if rectified > channel.envelope {
                self.detector_rise_coeff
            } else {
                self.detector_fall_coeff
            } * (rectified - channel.envelope);

            let mut state = channel.state.unwrap_or(GateState::Closed);
            match state {
                GateState::Closed => {
                    if channel.envelope >= self.open_threshold {
                        state = GateState::Open;
                    }
                }
                GateState::Open => {
                    if channel.envelope < self.close_threshold {
                        state = GateState::Holding;
                        channel.hold_counter = self.hold_samples;
                    }
                }
                GateState::Holding => {
                    if channel.envelope >= self.close_threshold {
                        state = GateState::Open;
                    } else {
                        channel.hold_counter -= 1;
                        if channel.hold_counter <= 0 {
                            state = GateState::Closed;
                        }
                    }
                }
            }
            channel.state = Some(state);

            let target_gain = if state == GateState::Closed {
                let ratio = channel.envelope * self.inv_open_threshold;
                (ratio * ratio * ratio).max(self.floor_gain)
            } else {
                1.0
            };
            channel.gain += if target_gain > channel.gain {
                self.attack_coeff
            } else {
                self.release_coeff
            } * (target_gain - channel.gain);
            *sample = input * channel.gain;
        }
    }

    fn apply_threshold(&mut self) {
        self.open_threshold = db_to_gain(self.params.threshold_db);
        self.close_threshold = db_to_gain(self.params.threshold_db - 5.0);
        self.inv_open_threshold = 1.0 / self.open_threshold;
    }

    fn apply_release(&mut self) {
        self.release_coeff = one_pole_coeff(self.params.release_ms, self.sample_rate);
    }

    fn apply_hold(&mut self) {
        self.hold_samples = (self.params.hold_ms * 0.001 * self.sample_rate as f32) as i64;
    }

    fn apply_range(&mut self) {
        self.floor_gain = db_to_gain(-self.params.range_db);
    }
}

fn one_pole_coeff(milliseconds: f32, sample_rate: f64) -> f32 {
    if milliseconds <= 0.0 || sample_rate <= 0.0 {
        return 1.0;
    }
    1.0 - (-1.0 / (0.001 * milliseconds * sample_rate as f32)).exp()
}

fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

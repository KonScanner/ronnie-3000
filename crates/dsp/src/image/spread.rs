use crate::{ChainError, PlanarBuffer, block::gain::LinearGain};

use super::deck::{
    DECK_WOBBLE_MAX_MS, DeckCorrelation, DeckDiffuser, DeckWobble, DelayLine, Lr4,
    deck_crossover_hz, new_smoother,
};

/// Mono-to-stereo doubler controls in real units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpreadParams {
    /// Signed base delay; positive values lag the right channel.
    pub offset_ms: f32,
    /// Random-walk depth from zero to one.
    pub wobble_depth: f32,
    /// LR4 crossover frequency in hertz.
    pub crossover_hz: f32,
    /// Whether the mono-low crossover is active.
    pub crossover_on: bool,
    /// Whether the lag-side diffusion cascade is active.
    pub diffuse_on: bool,
}

impl SpreadParams {
    /// Decodes normalized knob values with a true center detent.
    #[must_use]
    pub fn from_normalized(
        offset_norm: f32,
        wobble_norm: f32,
        crossover_norm: f32,
        wobble_on: bool,
        crossover_on: bool,
        diffuse_on: bool,
    ) -> Self {
        let bipolar = offset_norm.clamp(0.0, 1.0) * 2.0 - 1.0;
        Self {
            offset_ms: if bipolar.abs() < 0.005 {
                0.0
            } else {
                bipolar * 24.0
            },
            wobble_depth: if wobble_on {
                wobble_norm.clamp(0.0, 1.0)
            } else {
                0.0
            },
            crossover_hz: deck_crossover_hz(crossover_norm),
            crossover_on,
            diffuse_on,
        }
    }
}

impl Default for SpreadParams {
    fn default() -> Self {
        Self {
            offset_ms: 0.0,
            wobble_depth: 0.25,
            crossover_hz: 130.0,
            crossover_on: true,
            diffuse_on: true,
        }
    }
}

/// Mono-chain ADT-style stereo doubler.
pub struct Spread {
    crossover: Lr4,
    delay: DelayLine,
    diffuser: DeckDiffuser,
    wobble: DeckWobble,
    correlation: DeckCorrelation,
    crossover_mix: LinearGain,
    diffuse_mix: LinearGain,
    applied_crossover_hz: f32,
    running: bool,
    engaged: bool,
    target_offset_ms: f32,
    offset_state_ms: f32,
    offset_coeff: f32,
    wobble_depth: LinearGain,
    wet_gain: LinearGain,
    ms_to_samples: f32,
    lag_gain: f32,
}

impl Spread {
    /// Prepares delay, filter, wobble, and correlation state off the audio thread.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64, _max_block_size: usize) -> Result<Self, ChainError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(ChainError::InvalidLayout("Spread sample rate is invalid"));
        }
        let max_delay =
            ((24.0 + DECK_WOBBLE_MAX_MS) as f64 * 0.001 * sample_rate).ceil() as usize + 8;
        Ok(Self {
            crossover: Lr4::new(sample_rate, 130.0),
            delay: DelayLine::new(max_delay),
            diffuser: DeckDiffuser::new(sample_rate),
            wobble: DeckWobble::new(sample_rate),
            correlation: DeckCorrelation::new(sample_rate),
            crossover_mix: new_smoother(sample_rate),
            diffuse_mix: new_smoother(sample_rate),
            applied_crossover_hz: 130.0,
            running: false,
            engaged: false,
            target_offset_ms: 0.0,
            offset_state_ms: 0.0,
            offset_coeff: 1.0 - (-1.0 / (0.1 * sample_rate as f32)).exp(),
            wobble_depth: new_smoother(sample_rate),
            wet_gain: new_smoother(sample_rate),
            ms_to_samples: (sample_rate * 0.001) as f32,
            lag_gain: 10.0_f32.powf(1.5 / 20.0),
        })
    }

    /// Selects a Reference-compatible wobble stream for deterministic renders.
    pub fn set_wobble_seed(&mut self, seed: i64) {
        self.wobble.set_seed(seed);
    }

    /// Updates controls and starts or ends the 25 ms engage transition.
    pub fn set_target(&mut self, params: SpreadParams, engaged: bool) {
        if !self.running {
            if !engaged {
                return;
            }
            self.reset_deck();
            self.offset_state_ms = params.offset_ms;
            self.wobble_depth
                .set_current_and_target(params.wobble_depth);
            self.crossover_mix
                .set_current_and_target(if params.crossover_on { 1.0 } else { 0.0 });
            self.diffuse_mix
                .set_current_and_target(if params.diffuse_on { 1.0 } else { 0.0 });
            self.wet_gain.set_current_and_target(0.0);
            self.running = true;
        }
        self.engaged = engaged;
        self.target_offset_ms = params.offset_ms;
        self.wobble_depth.set_target(params.wobble_depth);
        self.crossover_mix
            .set_target(if params.crossover_on { 1.0 } else { 0.0 });
        self.diffuse_mix
            .set_target(if params.diffuse_on { 1.0 } else { 0.0 });
        if params.crossover_hz != self.applied_crossover_hz {
            self.applied_crossover_hz = params.crossover_hz;
            self.crossover.set_cutoff(params.crossover_hz);
        }
        self.wet_gain.set_target(if engaged { 1.0 } else { 0.0 });
    }

    /// Whether the engine still needs callbacks for its current transition.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    /// Stops immediately and returns correlation to its idle value.
    pub fn force_idle(&mut self) {
        self.running = false;
        self.engaged = false;
        self.correlation.reset();
    }

    /// Latest normalized stereo correlation, or one on silence and while idle.
    #[must_use]
    pub fn correlation(&self) -> f32 {
        self.correlation.value()
    }

    /// Builds a stereo image from channel zero without allocating.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error unless the buffer has two channels.
    pub fn process(&mut self, buffer: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
        if buffer.channels() != 2 {
            return Err(ChainError::InvalidLayout("Spread requires two channels"));
        }
        if !self.running {
            return Ok(());
        }
        let frames = buffer.frames();
        let mut sum_lr = 0.0;
        let mut sum_ll = 0.0;
        let mut sum_rr = 0.0;
        for frame in 0..frames {
            let left = buffer.channel(0)?[frame];
            let right = buffer.channel(1)?[frame];
            let (low, high) = self.crossover.process_split(0, left);
            let crossover_mix = self.crossover_mix.next();
            let low = low * crossover_mix;
            let high = left + (high - left) * crossover_mix;
            self.delay.push(high);

            let wobble_ms = DECK_WOBBLE_MAX_MS * self.wobble_depth.next() * self.wobble.next();
            self.offset_state_ms +=
                self.offset_coeff * (self.target_offset_ms - self.offset_state_ms);
            let amount_ms = self.offset_state_ms.abs();
            let lag_on_right = self.offset_state_ms >= 0.0;
            let delay_ms = (amount_ms + wobble_ms).clamp(0.0, 24.0 + DECK_WOBBLE_MAX_MS);
            let mut lag = self.delay.pop(delay_ms * self.ms_to_samples);
            lag += (self.diffuser.process(lag) - lag) * self.diffuse_mix.next();
            lag *= self.lag_gain;
            let blend = amount_ms.min(1.0);
            lag = high + (lag - high) * blend;

            let reference = low + high;
            let lagged = low + lag;
            let wet = self.wet_gain.next();
            let out_left = left + (if lag_on_right { reference } else { lagged } - left) * wet;
            let out_right = right + (if lag_on_right { lagged } else { reference } - right) * wet;
            buffer.channel_mut(0)?[frame] = out_left;
            buffer.channel_mut(1)?[frame] = out_right;
            sum_lr += out_left * out_right;
            sum_ll += out_left * out_left;
            sum_rr += out_right * out_right;
        }
        self.correlation.update(sum_lr, sum_ll, sum_rr, frames);
        if !self.engaged && !self.wet_gain.is_smoothing() && self.wet_gain.current() <= 0.0 {
            self.force_idle();
        }
        Ok(())
    }

    fn reset_deck(&mut self) {
        self.crossover.reset();
        self.delay.reset();
        self.diffuser.reset();
        self.wobble.reset();
        self.correlation.reset();
    }
}

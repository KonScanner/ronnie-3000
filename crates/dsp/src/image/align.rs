use crate::{ChainError, PlanarBuffer, block::gain::LinearGain};

use super::deck::{
    DECK_WOBBLE_MAX_MS, DeckCorrelation, DeckDiffuser, DeckWobble, DelayLine, Lr4,
    deck_crossover_hz, new_smoother,
};

/// Stereo-chain corrective delay and creative deck controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignParams {
    /// Channel to delay: zero for left, one for right.
    pub target_channel: usize,
    /// Corrective delay in milliseconds, up to 24 ms.
    pub offset_ms: f32,
    /// Random-walk depth from zero to one.
    pub wobble_depth: f32,
    /// LR4 crossover frequency in hertz.
    pub crossover_hz: f32,
    /// Whether lows bypass the corrective delay.
    pub crossover_on: bool,
    /// Whether the delayed side uses phase diffusion.
    pub diffuse_on: bool,
}

impl AlignParams {
    /// Decodes normalized controls with a sample-scale center detent.
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
        let amount = bipolar.abs();
        Self {
            target_channel: usize::from(bipolar >= 0.0),
            offset_ms: if amount < 0.001 { 0.0 } else { amount * 24.0 },
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

impl Default for AlignParams {
    fn default() -> Self {
        Self {
            target_channel: 1,
            offset_ms: 0.0,
            wobble_depth: 0.0,
            crossover_hz: 130.0,
            crossover_on: false,
            diffuse_on: false,
        }
    }
}

/// Stereo-chain short delay with click-free side and power transitions.
pub struct Align {
    crossover: Lr4,
    delay: DelayLine,
    delay_samples: LinearGain,
    diffuser: DeckDiffuser,
    wobble: DeckWobble,
    correlation: DeckCorrelation,
    wobble_depth: LinearGain,
    crossover_mix: LinearGain,
    diffuse_mix: LinearGain,
    applied_crossover_hz: f32,
    running: bool,
    engaged: bool,
    current_channel: usize,
    params: AlignParams,
    samples_since_clear: usize,
    ms_to_samples: f32,
    max_delay_samples: f32,
}

impl Align {
    /// Prepares maximum delay and all filter state off the audio thread.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64, _max_block_size: usize) -> Result<Self, ChainError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(ChainError::InvalidLayout("Align sample rate is invalid"));
        }
        let max_delay =
            ((24.0 + DECK_WOBBLE_MAX_MS) as f64 * 0.001 * sample_rate).ceil() as usize + 8;
        let mut delay_samples = LinearGain::new(sample_rate, 0.040, 0.0);
        delay_samples.set_current_and_target(0.0);
        Ok(Self {
            crossover: Lr4::new(sample_rate, 130.0),
            delay: DelayLine::new(max_delay),
            delay_samples,
            diffuser: DeckDiffuser::new(sample_rate),
            wobble: DeckWobble::new(sample_rate),
            correlation: DeckCorrelation::new(sample_rate),
            wobble_depth: new_smoother(sample_rate),
            crossover_mix: new_smoother(sample_rate),
            diffuse_mix: new_smoother(sample_rate),
            applied_crossover_hz: 130.0,
            running: false,
            engaged: false,
            current_channel: 1,
            params: AlignParams::default(),
            samples_since_clear: 0,
            ms_to_samples: (sample_rate * 0.001) as f32,
            max_delay_samples: max_delay as f32,
        })
    }

    /// Selects a Reference-compatible wobble stream for deterministic renders.
    pub fn set_wobble_seed(&mut self, seed: i64) {
        self.wobble.set_seed(seed);
    }

    /// Updates controls and starts or ends the identity transition.
    pub fn set_target(&mut self, params: AlignParams, engaged: bool) {
        if !self.running {
            if !engaged {
                return;
            }
            self.delay.reset();
            self.samples_since_clear = 0;
            self.crossover.reset();
            self.diffuser.reset();
            self.wobble.reset();
            self.correlation.reset();
            self.current_channel = params.target_channel;
            self.delay_samples.set_current_and_target(0.0);
            self.wobble_depth.set_current_and_target(0.0);
            self.crossover_mix.set_current_and_target(0.0);
            self.diffuse_mix.set_current_and_target(0.0);
            self.running = true;
        }
        self.engaged = engaged;
        self.params = params;
        if params.crossover_hz != self.applied_crossover_hz {
            self.applied_crossover_hz = params.crossover_hz;
            self.crossover.set_cutoff(params.crossover_hz);
        }
        if !engaged || params.target_channel != self.current_channel {
            self.delay_samples.set_target(0.0);
            self.wobble_depth.set_target(0.0);
            self.diffuse_mix.set_target(0.0);
            self.crossover_mix
                .set_target(if engaged && params.crossover_on {
                    1.0
                } else {
                    0.0
                });
        } else {
            self.retarget_deck();
        }
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

    /// Applies the selected delay and deck without allocating.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error unless the buffer has two channels.
    pub fn process(&mut self, buffer: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
        if buffer.channels() != 2 {
            return Err(ChainError::InvalidLayout("Align requires two channels"));
        }
        if !self.running {
            return Ok(());
        }
        let deck = self.current_channel;
        let other = 1 - deck;
        let frames = buffer.frames();
        let mut sum_lr = 0.0;
        let mut sum_ll = 0.0;
        let mut sum_rr = 0.0;
        for frame in 0..frames {
            let crossover_mix = self.crossover_mix.next();
            let input = [buffer.channel(0)?[frame], buffer.channel(1)?[frame]];
            let mut low = [0.0; 2];
            let mut high = [0.0; 2];
            for (channel, sample) in input.into_iter().enumerate() {
                let (lo, hi) = self.crossover.process_split(channel, sample);
                low[channel] = lo * crossover_mix;
                high[channel] = sample + (hi - sample) * crossover_mix;
            }
            let fresh_limit = self.samples_since_clear.saturating_sub(2) as f32;
            self.samples_since_clear = self.samples_since_clear.saturating_add(1).min(1 << 30);
            let wobble_ms = DECK_WOBBLE_MAX_MS * self.wobble_depth.next() * self.wobble.next();
            let delay = (self.delay_samples.next() + wobble_ms * self.ms_to_samples)
                .clamp(0.0, self.max_delay_samples.min(fresh_limit));
            self.delay.push(high[deck]);
            let mut wet = self.delay.pop(delay);
            wet += (self.diffuser.process(wet) - wet) * self.diffuse_mix.next();
            let deck_output = low[deck] + wet;
            let other_output = low[other] + high[other];
            buffer.channel_mut(deck)?[frame] = deck_output;
            buffer.channel_mut(other)?[frame] = other_output;
            let left = buffer.channel(0)?[frame];
            let right = buffer.channel(1)?[frame];
            sum_lr += left * right;
            sum_ll += left * left;
            sum_rr += right * right;
        }
        self.correlation.update(sum_lr, sum_ll, sum_rr, frames);

        let delay_landed =
            !self.delay_samples.is_smoothing() && self.delay_samples.current() <= 0.0;
        let wobble_landed = !self.wobble_depth.is_smoothing() && self.wobble_depth.current() <= 0.0;
        let diffuse_landed = !self.diffuse_mix.is_smoothing() && self.diffuse_mix.current() <= 0.0;
        if delay_landed && wobble_landed && diffuse_landed {
            if !self.engaged {
                if !self.crossover_mix.is_smoothing() && self.crossover_mix.current() <= 0.0 {
                    self.force_idle();
                }
            } else if self.params.target_channel != self.current_channel {
                self.current_channel = self.params.target_channel;
                self.delay.reset();
                self.samples_since_clear = 0;
                self.diffuser.reset();
                self.retarget_deck();
            }
        }
        Ok(())
    }

    fn retarget_deck(&mut self) {
        let total_ms = self.params.offset_ms.clamp(0.0, 24.0);
        self.delay_samples.set_target(total_ms * self.ms_to_samples);
        self.wobble_depth.set_target(self.params.wobble_depth);
        self.crossover_mix
            .set_target(if self.params.crossover_on { 1.0 } else { 0.0 });
        self.diffuse_mix
            .set_target(if self.params.diffuse_on { 1.0 } else { 0.0 });
    }
}

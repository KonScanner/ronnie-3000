use ronie_core::{DspError, DspResult};
use std::{num::NonZeroUsize, sync::Arc};

use super::state::{A2HistoryInfo, PreparedState};
use super::{CompiledA2, HEAD_KERNEL_SIZE};

/// A2 model state with independent history and shared immutable weights.
pub struct A2Engine<const C: usize> {
    model: Arc<CompiledA2>,
    state: Option<PreparedState<C>>,
}

impl<const C: usize> A2Engine<C> {
    /// Creates an unprepared engine from shared compiled weights.
    ///
    /// # Errors
    ///
    /// Returns [`DspError::ChannelMismatch`] when `C` differs from the model.
    pub fn new(model: Arc<CompiledA2>) -> DspResult<Self> {
        let actual = model.channels().count();
        if actual != C {
            return Err(DspError::ChannelMismatch {
                expected: C,
                actual,
            });
        }
        Ok(Self { model, state: None })
    }

    /// Allocates and resets model state, optionally prewarming with zero input.
    ///
    /// # Errors
    ///
    /// Returns an error for a nonpositive or nonfinite sample rate, or a layout
    /// whose sizes cannot be represented and allocated safely.
    pub fn prepare(
        &mut self,
        sample_rate: f64,
        max_frames: NonZeroUsize,
        prewarm: bool,
    ) -> DspResult<()> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(DspError::InvalidSampleRate);
        }

        let max_frames = max_frames.get();
        let mut state = PreparedState::new(&self.model, max_frames)?;
        if prewarm {
            Self::prewarm_state(&self.model, &mut state);
        }
        self.state = Some(state);
        Ok(())
    }

    pub(crate) fn invalidate(&mut self) {
        self.state = None;
    }

    /// Resets prepared state without allocating, optionally prewarming it.
    ///
    /// # Errors
    ///
    /// Returns [`DspError::NotPrepared`] when no state has been prepared.
    pub fn reset_prepared(&mut self, prewarm: bool) -> DspResult<()> {
        let state = self.state.as_mut().ok_or(DspError::NotPrepared)?;
        state.reset();
        if prewarm {
            Self::prewarm_state(&self.model, state);
        }
        Ok(())
    }

    /// Expected model sample rate, or -1 when unknown.
    #[must_use]
    pub fn expected_sample_rate(&self) -> f64 {
        self.model.expected_sample_rate()
    }

    /// Loudness and calibration metadata for this model.
    #[must_use]
    pub fn metadata(&self) -> &ronie_core::NamMetadata {
        &self.model.metadata
    }

    /// Number of zero frames used by a full prewarm.
    #[must_use]
    pub fn prewarm_samples(&self) -> usize {
        self.model.prewarm_samples()
    }

    /// Prepared maximum callback frames, or zero before preparation.
    #[must_use]
    pub const fn max_frames(&self) -> usize {
        match &self.state {
            Some(state) => state.max_frames,
            None => 0,
        }
    }

    fn prewarm_state(model: &CompiledA2, state: &mut PreparedState<C>) {
        let input = std::mem::take(&mut state.prewarm_input);
        let mut output = std::mem::take(&mut state.prewarm_output);
        let mut processed = 0_usize;
        while processed < model.prewarm_samples() {
            process_prepared(model, state, &input, &mut output);
            processed += state.max_frames;
        }
        state.prewarm_input = input;
        state.prewarm_output = output;
    }

    /// Read-only layout information for one prepared layer history.
    #[must_use]
    pub fn layer_history(&self, index: usize) -> Option<A2HistoryInfo> {
        self.state
            .as_ref()?
            .layer_histories
            .get(index)
            .map(|history| history.info())
    }

    /// Read-only layout information for the prepared head history.
    #[must_use]
    pub fn head_history(&self) -> Option<A2HistoryInfo> {
        self.state.as_ref().map(|state| state.head_history.info())
    }

    /// Processes one callback without allocating or resizing.
    ///
    /// # Errors
    ///
    /// Returns an error before state mutation when the engine is unprepared,
    /// lengths differ, or the callback exceeds the prepared maximum.
    pub fn process(&mut self, input: &[f64], output: &mut [f64]) -> DspResult<()> {
        let state = self.state.as_mut().ok_or(DspError::NotPrepared)?;
        if input.len() != output.len() {
            return Err(DspError::InvalidBufferLength {
                input: input.len(),
                output: output.len(),
            });
        }
        if input.len() > state.max_frames {
            return Err(DspError::BlockTooLarge {
                max: state.max_frames,
                actual: input.len(),
            });
        }
        process_prepared(&self.model, state, input, output);
        Ok(())
    }
}

pub(crate) fn process_prepared<const C: usize>(
    model: &CompiledA2,
    state: &mut PreparedState<C>,
    input: &[f64],
    output: &mut [f64],
) {
    let frames = input.len();
    debug_assert_eq!(frames, output.len());
    debug_assert!(frames <= state.max_frames);

    let PreparedState {
        layer_histories,
        head_history,
        layer_in,
        head_sum,
        z,
        projected,
        cond,
        head_out,
        prewarm_input: _,
        prewarm_output: _,
        max_frames: _,
    } = state;

    for frame in 0..frames {
        let sample = input[frame] as f32;
        cond[frame] = sample;
        for channel in 0..C {
            layer_in[frame * C + channel] = model.rechannel_weights[channel] * sample;
        }
    }
    head_sum[..frames * C].fill(0.0);

    for (layer, history) in model.layers.iter().zip(layer_histories.iter_mut()) {
        history.write(&layer_in[..frames * C]);
        let kernel_size = layer.kernel_size;
        let dilation = layer.dilation;

        for frame in 0..frames {
            for channel in 0..C {
                z[frame * C + channel] = layer.conv_bias[channel];
            }
        }
        for tap in 0..kernel_size {
            let taps_back = (kernel_size - 1 - tap) * dilation;
            let base = history.tap_base(frames, taps_back);
            let tap_weights = &layer.conv_weights[tap * C * C..(tap + 1) * C * C];
            for input_channel in 0..C {
                let weights = &tap_weights[input_channel * C..(input_channel + 1) * C];
                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                if C == 8 && super::simd::available() {
                    super::simd::accumulate_convolution_8(
                        z,
                        weights,
                        history.values(),
                        base,
                        input_channel,
                        frames,
                    );
                    continue;
                }
                for frame in 0..frames {
                    let history_value = history.read(base, frame, input_channel);
                    for (output_channel, weight) in weights.iter().enumerate() {
                        let index = frame * C + output_channel;
                        z[index] += weight * history_value;
                    }
                }
            }
        }

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if C == 8 && super::simd::available() {
            super::simd::activate_8(z, head_sum, &layer.mixin_weights, cond, frames);
        } else {
            for (frame, condition) in cond.iter().enumerate().take(frames) {
                for channel in 0..C {
                    let index = frame * C + channel;
                    let mixed = z[index] + layer.mixin_weights[channel] * condition;
                    z[index] = if mixed >= 0.0 { mixed } else { mixed * 0.01 };
                    head_sum[index] += z[index];
                }
            }
        }
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        for (frame, condition) in cond.iter().enumerate().take(frames) {
            for channel in 0..C {
                let index = frame * C + channel;
                let mixed = z[index] + layer.mixin_weights[channel] * condition;
                z[index] = if mixed >= 0.0 { mixed } else { mixed * 0.01 };
                head_sum[index] += z[index];
            }
        }
        for frame in 0..frames {
            for channel in 0..C {
                projected[frame * C + channel] = layer.layer1x1_bias[channel];
            }
        }
        for input_channel in 0..C {
            let weights = &layer.layer1x1_weights[input_channel * C..(input_channel + 1) * C];
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            if C == 8 && super::simd::available() {
                super::simd::accumulate_projection_8(projected, z, weights, input_channel, frames);
                continue;
            }
            for frame in 0..frames {
                let value = z[frame * C + input_channel];
                for (output_channel, weight) in weights.iter().enumerate() {
                    projected[frame * C + output_channel] += weight * value;
                }
            }
        }
        for index in 0..frames * C {
            layer_in[index] += projected[index];
        }
    }

    head_history.write(&head_sum[..frames * C]);
    head_out[..frames].fill(model.head_bias);
    for tap in 0..HEAD_KERNEL_SIZE {
        let base = head_history.tap_base(frames, HEAD_KERNEL_SIZE - 1 - tap);
        for (frame, output) in head_out.iter_mut().enumerate().take(frames) {
            for channel in 0..C {
                *output +=
                    model.head_weights[tap * C + channel] * head_history.read(base, frame, channel);
            }
        }
    }
    for frame in 0..frames {
        output[frame] = (head_out[frame] * model.head_scale) as f64;
    }
}

use std::sync::Arc;

use super::{CompiledIr, nonuniform::HeadTailEngine};
use crate::{ChainError, PlanarBuffer, block::gain::LinearGain, oversampling::ChainOversampler};

const MAX_CONVOLUTION_CHUNK: usize = 256;
/// The pair mixes at the 48 kHz base rate inside the IR island.
const PAIR_GAIN_RATE: f64 = 48_000.0;
/// Blend, level, and invert changes ramp over about 50 ms.
const PAIR_GAIN_SECONDS: f64 = 0.05;

/// Independent real-time convolution state for one prepared IR and channel mode.
pub struct PreparedIr {
    compiled: Arc<CompiledIr>,
    stereo: bool,
    states: [Option<PreparedState>; 2],
    input_scratch: Vec<f32>,
}

struct PreparedState {
    channels: [HeadTailEngine; 2],
}

impl PreparedIr {
    /// Creates an unprepared processor. `stereo` selects channel-specific kernels
    /// when available; `false` fans the first kernel out to every channel.
    #[must_use]
    pub fn new(compiled: Arc<CompiledIr>, stereo: bool) -> Self {
        Self {
            compiled,
            stereo,
            states: [None, None],
            input_scratch: Vec::new(),
        }
    }

    /// Prepares all FFT plans, partitions, and scratch off the audio thread.
    /// The convolution partition is capped at 256 frames; larger callbacks are
    /// accepted later and processed in chunks.
    ///
    /// # Errors
    ///
    /// Returns a layout error for a zero frame count. A failed candidate does
    /// not replace an earlier prepared state.
    pub fn prepare(&mut self, max_base_frames: usize) -> Result<(), ChainError> {
        if max_base_frames == 0 {
            return Err(ChainError::InvalidLayout(
                "IR preparation requires nonzero frames",
            ));
        }
        let chunk = max_base_frames.min(MAX_CONVOLUTION_CHUNK);
        let mono = self.prepare_state(false, chunk);
        let stereo = self.prepare_state(true, chunk);
        self.input_scratch = vec![0.0; chunk];
        self.states = [Some(mono), Some(stereo)];
        Ok(())
    }

    fn prepare_state(&self, stereo: bool, chunk: usize) -> PreparedState {
        PreparedState {
            channels: [
                HeadTailEngine::new(
                    &self.compiled.channels[0],
                    self.compiled.engine,
                    chunk,
                    chunk,
                ),
                HeadTailEngine::new(
                    &self.compiled.channels
                        [usize::from(stereo && self.compiled.channels.len() > 1)],
                    self.compiled.engine,
                    chunk,
                    chunk,
                ),
            ],
        }
    }

    /// Processes planar audio through the base-rate IR island.
    ///
    /// # Errors
    ///
    /// Returns a layout error before mutation when the processor is unprepared
    /// or the oversampled dimensions are invalid.
    pub fn process_planar(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        oversampler: &mut ChainOversampler,
    ) -> Result<(), ChainError> {
        let stereo = self.stereo;
        self.process_planar_with_stereo(buffer, oversampler, stereo)
    }

    /// Processes with an explicit channel mode while retaining both prepared states.
    ///
    /// # Errors
    ///
    /// Returns a layout error before mutation for invalid dimensions.
    pub fn process_planar_with_stereo(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        oversampler: &mut ChainOversampler,
        stereo: bool,
    ) -> Result<(), ChainError> {
        if self.states[usize::from(stereo)].is_none() || !matches!(buffer.channels(), 1 | 2) {
            return Err(ChainError::InvalidLayout("IR processor is not prepared"));
        }
        oversampler.base_rate_island(buffer, |base| self.process_base(base, stereo))
    }

    pub(crate) fn process_base(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        stereo: bool,
    ) -> Result<(), ChainError> {
        let state = self.states[usize::from(stereo)]
            .as_mut()
            .ok_or(ChainError::InvalidLayout("IR processor is not prepared"))?;
        for start in (0..buffer.frames()).step_by(MAX_CONVOLUTION_CHUNK) {
            let end = (start + MAX_CONVOLUTION_CHUNK).min(buffer.frames());
            let count = end - start;
            for channel in 0..buffer.channels() {
                self.input_scratch[..count].copy_from_slice(&buffer.channel(channel)?[start..end]);
                state.channels[channel].process(
                    &self.input_scratch[..count],
                    &mut buffer.channel_mut(channel)?[start..end],
                );
            }
        }
        Ok(())
    }
}

/// How a pair mixes its two IRs: blend and linear levels, with B's polarity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PairMix {
    /// 0 plays only A, 1 plays only B.
    pub(crate) blend: f32,
    /// Linear gain of A.
    pub(crate) level_a: f32,
    /// Linear gain of B.
    pub(crate) level_b: f32,
    /// Flip B's polarity.
    pub(crate) invert_b: bool,
}

/// Two prepared IRs that run on the same input and mix into one output.
///
/// Both convolvers always run, so a blend change never restarts either tail. Each IR keeps
/// its own unit-energy normalization inside its mix gain; the block normalizes the sum by 1.
pub struct PreparedIrPair {
    a: PreparedIr,
    b: PreparedIr,
    normalization: [f32; 2],
    scratch: Vec<f32>,
    gain_a: LinearGain,
    gain_b: LinearGain,
}

impl PreparedIrPair {
    pub(crate) fn new(a: Arc<CompiledIr>, b: Arc<CompiledIr>, stereo: bool, mix: PairMix) -> Self {
        let normalization = [
            a.normalization_gain.clamp(0.0, 1.0),
            b.normalization_gain.clamp(0.0, 1.0),
        ];
        let mut pair = Self {
            a: PreparedIr::new(a, stereo),
            b: PreparedIr::new(b, stereo),
            normalization,
            scratch: Vec::new(),
            gain_a: LinearGain::new(PAIR_GAIN_RATE, PAIR_GAIN_SECONDS, 0.0),
            gain_b: LinearGain::new(PAIR_GAIN_RATE, PAIR_GAIN_SECONDS, 0.0),
        };
        let (gain_a, gain_b) = pair.gains(mix);
        pair.gain_a.set_current_and_target(gain_a);
        pair.gain_b.set_current_and_target(gain_b);
        pair
    }

    /// Prepares both convolvers and the scratch that holds B's output, off the audio thread.
    pub(crate) fn prepare(&mut self, max_base_frames: usize) -> Result<(), ChainError> {
        self.a.prepare(max_base_frames)?;
        self.b.prepare(max_base_frames)?;
        self.scratch = vec![0.0; 2 * max_base_frames];
        Ok(())
    }

    fn gains(&self, mix: PairMix) -> (f32, f32) {
        let blend = if mix.blend.is_finite() {
            mix.blend.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let polarity = if mix.invert_b { -1.0 } else { 1.0 };
        (
            (1.0 - blend) * mix.level_a * self.normalization[0],
            blend * mix.level_b * self.normalization[1] * polarity,
        )
    }

    /// Ramps the two mix gains toward `mix`.
    pub(crate) fn set_mix(&mut self, mix: PairMix) {
        let (gain_a, gain_b) = self.gains(mix);
        self.gain_a.set_target(gain_a);
        self.gain_b.set_target(gain_b);
    }

    /// Runs A and B on the same input in one base-rate island and mixes them.
    pub(crate) fn process_planar_with_stereo(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        oversampler: &mut ChainOversampler,
        stereo: bool,
    ) -> Result<(), ChainError> {
        let state = usize::from(stereo);
        if self.a.states[state].is_none()
            || self.b.states[state].is_none()
            || !matches!(buffer.channels(), 1 | 2)
        {
            return Err(ChainError::InvalidLayout("IR pair is not prepared"));
        }
        oversampler.base_rate_island(buffer, |base| {
            let channels = base.channels();
            let frames = base.frames();
            let samples = channels * frames;
            if samples > self.scratch.len() {
                return Err(ChainError::InvalidLayout(
                    "IR pair callback exceeds its scratch",
                ));
            }
            self.scratch[..samples].copy_from_slice(base.as_slice());
            self.a.process_base(base, stereo)?;
            self.b.process_base(
                &mut PlanarBuffer::new(&mut self.scratch[..samples], channels, frames)?,
                stereo,
            )?;
            let output = base.as_mut_slice();
            for frame in 0..frames {
                let gain_a = self.gain_a.next();
                let gain_b = self.gain_b.next();
                for channel in 0..channels {
                    let index = channel * frames + frame;
                    output[index] = output[index] * gain_a + self.scratch[index] * gain_b;
                }
            }
            Ok(())
        })
    }
}

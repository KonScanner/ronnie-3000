use std::sync::Arc;

use super::{CompiledIr, nonuniform::HeadTailEngine};
use crate::{ChainError, PlanarBuffer, oversampling::ChainOversampler};

const MAX_CONVOLUTION_CHUNK: usize = 256;

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

    fn process_base(
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

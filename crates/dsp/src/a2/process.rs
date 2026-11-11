use ronie_core::DspResult;
use std::{num::NonZeroUsize, sync::Arc};

use super::{A2Channels, A2Engine, CompiledA2};

/// Channel-specialized A2 engine with shared compiled weights.
pub enum A2Model {
    /// Three-channel model.
    Nano(A2Engine<3>),
    /// Eight-channel model.
    Standard(A2Engine<8>),
}

impl A2Model {
    /// Creates an unprepared model with independent mutable state.
    ///
    /// # Errors
    ///
    /// Returns an error if engine construction rejects the compiled model.
    pub fn new(model: Arc<CompiledA2>) -> DspResult<Self> {
        match model.channels() {
            A2Channels::Nano => Ok(Self::Nano(A2Engine::new(model)?)),
            A2Channels::Standard => Ok(Self::Standard(A2Engine::new(model)?)),
        }
    }

    /// Allocates and resets state, optionally prewarming with zero input.
    ///
    /// # Errors
    ///
    /// Returns the selected engine's preparation error.
    pub fn prepare(
        &mut self,
        sample_rate: f64,
        max_frames: NonZeroUsize,
        prewarm: bool,
    ) -> DspResult<()> {
        match self {
            Self::Nano(engine) => engine.prepare(sample_rate, max_frames, prewarm),
            Self::Standard(engine) => engine.prepare(sample_rate, max_frames, prewarm),
        }
    }

    pub(crate) fn invalidate(&mut self) {
        match self {
            Self::Nano(engine) => engine.invalidate(),
            Self::Standard(engine) => engine.invalidate(),
        }
    }

    /// Number of zero frames used by a full prewarm.
    #[must_use]
    pub fn prewarm_samples(&self) -> usize {
        match self {
            Self::Nano(engine) => engine.prewarm_samples(),
            Self::Standard(engine) => engine.prewarm_samples(),
        }
    }

    /// Prepared maximum callback frames, or zero before preparation.
    #[must_use]
    pub fn max_frames(&self) -> usize {
        match self {
            Self::Nano(engine) => engine.max_frames(),
            Self::Standard(engine) => engine.max_frames(),
        }
    }

    /// Expected model sample rate, or -1 when unknown.
    #[must_use]
    pub fn expected_sample_rate(&self) -> f64 {
        match self {
            Self::Nano(engine) => engine.expected_sample_rate(),
            Self::Standard(engine) => engine.expected_sample_rate(),
        }
    }

    /// Loudness and calibration metadata for this model.
    #[must_use]
    pub fn metadata(&self) -> &ronie_core::NamMetadata {
        match self {
            Self::Nano(engine) => engine.metadata(),
            Self::Standard(engine) => engine.metadata(),
        }
    }

    /// Resets prepared state without allocating, optionally prewarming it.
    ///
    /// # Errors
    ///
    /// Returns [`ronie_core::DspError::NotPrepared`] before preparation.
    pub fn reset_prepared(&mut self, prewarm: bool) -> DspResult<()> {
        match self {
            Self::Nano(engine) => engine.reset_prepared(prewarm),
            Self::Standard(engine) => engine.reset_prepared(prewarm),
        }
    }

    /// Processes one callback without allocating or resizing.
    ///
    /// # Errors
    ///
    /// Returns an error before state mutation when the engine is unprepared,
    /// lengths differ, or the callback exceeds the prepared maximum.
    pub fn process(&mut self, input: &[f64], output: &mut [f64]) -> DspResult<()> {
        match self {
            Self::Nano(engine) => engine.process(input, output),
            Self::Standard(engine) => engine.process(input, output),
        }
    }
}

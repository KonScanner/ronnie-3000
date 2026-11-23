/// Input noise gate.
mod gate;
/// Correlation-spliced pitch shifting.
mod pitch;

pub use gate::{GateParams, NoiseGate};
pub use pitch::{PitchParams, PitchShifter, PitchWindow};

use crate::{ChainError, PlanarBuffer};

/// Immutable controls for the complete host-rate input stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputControls {
    /// Linear gain applied before the gate.
    pub input_gain: f32,
    /// Whether the noise gate is active.
    pub gate_enabled: bool,
    /// Noise-gate controls.
    pub gate: GateParams,
    /// Whether pitch processing is active.
    pub pitch_enabled: bool,
    /// Round the continuous semitone control to whole steps.
    pub pitch_step: bool,
    /// Pitch-shifter controls.
    pub pitch: PitchParams,
}

impl Default for InputControls {
    fn default() -> Self {
        Self {
            input_gain: 1.0,
            gate_enabled: true,
            gate: GateParams::default(),
            pitch_enabled: false,
            pitch_step: true,
            pitch: PitchParams::default(),
        }
    }
}

/// Host-rate input gain, gate, and pitch processor.
pub struct InputStage {
    gate: NoiseGate,
    gate_was_enabled: bool,
    pitch: PitchShifter,
}

impl InputStage {
    /// Prepares the input processors off the audio thread.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for an invalid sample rate.
    pub fn new(sample_rate: f64, max_block_samples: usize) -> Result<Self, ChainError> {
        Ok(Self {
            gate: NoiseGate::new(sample_rate)?,
            gate_was_enabled: false,
            pitch: PitchShifter::new(sample_rate, max_block_samples)?,
        })
    }

    /// Applies input gain, gate, then pitch without allocating.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for more than two channels.
    pub fn process(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        controls: InputControls,
    ) -> Result<(), ChainError> {
        if !matches!(buffer.channels(), 1 | 2) {
            return Err(ChainError::InvalidLayout(
                "input supports one or two channels",
            ));
        }
        for channel in 0..buffer.channels() {
            for sample in buffer.channel_mut(channel)? {
                *sample *= controls.input_gain;
            }
        }
        if controls.gate_enabled {
            if !self.gate_was_enabled {
                self.gate.reset();
            }
            self.gate.set_params(controls.gate);
            self.gate.process(buffer)?;
        }
        self.gate_was_enabled = controls.gate_enabled;
        self.pitch.set_enabled(controls.pitch_enabled);
        if self.pitch.is_running() {
            let mut params = controls.pitch;
            if controls.pitch_step {
                params.semitones = params.semitones.round();
            }
            self.pitch.set_params(params);
            self.pitch.process(buffer)?;
        }
        Ok(())
    }

    /// Host-visible delay for the current power and window controls.
    #[must_use]
    pub fn latency_samples(&self, controls: InputControls) -> u64 {
        if controls.pitch_enabled {
            (PitchShifter::min_delay_samples(self.pitch.sample_rate)
                + PitchShifter::window_samples(controls.pitch.window, self.pitch.sample_rate))
                / 2
        } else {
            0
        }
    }
}

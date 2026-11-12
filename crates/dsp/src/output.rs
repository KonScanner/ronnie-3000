use crate::{
    ChainError, PlanarBuffer,
    block::gain::LinearGain,
    image::{ImageControls, ImageStage},
    tone_stack::{ToneControls, ToneStack},
};

/// Complete post-chain output controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputControls {
    /// Image, balance, pan, solo, polarity, and measurement controls.
    pub image: ImageControls,
    /// Global three-band tone controls.
    pub tone: ToneControls,
    /// Main output level from zero to one; 0.5 is unity.
    pub output_level: f32,
}

impl Default for OutputControls {
    fn default() -> Self {
        Self {
            image: ImageControls::default(),
            tone: ToneControls::default(),
            output_level: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DcBlocker {
    b0: f32,
    b1: f32,
    a1: f32,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl DcBlocker {
    fn new(sample_rate: f64) -> Self {
        let n = (std::f64::consts::PI * 5.0 / sample_rate).tan() as f32;
        Self {
            b0: 1.0 / (n + 1.0),
            b1: -1.0 / (n + 1.0),
            a1: (n - 1.0) / (n + 1.0),
            z1: [0.0; 2],
            z2: [0.0; 2],
        }
    }

    fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }

    fn process(&mut self, input: f32, channel: usize) -> f32 {
        let output = self.b0 * input + self.z1[channel];
        self.z1[channel] = self.b1 * input - self.a1 * output + self.z2[channel];
        self.z2[channel] = 0.0;
        output
    }
}

/// Image, DC blocking, tone stack, and smoothed main output gain.
pub struct OutputStage {
    image: ImageStage,
    dc: DcBlocker,
    tone: ToneStack,
    output_gain: LinearGain,
    /// Whether a block ran. The first block starts at the set level, with no ramp from unity.
    started: bool,
}

impl OutputStage {
    /// Prepares the complete host-rate output path.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64, max_block_size: usize) -> Result<Self, ChainError> {
        Ok(Self {
            image: ImageStage::new(sample_rate, max_block_size)?,
            dc: DcBlocker::new(sample_rate),
            tone: ToneStack::new(sample_rate)?,
            output_gain: LinearGain::new(sample_rate, 0.020, 1.0),
            started: false,
        })
    }

    /// Clears filter and engine state after a non-finite sample without allocating.
    pub fn reset(&mut self) {
        self.image.reset();
        self.dc.reset();
        self.tone.reset();
        if !self.output_gain.current().is_finite() {
            self.output_gain.set_current_and_target(1.0);
        }
        // The next block starts at the set level, as the first block of a new stage does.
        self.started = false;
    }

    /// Processes the complete output path without allocating.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error unless the buffer has two channels.
    pub fn process(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        controls: OutputControls,
        on_auto_balance: impl FnMut(&[f32], &[f32]),
    ) -> Result<(), ChainError> {
        self.image
            .process(buffer, controls.image, on_auto_balance)?;
        for channel in 0..buffer.channels() {
            for sample in buffer.channel_mut(channel)? {
                *sample = self.dc.process(*sample, channel);
            }
        }
        self.tone.set_controls(controls.tone);
        self.tone.process(buffer)?;
        let gain = 10.0_f32.powf((controls.output_level - 0.5) * 48.0 / 20.0);
        if self.started {
            self.output_gain.set_target(gain);
        } else {
            self.started = true;
            self.output_gain.set_current_and_target(gain);
        }
        for frame in 0..buffer.frames() {
            let gain = self.output_gain.next();
            for channel in 0..buffer.channels() {
                buffer.channel_mut(channel)?[frame] *= gain;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The output gain of each sample of one block of ones, at `output_level`.
    fn gains(stage: &mut OutputStage, output_level: f32) -> Vec<f32> {
        let mut samples = vec![1.0_f32; 2 * 64];
        let mut buffer = PlanarBuffer::new(&mut samples, 2, 64).expect("buffer");
        let controls = OutputControls {
            output_level,
            ..OutputControls::default()
        };
        stage
            .process(&mut buffer, controls, |_, _| {})
            .expect("process");
        let unity = {
            let mut reference = OutputStage::new(48_000.0, 64).expect("stage");
            let mut ones = vec![1.0_f32; 2 * 64];
            let mut buffer = PlanarBuffer::new(&mut ones, 2, 64).expect("buffer");
            let unity = OutputControls {
                output_level: 0.5,
                ..OutputControls::default()
            };
            reference
                .process(&mut buffer, unity, |_, _| {})
                .expect("process");
            ones
        };
        samples
            .iter()
            .zip(&unity)
            .map(|(sample, reference)| sample / reference)
            .collect()
    }

    #[test]
    fn a_new_output_stage_starts_at_the_set_level() {
        let mut stage = OutputStage::new(48_000.0, 64).expect("stage");
        let quiet = 10.0_f32.powf(-24.0 / 20.0);

        let first = gains(&mut stage, 0.0);

        assert!((first[0] - quiet).abs() < 1e-3, "{}", first[0]);
    }

    #[test]
    fn a_later_level_change_still_ramps() {
        let mut stage = OutputStage::new(48_000.0, 64).expect("stage");
        let _ = gains(&mut stage, 0.5);

        let changed = gains(&mut stage, 0.0);

        assert!(
            changed[0] > 0.9,
            "the change starts near unity: {}",
            changed[0]
        );
        assert!(changed[63] < changed[0], "and goes down");
    }
}

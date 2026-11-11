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
        self.output_gain
            .set_target(10.0_f32.powf((controls.output_level - 0.5) * 48.0 / 20.0));
        for frame in 0..buffer.frames() {
            let gain = self.output_gain.next();
            for channel in 0..buffer.channels() {
                buffer.channel_mut(channel)?[frame] *= gain;
            }
        }
        Ok(())
    }
}

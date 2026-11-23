/// Six-section first-order-allpass half-band filters.
pub mod halfband;
mod lanczos;

use crate::{ChainError, PlanarBuffer};
use lanczos::Lanczos12;

const CHAIN_RATE: f64 = 48_000.0;

/// One Lanczos-12 host boundary around the 48 kHz chain domain.
pub struct ChainBoundaryResampler {
    mode: Option<BoundaryMode>,
}

enum BoundaryMode {
    Direct { channels: usize, max_frames: usize },
    Resampled(Box<ActiveBoundary>),
}

struct ActiveBoundary {
    channels: usize,
    max_host_frames: usize,
    max_chain_frames: usize,
    ratio: f64,
    latency: u64,
    input_resampler: Lanczos12,
    output_resampler: Lanczos12,
    chain_input: Vec<f32>,
    chain_output: Vec<f32>,
}

impl ChainBoundaryResampler {
    /// Creates an unprepared boundary without allocating.
    #[must_use]
    pub const fn new() -> Self {
        Self { mode: None }
    }

    /// Prepares a host conversion or a transparent 48 kHz direct path.
    ///
    /// Preparation is atomic: a failed candidate leaves the previous boundary
    /// unchanged. All filter history and scratch are allocated here.
    ///
    /// # Errors
    ///
    /// Returns layout or allocation errors for invalid dimensions.
    pub fn prepare(
        &mut self,
        host_rate: f64,
        channels: usize,
        max_frames: usize,
    ) -> Result<(), ChainError> {
        if !matches!(channels, 1 | 2)
            || max_frames == 0
            || !host_rate.is_finite()
            || host_rate <= 0.0
        {
            return Err(ChainError::InvalidLayout(
                "invalid host boundary dimensions",
            ));
        }
        let candidate = if host_rate == CHAIN_RATE {
            BoundaryMode::Direct {
                channels,
                max_frames,
            }
        } else {
            BoundaryMode::Resampled(Box::new(ActiveBoundary::new(
                host_rate, channels, max_frames,
            )?))
        };
        self.mode = Some(candidate);
        Ok(())
    }

    /// Host-rate latency in samples, or zero when direct or unprepared.
    #[must_use]
    pub fn latency_samples(&self) -> u64 {
        match &self.mode {
            Some(BoundaryMode::Resampled(boundary)) => boundary.latency,
            _ => 0,
        }
    }

    /// Maximum chain-domain frames prepared for one host callback.
    #[must_use]
    pub fn max_chain_frames(&self) -> usize {
        match &self.mode {
            Some(BoundaryMode::Direct { max_frames, .. }) => *max_frames,
            Some(BoundaryMode::Resampled(boundary)) => boundary.max_chain_frames,
            None => 0,
        }
    }

    /// Converts into the chain domain, invokes the chain, and converts back.
    ///
    /// # Errors
    ///
    /// Invalid layouts fail before mutation. Chain processing failures are
    /// propagated to the caller.
    pub fn process(
        &mut self,
        input: &PlanarBuffer<'_>,
        output: &mut PlanarBuffer<'_>,
        mut chain: impl FnMut(&mut PlanarBuffer<'_>) -> Result<(), ChainError>,
    ) -> Result<(), ChainError> {
        match self.mode.as_mut() {
            Some(BoundaryMode::Direct {
                channels,
                max_frames,
            }) => {
                validate_pair(input, output, *channels, *max_frames)?;
                output.as_mut_slice().copy_from_slice(input.as_slice());
                chain(output)
            }
            Some(BoundaryMode::Resampled(boundary)) => {
                validate_pair(input, output, boundary.channels, boundary.max_host_frames)?;
                boundary.process(input, output, chain)
            }
            None => Err(ChainError::InvalidLayout("host boundary is not prepared")),
        }
    }
}

impl Default for ChainBoundaryResampler {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveBoundary {
    fn new(host_rate: f64, channels: usize, max_host_frames: usize) -> Result<Self, ChainError> {
        let ratio = host_rate / CHAIN_RATE;
        let max_chain_frames = (max_host_frames as f64 / ratio).ceil();
        if !max_chain_frames.is_finite()
            || max_chain_frames < 1.0
            || max_chain_frames > usize::MAX as f64
        {
            return Err(ChainError::InvalidLayout("invalid chain buffer capacity"));
        }
        let max_chain_frames = max_chain_frames as usize;
        let mut input_resampler = Lanczos12::new(host_rate, CHAIN_RATE, channels)?;
        let mut output_resampler = Lanczos12::new(CHAIN_RATE, host_rate, channels)?;
        let middle_samples = output_resampler.samples_required_for(1);
        let max_chain_frames = max_chain_frames.max(middle_samples);
        let scratch_samples = channels
            .checked_mul(max_chain_frames)
            .ok_or(ChainError::InvalidLayout("chain scratch overflow"))?;
        let mut chain_input = allocate_samples(scratch_samples)?;
        let mut chain_output = allocate_samples(scratch_samples)?;
        let latency = input_resampler.samples_required_for(middle_samples);
        let warmup = allocate_samples(
            channels
                .checked_mul(latency)
                .ok_or(ChainError::InvalidLayout("boundary warm-up overflow"))?,
        )?;
        input_resampler.push_block(&warmup, latency, latency)?;
        let populated =
            input_resampler.pop_block(&mut chain_input, max_chain_frames, middle_samples)?;
        if populated < middle_samples {
            return Err(ChainError::Preparation(
                "boundary warm-up was incomplete".into(),
            ));
        }
        for channel in 0..channels {
            let start = channel * max_chain_frames;
            chain_output[start..start + populated]
                .copy_from_slice(&chain_input[start..start + populated]);
        }
        output_resampler.push_block(&chain_output, max_chain_frames, populated)?;
        Ok(Self {
            channels,
            max_host_frames,
            max_chain_frames,
            ratio,
            latency: latency as u64,
            input_resampler,
            output_resampler,
            chain_input,
            chain_output,
        })
    }

    fn process(
        &mut self,
        input: &PlanarBuffer<'_>,
        output: &mut PlanarBuffer<'_>,
        mut chain: impl FnMut(&mut PlanarBuffer<'_>) -> Result<(), ChainError>,
    ) -> Result<(), ChainError> {
        let frames = input.frames();
        self.input_resampler
            .push_block(input.as_slice(), frames, frames)?;
        let maximum_chain_frames = (frames as f64 / self.ratio).ceil() as usize;
        while self.input_resampler.samples_required_for(1) == 0 {
            let populated = self.input_resampler.pop_block(
                &mut self.chain_input,
                self.max_chain_frames,
                maximum_chain_frames,
            )?;
            if populated == 0 {
                return Err(ChainError::Processing("boundary made no chain progress"));
            }
            for channel in 0..self.channels {
                let start = channel * self.max_chain_frames;
                self.chain_output[start..start + populated]
                    .copy_from_slice(&self.chain_input[start..start + populated]);
            }
            {
                let mut chain_buffer = PlanarBuffer::new(
                    &mut self.chain_output[..self.channels * populated],
                    self.channels,
                    populated,
                )?;
                chain(&mut chain_buffer)?;
            }
            self.output_resampler.push_block(
                &self.chain_output,
                self.max_chain_frames,
                populated,
            )?;
        }
        let populated = self
            .output_resampler
            .pop_block(output.as_mut_slice(), frames, frames)?;
        if populated < frames {
            for channel in 0..self.channels {
                let last = if populated == 0 {
                    0.0
                } else {
                    output.channel(channel)?[populated - 1]
                };
                output.channel_mut(channel)?[populated..].fill(last);
            }
        }
        self.input_resampler.renormalize_phases();
        self.output_resampler.renormalize_phases();
        Ok(())
    }
}

fn validate_pair(
    input: &PlanarBuffer<'_>,
    output: &PlanarBuffer<'_>,
    channels: usize,
    max_frames: usize,
) -> Result<(), ChainError> {
    if input.channels() != channels
        || output.channels() != channels
        || input.frames() != output.frames()
        || input.frames() > max_frames
    {
        return Err(ChainError::InvalidLayout(
            "host input and output dimensions are incompatible",
        ));
    }
    Ok(())
}

fn allocate_samples(count: usize) -> Result<Vec<f32>, ChainError> {
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(count)
        .map_err(|_| ChainError::Preparation("boundary scratch allocation failed".into()))?;
    samples.resize(count, 0.0);
    Ok(samples)
}

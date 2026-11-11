use crate::{
    ChainError, PlanarBuffer,
    rate::halfband::{BranchStates, CHANNELS, decimate_stage, upsample_stage},
};

const WORK_BUFFERS: usize = 3;

/// Minimum-phase power-of-two oversampling around a chain or IR island.
pub struct ChainOversampler {
    factor: usize,
    max_base_frames: usize,
    capacity_per_channel: usize,
    work: [Vec<f32>; WORK_BUFFERS],
    up_a: BranchStates,
    up_b: BranchStates,
    down_even: BranchStates,
    down_odd: BranchStates,
}

impl ChainOversampler {
    /// Prepares a factor of 1, 2, 4, or 8 and every required scratch buffer.
    ///
    /// # Errors
    ///
    /// Returns [`ChainError::InvalidLayout`] for unsupported dimensions and a
    /// preparation error when scratch storage cannot be allocated.
    pub fn new(factor: u8, max_base_frames: usize) -> Result<Self, ChainError> {
        if !matches!(factor, 1 | 2 | 4 | 8) || max_base_frames == 0 {
            return Err(ChainError::InvalidLayout(
                "oversampling requires factor 1/2/4/8 and nonzero frames",
            ));
        }
        let factor = factor as usize;
        let capacity_per_channel = max_base_frames
            .checked_mul(factor)
            .ok_or(ChainError::InvalidLayout("oversampled capacity overflow"))?;
        let buffer_samples = capacity_per_channel
            .checked_mul(CHANNELS)
            .ok_or(ChainError::InvalidLayout("oversampled buffer overflow"))?;
        let mut work = [Vec::new(), Vec::new(), Vec::new()];
        for buffer in &mut work {
            buffer.try_reserve_exact(buffer_samples).map_err(|_| {
                ChainError::Preparation("oversampled scratch allocation failed".into())
            })?;
            buffer.resize(buffer_samples, 0.0);
        }
        Ok(Self {
            factor,
            max_base_frames,
            capacity_per_channel,
            work,
            up_a: crate::rate::halfband::EMPTY_STATES,
            up_b: crate::rate::halfband::EMPTY_STATES,
            down_even: crate::rate::halfband::EMPTY_STATES,
            down_odd: crate::rate::halfband::EMPTY_STATES,
        })
    }

    /// Oversampling factor.
    #[must_use]
    pub const fn factor(&self) -> usize {
        self.factor
    }

    /// Minimum-phase filters add no reported host latency.
    #[must_use]
    pub const fn latency_samples(&self) -> u64 {
        0
    }

    /// Clears every allpass delay line without reallocating.
    pub fn reset(&mut self) {
        self.up_a = crate::rate::halfband::EMPTY_STATES;
        self.up_b = crate::rate::halfband::EMPTY_STATES;
        self.down_even = crate::rate::halfband::EMPTY_STATES;
        self.down_odd = crate::rate::halfband::EMPTY_STATES;
    }

    /// Interpolates, processes at the chain rate, and decimates exactly once.
    ///
    /// # Errors
    ///
    /// Returns a layout error before mutation for incompatible channels,
    /// excessive frames, or an unprepared factor. Processing errors from the
    /// chain are returned to the caller.
    pub fn process(
        &mut self,
        input: &PlanarBuffer<'_>,
        output: &mut PlanarBuffer<'_>,
        mut chain: impl FnMut(&mut PlanarBuffer<'_>) -> Result<(), ChainError>,
    ) -> Result<(), ChainError> {
        self.validate_pair(input, output)?;
        if self.factor == 1 {
            copy_planar(input, output)?;
            return chain(output);
        }
        let channels = input.channels();
        let frames = input.frames();
        let oversampled_frames = frames * self.factor;
        let oversampled_samples = channels * oversampled_frames;
        let mut final_buffer = std::mem::take(&mut self.work[0]);
        run_up(
            input.as_slice(),
            channels,
            frames,
            &mut final_buffer[..oversampled_samples],
            &mut self.work,
            &mut self.up_a,
            &mut self.up_b,
        )?;
        self.work[0] = final_buffer;
        {
            let mut inner = PlanarBuffer::new(
                &mut self.work[0][..oversampled_samples],
                channels,
                oversampled_frames,
            )?;
            chain(&mut inner)?;
        }
        let inner = std::mem::take(&mut self.work[0]);
        run_down(
            &inner[..oversampled_samples],
            channels,
            oversampled_frames,
            output.as_mut_slice(),
            &mut self.work,
            &mut self.down_even,
            &mut self.down_odd,
        )?;
        self.work[0] = inner;
        Ok(())
    }

    /// Decimates an oversampled buffer, runs at base rate, then interpolates.
    ///
    /// # Errors
    ///
    /// Returns a layout error before mutation when the frame count is not
    /// divisible by the factor or exceeds prepared capacity.
    pub fn base_rate_island(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        mut base: impl FnMut(&mut PlanarBuffer<'_>) -> Result<(), ChainError>,
    ) -> Result<(), ChainError> {
        if buffer.channels() > CHANNELS
            || !buffer.frames().is_multiple_of(self.factor)
            || buffer.frames() > self.capacity_per_channel
        {
            return Err(ChainError::InvalidLayout(
                "IR island dimensions exceed the prepared factor or capacity",
            ));
        }
        if self.factor == 1 {
            return base(buffer);
        }
        let channels = buffer.channels();
        let oversampled_frames = buffer.frames();
        let base_frames = oversampled_frames / self.factor;
        let base_samples = channels * base_frames;
        let mut final_buffer = std::mem::take(&mut self.work[0]);
        run_down(
            buffer.as_slice(),
            channels,
            oversampled_frames,
            &mut final_buffer[..base_samples],
            &mut self.work,
            &mut self.down_even,
            &mut self.down_odd,
        )?;
        self.work[0] = final_buffer;
        {
            let mut inner =
                PlanarBuffer::new(&mut self.work[0][..base_samples], channels, base_frames)?;
            base(&mut inner)?;
        }
        let inner = std::mem::take(&mut self.work[0]);
        run_up(
            &inner[..base_samples],
            channels,
            base_frames,
            buffer.as_mut_slice(),
            &mut self.work,
            &mut self.up_a,
            &mut self.up_b,
        )?;
        self.work[0] = inner;
        Ok(())
    }

    fn validate_pair(
        &self,
        input: &PlanarBuffer<'_>,
        output: &PlanarBuffer<'_>,
    ) -> Result<(), ChainError> {
        if input.channels() == 0
            || input.channels() > CHANNELS
            || input.channels() != output.channels()
            || input.frames() != output.frames()
            || input.frames() > self.max_base_frames
        {
            return Err(ChainError::InvalidLayout(
                "oversampling input and output dimensions are incompatible",
            ));
        }
        Ok(())
    }
}

fn copy_planar(input: &PlanarBuffer<'_>, output: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
    output.as_mut_slice().copy_from_slice(input.as_slice());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_up(
    input: &[f32],
    channels: usize,
    frames: usize,
    final_output: &mut [f32],
    work: &mut [Vec<f32>; WORK_BUFFERS],
    up_a: &mut BranchStates,
    up_b: &mut BranchStates,
) -> Result<(), ChainError> {
    let stages = (final_output.len() / channels / frames).ilog2() as usize;
    if !matches!(stages, 1..=3) {
        return Err(ChainError::InvalidLayout("invalid up-cascade dimensions"));
    }
    for channel in 0..channels {
        let input_channel = &input[channel * frames..(channel + 1) * frames];
        if stages == 1 {
            let output = &mut final_output[channel * frames * 2..(channel + 1) * frames * 2];
            upsample_stage(0, channel, input_channel, output, up_a, up_b);
        } else {
            let output = &mut work[1][channel * frames * 2..(channel + 1) * frames * 2];
            upsample_stage(0, channel, input_channel, output, up_a, up_b);
        }
    }
    if stages == 2 {
        let stage_frames = frames * 2;
        for channel in 0..channels {
            let source = &work[1][channel * stage_frames..(channel + 1) * stage_frames];
            let output =
                &mut final_output[channel * stage_frames * 2..(channel + 1) * stage_frames * 2];
            upsample_stage(1, channel, source, output, up_a, up_b);
        }
    } else if stages == 3 {
        let stage_frames = frames * 2;
        let (lower, upper) = work.split_at_mut(2);
        for channel in 0..channels {
            let source = &lower[1][channel * stage_frames..(channel + 1) * stage_frames];
            let output =
                &mut upper[0][channel * stage_frames * 2..(channel + 1) * stage_frames * 2];
            upsample_stage(1, channel, source, output, up_a, up_b);
        }
        let stage_frames = frames * 4;
        for channel in 0..channels {
            let source = &work[2][channel * stage_frames..(channel + 1) * stage_frames];
            let output =
                &mut final_output[channel * stage_frames * 2..(channel + 1) * stage_frames * 2];
            upsample_stage(2, channel, source, output, up_a, up_b);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_down(
    input: &[f32],
    channels: usize,
    frames: usize,
    final_output: &mut [f32],
    work: &mut [Vec<f32>; WORK_BUFFERS],
    down_even: &mut BranchStates,
    down_odd: &mut BranchStates,
) -> Result<(), ChainError> {
    let stages = (frames / (final_output.len() / channels)).ilog2() as usize;
    if !matches!(stages, 1..=3) {
        return Err(ChainError::InvalidLayout("invalid down-cascade dimensions"));
    }
    if stages == 1 {
        for channel in 0..channels {
            decimate_stage(
                0,
                channel,
                &input[channel * frames..(channel + 1) * frames],
                &mut final_output[channel * frames / 2..(channel + 1) * frames / 2],
                down_even,
                down_odd,
            );
        }
        return Ok(());
    }
    let intermediate_frames = frames / 2;
    let intermediate = if stages == 3 {
        &mut work[1]
    } else {
        &mut work[2]
    };
    for channel in 0..channels {
        decimate_stage(
            stages - 1,
            channel,
            &input[channel * frames..(channel + 1) * frames],
            &mut intermediate[channel * intermediate_frames..(channel + 1) * intermediate_frames],
            down_even,
            down_odd,
        );
    }
    if stages == 3 {
        let (lower, upper) = work.split_at_mut(2);
        for channel in 0..channels {
            decimate_stage(
                1,
                channel,
                &lower[1][channel * intermediate_frames..(channel + 1) * intermediate_frames],
                &mut upper[0]
                    [channel * intermediate_frames / 2..(channel + 1) * intermediate_frames / 2],
                down_even,
                down_odd,
            );
        }
    }
    let final_frames = final_output.len() / channels;
    for channel in 0..channels {
        decimate_stage(
            0,
            channel,
            &work[2][channel * final_frames * 2..(channel + 1) * final_frames * 2],
            &mut final_output[channel * final_frames..(channel + 1) * final_frames],
            down_even,
            down_odd,
        );
    }
    Ok(())
}

use ronie_core::{DspError, DspResult};

use super::{CompiledA2, HEAD_KERNEL_SIZE};

/// Read-only history layout information for preparation verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct A2HistoryInfo {
    /// Power-of-two ring column capacity.
    pub ring_capacity: usize,
    /// Total stored `f32` values, including the mirrored tail.
    pub storage_values: usize,
    /// Number of values in the mirrored tail.
    pub tail_values: usize,
    /// Whether every stored history value is zero.
    pub all_zero: bool,
}

pub(crate) struct PreparedState<const C: usize> {
    pub(crate) layer_histories: Vec<RingHistory<C>>,
    pub(crate) head_history: RingHistory<C>,
    pub(crate) layer_in: Vec<f32>,
    pub(crate) head_sum: Vec<f32>,
    pub(crate) z: Vec<f32>,
    pub(crate) projected: Vec<f32>,
    pub(crate) cond: Vec<f32>,
    pub(crate) head_out: Vec<f32>,
    pub(crate) prewarm_input: Vec<f64>,
    pub(crate) prewarm_output: Vec<f64>,
    pub(crate) max_frames: usize,
}

impl<const C: usize> PreparedState<C> {
    pub(crate) fn new(model: &CompiledA2, max_frames: usize) -> DspResult<Self> {
        let mut layer_histories = Vec::with_capacity(model.layers.len());
        for layer in model.layers.iter() {
            layer_histories.push(RingHistory::new(layer.max_lookback(), max_frames)?);
        }
        let head_history = RingHistory::new(HEAD_KERNEL_SIZE - 1, max_frames)?;
        let scratch_values = C
            .checked_mul(max_frames)
            .ok_or(DspError::InvalidBufferSize)?;

        Ok(Self {
            layer_histories,
            head_history,
            layer_in: zero_f32(scratch_values)?,
            head_sum: zero_f32(scratch_values)?,
            z: zero_f32(scratch_values)?,
            projected: zero_f32(scratch_values)?,
            cond: zero_f32(max_frames)?,
            head_out: zero_f32(max_frames)?,
            prewarm_input: zero_f64(max_frames)?,
            prewarm_output: zero_f64(max_frames)?,
            max_frames,
        })
    }

    pub(crate) fn reset(&mut self) {
        for history in &mut self.layer_histories {
            history.reset();
        }
        self.head_history.reset();
        self.layer_in.fill(0.0);
        self.head_sum.fill(0.0);
        self.z.fill(0.0);
        self.projected.fill(0.0);
        self.cond.fill(0.0);
        self.head_out.fill(0.0);
        self.prewarm_input.fill(0.0);
        self.prewarm_output.fill(0.0);
    }
}

pub(crate) struct RingHistory<const C: usize> {
    values: Vec<f32>,
    capacity: usize,
    tail_values: usize,
    lookback: usize,
    write_pos: usize,
}

impl<const C: usize> RingHistory<C> {
    fn new(lookback: usize, max_frames: usize) -> DspResult<Self> {
        let required = lookback
            .checked_add(max_frames)
            .ok_or(DspError::InvalidBufferSize)?;
        let capacity = required
            .checked_next_power_of_two()
            .ok_or(DspError::InvalidBufferSize)?;
        let columns = capacity
            .checked_add(max_frames)
            .ok_or(DspError::InvalidBufferSize)?;
        let value_count = C.checked_mul(columns).ok_or(DspError::InvalidBufferSize)?;
        let tail_values = C
            .checked_mul(max_frames)
            .ok_or(DspError::InvalidBufferSize)?;

        Ok(Self {
            values: zero_f32(value_count)?,
            capacity,
            tail_values,
            lookback,
            write_pos: lookback,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.values.fill(0.0);
        self.write_pos = self.lookback;
    }

    pub(crate) fn info(&self) -> A2HistoryInfo {
        A2HistoryInfo {
            ring_capacity: self.capacity,
            storage_values: self.values.len(),
            tail_values: self.tail_values,
            all_zero: self.values.iter().all(|value| *value == 0.0),
        }
    }

    pub(crate) fn write(&mut self, input: &[f32]) {
        let frames = input.len() / C;
        for frame in 0..frames {
            let destination = (self.write_pos + frame) & (self.capacity - 1);
            for channel in 0..C {
                self.values[destination * C + channel] = input[frame * C + channel];
            }
        }

        let (ring, tail) = self.values.split_at_mut(self.capacity * C);
        tail.copy_from_slice(&ring[..self.tail_values]);
        self.write_pos = (self.write_pos + frames) & (self.capacity - 1);
    }

    pub(crate) fn tap_base(&self, frames: usize, taps_back: usize) -> usize {
        (self.write_pos + self.capacity - frames - taps_back) & (self.capacity - 1)
    }

    pub(crate) fn read(&self, base: usize, frame: usize, channel: usize) -> f32 {
        self.values[(base + frame) * C + channel]
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    pub(crate) fn values(&self) -> &[f32] {
        &self.values
    }
}

fn zero_f32(count: usize) -> DspResult<Vec<f32>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| DspError::InvalidBufferSize)?;
    values.resize(count, 0.0);
    Ok(values)
}

fn zero_f64(count: usize) -> DspResult<Vec<f64>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| DspError::InvalidBufferSize)?;
    values.resize(count, 0.0);
    Ok(values)
}

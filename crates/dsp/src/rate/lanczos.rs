use std::sync::OnceLock;

use crate::ChainError;

const A: usize = 12;
const FILTER_WIDTH: usize = A * 2;
const TABLE_POINTS: usize = 8192;
const BUFFER_SIZE: usize = 131_072;

struct LanczosTables {
    values: Vec<[f32; FILTER_WIDTH]>,
    deltas: Vec<[f32; FILTER_WIDTH]>,
}

static TABLES: OnceLock<LanczosTables> = OnceLock::new();

fn tables() -> &'static LanczosTables {
    TABLES.get_or_init(|| {
        let mut values = vec![[0.0; FILTER_WIDTH]; TABLE_POINTS + 1];
        for (table_index, row) in values.iter_mut().enumerate() {
            let x0 = table_index as f64 / TABLE_POINTS as f64;
            for (tap, coefficient) in row.iter_mut().enumerate() {
                let x = x0 + tap as f64 - A as f64;
                *coefficient = if x.abs() < 1.0e-7 {
                    1.0
                } else {
                    (A as f64
                        * (std::f64::consts::PI * x).sin()
                        * (std::f64::consts::PI * x / A as f64).sin()
                        / (std::f64::consts::PI.powi(2) * x * x)) as f32
                };
            }
        }
        let mut deltas = vec![[0.0; FILTER_WIDTH]; TABLE_POINTS + 1];
        for table_index in 0..TABLE_POINTS {
            for tap in 0..FILTER_WIDTH {
                deltas[table_index][tap] = values[table_index + 1][tap] - values[table_index][tap];
            }
        }
        deltas[TABLE_POINTS] = deltas[0];
        LanczosTables { values, deltas }
    })
}

pub(crate) struct Lanczos12 {
    channels: usize,
    history: Vec<f32>,
    write_position: usize,
    phase_in: i64,
    phase_out: i64,
    phase_in_increment: i64,
    phase_out_increment: i64,
    phase_denominator: i64,
}

impl Lanczos12 {
    pub(crate) fn new(
        input_rate: f64,
        output_rate: f64,
        channels: usize,
    ) -> Result<Self, ChainError> {
        if !matches!(channels, 1 | 2) {
            return Err(ChainError::InvalidLayout(
                "Lanczos supports one or two channels",
            ));
        }
        let input_rate = rate_as_integer(input_rate)?;
        let output_rate = rate_as_integer(output_rate)?;
        let divisor = gcd(input_rate, output_rate);
        let phase_in_increment = output_rate / divisor;
        let phase_out_increment = input_rate / divisor;
        let samples = channels
            .checked_mul(BUFFER_SIZE * 2)
            .ok_or(ChainError::InvalidLayout("Lanczos history overflow"))?;
        let mut history = Vec::new();
        history
            .try_reserve_exact(samples)
            .map_err(|_| ChainError::Preparation("Lanczos history allocation failed".into()))?;
        history.resize(samples, 0.0);
        let _ = tables();
        Ok(Self {
            channels,
            history,
            write_position: 0,
            phase_in: 0,
            phase_out: 0,
            phase_in_increment,
            phase_out_increment,
            phase_denominator: phase_in_increment,
        })
    }

    pub(crate) fn samples_required_for(&self, output_samples: usize) -> usize {
        let output_samples = output_samples as i64;
        let required = A as f64 + 1.0
            - (self.phase_in - self.phase_out - self.phase_out_increment * output_samples) as f64
                / self.phase_denominator as f64;
        (required + 1.0).max(0.0) as usize
    }

    pub(crate) fn push_block(
        &mut self,
        input: &[f32],
        input_stride: usize,
        frames: usize,
    ) -> Result<(), ChainError> {
        if input_stride < frames || input.len() != self.channels * input_stride {
            return Err(ChainError::InvalidLayout("Lanczos input length mismatch"));
        }
        let stride = BUFFER_SIZE * 2;
        for frame in 0..frames {
            for channel in 0..self.channels {
                let sample = input[channel * input_stride + frame];
                self.history[channel * stride + self.write_position] = sample;
                self.history[channel * stride + self.write_position + BUFFER_SIZE] = sample;
            }
            self.write_position = (self.write_position + 1) & (BUFFER_SIZE - 1);
            self.phase_in = self
                .phase_in
                .checked_add(self.phase_in_increment)
                .ok_or(ChainError::Processing("Lanczos input phase overflow"))?;
        }
        Ok(())
    }

    pub(crate) fn pop_block(
        &mut self,
        output: &mut [f32],
        output_frames: usize,
        maximum: usize,
    ) -> Result<usize, ChainError> {
        if maximum > output_frames || output.len() != self.channels * output_frames {
            return Err(ChainError::InvalidLayout("Lanczos output length mismatch"));
        }
        let mut populated = 0;
        while populated < maximum
            && self.phase_in - self.phase_out > self.phase_denominator * (A as i64 + 1)
        {
            let back = (self.phase_in - self.phase_out) as f64 / self.phase_denominator as f64;
            self.read_samples(back, output, output_frames, populated);
            self.phase_out = self
                .phase_out
                .checked_add(self.phase_out_increment)
                .ok_or(ChainError::Processing("Lanczos output phase overflow"))?;
            populated += 1;
        }
        Ok(populated)
    }

    pub(crate) fn renormalize_phases(&mut self) {
        self.phase_in -= self.phase_out;
        self.phase_out = 0;
    }

    fn read_samples(&self, back: f64, output: &mut [f32], stride: usize, index: usize) {
        let tables = tables();
        let read_position = self.write_position as f64 - back;
        let mut read_index = read_position.floor() as isize;
        let fraction = 1.0 - (read_position - read_index as f64);
        read_index = (read_index + BUFFER_SIZE as isize) & (BUFFER_SIZE as isize - 1);
        if read_index <= A as isize {
            read_index += BUFFER_SIZE as isize;
        }
        let table_position = fraction * TABLE_POINTS as f64;
        let table_index = table_position as usize;
        let table_fraction = table_position - table_index as f64;
        let history_stride = BUFFER_SIZE * 2;

        for channel in 0..self.channels {
            let mut sum = 0.0_f32;
            for tap in 0..A {
                let f0 = (tables.values[table_index][tap] as f64
                    + tables.deltas[table_index][tap] as f64 * table_fraction)
                    as f32;
                let f1 = (tables.values[table_index][A + tap] as f64
                    + tables.deltas[table_index][A + tap] as f64 * table_fraction)
                    as f32;
                let d0 = self.history[channel * history_stride + read_index as usize - A + tap];
                let d1 = self.history[channel * history_stride + read_index as usize + tap];
                sum += f0 * d0 + f1 * d1;
            }
            output[channel * stride + index] = sum;
        }
    }
}

fn rate_as_integer(rate: f64) -> Result<i64, ChainError> {
    let rate = rate as f32;
    if !rate.is_finite() || rate <= 0.0 {
        return Err(ChainError::InvalidLayout("sample rates must be positive"));
    }
    let integer = rate as i64;
    if integer == 0 {
        return Err(ChainError::InvalidLayout("sample rate truncates to zero"));
    }
    Ok(integer)
}

fn gcd(mut left: i64, mut right: i64) -> i64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

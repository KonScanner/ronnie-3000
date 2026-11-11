use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{ChainError, PlanarBuffer};

const SWEEP_SECONDS: f64 = 0.28;
const SWEEP_LOW_HZ: f64 = 30.0;
const SWEEP_HIGH_FRACTION: f64 = 0.45;
const PROBE_EDGE_FADE_SECONDS: f64 = 0.005;
const PROBE_AMPLITUDE: f32 = 0.25;
const TAIL_SECONDS: f64 = 0.10;
const MUTE_FADE_SECONDS: f64 = 0.005;
const RAMP_BACK_SECONDS: f64 = 0.010;
const PHAT_RHO: f32 = 0.8;
const FINE_STEPS_PER_SAMPLE: i32 = 8;
const MAX_LAG_MS: f32 = 24.0;

/// Probe and analysis states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AutoOffsetState {
    /// No measurement is active.
    Idle = 0,
    /// Output is fading to silence.
    FadeOut,
    /// The exponential sweep is playing.
    Probing,
    /// Chain tails are being captured.
    Tail,
    /// Capture is ready for off-thread analysis.
    Captured,
    /// Estimation is complete; parameters must be applied before resume.
    Analyzing,
    /// Output is returning to unity.
    RampBack,
}

/// Auto-align measurement in the Align parameter convention.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AutoOffsetResult {
    /// Corrective delay in ms; positive delays the right chain.
    pub offset_ms: f32,
    /// Whether the chains are polarity-inverted relative to each other.
    pub inverted: bool,
    /// Normalized raw-waveform correlation at the winning lag.
    pub confidence: f32,
    /// Winning peak against the best peak more than 1 ms away.
    pub peak_sharpness: f32,
}

/// Deterministic sweep, capture, and soft-PHAT alignment workflow.
pub struct AutoOffset {
    sample_rate: f64,
    sweep_samples: usize,
    capacity: usize,
    probe: Vec<f32>,
    capture: [Vec<f32>; 2],
    probe_pos: usize,
    output_gain: f32,
    mute_step: f32,
    ramp_step: f32,
    written: AtomicUsize,
    state: AtomicUsize,
}

impl AutoOffset {
    /// Builds the sweep and allocates the full capture off the audio thread.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64) -> Result<Self, ChainError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(ChainError::InvalidLayout(
                "auto-offset sample rate is invalid",
            ));
        }
        let sweep_samples = (SWEEP_SECONDS * sample_rate).round() as usize;
        let max_lag = (MAX_LAG_MS as f64 * 0.001 * sample_rate).ceil() as usize;
        let tail_samples = ((TAIL_SECONDS * sample_rate).round() as usize).max(2 * max_lag);
        let capacity = sweep_samples + tail_samples;
        let mut offset = Self {
            sample_rate,
            sweep_samples,
            capacity,
            probe: vec![0.0; sweep_samples],
            capture: [vec![0.0; capacity], vec![0.0; capacity]],
            probe_pos: 0,
            output_gain: 1.0,
            mute_step: 0.0,
            ramp_step: 0.0,
            written: AtomicUsize::new(0),
            state: AtomicUsize::new(AutoOffsetState::Idle as usize),
        };
        offset.build_sweep();
        Ok(offset)
    }

    /// Starts the fade-out and a new fixed capture schedule.
    pub fn arm(&mut self) {
        if self.state() != AutoOffsetState::Idle {
            return;
        }
        self.probe_pos = 0;
        self.mute_step = -1.0 / (MUTE_FADE_SECONDS * self.sample_rate).max(1.0) as f32;
        self.ramp_step = 1.0 / (RAMP_BACK_SECONDS * self.sample_rate).max(1.0) as f32;
        self.written.store(0, Ordering::Relaxed);
        self.set_state(AutoOffsetState::FadeOut);
    }

    /// Cancels and ramps back to idle without publishing a result.
    pub fn cancel(&mut self) {
        if self.state() != AutoOffsetState::Idle {
            self.set_state(AutoOffsetState::RampBack);
        }
    }

    /// Allows output to ramp back after parameters are applied.
    pub fn resume(&mut self) {
        if self.state() == AutoOffsetState::Analyzing {
            self.set_state(AutoOffsetState::RampBack);
        }
    }

    /// Current state.
    #[must_use]
    pub fn state(&self) -> AutoOffsetState {
        match self.state.load(Ordering::Acquire) {
            1 => AutoOffsetState::FadeOut,
            2 => AutoOffsetState::Probing,
            3 => AutoOffsetState::Tail,
            4 => AutoOffsetState::Captured,
            5 => AutoOffsetState::Analyzing,
            6 => AutoOffsetState::RampBack,
            _ => AutoOffsetState::Idle,
        }
    }

    /// Capture progress from zero to one.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.capacity == 0 {
            return 0.0;
        }
        (self.written.load(Ordering::Relaxed) as f32 / self.capacity as f32).clamp(0.0, 1.0)
    }

    /// Fills the chain input with sweep then tail silence while those states own it.
    pub fn render_probe_input(&mut self, output: &mut [f32]) -> bool {
        let state = self.state();
        if !matches!(state, AutoOffsetState::Probing | AutoOffsetState::Tail) {
            return false;
        }
        for sample in output {
            *sample = if self.probe_pos < self.sweep_samples {
                self.probe[self.probe_pos]
            } else {
                0.0
            };
            self.probe_pos += 1;
        }
        if state == AutoOffsetState::Probing && self.probe_pos >= self.sweep_samples {
            self.set_state(AutoOffsetState::Tail);
        }
        true
    }

    /// Appends raw chain outputs while probing without allocating.
    pub fn capture_chain_outputs(&mut self, left: &[f32], right: &[f32]) {
        let state = self.state();
        if !matches!(state, AutoOffsetState::Probing | AutoOffsetState::Tail) {
            return;
        }
        let have = self.written.load(Ordering::Relaxed);
        let take = left.len().min(right.len()).min(self.capacity - have);
        if take == 0 {
            return;
        }
        self.capture[0][have..have + take].copy_from_slice(&left[..take]);
        self.capture[1][have..have + take].copy_from_slice(&right[..take]);
        self.written.store(have + take, Ordering::Relaxed);
        if have + take >= self.capacity {
            self.set_state(AutoOffsetState::Captured);
        }
    }

    /// Applies mute fade, silent hold, and ramp-back to a host buffer.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for an invalid channel view.
    pub fn apply_output_gain(&mut self, output: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
        let state = self.state();
        if state == AutoOffsetState::Idle {
            self.output_gain = 1.0;
            return Ok(());
        }
        if state == AutoOffsetState::FadeOut {
            for frame in 0..output.frames() {
                self.output_gain = (self.output_gain + self.mute_step).max(0.0);
                for channel in 0..output.channels() {
                    output.channel_mut(channel)?[frame] *= self.output_gain;
                }
            }
            if self.output_gain <= 0.0 {
                self.set_state(AutoOffsetState::Probing);
            }
            return Ok(());
        }
        if state == AutoOffsetState::RampBack {
            for frame in 0..output.frames() {
                self.output_gain = (self.output_gain + self.ramp_step).min(1.0);
                for channel in 0..output.channels() {
                    output.channel_mut(channel)?[frame] *= self.output_gain;
                }
            }
            if self.output_gain >= 1.0 {
                self.set_state(AutoOffsetState::Idle);
            }
            return Ok(());
        }
        for sample in output.as_mut_slice() {
            *sample = 0.0;
        }
        self.output_gain = 0.0;
        Ok(())
    }

    /// Runs one soft-PHAT estimation off the audio thread.
    #[must_use]
    pub fn analyze(&mut self) -> AutoOffsetResult {
        let mut result = AutoOffsetResult::default();
        if self.state() != AutoOffsetState::Captured {
            self.cancel();
            return result;
        }
        self.set_state(AutoOffsetState::Analyzing);
        let count = self.written.load(Ordering::Relaxed);
        let max_lag = (MAX_LAG_MS as f64 * 0.001 * self.sample_rate).ceil() as i32;
        let left = &self.capture[0][..count];
        let right = &self.capture[1][..count];
        let fft_order = (count + max_lag as usize)
            .max(2)
            .next_power_of_two()
            .ilog2();
        let fft_size = 1 << fft_order;
        let fft = FftPlan::new(fft_size);
        let mut spectrum_left = vec![Complex::default(); fft_size];
        let mut spectrum_right = vec![Complex::default(); fft_size];
        fft.forward(left, &mut spectrum_left);
        fft.forward(right, &mut spectrum_right);
        let epsilon = 1.0e-12_f32;
        for bin in 0..fft_size {
            let ar = spectrum_left[bin].re;
            let ai = spectrum_left[bin].im;
            let br = spectrum_right[bin].re;
            let bi = spectrum_right[bin].im;
            let xr = ar * br + ai * bi;
            let xi = ai * br - ar * bi;
            let magnitude = (xr * xr + xi * xi).sqrt();
            let weight = 1.0 / magnitude.powf(PHAT_RHO).max(epsilon);
            spectrum_left[bin] = Complex {
                re: xr * weight,
                im: xi * weight,
            };
        }
        let cross_spectrum = spectrum_left.clone();
        let correlation = fft.inverse(&mut spectrum_left);
        let corr_at = |lag: i32| {
            correlation[if lag >= 0 {
                lag as usize
            } else {
                (fft_size as i32 + lag) as usize
            }]
        };
        let mut best_lag = 0;
        let mut best_abs = corr_at(0).abs();
        for lag in -max_lag..=max_lag {
            let magnitude = corr_at(lag).abs();
            if magnitude > best_abs {
                best_abs = magnitude;
                best_lag = lag;
            }
        }
        let inverted = corr_at(best_lag) < 0.0;
        let guard = (0.001 * self.sample_rate).round() as i32;
        let mut second_abs = 0.0_f32;
        for lag in -max_lag..=max_lag {
            if (lag - best_lag).abs() > guard {
                second_abs = second_abs.max(corr_at(lag).abs());
            }
        }
        result.peak_sharpness = best_abs / second_abs.max(epsilon);

        let corr_fractional = |tau: f64| {
            let mut accumulated = 0.0;
            let w0 = 2.0 * std::f64::consts::PI * tau / fft_size as f64;
            for (bin, value) in cross_spectrum.iter().enumerate() {
                let signed_bin = if bin <= fft_size / 2 {
                    bin as f64
                } else {
                    bin as f64 - fft_size as f64
                };
                let theta = w0 * signed_bin;
                accumulated +=
                    f64::from(value.re) * theta.cos() - f64::from(value.im) * theta.sin();
            }
            if inverted { -accumulated } else { accumulated }
        };
        let step = 1.0 / FINE_STEPS_PER_SAMPLE as f64;
        let mut fine_tau = f64::from(best_lag);
        let mut fine_value = corr_fractional(fine_tau);
        let mut tau = f64::from(best_lag) - 1.0;
        while tau <= f64::from(best_lag) + 1.0 + 1.0e-9 {
            let value = corr_fractional(tau);
            if value > fine_value {
                fine_value = value;
                fine_tau = tau;
            }
            tau += step;
        }
        let lower = corr_fractional(fine_tau - step);
        let upper = corr_fractional(fine_tau + step);
        let denominator = lower - 2.0 * fine_value + upper;
        if denominator.abs() > 1.0e-20 {
            fine_tau += (0.5 * (lower - upper) / denominator).clamp(-0.5, 0.5) * step;
        }

        let lag = fine_tau.round() as i32;
        let from = lag.max(0) as usize;
        let to = ((count as i32 + lag).min(count as i32)) as usize;
        let mut dot = 0.0;
        let mut energy_left = 0.0;
        let mut energy_right = 0.0;
        for index in from..to {
            dot += f64::from(left[index]) * f64::from(right[(index as i32 - lag) as usize]);
            energy_left += f64::from(left[index]) * f64::from(left[index]);
            energy_right += f64::from(right[(index as i32 - lag) as usize])
                * f64::from(right[(index as i32 - lag) as usize]);
        }
        let denominator = (energy_left * energy_right).max(1.0e-24).sqrt();
        result.offset_ms =
            (fine_tau as f32 * 1000.0 / self.sample_rate as f32).clamp(-MAX_LAG_MS, MAX_LAG_MS);
        result.inverted = inverted;
        result.confidence = (dot.abs() / denominator).clamp(0.0, 1.0) as f32;
        result
    }

    fn set_state(&mut self, state: AutoOffsetState) {
        self.state.store(state as usize, Ordering::Release);
    }

    fn build_sweep(&mut self) {
        let low = SWEEP_LOW_HZ;
        let high = SWEEP_HIGH_FRACTION * self.sample_rate.min(48_000.0);
        let duration = self.sweep_samples as f64 / self.sample_rate;
        let log_ratio = (high / low).ln();
        let k = 2.0 * std::f64::consts::PI * low * duration / log_ratio;
        let fade_samples = (self.sweep_samples / 4)
            .min((PROBE_EDGE_FADE_SECONDS * self.sample_rate).round() as usize);
        for index in 0..self.sweep_samples {
            let time = index as f64 / self.sample_rate;
            let phase = k * ((time / duration * log_ratio).exp() - 1.0);
            let mut sample = PROBE_AMPLITUDE * phase.sin() as f32;
            if index < fade_samples {
                sample *=
                    0.5 - 0.5 * (std::f32::consts::PI * index as f32 / fade_samples as f32).cos();
            } else if index >= self.sweep_samples - fade_samples {
                let edge = self.sweep_samples - 1 - index;
                sample *=
                    0.5 - 0.5 * (std::f32::consts::PI * edge as f32 / fade_samples as f32).cos();
            }
            self.probe[index] = sample;
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    fn multiply(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
}

struct FftPlan {
    size: usize,
    bit_reversal: Vec<usize>,
    twiddles: Vec<Vec<Complex>>,
}

impl FftPlan {
    fn new(size: usize) -> Self {
        let levels = size.ilog2() as usize;
        let mut bit_reversal = Vec::with_capacity(size);
        for index in 0..size {
            bit_reversal.push(index.reverse_bits() >> (usize::BITS as usize - levels));
        }
        let mut twiddles = Vec::with_capacity(levels + 1);
        twiddles.push(Vec::new());
        for level in 1..=levels {
            let length = 1 << level;
            let mut stage = Vec::with_capacity(length / 2);
            for index in 0..length / 2 {
                let angle = -2.0 * std::f64::consts::PI * index as f64 / length as f64;
                stage.push(Complex {
                    re: angle.cos() as f32,
                    im: angle.sin() as f32,
                });
            }
            twiddles.push(stage);
        }
        Self {
            size,
            bit_reversal,
            twiddles,
        }
    }

    fn forward(&self, input: &[f32], output: &mut [Complex]) {
        for (index, value) in input.iter().enumerate() {
            output[index] = Complex {
                re: *value,
                im: 0.0,
            };
        }
        self.transform(output, false);
    }

    fn inverse(&self, input: &mut [Complex]) -> Vec<f32> {
        self.transform(input, true);
        input
            .iter()
            .map(|value| value.re / self.size as f32)
            .collect()
    }

    fn transform(&self, values: &mut [Complex], inverse: bool) {
        for (index, reversed) in self.bit_reversal.iter().enumerate() {
            if index < *reversed {
                values.swap(index, *reversed);
            }
        }
        let mut length = 2;
        let mut stage = 1;
        while length <= self.size {
            let half = length / 2;
            for start in (0..self.size).step_by(length) {
                for index in 0..half {
                    let left = values[start + index];
                    let mut twiddle = self.twiddles[stage][index];
                    if inverse {
                        twiddle.im = -twiddle.im;
                    }
                    let right = values[start + index + half].multiply(twiddle);
                    values[start + index] = Complex {
                        re: left.re + right.re,
                        im: left.im + right.im,
                    };
                    values[start + index + half] = Complex {
                        re: left.re - right.re,
                        im: left.im - right.im,
                    };
                }
            }
            length *= 2;
            stage += 1;
        }
    }
}

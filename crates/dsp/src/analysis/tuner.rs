use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::ChainError;

const RING_SIZE: usize = 1 << 15;
const MAX_DECIMATION: usize = 16;
const WINDOW_SIZE: usize = 2048;
const TARGET_ANALYSIS_RATE: f64 = 12_000.0;
const MIN_FREQUENCY: f64 = 28.0;
const MAX_FREQUENCY: f64 = 1500.0;
const SILENCE_DB: f64 = -55.0;
const YIN_THRESHOLD: f64 = 0.15;
const ANALYSIS_INTERVAL_MS: i64 = 25;

/// Plain tuner display data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TunerReading {
    /// Detected fundamental in hertz, or zero when no pitch is reliable.
    pub frequency_hz: f64,
    /// YIN confidence from zero to one.
    pub confidence: f64,
    /// Decimated input level in dBFS.
    pub level_db: f64,
}

impl TunerReading {
    /// Signed pitch error in cents relative to a target note.
    #[must_use]
    pub fn cents_off(self, target_hz: f64) -> f64 {
        if self.frequency_hz > 0.0 && target_hz > 0.0 {
            1200.0 * (self.frequency_hz / target_hz).log2()
        } else {
            0.0
        }
    }

    /// Whether the detected note is within ±5 cents of the target.
    #[must_use]
    pub fn in_tune(self, target_hz: f64) -> bool {
        self.cents_off(target_hz).abs() <= 5.0
    }
}

/// Monophonic YIN detector with lock-free sample publication.
pub struct TunerDetector {
    ring: Vec<f32>,
    write_pos: AtomicUsize,
    enabled: AtomicBool,
    sample_rate: f64,
    decimation: usize,
    raw_window: Vec<f32>,
    window: Vec<f32>,
    yin: Vec<f32>,
    last_frequency: f64,
    last_confidence: f64,
    last_level_db: f64,
    last_analysis_ms: i64,
}

impl TunerDetector {
    /// Allocates the ring and all analysis scratch at the largest decimation.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64) -> Result<Self, ChainError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(ChainError::InvalidLayout("tuner sample rate is invalid"));
        }
        let decimation = (sample_rate / TARGET_ANALYSIS_RATE)
            .round()
            .clamp(1.0, MAX_DECIMATION as f64) as usize;
        Ok(Self {
            ring: vec![0.0; RING_SIZE],
            write_pos: AtomicUsize::new(0),
            enabled: AtomicBool::new(false),
            sample_rate,
            decimation,
            raw_window: vec![0.0; WINDOW_SIZE * MAX_DECIMATION],
            window: vec![0.0; WINDOW_SIZE],
            yin: vec![0.0; WINDOW_SIZE / 2],
            last_frequency: 0.0,
            last_confidence: 0.0,
            last_level_db: -120.0,
            last_analysis_ms: 0,
        })
    }

    /// Enables or disables raw input publication.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    /// Whether raw input is currently accepted.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Copies raw pre-gain/pre-gate samples into the ring without allocating.
    pub fn push_samples(&mut self, samples: &[f32]) {
        if !self.is_enabled() {
            return;
        }
        let mut position = self.write_pos.load(Ordering::Relaxed);
        for sample in samples {
            self.ring[position] = *sample;
            position = (position + 1) & (RING_SIZE - 1);
        }
        self.write_pos.store(position, Ordering::Release);
    }

    /// Analyzes at most once per 25 ms and returns display data without allocating.
    #[must_use]
    pub fn get_reading(&mut self, now_ms: i64) -> TunerReading {
        if !self.is_enabled() {
            return TunerReading {
                frequency_hz: self.last_frequency,
                confidence: self.last_confidence,
                level_db: self.last_level_db,
            };
        }
        if now_ms - self.last_analysis_ms >= ANALYSIS_INTERVAL_MS {
            self.last_analysis_ms = now_ms;
            self.analyze();
        }
        TunerReading {
            frequency_hz: self.last_frequency,
            confidence: self.last_confidence,
            level_db: self.last_level_db,
        }
    }

    fn analyze(&mut self) {
        let raw_count = WINDOW_SIZE * self.decimation;
        let end = self.write_pos.load(Ordering::Acquire);
        let start = (end.wrapping_sub(raw_count)) & (RING_SIZE - 1);
        for (index, sample) in self.raw_window.iter_mut().take(raw_count).enumerate() {
            *sample = self.ring[(start + index) & (RING_SIZE - 1)];
        }

        let mut sum_squares = 0.0;
        for index in 0..WINDOW_SIZE {
            let mut accumulated = 0.0;
            for decimated in 0..self.decimation {
                accumulated += self.raw_window[index * self.decimation + decimated];
            }
            let sample = accumulated / self.decimation as f32;
            self.window[index] = sample;
            sum_squares += f64::from(sample) * f64::from(sample);
        }
        let rms = (sum_squares / WINDOW_SIZE as f64).sqrt();
        self.last_level_db = if rms > 1.0e-9 {
            20.0 * rms.log10()
        } else {
            -120.0
        };
        if self.last_level_db < SILENCE_DB {
            self.last_frequency = 0.0;
            self.last_confidence = 0.0;
            return;
        }

        let decimated_rate = self.sample_rate / self.decimation as f64;
        let tau_max = ((decimated_rate / MIN_FREQUENCY) as usize).min(WINDOW_SIZE / 2 - 1);
        let tau_min = (decimated_rate / MAX_FREQUENCY).max(2.0) as usize;
        let integration_length = WINDOW_SIZE - tau_max;
        self.yin[0] = 1.0;
        let mut running_sum = 0.0;
        for tau in 1..=tau_max {
            let mut difference = 0.0;
            for index in 0..integration_length {
                let delta = f64::from(self.window[index]) - f64::from(self.window[index + tau]);
                difference += delta * delta;
            }
            running_sum += difference;
            self.yin[tau] = if running_sum > 0.0 {
                (difference * tau as f64 / running_sum) as f32
            } else {
                1.0
            };
        }

        let mut best_tau = None;
        for tau in tau_min..=tau_max {
            if self.yin[tau] < YIN_THRESHOLD as f32 {
                let mut minimum = tau;
                while minimum < tau_max && self.yin[minimum + 1] < self.yin[minimum] {
                    minimum += 1;
                }
                best_tau = Some(minimum);
                break;
            }
        }
        let best_tau = match best_tau {
            Some(tau) => tau,
            None => {
                let mut minimum_value = 1.0;
                let mut minimum_tau = tau_min;
                for tau in tau_min..=tau_max {
                    if self.yin[tau] < minimum_value {
                        minimum_value = self.yin[tau];
                        minimum_tau = tau;
                    }
                }
                if minimum_value > 0.3 {
                    self.last_frequency = 0.0;
                    self.last_confidence = 0.0;
                    return;
                }
                minimum_tau
            }
        };

        let mut refined_tau = best_tau as f64;
        if best_tau > tau_min && best_tau < tau_max {
            let y0 = f64::from(self.yin[best_tau - 1]);
            let y1 = f64::from(self.yin[best_tau]);
            let y2 = f64::from(self.yin[best_tau + 1]);
            let denominator = y0 - 2.0 * y1 + y2;
            if denominator.abs() > 1.0e-12 {
                refined_tau += 0.5 * (y0 - y2) / denominator;
            }
        }
        self.last_frequency = decimated_rate / refined_tau;
        self.last_confidence = (1.0 - f64::from(self.yin[best_tau])).clamp(0.0, 1.0);
    }
}

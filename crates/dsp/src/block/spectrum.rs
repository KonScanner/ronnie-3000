use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use crate::ir::uniform::{Complex32, FftPlan};

const RING_SIZE: usize = 4096;
const FFT_SIZE: usize = 2048;
const BINS: usize = 64;
/// The tap stays open for this part of a second after each request.
const LEASES_PER_SECOND: f64 = 2.0;

/// Lock-free output of one block, from the callback to the thread that shows its spectrum.
///
/// Only the callback writes. The tap copies samples only for a short time after [`Self::open`],
/// so a view that closes needs to send no message, and a closed tap costs one atomic load.
#[derive(Debug)]
pub struct SpectrumTap {
    ring: Box<[AtomicU32]>,
    ring1: Box<[AtomicU32]>,
    written: AtomicUsize,
    independent: AtomicBool,
    lease: AtomicU32,
    sample_rate: f64,
}

impl SpectrumTap {
    fn new(sample_rate: f64) -> Arc<Self> {
        let ring = || (0..RING_SIZE).map(|_| AtomicU32::new(0)).collect();
        Arc::new(Self {
            ring: ring(),
            ring1: ring(),
            written: AtomicUsize::new(0),
            independent: AtomicBool::new(false),
            lease: AtomicU32::new(0),
            sample_rate,
        })
    }

    /// Asks the callback to copy this block's output for a short time.
    pub fn open(&self) {
        self.lease.store(
            (self.sample_rate / LEASES_PER_SECOND) as u32,
            Ordering::Release,
        );
    }

    /// Whether the callback still copies this block's output.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.lease.load(Ordering::Acquire) > 0
    }

    /// Total frames that the callback has copied.
    #[must_use]
    pub fn samples_written(&self) -> usize {
        self.written.load(Ordering::Acquire)
    }

    /// Spends the lease for one callback, and returns whether the callback must copy.
    ///
    /// A request that renews the lease at this moment can lose. The next request renews it again.
    fn spend_lease(&self, frames: usize) -> bool {
        let lease = self.lease.load(Ordering::Acquire);
        if lease == 0 {
            return false;
        }
        let frames = u32::try_from(frames).unwrap_or(u32::MAX);
        self.lease
            .store(lease.saturating_sub(frames), Ordering::Release);
        true
    }

    fn push(&self, ch0: &[f32], ch1: Option<&[f32]>, independent: bool) {
        let independent = independent && ch1.is_some();
        let mut position = self.written.load(Ordering::Relaxed);
        for (index, sample) in ch0.iter().enumerate() {
            let slot = position & (RING_SIZE - 1);
            let first = match ch1 {
                Some(right) if !independent => 0.5 * (*sample + right[index]),
                _ => *sample,
            };
            self.ring[slot].store(first.to_bits(), Ordering::Relaxed);
            if independent && let Some(right) = ch1 {
                self.ring1[slot].store(right[index].to_bits(), Ordering::Relaxed);
            }
            position = position.wrapping_add(1);
        }
        self.independent.store(independent, Ordering::Relaxed);
        self.written.store(position, Ordering::Release);
    }
}

/// Prepared per-block spectrum source. Its callback side only copies samples.
pub(super) struct Spectrum {
    tap: Arc<SpectrumTap>,
    enabled: bool,
    analyzer: SpectrumAnalyzer,
}

impl Spectrum {
    /// Allocates analysis state for a fixed chain sample rate.
    #[must_use]
    pub(super) fn new(sample_rate: f64) -> Self {
        let tap = SpectrumTap::new(sample_rate);
        Self {
            analyzer: SpectrumAnalyzer::new(Arc::clone(&tap)),
            tap,
            enabled: false,
        }
    }

    /// The tap that a reader on another thread uses.
    pub(super) fn tap(&self) -> Arc<SpectrumTap> {
        Arc::clone(&self.tap)
    }

    /// Enables or disables audio-thread ring writes.
    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Adds final block output without allocating or analyzing in the callback.
    pub(super) fn push_samples(&self, ch0: &[f32], ch1: Option<&[f32]>, independent: bool) {
        if self.tap.spend_lease(ch0.len()) || self.enabled {
            self.tap.push(ch0, ch1, independent);
        }
    }

    /// Whether this analyzer accepts samples, so that a caller can skip what it needs for them.
    #[must_use]
    pub(super) fn accepts_samples(&self) -> bool {
        self.enabled || self.tap.is_open()
    }

    /// Returns log-spaced dB bins, analyzing at most every 30 ms off-thread.
    #[must_use]
    pub(super) fn get_spectrum(&mut self, now_ms: i64) -> [f32; BINS] {
        if !self.enabled {
            return [-100.0; BINS];
        }
        self.analyzer.bins(now_ms)
    }
}

/// The FFT side of a spectrum. It runs on the thread that reads, never in the callback.
pub struct SpectrumAnalyzer {
    tap: Arc<SpectrumTap>,
    fft: FftPlan,
    window: Vec<f32>,
    time: Vec<f32>,
    spectrum: Vec<Complex32>,
    spectrum1: Vec<Complex32>,
    smoothed_db: [f32; BINS],
    last_analysis_ms: i64,
    last_written: usize,
}

impl SpectrumAnalyzer {
    /// Allocates analysis state for one tap.
    #[must_use]
    pub fn new(tap: Arc<SpectrumTap>) -> Self {
        let window = (0..FFT_SIZE)
            .map(|index| {
                0.5 * (1.0
                    - (2.0 * std::f32::consts::PI * index as f32 / (FFT_SIZE - 1) as f32).cos())
            })
            .collect();
        Self {
            tap,
            fft: FftPlan::new(FFT_SIZE),
            window,
            time: vec![0.0; FFT_SIZE],
            spectrum: vec![Complex32::default(); FFT_SIZE],
            spectrum1: vec![Complex32::default(); FFT_SIZE],
            smoothed_db: [-100.0; BINS],
            last_analysis_ms: 0,
            last_written: 0,
        }
    }

    /// The tap that this analyzer reads.
    #[must_use]
    pub const fn tap(&self) -> &Arc<SpectrumTap> {
        &self.tap
    }

    /// Returns log-spaced dB bins, and analyzes at most every 30 ms.
    #[must_use]
    pub fn bins(&mut self, now_ms: i64) -> [f32; BINS] {
        if now_ms - self.last_analysis_ms >= 30 {
            self.last_analysis_ms = now_ms;
            self.analyze();
        }
        self.smoothed_db
    }

    fn analyze(&mut self) {
        let written = self.tap.samples_written();
        let fresh = written != self.last_written;
        self.last_written = written;
        let independent = self.tap.independent.load(Ordering::Relaxed);
        if fresh {
            self.transform_ring(written, false);
            if independent {
                self.transform_ring(written, true);
            }
        } else {
            // A block that gets no audio has nothing to show, and the old window is not its output.
            self.spectrum.fill(Complex32::default());
            self.spectrum1.fill(Complex32::default());
        }
        let amplitude_scale = 4.0 / FFT_SIZE as f32;
        let bin_hz = self.tap.sample_rate / FFT_SIZE as f64;
        let log_ratio = (20_000.0_f64 / 20.0).ln();
        for index in 0..BINS {
            let f0 = 20.0 * (log_ratio * index as f64 / BINS as f64).exp();
            let f1 = 20.0 * (log_ratio * (index + 1) as f64 / BINS as f64).exp();
            let b0 = (f0 / bin_hz).ceil() as usize;
            let b1 = (f1 / bin_hz).floor() as usize;
            let magnitude = if b1 >= b0 {
                (b0..=b1)
                    .map(|bin| self.magnitude_at(bin, independent))
                    .fold(0.0_f32, f32::max)
            } else {
                let exact = (f0 + f1) * 0.5 / bin_hz;
                let lower = exact as usize;
                let fraction = (exact - lower as f64) as f32;
                self.magnitude_at(lower, independent) * (1.0 - fraction)
                    + self.magnitude_at(lower + 1, independent) * fraction
            };
            let amplitude = magnitude * amplitude_scale;
            let db = if amplitude > 1.0e-9 {
                (20.0 * amplitude.log10()).clamp(-100.0, 0.0)
            } else {
                -100.0
            };
            let coefficient = if db > self.smoothed_db[index] {
                0.7
            } else {
                0.18
            };
            self.smoothed_db[index] += (db - self.smoothed_db[index]) * coefficient;
        }
    }

    fn transform_ring(&mut self, written: usize, second: bool) {
        let start = written.wrapping_sub(FFT_SIZE);
        let source = if second {
            &self.tap.ring1
        } else {
            &self.tap.ring
        };
        for index in 0..FFT_SIZE {
            let slot = &source[start.wrapping_add(index) & (RING_SIZE - 1)];
            let sample = f32::from_bits(slot.load(Ordering::Relaxed));
            // A non-finite sample in one bin would fill the whole spectrum.
            let sample = if sample.is_finite() { sample } else { 0.0 };
            self.time[index] = sample * self.window[index];
        }
        let time = std::mem::take(&mut self.time);
        if second {
            self.fft.forward(&time, &mut self.spectrum1);
        } else {
            self.fft.forward(&time, &mut self.spectrum);
        }
        self.time = time;
    }

    fn magnitude_at(&self, bin: usize, independent: bool) -> f32 {
        let bin = bin.min(FFT_SIZE / 2 - 1);
        let magnitude = |data: &[Complex32]| {
            let value = data[bin];
            (value.re * value.re + value.im * value.im).sqrt()
        };
        let first = magnitude(&self.spectrum);
        if independent {
            first.max(magnitude(&self.spectrum1))
        } else {
            first
        }
    }
}

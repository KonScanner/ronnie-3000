use crate::{ChainError, PlanarBuffer};

/// Global three-band tone controls, with 5.0 as the flat value for each band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneControls {
    /// Whether the tone stack is active.
    pub enabled: bool,
    /// Low-shelf control from 0 to 10.
    pub bass: f32,
    /// Mid-bell control from 0 to 10.
    pub mid: f32,
    /// High-shelf control from 0 to 10.
    pub treble: f32,
}

impl Default for ToneControls {
    fn default() -> Self {
        Self {
            enabled: true,
            bass: 5.0,
            mid: 5.0,
            treble: 5.0,
        }
    }
}

/// Duration of the coefficient ramp after a tone control change.
const RAMP_SECONDS: f64 = 0.020;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coefficients {
    /// Moves one linear step toward `target`, landing on it when `steps` is one.
    fn toward(self, target: Self, steps: u32) -> Self {
        let share = 1.0 / steps as f32;
        let step = |from: f32, to: f32| from + (to - from) * share;
        Self {
            b0: step(self.b0, target.b0),
            b1: step(self.b1, target.b1),
            b2: step(self.b2, target.b2),
            a1: step(self.a1, target.a1),
            a2: step(self.a2, target.a2),
        }
    }

    fn as_array(self) -> [f32; 5] {
        [self.b0, self.b1, self.b2, self.a1, self.a2]
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    current: Coefficients,
    target: Coefficients,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl Biquad {
    fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }

    /// Sets new target coefficients and keeps the filter history.
    fn retarget(&mut self, target: Coefficients, ramp: bool) {
        self.target = target;
        if !ramp {
            self.current = target;
        }
    }

    fn advance(&mut self, steps: u32) {
        self.current = self.current.toward(self.target, steps);
    }

    fn process(&mut self, input: f32, channel: usize) -> f32 {
        let c = self.current;
        let output = c.b0 * input + self.z1[channel];
        self.z1[channel] = c.b1 * input - c.a1 * output + self.z2[channel];
        self.z2[channel] = c.b2 * input - c.a2 * output;
        output
    }
}

/// Prepared global bass, mid, and treble filters with independent stereo state.
///
/// A control change keeps the filter history and ramps the coefficients
/// linearly to their new values over 20 ms. Each band is a stable biquad at
/// both ends of the ramp, and the stable `(a1, a2)` region is a convex
/// triangle, so every point on the ramp is stable too.
pub struct ToneStack {
    sample_rate: f64,
    controls: ToneControls,
    bass: Biquad,
    mid: Biquad,
    treble: Biquad,
    was_enabled: bool,
    ramp_len: u32,
    ramp_left: u32,
}

impl ToneStack {
    /// Prepares flat filters at the host sample rate.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64) -> Result<Self, ChainError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(ChainError::InvalidLayout("tone sample rate is invalid"));
        }
        let mut stack = Self {
            sample_rate,
            controls: ToneControls::default(),
            bass: Biquad::default(),
            mid: Biquad::default(),
            treble: Biquad::default(),
            was_enabled: false,
            ramp_len: ((sample_rate * RAMP_SECONDS).round() as u32).max(1),
            ramp_left: 0,
        };
        stack.update_coefficients(false);
        Ok(stack)
    }

    /// Updates tone controls and resets filter history when power returns.
    ///
    /// While the stack runs, a change ramps the coefficients and keeps the
    /// filter history. While it is off, a change applies at once.
    pub fn set_controls(&mut self, controls: ToneControls) {
        let running = controls.enabled && self.was_enabled;
        if controls.enabled && !self.was_enabled {
            self.reset();
        }
        if controls != self.controls {
            self.controls = controls;
            self.update_coefficients(running);
        }
        self.was_enabled = controls.enabled;
    }

    /// Target normalized `[b0, b1, b2, a1, a2]` coefficients for bass, mid, and treble.
    ///
    /// The filters reach these values at the end of the 20 ms ramp.
    #[must_use]
    pub fn coefficients(&self) -> [[f32; 5]; 3] {
        [
            self.bass.target.as_array(),
            self.mid.target.as_array(),
            self.treble.target.as_array(),
        ]
    }

    /// Applies bass, mid, then treble in place without allocating.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for an invalid channel view.
    pub fn process(&mut self, buffer: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
        if !self.controls.enabled {
            return Ok(());
        }
        let channels = buffer.channels();
        if self.ramp_left == 0 {
            for channel in 0..channels {
                for sample in buffer.channel_mut(channel)? {
                    *sample = self.filter(*sample, channel);
                }
            }
            return Ok(());
        }
        let frames = buffer.frames();
        let data = buffer.as_mut_slice();
        for frame in 0..frames {
            if self.ramp_left > 0 {
                let steps = self.ramp_left;
                self.bass.advance(steps);
                self.mid.advance(steps);
                self.treble.advance(steps);
                self.ramp_left -= 1;
            }
            for channel in 0..channels {
                let sample = &mut data[channel * frames + frame];
                *sample = self.filter(*sample, channel);
            }
        }
        Ok(())
    }

    fn filter(&mut self, sample: f32, channel: usize) -> f32 {
        let sample = self.bass.process(sample, channel);
        let sample = self.mid.process(sample, channel);
        self.treble.process(sample, channel)
    }

    pub(crate) fn reset(&mut self) {
        self.bass.reset();
        self.mid.reset();
        self.treble.reset();
    }

    fn update_coefficients(&mut self, ramp: bool) {
        let bass_db = 4.0 * (self.controls.bass - 5.0);
        let mid_db = 3.0 * (self.controls.mid - 5.0);
        let treble_db = 2.0 * (self.controls.treble - 5.0);
        self.bass.retarget(
            shelf(self.sample_rate, 150.0, 0.707, bass_db, ShelfKind::Low),
            ramp,
        );
        self.mid.retarget(
            peak(
                self.sample_rate,
                425.0,
                if mid_db < 0.0 { 1.5 } else { 0.7 },
                mid_db,
            ),
            ramp,
        );
        self.treble.retarget(
            shelf(self.sample_rate, 1800.0, 0.707, treble_db, ShelfKind::High),
            ramp,
        );
        self.ramp_left = if ramp { self.ramp_len } else { 0 };
    }
}

#[derive(Clone, Copy)]
enum ShelfKind {
    Low,
    High,
}

fn shelf(sample_rate: f64, frequency: f32, q: f32, gain_db: f32, kind: ShelfKind) -> Coefficients {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let frequency = frequency.min((sample_rate * 0.49) as f32);
    let omega = 2.0 * std::f32::consts::PI * frequency / sample_rate as f32;
    let cosine = omega.cos();
    let beta = omega.sin() * a.sqrt() / q;
    let minus = a - 1.0;
    let plus = a + 1.0;
    let coefficients = match kind {
        ShelfKind::Low => [
            a * (plus - minus * cosine + beta),
            2.0 * a * (minus - plus * cosine),
            a * (plus - minus * cosine - beta),
            plus + minus * cosine + beta,
            -2.0 * (minus + plus * cosine),
            plus + minus * cosine - beta,
        ],
        ShelfKind::High => [
            a * (plus + minus * cosine + beta),
            -2.0 * a * (minus + plus * cosine),
            a * (plus + minus * cosine - beta),
            plus - minus * cosine + beta,
            2.0 * (minus - plus * cosine),
            plus - minus * cosine - beta,
        ],
    };
    biquad(coefficients)
}

fn peak(sample_rate: f64, frequency: f32, q: f32, gain_db: f32) -> Coefficients {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let frequency = frequency.min((sample_rate * 0.49) as f32);
    let omega = 2.0 * std::f32::consts::PI * frequency / sample_rate as f32;
    let alpha = omega.sin() / (2.0 * q);
    let c2 = -2.0 * omega.cos();
    biquad([
        1.0 + alpha * a,
        c2,
        1.0 - alpha * a,
        1.0 + alpha / a,
        c2,
        1.0 - alpha / a,
    ])
}

fn biquad(coefficients: [f32; 6]) -> Coefficients {
    let a0 = coefficients[3];
    Coefficients {
        b0: coefficients[0] / a0,
        b1: coefficients[1] / a0,
        b2: coefficients[2] / a0,
        a1: coefficients[4] / a0,
        a2: coefficients[5] / a0,
    }
}

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

use crate::block::gain::LinearGain;

pub(crate) const DECK_WOBBLE_MAX_MS: f32 = 1.2;
pub(crate) const DECK_FADE_SECONDS: f64 = 0.025;

/// Maps a normalized crossover control to 32.5–520 Hz logarithmically.
#[must_use]
pub fn deck_crossover_hz(normalized: f32) -> f32 {
    32.5 * 16.0_f32.powf(normalized.clamp(0.0, 1.0))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Lr4 {
    sample_rate: f64,
    cutoff: f32,
    g: f32,
    r2: f32,
    h: f32,
    states: [[f32; 2]; 4],
}

impl Lr4 {
    pub(crate) fn new(sample_rate: f64, cutoff: f32) -> Self {
        let mut filter = Self {
            sample_rate,
            cutoff,
            g: 0.0,
            r2: std::f32::consts::SQRT_2,
            h: 0.0,
            states: [[0.0; 2]; 4],
        };
        filter.update();
        filter
    }

    pub(crate) fn reset(&mut self) {
        self.states = [[0.0; 2]; 4];
    }

    pub(crate) fn set_cutoff(&mut self, cutoff: f32) {
        self.cutoff = cutoff;
        self.update();
    }

    fn update(&mut self) {
        self.g = (std::f64::consts::PI * f64::from(self.cutoff) / self.sample_rate).tan() as f32;
        self.h = 1.0 / (1.0 + self.r2 * self.g + self.g * self.g);
    }

    pub(crate) fn process_split(&mut self, channel: usize, input: f32) -> (f32, f32) {
        let s1 = self.states[0][channel];
        let s2 = self.states[1][channel];
        let yh = (input - (self.r2 + self.g) * s1 - s2) * self.h;
        let yb = self.g * yh + s1;
        self.states[0][channel] = self.g * yh + yb;
        let yl = self.g * yb + s2;
        self.states[1][channel] = self.g * yb + yl;

        let s3 = self.states[2][channel];
        let s4 = self.states[3][channel];
        let yh2 = (yl - (self.r2 + self.g) * s3 - s4) * self.h;
        let yb2 = self.g * yh2 + s3;
        self.states[2][channel] = self.g * yh2 + yb2;
        let yl2 = self.g * yb2 + s4;
        self.states[3][channel] = self.g * yb2 + yl2;
        (yl2, yl - self.r2 * yb + yh - yl2)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DelayLine {
    buffer: Vec<f32>,
    write_pos: usize,
    read_pos: usize,
    delay_int: i32,
    delay_frac: f32,
}

impl DelayLine {
    pub(crate) fn new(maximum_delay: usize) -> Self {
        Self {
            buffer: vec![0.0; (maximum_delay + 2).max(4)],
            write_pos: 0,
            read_pos: 0,
            delay_int: 0,
            delay_frac: 0.0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        self.read_pos = 0;
        self.delay_int = 0;
        self.delay_frac = 0.0;
    }

    pub(crate) fn push(&mut self, sample: f32) {
        self.buffer[self.write_pos] = sample;
        self.write_pos = (self.write_pos + self.buffer.len() - 1) % self.buffer.len();
    }

    pub(crate) fn pop(&mut self, delay: f32) -> f32 {
        let upper = (self.buffer.len() - 2) as f32;
        let delay = delay.clamp(0.0, upper);
        self.delay_int = delay.floor() as i32;
        self.delay_frac = delay - self.delay_int as f32;
        if self.delay_frac < 2.0 && self.delay_int >= 1 {
            self.delay_frac += 1.0;
            self.delay_int -= 1;
        }
        let index1 = (self.read_pos + self.delay_int as usize) % self.buffer.len();
        let index2 = (index1 + 1) % self.buffer.len();
        let index3 = (index2 + 1) % self.buffer.len();
        let index4 = (index3 + 1) % self.buffer.len();
        let value1 = self.buffer[index1];
        let value2 = self.buffer[index2];
        let value3 = self.buffer[index3];
        let value4 = self.buffer[index4];
        let d1 = self.delay_frac - 1.0;
        let d2 = self.delay_frac - 2.0;
        let d3 = self.delay_frac - 3.0;
        let c1 = -d1 * d2 * d3 / 6.0;
        let c2 = d2 * d3 * 0.5;
        let c3 = -d1 * d3 * 0.5;
        let c4 = d1 * d2 / 6.0;
        let output = value1 * c1 + self.delay_frac * (value2 * c2 + value3 * c3 + value4 * c4);
        self.read_pos = (self.read_pos + self.buffer.len() - 1) % self.buffer.len();
        output
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Allpass {
    a: f32,
    z: f32,
}

impl Allpass {
    fn process(&mut self, input: f32) -> f32 {
        let v = input - self.a * self.z;
        let output = self.a * v + self.z;
        self.z = v;
        output
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DeckDiffuser {
    stages: [Allpass; 6],
}

impl DeckDiffuser {
    pub(crate) fn new(sample_rate: f64) -> Self {
        let mut diffuser = Self::default();
        for (index, stage) in diffuser.stages.iter_mut().enumerate() {
            let corner = 300.0 * (20.0_f64).powf(index as f64 / 5.0);
            let safe = corner.min(sample_rate * 0.49);
            let tangent = (std::f64::consts::PI * safe / sample_rate).tan();
            stage.a = ((tangent - 1.0) / (tangent + 1.0)) as f32;
        }
        diffuser
    }

    pub(crate) fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.z = 0.0;
        }
    }

    pub(crate) fn process(&mut self, mut input: f32) -> f32 {
        for stage in &mut self.stages {
            input = stage.process(input);
        }
        input
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DeckWobble {
    seed: i64,
    state1: f32,
    state2: f32,
    coeff: f32,
    norm: f32,
}

static NEXT_SEED: AtomicI64 = AtomicI64::new(1);

fn default_seed() -> i64 {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as i64);
    time ^ NEXT_SEED.fetch_add(0x5deece66d, Ordering::Relaxed)
}

impl DeckWobble {
    pub(crate) fn new(sample_rate: f64) -> Self {
        let coeff = 1.0 - (-std::f32::consts::TAU * 0.3 / sample_rate as f32).exp();
        let k = f64::from(coeff);
        let a = 1.0 - k;
        let gain_sq =
            k * k * k * k * (1.0 + a * a) / ((1.0 - a * a) * (1.0 - a * a) * (1.0 - a * a));
        let sigma = (gain_sq / 3.0).sqrt();
        Self {
            seed: default_seed(),
            state1: 0.0,
            state2: 0.0,
            coeff,
            norm: (1.0 / (3.0 * sigma)) as f32,
        }
    }

    pub(crate) fn set_seed(&mut self, seed: i64) {
        self.seed = seed;
    }

    pub(crate) fn reset(&mut self) {
        self.state1 = 0.0;
        self.state2 = 0.0;
    }

    pub(crate) fn next(&mut self) -> f32 {
        let random = self.next_float();
        self.state1 += self.coeff * (random * 2.0 - 1.0 - self.state1);
        self.state2 += self.coeff * (self.state1 - self.state2);
        (self.state2 * self.norm).clamp(-1.0, 1.0)
    }

    fn next_int(&mut self) -> i32 {
        self.seed = ((self.seed as u64)
            .wrapping_mul(0x5deece66d)
            .wrapping_add(11)
            & 0xffffffffffff) as i64;
        (self.seed >> 16) as i32
    }

    fn next_float(&mut self) -> f32 {
        let result = (self.next_int() as u32) as f32 / (u32::MAX as f32 + 1.0);
        result.min(1.0 - f32::EPSILON)
    }
}

#[derive(Debug)]
pub(crate) struct DeckCorrelation {
    lr: f32,
    ll: f32,
    rr: f32,
    output: AtomicU32,
    sample_rate: f64,
}

impl DeckCorrelation {
    pub(crate) fn new(sample_rate: f64) -> Self {
        let mut meter = Self {
            lr: 0.0,
            ll: 0.0,
            rr: 0.0,
            output: AtomicU32::new(1.0_f32.to_bits()),
            sample_rate,
        };
        meter.reset();
        meter
    }

    pub(crate) fn reset(&mut self) {
        self.lr = 0.0;
        self.ll = 0.0;
        self.rr = 0.0;
        self.output.store(1.0_f32.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn update(&mut self, sum_lr: f32, sum_ll: f32, sum_rr: f32, samples: usize) {
        let norm = 1.0 / samples as f32;
        let coeff = 1.0 - (-(samples as f32) / (0.3 * self.sample_rate as f32)).exp();
        self.lr += coeff * (sum_lr * norm - self.lr);
        self.ll += coeff * (sum_ll * norm - self.ll);
        self.rr += coeff * (sum_rr * norm - self.rr);
        let energy = self.ll * self.rr;
        let value = if energy > 1.0e-10 * 1.0e-10 {
            (self.lr / energy.sqrt()).clamp(-1.0, 1.0)
        } else {
            1.0
        };
        self.output.store(value.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn value(&self) -> f32 {
        f32::from_bits(self.output.load(Ordering::Relaxed))
    }
}

pub(crate) fn new_smoother(sample_rate: f64) -> LinearGain {
    LinearGain::new(sample_rate, DECK_FADE_SECONDS, 0.0)
}

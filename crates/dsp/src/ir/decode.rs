use std::sync::Arc;

use ronie_core::AudioDocument;

use super::{CompiledIr, IrEngineKind, IrKind};
use crate::ChainError;

const BASE_RATE: f64 = 48_000.0;
const MAX_SECONDS: f64 = 10.0;
const SHORT_MAX_SAMPLES: usize = 48_000;
const NONUNIFORM_HEAD: usize = 8192;

/// Compiles decoded audio into an immutable base-rate IR.
///
/// # Errors
///
/// Returns preparation errors for empty, inconsistent, or invalid audio.
pub fn compile_ir(
    document: &AudioDocument,
    gear: Option<&str>,
) -> Result<Arc<CompiledIr>, ChainError> {
    if document.channels.is_empty()
        || document.channels[0].is_empty()
        || !document.sample_rate.is_finite()
        || document.sample_rate <= 0.0
    {
        return Err(ChainError::Preparation("IR audio must be nonempty".into()));
    }
    let source_rate = document.sample_rate;
    let source_frames = document.channels[0].len();
    if document
        .channels
        .iter()
        .any(|channel| channel.len() != source_frames)
    {
        return Err(ChainError::Preparation("IR channel lengths differ".into()));
    }
    let maximum_samples = (MAX_SECONDS * source_rate) as usize;
    let capped_frames = source_frames.min(maximum_samples);
    if capped_frames == 0 {
        return Err(ChainError::Preparation("IR time cap is empty".into()));
    }
    let normalization_gain = normalization_gain(document, capped_frames, source_rate);
    let upper_bound = (capped_frames as f64 * BASE_RATE / source_rate).round() as usize;
    let engine = if upper_bound > SHORT_MAX_SAMPLES {
        IrEngineKind::NonUniform {
            head: NONUNIFORM_HEAD,
        }
    } else {
        IrEngineKind::Uniform
    };

    let capped: Vec<Vec<f32>> = document
        .channels
        .iter()
        .take(2)
        .map(|channel| channel[..capped_frames].to_vec())
        .collect();
    let (begin, end) = trim_range(&capped);
    let mut channels = Vec::with_capacity(capped.len());
    for source in &capped {
        channels.push(resample(&source[begin..source.len() - end], source_rate));
    }
    let base_len = channels[0].len();
    let long_by_gear = match gear {
        Some(value) if value.eq_ignore_ascii_case("cab") => Some(false),
        Some(value) if value.eq_ignore_ascii_case("space") => Some(true),
        _ => None,
    };
    let kind = if long_by_gear.unwrap_or(base_len > SHORT_MAX_SAMPLES) {
        IrKind::Reverb
    } else {
        IrKind::Cab
    };
    Ok(Arc::new(CompiledIr {
        channels,
        base_len,
        kind,
        engine,
        normalization_gain,
    }))
}

fn normalization_gain(document: &AudioDocument, frames: usize, source_rate: f64) -> f32 {
    let mut sum_squares = 0.0_f64;
    for channel in &document.channels {
        for sample in &channel[..frames] {
            sum_squares += f64::from(*sample) * f64::from(*sample);
        }
    }
    let l2 = (sum_squares * source_rate / BASE_RATE).sqrt();
    if !l2.is_finite() || l2 <= 0.0 {
        return 1.0;
    }
    (1.0 / l2).clamp(10.0_f64.powf(-48.0 / 20.0), 1.0) as f32
}

fn trim_range(channels: &[Vec<f32>]) -> (usize, usize) {
    let threshold = 10.0_f32.powf(-80.0 / 20.0);
    let frames = channels[0].len();
    let mut begin = frames;
    let mut end = frames;
    for channel in channels {
        begin = begin.min(
            channel
                .iter()
                .position(|sample| sample.abs() >= threshold)
                .unwrap_or(frames),
        );
        end = end.min(
            channel
                .iter()
                .rev()
                .position(|sample| sample.abs() >= threshold)
                .unwrap_or(frames),
        );
    }
    if begin == frames {
        return (0, frames - 1);
    }
    (begin, end)
}

fn resample(samples: &[f32], source_rate: f64) -> Vec<f32> {
    if source_rate == BASE_RATE {
        return samples.to_vec();
    }
    let ratio = source_rate / BASE_RATE;
    let final_size = ((samples.len() as f64 / ratio).round().max(1.0)) as usize;
    let filtered_input = if ratio > 1.0001 {
        let needed = (final_size as f64 * ratio).round() as usize + 3;
        let mut filter = LowPass::new(ratio);
        (0..needed)
            .map(|index| filter.process(samples.get(index).copied().unwrap_or(0.0)))
            .collect::<Vec<_>>()
    } else {
        samples.to_vec()
    };
    let mut output = Vec::with_capacity(final_size);
    let mut position = 0_usize;
    let mut offset = 0.0_f64;
    for _ in 0..final_size {
        let current = filtered_input.get(position).copied().unwrap_or(0.0);
        let next = filtered_input.get(position + 1).copied().unwrap_or(0.0);
        let alpha = offset as f32;
        output.push(current + alpha * (next - current));
        offset += ratio;
        while offset >= 1.0 {
            position += 1;
            offset -= 1.0;
        }
    }
    if ratio < 0.9999 {
        let mut filter = LowPass::new(ratio);
        for sample in &mut output {
            *sample = filter.process(*sample);
        }
    }
    let rate_gain = (source_rate / BASE_RATE) as f32;
    for sample in &mut output {
        *sample *= rate_gain;
    }
    output
}

struct LowPass {
    coefficients: [f64; 6],
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl LowPass {
    fn new(ratio: f64) -> Self {
        let proportional_rate = if ratio > 1.0 {
            0.5 / ratio
        } else {
            0.5 * ratio
        };
        let n = 1.0 / (std::f64::consts::PI * proportional_rate.max(0.001)).tan();
        let n_squared = n * n;
        let c1 = 1.0 / (1.0 + std::f64::consts::SQRT_2 * n + n_squared);
        Self {
            coefficients: [
                c1,
                c1 * 2.0,
                c1,
                1.0,
                c1 * 2.0 * (1.0 - n_squared),
                c1 * (1.0 - std::f64::consts::SQRT_2 * n + n_squared),
            ],
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let input = f64::from(input);
        let mut output = self.coefficients[0] * input
            + self.coefficients[1] * self.x1
            + self.coefficients[2] * self.x2
            - self.coefficients[4] * self.y1
            - self.coefficients[5] * self.y2;
        if !(-1.0e-8..=1.0e-8).contains(&output) {
        } else {
            output = 0.0;
        }
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output as f32
    }
}

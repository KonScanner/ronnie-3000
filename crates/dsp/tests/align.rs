//! Stereo-chain Align delay, deck, lifecycle, and correlation tests.

use ronie_dsp::{
    PlanarBuffer,
    image::{Align, AlignParams},
};

const FS: f64 = 48_000.0;
const BLOCK: usize = 512;

struct StereoResult {
    left: Vec<f32>,
    right: Vec<f32>,
}

#[derive(Clone, Copy)]
struct Settings {
    offset_norm: f32,
    wobble_norm: f32,
    crossover_norm: f32,
    wobble_on: bool,
    crossover_on: bool,
    diffuse_on: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            offset_norm: 0.5,
            wobble_norm: 0.25,
            crossover_norm: 0.5,
            wobble_on: false,
            crossover_on: false,
            diffuse_on: false,
        }
    }
}

fn noise(frames: usize, mut seed: u32, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            amplitude * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0)
        })
        .collect()
}

fn sine(frames: usize, hz: f64, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|index| amplitude * (2.0 * std::f64::consts::PI * hz * index as f64 / FS).sin() as f32)
        .collect()
}

fn run_align(input: &[f32], settings: Settings) -> StereoResult {
    let mut align = Align::new(FS, BLOCK).expect("Align preparation");
    align.set_wobble_seed(12_345);
    let params = AlignParams::from_normalized(
        settings.offset_norm,
        settings.wobble_norm,
        settings.crossover_norm,
        settings.wobble_on,
        settings.crossover_on,
        settings.diffuse_on,
    );
    let mut left = Vec::with_capacity(input.len());
    let mut right = Vec::with_capacity(input.len());
    for source in input.chunks(BLOCK) {
        let mut data = Vec::with_capacity(source.len() * 2);
        data.extend_from_slice(source);
        data.extend_from_slice(source);
        align.set_target(params, true);
        align
            .process(&mut PlanarBuffer::new(&mut data, 2, source.len()).expect("stereo buffer"))
            .expect("Align processing");
        let (l, r) = data.split_at(source.len());
        left.extend_from_slice(l);
        right.extend_from_slice(r);
    }
    StereoResult { left, right }
}

fn rms(samples: &[f32]) -> f64 {
    (samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt()
}

fn null_depth_db(output: &StereoResult, settled: usize) -> f64 {
    let difference = output.left[settled..]
        .iter()
        .zip(&output.right[settled..])
        .map(|(left, right)| left - right)
        .collect::<Vec<_>>();
    20.0 * (rms(&difference) / rms(&output.left[settled..])).log10()
}

#[test]
fn delays_the_chosen_side_by_the_dialed_ms() {
    let input = noise(96 * BLOCK, 7, 0.5);
    let output = run_align(
        &input,
        Settings {
            offset_norm: 1.0,
            ..Default::default()
        },
    );
    assert_eq!(output.left, input);
    for index in 48_000..input.len() {
        assert!((output.right[index] - input[index - 1152]).abs() < 1.0e-4);
    }
}

#[test]
fn disabled_wobble_is_time_invariant() {
    let cycle = sine(128, FS / 128.0, 0.5);
    let input = (0..288 * BLOCK)
        .map(|index| cycle[index % cycle.len()])
        .collect::<Vec<_>>();
    let output = run_align(
        &input,
        Settings {
            offset_norm: 0.75,
            wobble_norm: 1.0,
            ..Default::default()
        },
    );
    for channel in [output.left, output.right] {
        for index in 96_000..channel.len() {
            assert!((channel[index] - channel[index - 128]).abs() < 1.0e-6);
        }
    }
}

#[test]
fn crossover_keeps_lows_out_of_the_deck() {
    let input = sine(64 * BLOCK, 30.0, 0.5);
    let on = run_align(
        &input,
        Settings {
            offset_norm: 1.0,
            crossover_on: true,
            ..Default::default()
        },
    );
    let off = run_align(
        &input,
        Settings {
            offset_norm: 1.0,
            ..Default::default()
        },
    );
    assert!(null_depth_db(&on, 24_000) < -35.0);
    assert!(null_depth_db(&off, 24_000) > -10.0);
}

#[test]
fn diffusion_has_no_level_trim() {
    let input = noise(96 * BLOCK, 31_337, 0.5);
    let output = run_align(
        &input,
        Settings {
            offset_norm: 0.75,
            diffuse_on: true,
            ..Default::default()
        },
    );
    let gain_db = 20.0 * (rms(&output.right[48_000..]) / rms(&input[48_000..])).log10();
    assert!(gain_db.abs() < 0.25);
}

#[test]
fn side_swap_with_the_full_deck_never_clicks() {
    let input = sine(240 * BLOCK, 440.0, 0.5);
    let mut align = Align::new(FS, BLOCK).expect("Align preparation");
    let mut left = Vec::new();
    let mut right = Vec::new();
    for (block, source) in input.chunks(BLOCK).enumerate() {
        let mut data = Vec::with_capacity(BLOCK * 2);
        data.extend_from_slice(source);
        data.extend_from_slice(source);
        let offset_norm = if (block / 40) % 2 == 0 { 1.0 } else { 0.0 };
        align.set_target(
            AlignParams::from_normalized(offset_norm, 1.0, 0.5, true, true, true),
            true,
        );
        align
            .process(&mut PlanarBuffer::new(&mut data, 2, BLOCK).expect("stereo buffer"))
            .expect("Align processing");
        let (l, r) = data.split_at(BLOCK);
        left.extend_from_slice(l);
        right.extend_from_slice(r);
    }
    for channel in [left, right] {
        for index in 4801..channel.len() {
            assert!((channel[index] - channel[index - 1]).abs() < 0.1);
        }
    }
}

#[test]
fn idle_bypass_and_correlation_are_exact() {
    let input = noise(64 * BLOCK, 42, 0.4);
    let mut align = Align::new(FS, BLOCK).expect("Align preparation");
    assert!(!align.is_running());
    assert_eq!(align.correlation(), 1.0);
    let mut data = Vec::with_capacity(input.len() * 2);
    data.extend_from_slice(&input);
    data.extend_from_slice(&input);
    let original = data.clone();
    align.set_target(AlignParams::default(), true);
    align
        .process(&mut PlanarBuffer::new(&mut data, 2, input.len()).expect("stereo buffer"))
        .expect("Align processing");
    assert!((align.correlation() - 1.0).abs() < 1.0e-6);
    align.set_target(AlignParams::default(), false);
    for _ in 0..4 {
        align
            .process(&mut PlanarBuffer::new(&mut data, 2, input.len()).expect("stereo buffer"))
            .expect("Align processing");
    }
    assert!(!align.is_running());
    assert_eq!(data, original);
    assert_eq!(align.correlation(), 1.0);
}

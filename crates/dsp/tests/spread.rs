//! Mono-to-stereo Spread deck and transition tests.

use ronie_dsp::{
    PlanarBuffer,
    image::{Spread, SpreadParams, deck_crossover_hz},
};

const FS: f64 = 48_000.0;
const BLOCK: usize = 512;

#[derive(Clone)]
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
            wobble_norm: 0.0,
            crossover_norm: 0.5,
            wobble_on: true,
            crossover_on: true,
            diffuse_on: true,
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

fn run_spread(input: &[f32], settings: Settings) -> StereoResult {
    let mut spread = Spread::new(FS, BLOCK).expect("Spread preparation");
    spread.set_wobble_seed(12_345);
    let params = SpreadParams::from_normalized(
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
        spread.set_target(params, true);
        spread
            .process(&mut PlanarBuffer::new(&mut data, 2, source.len()).expect("stereo buffer"))
            .expect("Spread processing");
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

fn goertzel_power(samples: &[f32], hz: f64) -> f64 {
    let omega = 2.0 * std::f64::consts::PI * hz / FS;
    let coefficient = 2.0 * omega.cos();
    let mut previous = 0.0;
    let mut current = 0.0;
    for sample in samples {
        let next = f64::from(*sample) + coefficient * current - previous;
        previous = current;
        current = next;
    }
    let real = current - previous * omega.cos();
    let imaginary = previous * omega.sin();
    (real * real + imaginary * imaginary) / (samples.len() * samples.len()) as f64
}

#[test]
fn center_detent_is_exactly_dual_mono() {
    let input = noise(64 * BLOCK, 1234, 0.5);
    let output = run_spread(
        &input,
        Settings {
            wobble_norm: 0.5,
            ..Default::default()
        },
    );
    assert_eq!(output.left, output.right);
}

#[test]
fn opposite_signs_mirror_exactly() {
    let input = noise(64 * BLOCK, 99, 0.5);
    let plus = run_spread(
        &input,
        Settings {
            offset_norm: 0.75,
            ..Default::default()
        },
    );
    let minus = run_spread(
        &input,
        Settings {
            offset_norm: 0.25,
            ..Default::default()
        },
    );
    assert_eq!(plus.left, minus.right);
    assert_eq!(plus.right, minus.left);
}

#[test]
fn low_band_crossover_and_cutoff_follow_the_controls() {
    let input = sine(64 * BLOCK, 30.0, 0.5);
    let output = run_spread(
        &input,
        Settings {
            offset_norm: 0.75,
            ..Default::default()
        },
    );
    assert!(null_depth_db(&output, 24_000) < -35.0);
    let output = run_spread(
        &input,
        Settings {
            offset_norm: 0.75,
            crossover_on: false,
            ..Default::default()
        },
    );
    assert!(null_depth_db(&output, 24_000) > -10.0);

    let input = sine(64 * BLOCK, 250.0, 0.5);
    let mono = run_spread(
        &input,
        Settings {
            offset_norm: 0.75,
            crossover_norm: 1.0,
            ..Default::default()
        },
    );
    let doubled = run_spread(
        &input,
        Settings {
            offset_norm: 0.75,
            crossover_norm: 0.0,
            ..Default::default()
        },
    );
    assert!(null_depth_db(&mono, 24_000) < -15.0);
    assert!(null_depth_db(&doubled, 24_000) > -10.0);
}

#[test]
fn diffuse_off_produces_a_pure_delay_with_precedence_trim() {
    let input = noise(64 * BLOCK, 4242, 0.5);
    let output = run_spread(
        &input,
        Settings {
            offset_norm: 0.75,
            crossover_on: false,
            diffuse_on: false,
            ..Default::default()
        },
    );
    assert_eq!(output.left, input);
    let lag_gain = 10.0_f32.powf(1.5 / 20.0);
    for index in 4800..input.len() {
        assert!((output.right[index] - input[index - 576] * lag_gain).abs() < 1.0e-4);
    }
}

#[test]
fn zero_wobble_and_disabled_wobble_are_time_invariant() {
    let cycle = sine(128, FS / 128.0, 0.5);
    let input = (0..288 * BLOCK)
        .map(|index| cycle[index % cycle.len()])
        .collect::<Vec<_>>();
    for settings in [
        Settings {
            offset_norm: 0.75,
            ..Default::default()
        },
        Settings {
            offset_norm: 0.75,
            wobble_norm: 1.0,
            wobble_on: false,
            ..Default::default()
        },
    ] {
        let output = run_spread(&input, settings);
        for channel in [output.left, output.right] {
            for index in 96_000..channel.len() {
                assert!((channel[index] - channel[index - 128]).abs() < 1.0e-6);
            }
        }
    }
}

#[test]
fn crossover_detents_and_offset_range_match_the_parameter_map() {
    assert!((deck_crossover_hz(0.0) - 32.5).abs() < 1.0e-3);
    assert!((deck_crossover_hz(0.5) - 130.0).abs() < 1.0e-3);
    assert!((deck_crossover_hz(1.0) - 520.0).abs() < 1.0e-3);
    let params = SpreadParams::from_normalized(1.0, 0.0, 0.5, true, true, true);
    assert_eq!(params.offset_ms, 24.0);
    let params = SpreadParams::from_normalized(0.0, 0.0, 0.5, true, true, true);
    assert_eq!(params.offset_ms, -24.0);
    let params = SpreadParams::from_normalized(0.5, 1.0, 0.5, false, true, true);
    assert_eq!(params.offset_ms, 0.0);
    assert_eq!(params.wobble_depth, 0.0);
}

#[test]
fn hard_sign_flips_never_click() {
    let input = sine(120 * BLOCK, 440.0, 0.5);
    let mut spread = Spread::new(FS, BLOCK).expect("Spread preparation");
    let mut left = Vec::new();
    let mut right = Vec::new();
    for (block, source) in input.chunks(BLOCK).enumerate() {
        let mut data = Vec::with_capacity(BLOCK * 2);
        data.extend_from_slice(source);
        data.extend_from_slice(source);
        let offset_norm = if (block / 20) % 2 == 0 { 1.0 } else { 0.0 };
        spread.set_target(
            SpreadParams::from_normalized(offset_norm, 0.5, 0.5, true, true, true),
            true,
        );
        spread
            .process(&mut PlanarBuffer::new(&mut data, 2, BLOCK).expect("stereo buffer"))
            .expect("Spread processing");
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
fn wobble_adds_no_broadband_fizz_and_seed_is_repeatable() {
    let input = sine(288 * BLOCK, 500.0, 0.5);
    let settings = Settings {
        offset_norm: 0.5 + 20.0 / 48.0,
        wobble_norm: 1.0,
        ..Default::default()
    };
    let first = run_spread(&input, settings);
    let second = run_spread(&input, settings);
    assert_eq!(first.left, second.left);
    assert_eq!(first.right, second.right);
    let settled = &first.right[48_000..];
    let carrier = goertzel_power(settled, 500.0).max(1.0e-30).log10() * 10.0;
    for frequency in [2917.0, 4111.0, 6373.0, 9241.0, 13_687.0] {
        let noise = goertzel_power(settled, frequency).max(1.0e-30).log10() * 10.0;
        assert!(noise - carrier < -90.0, "fizz at {frequency} Hz");
    }
}

#[test]
fn correlation_and_idle_lifecycle_are_reported() {
    let input = noise(64 * BLOCK, 42, 0.4);
    let mut spread = Spread::new(FS, BLOCK).expect("Spread preparation");
    assert!(!spread.is_running());
    assert_eq!(spread.correlation(), 1.0);
    let mut data = Vec::with_capacity(input.len() * 2);
    data.extend_from_slice(&input);
    data.extend_from_slice(&input);
    spread.set_target(SpreadParams::default(), true);
    spread
        .process(&mut PlanarBuffer::new(&mut data, 2, input.len()).expect("stereo buffer"))
        .expect("Spread processing");
    assert!((spread.correlation() - 1.0).abs() < 1.0e-6);
    spread.set_target(SpreadParams::default(), false);
    for _ in 0..4 {
        spread
            .process(&mut PlanarBuffer::new(&mut data, 2, input.len()).expect("stereo buffer"))
            .expect("Spread processing");
    }
    assert!(!spread.is_running());
    assert_eq!(spread.correlation(), 1.0);
}

#[test]
#[ignore = "requires the private full-chain Reference runner"]
fn private_spread_output_matches_reference() {
    use std::collections::BTreeMap;

    use ronie_core::InputMode;
    use ronie_dsp::{
        PrepareSpec,
        image::ImageControls,
        output::{OutputControls, OutputStage},
    };
    use ronie_oracle::{ChainOracle, ChainScenario, OracleError};

    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let frames = 96 * BLOCK;
    let input = noise(frames, 4242, 0.5);
    let mut stereo_input = input.clone();
    stereo_input.extend_from_slice(&input);
    let scenario = ChainScenario {
        schema_version: 1,
        spec: PrepareSpec {
            host_sample_rate: FS,
            max_host_frames: BLOCK,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        callback_frames: vec![BLOCK; 96],
        input: stereo_input.clone(),
        parameters: BTreeMap::from([
            ("gateEnabled".to_owned(), 0.0),
            ("spreadEnabled".to_owned(), 1.0),
            ("spreadOffset".to_owned(), 0.75),
            ("spreadWobble".to_owned(), 0.0),
            ("spreadWobbleEnabled".to_owned(), 0.0),
            ("spreadCrossover".to_owned(), 0.5),
            ("spreadCrossoverEnabled".to_owned(), 0.0),
            ("spreadDiffuseEnabled".to_owned(), 0.0),
        ]),
        state_changes: Vec::new(),
        snapshot: serde_json::json!({"name": "Spread comparison"}),
    };
    let reference = oracle.render(&scenario).expect("Reference Spread render");
    let controls = OutputControls {
        image: ImageControls {
            stereo_chains: false,
            spread_enabled: true,
            spread: SpreadParams::from_normalized(0.75, 0.0, 0.5, false, false, false),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut stage = OutputStage::new(FS, BLOCK).expect("output preparation");
    let mut left_output = Vec::with_capacity(frames);
    let mut right_output = Vec::with_capacity(frames);
    let mut frame_offset = 0;
    for _ in 0..96 {
        let mut data = Vec::with_capacity(BLOCK * 2);
        data.extend_from_slice(&stereo_input[frame_offset..frame_offset + BLOCK]);
        data.extend_from_slice(&stereo_input[frames + frame_offset..frames + frame_offset + BLOCK]);
        stage
            .process(
                &mut PlanarBuffer::new(&mut data, 2, BLOCK).expect("stereo buffer"),
                controls,
                |_, _| {},
            )
            .expect("output processing");
        let (left, right) = data.split_at(BLOCK);
        left_output.extend_from_slice(left);
        right_output.extend_from_slice(right);
        frame_offset += BLOCK;
    }
    let output = [left_output, right_output].concat();
    for channel in 0..2 {
        let start = channel * frames + 48_000;
        for (index, (reference, output)) in reference
            .samples
            .iter()
            .zip(&output)
            .enumerate()
            .skip(start)
            .take(1152)
        {
            assert!(
                (reference - output).abs() < 1.0e-4,
                "sample {index}: Reference={reference} Rust={output}",
            );
        }
    }
}

//! Complete block pipeline, EQ, normalization, fade, meter, and spectrum tests.

use std::collections::BTreeMap;

use ronie_core::{AudioDocument, BlockParams, EqBand, EqBandType, EqParams, InputMode, decode_wav};
use ronie_dsp::{
    PlanarBuffer, PrepareSpec,
    block::{BlockControls, BlockModel, PreparedBlock, prepare_block},
    ir::compile_ir,
};
use ronie_oracle::{ChainOracle, ChainScenario, OracleError, reference_paths};
use serde_json::json;

fn spec() -> PrepareSpec {
    PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    }
}

fn identity_ir() -> BlockModel {
    BlockModel::Ir(
        compile_ir(
            &AudioDocument {
                sample_rate: 48_000.0,
                channels: vec![vec![1.0]],
            },
            None,
        )
        .expect("identity IR"),
    )
}

fn block(params: BlockParams) -> PreparedBlock {
    prepare_block(identity_ir(), spec(), params).expect("block preparation")
}

fn process(block: &mut PreparedBlock, input: &mut [f32], controls: BlockControls) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    for chunk in input.chunks_mut(64) {
        let frames = chunk.len();
        let mut buffer = PlanarBuffer::new(chunk, 1, frames).expect("input");
        block.process(&mut buffer, controls).expect("block process");
        output.extend_from_slice(chunk);
    }
    output
}

#[test]
fn output_gain_is_post_mix_and_cab_pad_is_wet_only() {
    let input = vec![0.25; 4096];
    let mut dry = block(BlockParams::default());
    let mut wet = block(BlockParams::default());
    let mut reduced = block(BlockParams::default());
    let dry_controls = BlockControls {
        mix: 0.0,
        ..Default::default()
    };
    let mut wet_controls = dry_controls;
    wet_controls.mix = 1.0;
    let mut reduced_controls = wet_controls;
    reduced_controls.mix = 0.5;
    reduced_controls.output_gain = 0.0;

    let dry_output = process(&mut dry, &mut input.clone(), dry_controls);
    let wet_output = process(&mut wet, &mut input.clone(), wet_controls);
    let actual = process(&mut reduced, &mut input.clone(), reduced_controls);
    let gain_ratio = 10.0_f32.powf(-24.0 / 20.0);
    let pad = 10.0_f32.powf(-18.0 / 20.0);
    for index in 3000..4096 {
        let expected = (dry_output[index] * 0.5 + wet_output[index] * 0.5) * gain_ratio;
        assert!((actual[index] - expected).abs() < 1.0e-6);
        assert!((wet_output[index] - input[index] * pad).abs() < 1.0e-6);
    }
}

#[test]
fn post_eq_changes_only_wet_audio_and_flat_eq_is_transparent() {
    let input: Vec<f32> = (0..4096)
        .map(|index| ((index as f32 + 1.0) * 0.17).sin() * 0.5)
        .collect();
    let mut dry = block(BlockParams::default());
    let mut wet = block(BlockParams::default());
    let mut dry_flat = block(BlockParams::default());
    let mut wet_flat = block(BlockParams::default());
    let shaped = EqParams {
        enabled: true,
        pre: false,
        bands: vec![EqBand {
            kind: EqBandType::Bell,
            freq_hz: 1500.0,
            gain_db: 12.0,
            q: 1.2,
        }],
    };
    let dry_controls = BlockControls {
        mix: 0.0,
        eq: shaped.clone().into(),
        ..Default::default()
    };
    let dry_output = process(&mut dry, &mut input.clone(), dry_controls);
    let wet_controls = BlockControls {
        eq: shaped.into(),
        ..Default::default()
    };
    let wet_output = process(&mut wet, &mut input.clone(), wet_controls);
    let dry_flat_controls = BlockControls {
        mix: 0.0,
        ..Default::default()
    };
    let dry_flat_output = process(&mut dry_flat, &mut input.clone(), dry_flat_controls);
    let wet_flat_output = process(&mut wet_flat, &mut input.clone(), BlockControls::default());

    assert_eq!(dry_output[3000..], dry_flat_output[3000..]);
    assert!(
        wet_output
            .iter()
            .zip(&wet_flat_output)
            .any(|(left, right)| (left - right).abs() > 1.0e-3)
    );
}

#[test]
fn eq_coefficients_follow_rbj_and_every_band_role() {
    let mut engine =
        ronie_dsp::block::eq::EqEngine::new(48_000.0, &ronie_dsp::block::eq::EqControls::default());
    let mut controls = ronie_dsp::block::eq::EqControls {
        enabled: true,
        pre: false,
        bands: [
            EqBand {
                kind: EqBandType::Lowcut,
                freq_hz: 100.0,
                gain_db: 0.0,
                q: 0.71,
            },
            EqBand {
                kind: EqBandType::Lowshelf,
                freq_hz: 100.0,
                gain_db: 6.0,
                q: 0.71,
            },
            EqBand {
                kind: EqBandType::Bell,
                freq_hz: 1500.0,
                gain_db: 12.0,
                q: 1.2,
            },
            EqBand::default(),
            EqBand::default(),
            EqBand {
                kind: EqBandType::Highshelf,
                freq_hz: 8000.0,
                gain_db: -6.0,
                q: 0.71,
            },
        ],
    };
    engine.update_controls(&controls);
    let coefficients = engine.coefficients(2).expect("bell coefficients");
    let a = 10.0_f64.powf(12.0 / 40.0);
    let omega = 2.0 * std::f64::consts::PI * 1500.0 / 48_000.0;
    let alpha = omega.sin() / (2.0 * 1.2);
    let norm = 1.0 / (1.0 + alpha / a);
    let expected = [
        (1.0 + alpha * a) * norm,
        -2.0 * omega.cos() * norm,
        (1.0 - alpha * a) * norm,
        -2.0 * omega.cos() * norm,
        (1.0 - alpha / a) * norm,
    ];
    for (actual, expected) in coefficients.iter().zip(expected) {
        assert!((f64::from(*actual) - expected).abs() < 1.0e-6);
    }
    assert!(engine.is_active());
    let mut audio = [1.0, 0.0, 0.0, 0.0];
    engine.process(&mut audio, 1);
    assert_ne!(audio[0], 1.0);
    for (index, kind) in [
        (0, EqBandType::Lowcut),
        (1, EqBandType::Lowshelf),
        (2, EqBandType::Bell),
        (4, EqBandType::Highshelf),
        (5, EqBandType::Highcut),
    ] {
        controls.bands[index].kind = kind;
        controls.bands[index].gain_db = 6.0;
        engine.update_controls(&controls);
        let mut audio = [1.0, 0.0, 0.0, 0.0];
        engine.process(&mut audio, 1);
        assert_ne!(audio[0], 1.0, "shape {kind:?} did not process");
    }
}

#[test]
fn normalization_uses_loudness_or_calibrated_handoff() {
    let mut document = ronie_oracle::synthetic::synthetic_a2_document(3).expect("document");
    document.metadata.loudness = Some(-30.0);
    document.metadata.input_level_dbu = Some(-12.0);
    document.metadata.output_level_dbu = Some(-6.0);
    let compiled = ronie_dsp::compile_model(&document).expect("compiled model");
    let mut block = prepare_block(BlockModel::Nam(compiled), spec(), BlockParams::default())
        .expect("NAM block");
    let mut controls = BlockControls {
        target_loudness_db: -18.0,
        input_calibration_dbu: 0.0,
        ..Default::default()
    };
    let mut input = [0.0; 64];
    block
        .process(
            &mut PlanarBuffer::new(&mut input, 1, 64).expect("input"),
            controls,
        )
        .expect("normalization");

    let expected = 10.0_f32.powf(12.0 / 20.0);
    assert!((block.normalization_gain() - expected).abs() < 1.0e-6);
    controls.calibrated_handoff = true;
    block
        .process(
            &mut PlanarBuffer::new(&mut input, 1, 64).expect("input"),
            controls,
        )
        .expect("calibration");
    let expected = 10.0_f32.powf(-6.0 / 20.0);
    assert!((block.normalization_gain() - expected).abs() < 1.0e-6);
}

#[test]
fn long_ir_defaults_to_half_wet_and_ir_normalization_is_retained() {
    let mut kernel = vec![0.25];
    kernel.resize(48_001, 0.0);
    kernel[48_000] = 0.25;
    let compiled = compile_ir(
        &AudioDocument {
            sample_rate: 48_000.0,
            channels: vec![kernel],
        },
        None,
    )
    .expect("long IR");
    let expected_gain = compiled.normalization_gain;
    let block = prepare_block(BlockModel::Ir(compiled), spec(), BlockParams::default())
        .expect("long IR block");

    assert_eq!(block.controls().mix, 0.5);
    assert_eq!(block.normalization_gain(), expected_gain);
}

#[test]
fn bypass_and_swap_fades_have_distinct_final_shapes() {
    let mut bypass = block(BlockParams::default());
    let input = vec![0.25; 4096];
    let mut controls = BlockControls {
        output_gain: 0.0,
        ..Default::default()
    };
    let _ = process(&mut bypass, &mut input[..2048].to_vec(), controls);
    controls.enabled = false;
    controls.swap_pending = true;
    controls.swap_mute_wet = false;
    let bypass_output = process(&mut bypass, &mut input[2048..].to_vec(), controls);
    assert_eq!(&bypass_output[1200..], &input[2048 + 1200..]);
    assert!(bypass.swap_fade_done());

    let mut swap = block(BlockParams::default());
    let controls = BlockControls {
        swap_pending: true,
        swap_mute_wet: true,
        ..Default::default()
    };
    let swap_output = process(&mut swap, &mut input.clone(), controls);
    assert!(
        swap_output[1200..]
            .iter()
            .all(|sample| sample.abs() < 1.0e-6)
    );
    assert!(swap.swap_fade_done());
}

#[test]
fn meters_and_spectrum_follow_their_taps_and_enable_state() {
    let mut block = block(BlockParams::default());
    let input: Vec<f32> = (0..4096)
        .map(|index| (std::f32::consts::TAU * 1000.0 * index as f32 / 48_000.0).sin() * 0.25)
        .collect();
    let controls = BlockControls {
        spectrum_enabled: true,
        ..Default::default()
    };
    let _ = process(&mut block, &mut input.clone(), controls);
    let spectrum = block.spectrum(100);
    let peak = spectrum
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .expect("spectrum peak")
        .0;
    let peak_hz = 20.0 * (1000.0_f64.ln() * (peak as f64 + 0.5) / 64.0).exp();
    assert!((peak_hz - 1000.0).abs() < 100.0, "peak at {peak_hz} Hz");
    assert!(block.input_meter_db() > -60.0);
    assert!(block.output_meter_db() > -60.0);

    let mut disabled = self::block(BlockParams::default());
    let _ = process(&mut disabled, &mut input.clone(), BlockControls::default());
    assert!(disabled.spectrum(100).iter().all(|bin| *bin == -100.0));
}

fn private_block_scenario(
    path: &std::path::Path,
    mix: f32,
    output_gain: f32,
    shaped_eq: bool,
) -> ChainScenario {
    let frames = 4096;
    let mut noise = 0x4242_u32;
    let mut input: Vec<f32> = (0..frames * 2)
        .map(|_| {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            (noise as f32 / u32::MAX as f32 - 0.5) * 0.5
        })
        .collect();
    let left_input = input[..frames].to_vec();
    input[frames..].copy_from_slice(&left_input);
    ChainScenario {
        schema_version: 1,
        spec: PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        callback_frames: vec![64; 64],
        input,
        parameters: BTreeMap::from([("gateEnabled".to_owned(), 0.0)]),
        state_changes: Vec::new(),
        snapshot: json!({
            "probe": "full-ir",
            "ir_path": path,
            "gear": "cab",
            "block": {
                "mix": mix,
                "inputGain": 0.5,
                "outputGain": output_gain,
                "enabled": true,
                "normalize": true,
                "pre": false,
                "eq": {
                    "index": 2,
                    "type": "bell",
                    "freqHz": 1500.0,
                    "gainDb": if shaped_eq { 12.0 } else { 0.0 },
                    "q": 1.2
                }
            }
        }),
    }
}

#[test]
#[ignore = "requires private Reference source, fixtures, and chain runner"]
fn private_output_gain_and_eq_match_full_processor() {
    let Some((root, _)) = reference_paths().expect("Reference configuration") else {
        return;
    };
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let path = root.join("test/files/cab-ir-test.wav");
    let document =
        decode_wav(&std::fs::read(&path).expect("official WAV")).expect("official decode");
    for (mix, output_gain, shaped_eq) in [(0.5, 0.0, false)] {
        let scenario = private_block_scenario(&path, mix, output_gain, shaped_eq);
        let reference = oracle.render(&scenario).expect("Reference block");
        let params = BlockParams {
            mix: f64::from(mix),
            output_gain: f64::from(output_gain),
            ..Default::default()
        };
        let mut block = prepare_block(
            BlockModel::Ir(compile_ir(&document, None).expect("official compile")),
            spec(),
            params,
        )
        .expect("block preparation");
        let controls = block.controls();
        let mut actual = process(&mut block, &mut scenario.input[..4096].to_vec(), controls);
        let n = (std::f32::consts::PI * 5.0 / 48_000.0).tan();
        let mut previous_input = 0.0_f32;
        let mut previous_output = 0.0_f32;
        for sample in &mut actual {
            let input = *sample;
            previous_output = (input - previous_input + (1.0 - n) * previous_output) / (1.0 + n);
            previous_input = input;
            *sample = previous_output;
        }
        let peak = reference.samples[3000..4096]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        let (error_index, (left, right)) = actual
            .iter()
            .skip(3000)
            .zip(&reference.samples[3000..])
            .enumerate()
            .max_by(|(_, (left_a, right_a)), (_, (left_b, right_b))| {
                (*left_a - *right_a)
                    .abs()
                    .total_cmp(&(*left_b - *right_b).abs())
            })
            .expect("block samples");
        let max_error = (left - right).abs();
        assert!(
            max_error <= peak * 3.0e-4 + 5.0e-5,
            "mix {mix}, EQ {shaped_eq}: {max_error} at {error_index} (peak {peak})"
        );
    }
}

#[test]
#[ignore = "requires private Reference source, fixtures, and chain runner"]
fn private_eq_engine_matches_reference() {
    let Some((root, _)) = reference_paths().expect("Reference configuration") else {
        return;
    };
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let path = root.join("test/files/cab-ir-test.wav");
    let mut scenario = private_block_scenario(&path, 1.0, 0.5, true);
    scenario.snapshot["probe"] = json!("eq");
    scenario.callback_frames = vec![4096];
    let reference = oracle.render(&scenario).expect("Reference EQ");
    let controls = ronie_dsp::block::eq::EqControls::from(&EqParams {
        enabled: true,
        pre: false,
        bands: vec![EqBand {
            kind: EqBandType::Bell,
            freq_hz: 1500.0,
            gain_db: 12.0,
            q: 1.2,
        }],
    });
    let mut engine = ronie_dsp::block::eq::EqEngine::new(48_000.0, &controls);
    let mut actual = scenario.input;
    engine.process(&mut actual, 2);

    let max_error = actual
        .iter()
        .zip(&reference.samples)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f32, f32::max);
    assert!(max_error < 1.0e-6, "direct EQ error {max_error}");
}

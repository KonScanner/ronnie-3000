//! IR decode, classification, streaming, and private differential tests.

use std::{collections::BTreeMap, sync::Arc};

use ronie_core::{AudioDocument, InputMode, ModelBytes, decode_wav};
use ronie_dsp::{
    PlanarBuffer,
    ir::{CompiledIr, IrEngineKind, IrKind, PreparedIr, compile_ir},
    oversampling::ChainOversampler,
};
use ronie_oracle::{ChainOracle, ChainScenario, OracleError, reference_paths};
use serde_json::json;

fn document(kernel: &[f32], sample_rate: f64) -> AudioDocument {
    AudioDocument {
        sample_rate,
        channels: vec![kernel.to_vec()],
    }
}

fn render_mono(
    prepared: &mut PreparedIr,
    factor: u8,
    input: &[f32],
    callback_frames: &[usize],
) -> Vec<f32> {
    let mut outer =
        ChainOversampler::new(factor, callback_frames.iter().max().copied().unwrap_or(1))
            .expect("outer oversampling");
    let mut island =
        ChainOversampler::new(factor, callback_frames.iter().max().copied().unwrap_or(1))
            .expect("IR island");
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    for frames in callback_frames {
        let end = (offset + frames).min(input.len());
        if offset == end {
            break;
        }
        let mut input_chunk = input[offset..end].to_vec();
        let input_buffer =
            PlanarBuffer::new(&mut input_chunk, 1, end - offset).expect("input chunk");
        let mut output_chunk = vec![0.0; end - offset];
        let mut output_buffer =
            PlanarBuffer::new(&mut output_chunk, 1, end - offset).expect("output chunk");
        outer
            .process(&input_buffer, &mut output_buffer, |oversampled| {
                prepared.process_planar(oversampled, &mut island)
            })
            .expect("IR process");
        output.extend_from_slice(&output_chunk);
        offset = end;
    }
    output
}

#[test]
fn audible_classification_uses_trimmed_length_then_unambiguous_gear() {
    let short = compile_ir(&document(&[1.0], 48_000.0), None).expect("short IR");
    let long = compile_ir(&document(&long_kernel(), 48_000.0), None).expect("long IR");

    assert_eq!(short.kind, IrKind::Cab);
    assert_eq!(short.default_mix(), 1.0);
    assert!((short.output_pad() - 10.0_f32.powf(-0.9)).abs() < 1.0e-7);
    assert_eq!(long.kind, IrKind::Reverb);
    assert_eq!(long.default_mix(), 0.5);
    assert_eq!(long.output_pad(), 1.0);
    assert_eq!(
        compile_ir(&document(&[1.0], 48_000.0), Some("space"))
            .expect("gear")
            .kind,
        IrKind::Reverb
    );
    assert_eq!(long_with_gear("cab").kind, IrKind::Cab);
    assert_eq!(long_with_gear("pedal").kind, IrKind::Reverb);
}

fn long_kernel() -> Vec<f32> {
    let mut kernel = vec![1.0];
    kernel.resize(48_001, 0.0);
    kernel[48_000] = 0.5;
    kernel
}

fn long_with_gear(gear: &str) -> Arc<CompiledIr> {
    compile_ir(&document(&long_kernel(), 48_000.0), Some(gear)).expect("gear IR")
}

#[test]
fn decode_trims_caps_normalizes_and_resamples_like_the_loader() {
    let trimmed = compile_ir(
        &AudioDocument {
            sample_rate: 48_000.0,
            channels: vec![vec![0.0, 0.25, 0.0, 0.0], vec![0.0; 4]],
        },
        None,
    )
    .expect("trimmed IR");
    assert_eq!(trimmed.base_len, 1);
    assert_eq!(trimmed.channels[0], [0.25]);
    assert_eq!(trimmed.channels[1], [0.0]);
    assert_eq!(trimmed.normalization_gain, 1.0);

    let mut capped_input = vec![0.0; 480_010];
    capped_input[0] = 1.0;
    capped_input[480_009] = 1.0;
    let capped = compile_ir(&document(&capped_input, 48_000.0), None).expect("capped IR");
    assert_eq!(capped.base_len, 1);
    assert_eq!(capped.channels[0], [1.0]);

    let resampled = compile_ir(&document(&vec![1.0; 192], 96_000.0), None).expect("resampled IR");
    assert_eq!(resampled.base_len, 96);
    assert_eq!(resampled.channels[0].len(), 96);
    assert!((resampled.channels[0][80] - 2.0).abs() < 1.0e-4);
    let expected_gain = 1.0 / (192.0_f64 * 2.0).sqrt() as f32;
    assert!((resampled.normalization_gain - expected_gain).abs() < 1.0e-7);
}

#[test]
fn engine_choice_uses_pre_trim_length_not_audible_classification() {
    let mut padded = vec![1.0];
    padded.resize(48_001, 0.0);
    padded.push(0.5);
    let compiled = compile_ir(&document(&padded, 48_000.0), None).expect("padded IR");

    assert!(matches!(
        compiled.engine,
        IrEngineKind::NonUniform { head: 8192 }
    ));
    assert_eq!(compiled.kind, IrKind::Reverb);
    assert!(compiled.base_len > 48_000);
}

#[test]
fn uniform_impulse_stream_matches_the_time_domain_kernel() {
    let kernel = [1.0, 0.5, -0.25, 0.1];
    let compiled = compile_ir(&document(&kernel, 48_000.0), None).expect("compiled IR");
    let input: Vec<f32> = std::iter::once(1.0)
        .chain(std::iter::repeat_n(0.0, 511))
        .collect();
    let expected: Vec<f32> = kernel
        .into_iter()
        .chain(std::iter::repeat_n(0.0, 508))
        .collect();

    for callbacks in [
        vec![512],
        vec![1; 512],
        vec![31; 17],
        vec![255, 257],
        vec![256; 2],
    ] {
        let mut prepared = PreparedIr::new(compiled.clone(), false);
        prepared.prepare(256).expect("preparation");
        let actual = render_mono(&mut prepared, 1, &input, &callbacks);
        let max_error = actual
            .iter()
            .zip(&expected)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_error < 1.0e-5, "callbacks {callbacks:?}: {max_error}");
    }
}

#[test]
fn nonuniform_head_and_tail_match_across_callback_shapes() {
    let mut kernel = vec![0.0; 50_000];
    kernel[0] = 1.0;
    kernel[8192] = 0.5;
    kernel[49_999] = -0.125;
    let compiled = compile_ir(&document(&kernel, 48_000.0), None).expect("compiled IR");
    assert!(matches!(
        compiled.engine,
        IrEngineKind::NonUniform { head: 8192 }
    ));
    let input: Vec<f32> = std::iter::once(1.0)
        .chain(std::iter::repeat_n(0.0, 49_999))
        .collect();

    for callbacks in [
        vec![50_000],
        vec![31; 1613],
        vec![64; 782],
        vec![255; 197],
        vec![256; 196],
        vec![257; 195],
        vec![4096; 13],
    ] {
        let mut prepared = PreparedIr::new(compiled.clone(), false);
        prepared.prepare(256).expect("preparation");
        let actual = render_mono(&mut prepared, 1, &input, &callbacks);
        assert_eq!(actual.len(), input.len());
        let max_error = actual
            .iter()
            .zip(&kernel)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_error < 2.0e-4, "callbacks {callbacks:?}: {max_error}");
    }
    assert_eq!(compiled.tail_seconds(), 50_000.0 / 48_000.0);
}

#[test]
fn nonuniform_tail_waits_for_its_large_partition() {
    let mut kernel = vec![0.0; 50_000];
    kernel[0] = 1.0;
    kernel[8192] = 0.5;
    let input: Vec<f32> = std::iter::once(1.0)
        .chain(std::iter::repeat_n(0.0, 8999))
        .collect();
    let compiled = compile_ir(&document(&kernel, 48_000.0), None).expect("compiled IR");
    assert!(matches!(
        compiled.engine,
        IrEngineKind::NonUniform { head: 8192 }
    ));
    let mut prepared = PreparedIr::new(compiled, false);
    prepared.prepare(256).expect("preparation");

    let actual = render_mono(&mut prepared, 1, &input, &[9000]);

    assert!((actual[0] - 1.0).abs() < 1.0e-5);
    assert!((actual[8192] - 0.5).abs() < 1.0e-5);
    assert!(actual[1..8192].iter().all(|sample| sample.abs() < 1.0e-5));
}

#[test]
fn nonuniform_noise_matches_naive_time_convolution() {
    let kernel: Vec<f32> = (0..9000)
        .map(|index| ((index as f32 + 1.0) * 0.013).sin() * 0.01)
        .collect();
    let input: Vec<f32> = (0..4096)
        .map(|index| ((index as f32 + 1.0) * 0.17).sin() * 0.5)
        .collect();
    let expected: Vec<f32> = (0..input.len())
        .map(|output| {
            (0..=output.min(kernel.len() - 1))
                .map(|tap| input[output - tap] * kernel[tap])
                .sum()
        })
        .collect();
    let compiled = compile_ir(&document(&kernel, 48_000.0), None).expect("compiled IR");
    let mut prepared = PreparedIr::new(compiled, false);
    prepared.prepare(256).expect("preparation");

    let actual = render_mono(&mut prepared, 1, &input, &[4096]);

    let max_error = actual
        .iter()
        .zip(&expected)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f32, f32::max);
    assert!(max_error < 2.0e-4, "max error {max_error}");
}

#[test]
fn nonfinite_and_denormal_inputs_use_explicit_numeric_paths() {
    let compiled = compile_ir(&document(&[1.0, 0.5], 48_000.0), None).expect("compiled IR");
    let mut prepared = PreparedIr::new(compiled, false);
    prepared.prepare(8).expect("preparation");
    let mut input = [
        f32::NAN,
        f32::INFINITY,
        f32::from_bits(1),
        -f32::from_bits(1),
    ];
    let mut buffer = PlanarBuffer::new(&mut input, 1, 4).expect("input");
    let mut oversampler = ChainOversampler::new(1, 4).expect("oversampling");

    prepared
        .process_planar(&mut buffer, &mut oversampler)
        .expect("numeric processing");

    assert!(buffer.as_slice().iter().any(|sample| sample.is_nan()));
}

fn official_scenario(
    path: &std::path::Path,
    nonuniform: bool,
    factor: u8,
    callbacks: Vec<usize>,
) -> ChainScenario {
    let frames: usize = callbacks.iter().sum();
    let mut noise = 0x1234_5678_u32;
    let mut input = Vec::with_capacity(frames * 2);
    for channel in 0..2 {
        for _ in 0..frames {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            input.push((noise as f32 / u32::MAX as f32 - 0.5) * 0.5);
        }
        if channel == 1 {
            break;
        }
    }
    ChainScenario {
        schema_version: 1,
        spec: ronie_dsp::PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 256,
            oversampling: factor,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        callback_frames: callbacks,
        input,
        parameters: BTreeMap::new(),
        state_changes: Vec::new(),
        snapshot: json!({"probe": "ir", "ir_path": path, "nonuniform": nonuniform, "stereo": true}),
    }
}

fn render_stereo(
    prepared: &mut PreparedIr,
    factor: u8,
    input: &[f32],
    callback_frames: &[usize],
) -> Vec<f32> {
    let frames = input.len() / 2;
    let mut outer =
        ChainOversampler::new(factor, callback_frames.iter().max().copied().unwrap_or(1))
            .expect("outer oversampling");
    let mut island =
        ChainOversampler::new(factor, callback_frames.iter().max().copied().unwrap_or(1))
            .expect("IR island");
    let mut output = vec![0.0; input.len()];
    let mut offset = 0;
    for callback in callback_frames {
        let end = (offset + callback).min(frames);
        if offset == end {
            break;
        }
        let count = end - offset;
        let mut input_chunk = Vec::with_capacity(count * 2);
        for channel in 0..2 {
            input_chunk
                .extend_from_slice(&input[channel * frames + offset..channel * frames + end]);
        }
        let input_buffer = PlanarBuffer::new(&mut input_chunk, 2, count).expect("input chunk");
        let mut output_chunk = vec![0.0; count * 2];
        let mut output_buffer =
            PlanarBuffer::new(&mut output_chunk, 2, count).expect("output chunk");
        outer
            .process(&input_buffer, &mut output_buffer, |oversampled| {
                prepared.process_planar(oversampled, &mut island)
            })
            .expect("IR process");
        for channel in 0..2 {
            output[channel * frames + offset..channel * frames + end]
                .copy_from_slice(&output_chunk[channel * count..(channel + 1) * count]);
        }
        offset = end;
    }
    output
}

#[test]
#[ignore = "requires private Reference source, fixtures, and chain runner"]
fn private_official_ir_and_islands_match_reference() {
    let Some((root, _)) = reference_paths().expect("Reference configuration") else {
        return;
    };
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    for (name, nonuniform) in [
        ("cab-ir-test.wav", false),
        ("reverb-ir-stereo-test.wav", true),
    ] {
        let path = root.join("test/files").join(name);
        let bytes = ModelBytes::from(std::fs::read(&path).expect("official WAV"));
        let document = decode_wav(bytes.as_ref()).expect("official decode");
        let compiled = compile_ir(&document, None).expect("official compile");
        for factor in [1, 2, 4, 8] {
            let callbacks = if factor == 1 {
                vec![31, 33, 64]
            } else {
                vec![32, 64, 32]
            };
            let scenario = official_scenario(&path, nonuniform, factor, callbacks.clone());
            let reference = oracle.render(&scenario).expect("Reference IR");
            assert_eq!(
                compiled.base_len as u64,
                reference.diagnostic_state["irSize"]
                    .as_u64()
                    .expect("IR size")
            );
            let mut prepared = PreparedIr::new(compiled.clone(), true);
            prepared.prepare(256).expect("preparation");
            let actual = render_stereo(&mut prepared, factor, &scenario.input, &callbacks);
            let peak = reference
                .samples
                .iter()
                .map(|sample| sample.abs())
                .fold(0.0_f32, f32::max);
            let (error_index, (left, right)) = actual
                .iter()
                .zip(&reference.samples)
                .enumerate()
                .max_by(|(_, (left_a, right_a)), (_, (left_b, right_b))| {
                    (*left_a - *right_a)
                        .abs()
                        .total_cmp(&(*left_b - *right_b).abs())
                })
                .expect("IR samples");
            let max_error = (left - right).abs();
            assert!(
                max_error <= peak * 2.0e-4 + 1.0e-6,
                "{name} factor {factor}: {max_error} at {error_index} (peak {peak})"
            );
        }
    }
}

#[test]
#[ignore = "requires private Reference source, fixtures, and chain runner"]
fn private_resampled_reverb_kernel_matches_reference() {
    let Some((root, _)) = reference_paths().expect("Reference configuration") else {
        return;
    };
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let path = root.join("test/files/reverb-ir-stereo-test.wav");
    let document =
        decode_wav(&std::fs::read(&path).expect("official WAV")).expect("official decode");
    let compiled = compile_ir(&document, None).expect("official compile");
    let mut scenario = official_scenario(&path, true, 1, vec![4096]);
    scenario.input.fill(0.0);
    scenario.input[0] = 1.0;
    scenario.input[4096] = 1.0;
    let reference = oracle.render(&scenario).expect("Reference kernel");
    let mut prepared = PreparedIr::new(compiled, true);
    prepared.prepare(256).expect("preparation");
    let actual = render_stereo(&mut prepared, 1, &scenario.input, &[4096]);

    let (error_index, (left, right)) = actual
        .iter()
        .zip(&reference.samples)
        .enumerate()
        .max_by(|(_, (left_a, right_a)), (_, (left_b, right_b))| {
            (*left_a - *right_a)
                .abs()
                .total_cmp(&(*left_b - *right_b).abs())
        })
        .expect("kernel samples");
    let max_error = (left - right).abs();
    assert!(
        max_error < 2.0e-4,
        "resampled kernel error {max_error} at {error_index}"
    );
}

#[test]
#[ignore = "requires private Reference source, fixtures, and chain runner"]
fn private_mono_fallback_and_large_callbacks_match_reference() {
    let Some((root, _)) = reference_paths().expect("Reference configuration") else {
        return;
    };
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let path = root.join("test/files/reverb-ir-stereo-test.wav");
    let document =
        decode_wav(&std::fs::read(&path).expect("official WAV")).expect("official decode");
    let compiled = compile_ir(&document, None).expect("official compile");
    let mut scenario = official_scenario(&path, true, 1, vec![4096]);
    let left_input = scenario.input[..4096].to_vec();
    scenario.input[4096..].copy_from_slice(&left_input);
    let reference = oracle.render(&scenario).expect("Reference IR");

    let mut stereo = PreparedIr::new(compiled.clone(), true);
    stereo.prepare(256).expect("stereo preparation");
    let stereo_output = render_stereo(&mut stereo, 1, &scenario.input, &callbacks(&scenario));
    assert!(
        stereo_output[..4096]
            .iter()
            .zip(&stereo_output[4096..])
            .any(|(l, r)| (l - r).abs() > 1.0e-5)
    );
    let peak = reference
        .samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let error = stereo_output
        .iter()
        .zip(&reference.samples)
        .map(|(l, r)| (l - r).abs())
        .fold(0.0_f32, f32::max);
    assert!(error <= peak * 2.0e-4 + 1.0e-6);

    let mut mono = PreparedIr::new(compiled, false);
    mono.prepare(256).expect("mono preparation");
    let mono_output = render_stereo(&mut mono, 1, &scenario.input, &callbacks(&scenario));
    for (left, right) in mono_output[..4096].iter().zip(&mono_output[4096..]) {
        assert!((left - right).abs() < 1.0e-6);
    }
}

fn callbacks(scenario: &ChainScenario) -> Vec<usize> {
    scenario.callback_frames.clone()
}

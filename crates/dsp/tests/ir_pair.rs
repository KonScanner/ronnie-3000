//! A cab block that blends two IRs inside one block.

use std::sync::Arc;

use ronie_core::{AudioDocument, BlockParams, ChainBlock, InputMode, ModelId, Tone};
use ronie_dsp::{
    LatencyReport, PlanarBuffer, PrepareSpec,
    block::{BlockControls, BlockModel, PreparedBlock, prepare_block},
    ir::{CompiledIr, compile_ir},
    lane::LaneSnapshot,
    prepare::PreparedChainChange,
};
use serde_json::json;

fn spec(oversampling: u8) -> PrepareSpec {
    PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    }
}

fn ir(channels: Vec<Vec<f32>>, sample_rate: f64) -> Arc<CompiledIr> {
    compile_ir(
        &AudioDocument {
            sample_rate,
            channels,
        },
        None,
    )
    .expect("compiled IR")
}

/// A decaying pseudo-random kernel, so that the convolver keeps a tail between callbacks.
fn kernel(seed: u32, len: usize) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|index| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0;
            noise * (-(index as f32) / (len as f32 / 4.0)).exp()
        })
        .collect()
}

fn noise(seed: u32, len: usize) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0) * 0.5
        })
        .collect()
}

fn pair(a: &Arc<CompiledIr>, b: &Arc<CompiledIr>) -> BlockModel {
    BlockModel::IrPair {
        a: Arc::clone(a),
        b: Arc::clone(b),
    }
}

fn controls(blend: f32) -> BlockControls {
    BlockControls {
        ir_blend: blend,
        ..BlockControls::default()
    }
}

fn params(blend: f64) -> BlockParams {
    BlockParams {
        ir_blend: blend,
        ..BlockParams::default()
    }
}

/// Processes planar `channels`-channel audio in 64-host-frame callbacks.
fn render(
    model: BlockModel,
    oversampling: u8,
    channels: usize,
    input: &[f32],
    controls: BlockControls,
) -> Vec<f32> {
    let blend = f64::from(controls.ir_blend);
    let mut block = prepare_block(model, spec(oversampling), params(blend)).expect("block");
    render_with(&mut block, channels, input, controls)
}

fn render_with(
    block: &mut PreparedBlock,
    channels: usize,
    input: &[f32],
    controls: BlockControls,
) -> Vec<f32> {
    let frames_total = input.len() / channels;
    let callback = 64;
    let mut output = vec![0.0; input.len()];
    for start in (0..frames_total).step_by(callback) {
        let frames = callback.min(frames_total - start);
        let mut chunk = vec![0.0; channels * frames];
        for channel in 0..channels {
            chunk[channel * frames..(channel + 1) * frames].copy_from_slice(
                &input[channel * frames_total + start..channel * frames_total + start + frames],
            );
        }
        block
            .process(
                &mut PlanarBuffer::new(&mut chunk, channels, frames).expect("chunk"),
                controls,
            )
            .expect("block process");
        for channel in 0..channels {
            output[channel * frames_total + start..channel * frames_total + start + frames]
                .copy_from_slice(&chunk[channel * frames..(channel + 1) * frames]);
        }
    }
    output
}

fn assert_close(actual: &[f32], expected: &[f32], tolerance: f32) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() <= tolerance,
            "sample {index}: {actual} != {expected}"
        );
    }
}

#[test]
fn a_pair_at_blend_zero_matches_a_alone() {
    let a = ir(vec![kernel(1, 900)], 48_000.0);
    let b = ir(vec![kernel(2, 1300)], 48_000.0);
    let input = noise(3, 2 * 4096);

    for oversampling in [1, 2] {
        let alone = render(
            BlockModel::Ir(Arc::clone(&a)),
            oversampling,
            2,
            &input,
            controls(0.0),
        );
        let paired = render(pair(&a, &b), oversampling, 2, &input, controls(0.0));
        if oversampling == 1 {
            assert_eq!(paired, alone);
        } else {
            // The pair's gains sit inside the base-rate island, before upsampling.
            assert_close(&paired, &alone, 1.0e-6);
        }
    }
}

#[test]
fn a_pair_at_blend_one_matches_b_alone() {
    let a = ir(vec![kernel(1, 900)], 48_000.0);
    let b = ir(vec![kernel(2, 1300)], 48_000.0);
    let input = noise(4, 2 * 4096);

    for oversampling in [1, 2] {
        let alone = render(
            BlockModel::Ir(Arc::clone(&b)),
            oversampling,
            2,
            &input,
            controls(1.0),
        );
        let paired = render(pair(&a, &b), oversampling, 2, &input, controls(1.0));
        if oversampling == 1 {
            assert_eq!(paired, alone);
        } else {
            // The pair's gains sit inside the base-rate island, before upsampling.
            assert_close(&paired, &alone, 1.0e-6);
        }
    }
}

/// Silence first, so the block's enable fade is done, then a unit impulse.
fn impulse_response(model: BlockModel, controls: BlockControls, len: usize) -> Vec<f32> {
    let warm_up = 64 * 40;
    let mut input = vec![0.0; warm_up + len];
    input[warm_up] = 1.0;
    render(model, 1, 1, &input, controls)[warm_up..].to_vec()
}

#[test]
fn a_pair_at_half_blend_is_the_mean() {
    // A is an identity; B is two taps under unit energy, so neither is normalized.
    let a = ir(vec![vec![1.0]], 48_000.0);
    let b = ir(vec![vec![0.5, 0.25]], 48_000.0);
    assert_eq!(a.normalization_gain, 1.0);
    assert_eq!(b.normalization_gain, 1.0);
    let pad = a.output_pad();

    let response = impulse_response(pair(&a, &b), controls(0.5), 64);

    let mut expected = vec![0.0; 64];
    expected[0] = (0.5 * 1.0 + 0.5 * 0.5) * pad;
    expected[1] = (0.5 * 0.25) * pad;
    assert_close(&response, &expected, 1.0e-6);
}

#[test]
fn each_ir_keeps_its_own_normalization_and_levels_scale_each_side() {
    // A at 2.0 normalizes to 0.5, so its normalized response is a unit impulse.
    let a = ir(vec![vec![2.0]], 48_000.0);
    let b = ir(vec![vec![0.0, 1.0]], 48_000.0);
    assert_eq!(a.normalization_gain, 0.5);
    let pad = a.output_pad();
    // +6 dB on A: normalized 0.5 + 6/48.
    let level_a = 0.5 + 6.0 / 48.0;
    let gain_a = 10.0_f32.powf(6.0 / 20.0);

    let response = impulse_response(
        pair(&a, &b),
        BlockControls {
            ir_blend: 0.25,
            ir_a_level: level_a,
            ..BlockControls::default()
        },
        8,
    );

    let mut expected = vec![0.0; 8];
    expected[0] = 0.75 * gain_a * pad + 0.25 * pad;
    assert_close(&response, &expected, 1.0e-5);
}

#[test]
fn inverting_b_subtracts_it() {
    let a = ir(vec![vec![1.0]], 48_000.0);
    let b = ir(vec![vec![0.5, 0.25]], 48_000.0);
    let pad = a.output_pad();

    let response = impulse_response(
        pair(&a, &b),
        BlockControls {
            ir_blend: 0.5,
            ir_b_invert: true,
            ..BlockControls::default()
        },
        64,
    );

    let mut expected = vec![0.0; 64];
    expected[0] = (0.5 * 1.0 - 0.5 * 0.5) * pad;
    expected[1] = -(0.5 * 0.25) * pad;
    assert_close(&response, &expected, 1.0e-6);
}

#[test]
fn mixed_lengths_and_rates_add_no_latency() {
    // A: short mono capture at 44.1 kHz. B: longer stereo capture at 96 kHz.
    let a = ir(vec![kernel(5, 700)], 44_100.0);
    let b = ir(vec![kernel(6, 3000), kernel(7, 3000)], 96_000.0);
    assert_eq!(a.channels.len(), 1);
    assert_eq!(b.channels.len(), 2);

    for channels in [1, 2] {
        let input = noise(8, channels * 4096);
        let only_a = render(
            BlockModel::Ir(Arc::clone(&a)),
            1,
            channels,
            &input,
            controls(0.5),
        );
        let only_b = render(
            BlockModel::Ir(Arc::clone(&b)),
            1,
            channels,
            &input,
            controls(0.5),
        );
        let paired = render(pair(&a, &b), 1, channels, &input, controls(0.5));
        let mean: Vec<f32> = only_a
            .iter()
            .zip(&only_b)
            .map(|(a, b)| 0.5 * a + 0.5 * b)
            .collect();
        assert_close(&paired, &mean, 1.0e-5);
    }

    // The pair's impulse response starts on the impulse, like each IR alone.
    let response = impulse_response(pair(&a, &b), controls(0.5), 256);
    let alone = impulse_response(BlockModel::Ir(Arc::clone(&a)), controls(0.5), 256);
    let onset = |samples: &[f32]| samples.iter().position(|sample| sample.abs() > 1.0e-6);
    assert_eq!(onset(&response), Some(0));
    assert_eq!(onset(&response), onset(&alone));
}

#[test]
fn the_pair_works_through_2x_4x_8x() {
    let a = ir(vec![kernel(9, 800)], 48_000.0);
    let b = ir(vec![kernel(10, 1500), kernel(11, 1500)], 48_000.0);
    let input = noise(12, 2 * 2048);

    for oversampling in [2, 4, 8] {
        let only_a = render(
            BlockModel::Ir(Arc::clone(&a)),
            oversampling,
            2,
            &input,
            controls(0.5),
        );
        let only_b = render(
            BlockModel::Ir(Arc::clone(&b)),
            oversampling,
            2,
            &input,
            controls(0.5),
        );
        let paired = render(pair(&a, &b), oversampling, 2, &input, controls(0.5));
        let mean: Vec<f32> = only_a
            .iter()
            .zip(&only_b)
            .map(|(a, b)| 0.5 * a + 0.5 * b)
            .collect();
        assert!(
            paired.iter().any(|sample| sample.abs() > 1.0e-3),
            "{oversampling}x is silent"
        );
        assert_close(&paired, &mean, 1.0e-4);
    }
}

#[test]
fn a_blend_change_ramps_instead_of_jumping() {
    let a = ir(vec![vec![1.0]], 48_000.0);
    let b = ir(vec![vec![0.5]], 48_000.0);
    let mut block = prepare_block(pair(&a, &b), spec(1), params(0.0)).expect("block");
    let input = vec![0.25; 64 * 40];
    render_with(&mut block, 1, &input, controls(0.0));

    let output = render_with(&mut block, 1, &vec![0.25; 64], controls(1.0));

    let pad = a.output_pad();
    let start = 0.25 * pad;
    let end = 0.25 * 0.5 * pad;
    assert!(output[0] < start && output[0] > end, "{}", output[0]);
    assert!(output[63] > end, "the ramp lasts about 50 ms");
    assert!(output.windows(2).all(|pair| pair[1] <= pair[0]));
}

#[test]
fn the_pair_tail_is_the_longer_ir() {
    let a = ir(vec![kernel(13, 500)], 48_000.0);
    let b = ir(vec![kernel(14, 4800)], 48_000.0);
    let block = ChainBlock::tone(
        "cab",
        Tone::from_value(json!({
            "id": 1,
            "title": "Cab",
            "format": "ir",
            "models": [{"id": 1, "name": "A"}, {"id": 2, "name": "B"}]
        }))
        .expect("tone"),
        ModelId(1),
    );
    let change = PreparedChainChange::new(
        1,
        spec(1),
        LaneSnapshot {
            blocks: vec![block],
            models: vec![Some(pair(&a, &b))],
        },
        LaneSnapshot {
            blocks: vec![],
            models: vec![],
        },
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("chain preparation");

    assert_eq!(change.latency.tail_seconds, b.tail_seconds());
    assert_eq!(change.latency.samples, 0);
}

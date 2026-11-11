//! Output image matrix, mono fold, DC blocker, tone stack, and gain tests.

use ronie_dsp::{
    PlanarBuffer,
    image::{ImageControls, ImageStage},
    output::{OutputControls, OutputStage},
    tone_stack::{ToneControls, ToneStack},
};

const FS: f64 = 48_000.0;
const BLOCK: usize = 512;

fn run_image(
    left: &[f32],
    right: &[f32],
    controls: ImageControls,
) -> (Vec<f32>, Vec<f32>, Vec<(f32, f32)>) {
    assert_eq!(left.len(), right.len());
    let mut stage = ImageStage::new(FS, BLOCK).expect("image preparation");
    let mut out_left = Vec::with_capacity(left.len());
    let mut out_right = Vec::with_capacity(right.len());
    let mut captured = Vec::new();
    for (left_chunk, right_chunk) in left.chunks(BLOCK).zip(right.chunks(BLOCK)) {
        let frames = left_chunk.len();
        let mut data = Vec::with_capacity(frames * 2);
        data.extend_from_slice(left_chunk);
        data.extend_from_slice(right_chunk);
        stage
            .process(
                &mut PlanarBuffer::new(&mut data, 2, frames).expect("stereo buffer"),
                controls,
                |left, right| captured.extend(left.iter().copied().zip(right.iter().copied())),
            )
            .expect("image processing");
        let (l, r) = data.split_at(frames);
        out_left.extend_from_slice(l);
        out_right.extend_from_slice(r);
    }
    (out_left, out_right, captured)
}

fn run_output(left: &[f32], right: &[f32], controls: OutputControls) -> (Vec<f32>, Vec<f32>) {
    let mut stage = OutputStage::new(FS, BLOCK).expect("output preparation");
    let mut out_left = Vec::with_capacity(left.len());
    let mut out_right = Vec::with_capacity(right.len());
    for (left_chunk, right_chunk) in left.chunks(BLOCK).zip(right.chunks(BLOCK)) {
        let frames = left_chunk.len();
        let mut data = Vec::with_capacity(frames * 2);
        data.extend_from_slice(left_chunk);
        data.extend_from_slice(right_chunk);
        stage
            .process(
                &mut PlanarBuffer::new(&mut data, 2, frames).expect("stereo buffer"),
                controls,
                |_, _| {},
            )
            .expect("output processing");
        let (l, r) = data.split_at(frames);
        out_left.extend_from_slice(l);
        out_right.extend_from_slice(r);
    }
    (out_left, out_right)
}

fn constant_controls() -> ImageControls {
    ImageControls::default()
}

#[test]
fn default_matrix_is_bit_exact_identity() {
    let left = [1.0, -0.5, 0.25];
    let right = [0.25, -0.25, 0.75];
    let (left_out, right_out, _) = run_image(&left, &right, constant_controls());
    assert_eq!(left_out, left);
    assert_eq!(right_out, right);
}

#[test]
fn constant_power_pan_mixes_before_output() {
    let left = vec![1.0; 2048];
    let right = vec![0.25; 2048];
    let controls = ImageControls {
        pan_left: 0.5,
        pan_right: 0.5,
        ..constant_controls()
    };
    let (left_out, right_out, _) = run_image(&left, &right, controls);
    let gain = 0.5_f32.sqrt();
    let expected_left = gain + 0.25 * gain;
    let expected_right = gain + 0.25 * gain;
    assert!((left_out[2047] - expected_left).abs() < 1.0e-6);
    assert!((right_out[2047] - expected_right).abs() < 1.0e-6);
}

#[test]
fn balance_solo_and_polarity_shape_the_chains() {
    let left = vec![1.0; 2048];
    let right = vec![0.25; 2048];
    let controls = ImageControls {
        solo_right: true,
        invert_left: true,
        ..constant_controls()
    };
    let (left_out, right_out, _) = run_image(&left, &right, controls);
    assert!(left_out[2047].abs() < 1.0e-6);
    assert_eq!(right_out[2047], 0.25);

    let controls = ImageControls {
        balance: 0.75,
        stereo_rig: false,
        ..constant_controls()
    };
    let (left_out, right_out, _) = run_image(&left, &right, controls);
    let balance_left = 10.0_f32.powf(-6.0 / 20.0);
    let balance_right = 10.0_f32.powf(6.0 / 20.0);
    let expected = 0.5 * (balance_left + 0.25 * balance_right);
    assert!((left_out[2047] - expected).abs() < 1.0e-6);
    assert!((left_out[2047] - right_out[2047]).abs() < 1.0e-6);
}

#[test]
fn auto_balance_tap_precedes_balance_and_pan() {
    let left = vec![1.0; 2048];
    let right = vec![0.25; 2048];
    let controls = ImageControls {
        balance: 0.75,
        pan_left: 0.5,
        pan_right: 0.5,
        auto_balance_listening: true,
        ..constant_controls()
    };
    let (left_out, _, captured) = run_image(&left, &right, controls);
    assert_eq!(captured.len(), 2048);
    assert!(captured.iter().all(|(l, r)| (*l, *r) == (1.0, 0.25)));
    assert!(left_out.iter().any(|sample| *sample != 1.0));
}

#[test]
fn mono_rig_keeps_spread_idle_but_runs_align_before_fold() {
    let left = noise(96 * BLOCK, 7, 0.5);
    let right = left.clone();
    let controls = ImageControls {
        stereo_chains: false,
        stereo_rig: false,
        spread_enabled: true,
        spread: ronie_dsp::image::SpreadParams::from_normalized(0.75, 0.0, 0.5, true, true, true),
        ..constant_controls()
    };
    let mut stage = ImageStage::new(FS, BLOCK).expect("image preparation");
    let mut data = Vec::with_capacity(left.len() * 2);
    data.extend_from_slice(&left);
    data.extend_from_slice(&right);
    let original = data.clone();
    stage
        .process(
            &mut PlanarBuffer::new(&mut data, 2, left.len()).expect("stereo buffer"),
            controls,
            |_, _| {},
        )
        .expect("image processing");
    assert_eq!(data, original);
    assert!(!stage.spread_is_running());

    let controls = ImageControls {
        stereo_rig: false,
        align_enabled: true,
        align: ronie_dsp::image::AlignParams::from_normalized(1.0, 0.0, 0.5, false, false, false),
        ..constant_controls()
    };
    let (left_out, right_out, _) = run_image(&left, &right, controls);
    for index in 48_000..left.len() {
        let expected = 0.5 * (left[index] + left[index - 1152]);
        assert!((left_out[index] - expected).abs() < 1.0e-5);
        assert_eq!(left_out[index], right_out[index]);
    }
}

#[test]
fn dc_blocker_removes_a_constant_offset() {
    let left = vec![1.0; 48_000];
    let right = left.clone();
    let (left_out, right_out) = run_output(&left, &right, OutputControls::default());
    assert!(left_out[48_000 - 1].abs() < 0.001);
    for (index, (left, right)) in left_out.iter().zip(&right_out).enumerate() {
        assert!((left - right).abs() < 1.0e-6, "channel mismatch at {index}");
    }
}

#[test]
fn tone_coefficients_match_the_reference_shelves_and_mid_q() {
    let mut stack = ToneStack::new(FS).expect("tone preparation");
    stack.set_controls(ToneControls {
        bass: 9.0,
        mid: 8.0,
        treble: 2.0,
        enabled: true,
    });
    let coefficients = stack.coefficients();
    let bass = low_shelf(150.0, 0.707, 16.0);
    let mid = peak(425.0, 0.7, 9.0);
    let treble = high_shelf(1800.0, 0.707, -6.0);
    for (actual, expected) in [
        (coefficients[0], bass),
        (coefficients[1], mid),
        (coefficients[2], treble),
    ] {
        for index in 0..5 {
            assert!(
                (actual[index] - expected[index]).abs() < 1.0e-5,
                "index {index}"
            );
        }
    }
    stack.set_controls(ToneControls {
        mid: 2.0,
        ..ToneControls::default()
    });
    let cut = peak(425.0, 1.5, -9.0);
    for (actual, expected) in stack.coefficients()[1].iter().zip(cut) {
        assert!((actual - expected).abs() < 1.0e-5);
    }
}

#[test]
fn output_gain_ramps_over_twenty_ms() {
    let input = (0..48_000 + 2048)
        .map(|index| 0.5 * (2.0 * std::f64::consts::PI * 220.0 * index as f64 / FS).sin() as f32)
        .collect::<Vec<_>>();
    let render = |controls: OutputControls| {
        let mut stage = OutputStage::new(FS, BLOCK).expect("output preparation");
        let mut warm = Vec::with_capacity(96_000);
        warm.extend_from_slice(&input[..48_000]);
        warm.extend_from_slice(&input[..48_000]);
        stage
            .process(
                &mut PlanarBuffer::new(&mut warm, 2, 48_000).expect("warm buffer"),
                OutputControls::default(),
                |_, _| {},
            )
            .expect("warm processing");
        let mut final_data = Vec::with_capacity(4096);
        final_data.extend_from_slice(&input[48_000..]);
        final_data.extend_from_slice(&input[48_000..]);
        stage
            .process(
                &mut PlanarBuffer::new(&mut final_data, 2, 2048).expect("final buffer"),
                controls,
                |_, _| {},
            )
            .expect("final processing");
        final_data
    };
    let reduced = render(OutputControls {
        output_level: 0.0,
        ..Default::default()
    });
    let unity = render(OutputControls::default());
    let expected = 10.0_f32.powf(-24.0 / 20.0);
    let final_ratio = reduced[2047] / unity[2047];
    assert!(
        (final_ratio - expected).abs() < 1.0e-5,
        "ratio {final_ratio}"
    );
    let ramp_ratio = reduced[512] / unity[512];
    assert!(
        ramp_ratio > expected && ramp_ratio < 1.0,
        "ratio {ramp_ratio}"
    );
}

fn low_shelf(frequency: f32, q: f32, gain_db: f32) -> [f32; 5] {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let omega = 2.0 * std::f32::consts::PI * frequency / FS as f32;
    let cosine = omega.cos();
    let beta = omega.sin() * a.sqrt() / q;
    let minus = a - 1.0;
    let plus = a + 1.0;
    normalize([
        a * (plus - minus * cosine + beta),
        2.0 * a * (minus - plus * cosine),
        a * (plus - minus * cosine - beta),
        plus + minus * cosine + beta,
        -2.0 * (minus + plus * cosine),
        plus + minus * cosine - beta,
    ])
}

fn high_shelf(frequency: f32, q: f32, gain_db: f32) -> [f32; 5] {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let omega = 2.0 * std::f32::consts::PI * frequency / FS as f32;
    let cosine = omega.cos();
    let beta = omega.sin() * a.sqrt() / q;
    let minus = a - 1.0;
    let plus = a + 1.0;
    normalize([
        a * (plus + minus * cosine + beta),
        -2.0 * a * (minus + plus * cosine),
        a * (plus + minus * cosine - beta),
        plus - minus * cosine + beta,
        2.0 * (minus - plus * cosine),
        plus - minus * cosine - beta,
    ])
}

fn peak(frequency: f32, q: f32, gain_db: f32) -> [f32; 5] {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let omega = 2.0 * std::f32::consts::PI * frequency / FS as f32;
    let alpha = omega.sin() / (2.0 * q);
    let c2 = -2.0 * omega.cos();
    normalize([
        1.0 + alpha * a,
        c2,
        1.0 - alpha * a,
        1.0 + alpha / a,
        c2,
        1.0 - alpha / a,
    ])
}

fn normalize(coefficients: [f32; 6]) -> [f32; 5] {
    let a0 = coefficients[3];
    [
        coefficients[0] / a0,
        coefficients[1] / a0,
        coefficients[2] / a0,
        coefficients[4] / a0,
        coefficients[5] / a0,
    ]
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

#[test]
fn reset_clears_a_non_finite_filter_state() {
    let mut stage = OutputStage::new(48_000.0, 64).expect("output stage");
    let mut poisoned = [f32::NAN; 128];
    stage
        .process(
            &mut PlanarBuffer::new(&mut poisoned, 2, 64).expect("buffer"),
            OutputControls::default(),
            |_, _| {},
        )
        .expect("process");
    let mut stuck = [0.25; 128];
    stage
        .process(
            &mut PlanarBuffer::new(&mut stuck, 2, 64).expect("buffer"),
            OutputControls::default(),
            |_, _| {},
        )
        .expect("process");
    assert!(
        stuck.iter().any(|sample| sample.is_nan()),
        "this regression needs filter state that holds NaN"
    );

    stage.reset();

    let mut clean = [0.25; 128];
    stage
        .process(
            &mut PlanarBuffer::new(&mut clean, 2, 64).expect("buffer"),
            OutputControls::default(),
            |_, _| {},
        )
        .expect("process");
    assert!(clean.iter().all(|sample| sample.is_finite()));
    assert!(clean.iter().any(|sample| sample.abs() > 0.1));
}

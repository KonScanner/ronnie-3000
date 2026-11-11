//! Deterministic auto-align probe, estimator, and lifecycle tests.

use ronie_dsp::{
    PlanarBuffer,
    analysis::{AutoOffset, AutoOffsetResult, AutoOffsetState},
    image::{Align, AlignParams},
};

const FS: f64 = 48_000.0;
const BLOCK: usize = 512;
const MS_PER_SAMPLE: f32 = 1000.0 / FS as f32;

fn impulse_fir(delay: usize, gain: f32) -> Vec<f32> {
    let mut taps = vec![0.0; delay + 1];
    taps[delay] = gain;
    taps
}

fn fractional_delay_fir(delay: f64) -> Vec<f32> {
    const HALF_WIDTH: i32 = 32;
    let lo = delay.floor() as i32 - HALF_WIDTH;
    let hi = delay.ceil() as i32 + HALF_WIDTH;
    assert!(lo >= 0);
    let mut taps = vec![0.0; hi as usize + 1];
    for index in lo..=hi {
        let x = index as f64 - delay;
        let sinc = if x == 0.0 {
            1.0
        } else {
            (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
        };
        let t = x / (HALF_WIDTH + 1) as f64;
        let window = 0.42
            + 0.5 * (std::f64::consts::PI * t).cos()
            + 0.08 * (2.0 * std::f64::consts::PI * t).cos();
        taps[index as usize] = (sinc * window) as f32;
    }
    taps
}

fn one_pole_lp_fir(frequency: f64) -> Vec<f32> {
    let a = (-2.0 * std::f64::consts::PI * frequency / FS).exp();
    let mut coefficient = 1.0 - a;
    (0..256)
        .map(|_| {
            let tap = coefficient as f32;
            coefficient *= a;
            tap
        })
        .collect()
}

fn delayed_fir(taps: &[f32], delay: usize) -> Vec<f32> {
    [vec![0.0; delay], taps.to_vec()].concat()
}

fn scaled_fir(mut taps: Vec<f32>, gain: f32) -> Vec<f32> {
    taps.iter_mut().for_each(|tap| *tap *= gain);
    taps
}

fn measure(fir_l: &[f32], fir_r: &[f32]) -> AutoOffsetResult {
    let mut offset = AutoOffset::new(FS).expect("probe preparation");
    offset.arm();
    let mut probe = Vec::new();
    for _ in 0..200 {
        if offset.state() == AutoOffsetState::Captured {
            break;
        }
        let mut block = [0.0; BLOCK];
        if offset.render_probe_input(&mut block) {
            let base = probe.len();
            probe.extend_from_slice(&block);
            let left = (0..BLOCK)
                .map(|index| {
                    let sample = base + index;
                    (0..=sample.min(fir_l.len() - 1))
                        .map(|tap| fir_l[tap] * probe[sample - tap])
                        .sum::<f32>()
                })
                .collect::<Vec<_>>();
            let right = (0..BLOCK)
                .map(|index| {
                    let sample = base + index;
                    (0..=sample.min(fir_r.len() - 1))
                        .map(|tap| fir_r[tap] * probe[sample - tap])
                        .sum::<f32>()
                })
                .collect::<Vec<_>>();
            offset.capture_chain_outputs(&left, &right);
        }
        let mut host = [1.0; BLOCK * 2];
        offset
            .apply_output_gain(&mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"))
            .expect("mute processing");
    }
    assert_eq!(offset.state(), AutoOffsetState::Captured);
    offset.analyze()
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
fn measures_integer_delays_in_both_directions() {
    let result = measure(&impulse_fir(150, 1.0), &impulse_fir(0, 1.0));
    assert!((result.offset_ms - 150.0 * MS_PER_SAMPLE).abs() < 0.05 * MS_PER_SAMPLE);
    assert!(!result.inverted);
    assert!(result.confidence > 0.9);
    assert!(result.peak_sharpness > 3.0);

    let result = measure(&impulse_fir(0, 1.0), &impulse_fir(112, 1.0));
    assert!((result.offset_ms + 112.0 * MS_PER_SAMPLE).abs() < 0.05 * MS_PER_SAMPLE);
    assert!(!result.inverted);
    assert!(result.confidence > 0.9);
}

#[test]
fn fractional_delay_is_exact_and_repeatable() {
    let fir_l = fractional_delay_fir(37.36);
    let fir_r = impulse_fir(0, 1.0);
    let first = measure(&fir_l, &fir_r);
    assert!((first.offset_ms - 37.36 * MS_PER_SAMPLE).abs() < 0.05 * MS_PER_SAMPLE);
    assert!(first.confidence > 0.9);
    let second = measure(&fir_l, &fir_r);
    assert_eq!(first.offset_ms, second.offset_ms);
}

#[test]
fn gain_and_voicing_mismatch_do_not_move_the_estimate() {
    let voiced = delayed_fir(&one_pole_lp_fir(1000.0), 200);
    let flat = impulse_fir(0, 1.0);
    let even = measure(&voiced, &flat);
    assert!((even.offset_ms - 200.0 * MS_PER_SAMPLE).abs() < 0.2);
    assert!(even.peak_sharpness > 3.0);
    let mismatched = measure(&scaled_fir(voiced, 16.0), &scaled_fir(flat, 1.0 / 16.0));
    assert!((mismatched.offset_ms - even.offset_ms).abs() < 1.0e-4);
}

#[test]
fn measures_inverted_chains() {
    let result = measure(&impulse_fir(0, 1.0), &impulse_fir(150, -1.0));
    assert!((result.offset_ms + 150.0 * MS_PER_SAMPLE).abs() < 0.05 * MS_PER_SAMPLE);
    assert!(result.inverted);
    assert!(result.confidence > 0.9);

    let result = measure(&impulse_fir(0, 1.0), &impulse_fir(0, -1.0));
    assert!(result.offset_ms.abs() < 0.05 * MS_PER_SAMPLE);
    assert!(result.inverted);
    assert!(result.confidence > 0.9);
}

#[test]
fn rejects_lag_beyond_the_knob_range() {
    let result = measure(&impulse_fir(1920, 1.0), &impulse_fir(0, 1.0));
    assert!(result.peak_sharpness < 2.0);
}

#[test]
fn output_mutes_on_schedule_and_ramps_back_after_resume() {
    let mut offset = AutoOffset::new(FS).expect("probe preparation");
    let mut host = [1.0; BLOCK * 2];
    offset
        .apply_output_gain(&mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"))
        .expect("idle gain");
    assert_eq!(host[0], 1.0);

    offset.arm();
    let mut probe = [0.0; BLOCK];
    assert!(!offset.render_probe_input(&mut probe));
    host.fill(1.0);
    offset
        .apply_output_gain(&mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"))
        .expect("fade processing");
    assert_eq!(host[BLOCK - 1], 0.0);
    assert_eq!(offset.state(), AutoOffsetState::Probing);

    while matches!(
        offset.state(),
        AutoOffsetState::Probing | AutoOffsetState::Tail
    ) {
        assert!(offset.render_probe_input(&mut probe));
        offset.capture_chain_outputs(&probe, &probe);
        host.fill(1.0);
        offset
            .apply_output_gain(&mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"))
            .expect("mute processing");
        assert_eq!(host[BLOCK + 1], 0.0);
    }
    assert_eq!(offset.state(), AutoOffsetState::Captured);
    assert_eq!(offset.progress(), 1.0);
    assert!(!offset.render_probe_input(&mut probe));
    let result = offset.analyze();
    assert!(result.confidence > 0.99);
    host.fill(1.0);
    offset
        .apply_output_gain(&mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"))
        .expect("analyzing mute");
    assert_eq!(host[BLOCK - 1], 0.0);
    assert_eq!(offset.state(), AutoOffsetState::Analyzing);

    offset.resume();
    host.fill(1.0);
    offset
        .apply_output_gain(&mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"))
        .expect("ramp processing");
    assert_eq!(host[BLOCK - 1], 1.0);
    assert_eq!(offset.state(), AutoOffsetState::Idle);
}

#[test]
fn cancellation_returns_to_idle_without_a_measurement() {
    let mut offset = AutoOffset::new(FS).expect("probe preparation");
    offset.arm();
    let mut host = [1.0; BLOCK * 2];
    offset
        .apply_output_gain(&mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"))
        .expect("fade processing");
    assert_eq!(offset.state(), AutoOffsetState::Probing);
    offset.cancel();
    host.fill(1.0);
    offset
        .apply_output_gain(&mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"))
        .expect("ramp processing");
    assert_eq!(offset.state(), AutoOffsetState::Idle);
    assert_eq!(host[BLOCK - 1], 1.0);
}

#[test]
fn measure_then_align_removes_the_residual_lag() {
    const LAG: usize = 300;
    let result = measure(&impulse_fir(LAG, 1.0), &impulse_fir(0, 1.0));
    assert!(result.confidence > 0.9);
    let normalized: f32 = (0.5 + result.offset_ms / 48.0).clamp(0.0, 1.0);
    let input = noise(440 * BLOCK, 47, 0.5);
    let mut align = Align::new(FS, BLOCK).expect("Align preparation");
    let mut left = Vec::new();
    let mut right = Vec::new();
    for (block, source) in input.chunks(BLOCK).enumerate() {
        let mut data = Vec::with_capacity(BLOCK * 2);
        data.extend((0..BLOCK).map(|index| {
            let sample = block * BLOCK + index;
            if sample >= LAG {
                input[sample - LAG]
            } else {
                0.0
            }
        }));
        data.extend_from_slice(source);
        align.set_target(
            AlignParams::from_normalized(normalized, 0.0, 0.5, false, false, false),
            true,
        );
        align
            .process(&mut PlanarBuffer::new(&mut data, 2, BLOCK).expect("stereo buffer"))
            .expect("Align processing");
        let (l, r) = data.split_at(BLOCK);
        left.extend_from_slice(l);
        right.extend_from_slice(r);
    }
    for index in 48_000..input.len() {
        assert!((left[index] - right[index]).abs() < 1.0e-4);
    }
}

#[test]
fn silent_capture_has_no_confidence_or_sharpness() {
    let result = measure(&[0.0; 1], &[0.0; 1]);
    assert_eq!(result.confidence, 0.0);
    assert_eq!(result.peak_sharpness, 0.0);
}

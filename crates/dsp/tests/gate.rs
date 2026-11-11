//! Input gate detector, timing, range, reset, and ordering tests.

use ronie_dsp::{
    PlanarBuffer,
    input::{GateParams, InputControls, InputStage, NoiseGate},
};

const FS: f64 = 48_000.0;
const BLOCK: usize = 256;

fn ms(milliseconds: f64) -> usize {
    (milliseconds * 0.001 * FS).round() as usize
}

fn sine(frames: usize, hz: f64, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|index| amplitude * (2.0 * std::f64::consts::PI * hz * index as f64 / FS).sin() as f32)
        .collect()
}

fn burst_then_floor(burst: usize, tail: usize, floor: f32) -> Vec<f32> {
    let mut input = sine(burst + tail, 1000.0, 0.5);
    for sample in &mut input[burst..] {
        *sample *= floor / 0.5;
    }
    input
}

fn run_gate(input: &[f32], params: GateParams) -> Vec<f32> {
    let mut gate = NoiseGate::new(FS).expect("gate preparation");
    gate.set_params(params);
    let mut output = Vec::with_capacity(input.len());
    for source in input.chunks(BLOCK) {
        let mut data = source.to_vec();
        gate.process(&mut PlanarBuffer::new(&mut data, 1, source.len()).expect("buffer"))
            .expect("gate processing");
        output.extend_from_slice(&data);
    }
    output
}

fn gain_db_at(output: &[f32], input: &[f32], at: usize) -> f64 {
    let window = ms(2.0);
    let mut out_power = 0.0;
    let mut in_power = 0.0;
    for index in at..at + window {
        out_power += f64::from(output[index]) * f64::from(output[index]);
        in_power += f64::from(input[index]) * f64::from(input[index]);
    }
    10.0 * (out_power.max(1.0e-30) / in_power.max(1.0e-30)).log10()
}

fn first_below(output: &[f32], input: &[f32], from: usize, db_below: f64) -> Option<usize> {
    let step = ms(0.5);
    (from..output.len() - ms(2.0))
        .step_by(step)
        .find(|at| gain_db_at(output, input, *at) < db_below)
}

fn measured_tau_ms(output: &[f32], input: &[f32], from: usize) -> f64 {
    let close_start = first_below(output, input, from, -3.0).expect("close start");
    let at_twenty = first_below(output, input, from, -20.0).expect("-20 dB point");
    (at_twenty - close_start) as f64 / (0.001 * FS) / (10.0_f64.ln() - 0.36)
}

#[test]
fn defaults_match_the_retuned_gate() {
    let params = GateParams::default();
    assert_eq!(params.threshold_db, -80.0);
    assert_eq!(params.release_ms, 50.0);
    assert_eq!(params.hold_ms, 20.0);
    assert_eq!(params.range_db, 80.0);
}

#[test]
fn release_sets_the_tail_time_constant() {
    let burst = ms(200.0);
    let input = burst_then_floor(burst, ms(3000.0), 0.0005);
    let tau_for = |release_ms| {
        let params = GateParams {
            threshold_db: -40.0,
            release_ms,
            hold_ms: 0.0,
            ..Default::default()
        };
        measured_tau_ms(&run_gate(&input, params), &input, burst)
    };
    assert!((tau_for(100.0) - 100.0).abs() < 25.0);
    assert!((tau_for(500.0) - 500.0).abs() < 125.0);
    assert!(tau_for(5.0) < 10.0);
}

#[test]
fn hold_delays_the_close_by_its_duration() {
    let burst = ms(200.0);
    let input = burst_then_floor(burst, ms(1500.0), 0.0005);
    let close_start = |hold_ms| {
        let params = GateParams {
            threshold_db: -40.0,
            release_ms: 5.0,
            hold_ms,
            ..Default::default()
        };
        first_below(&run_gate(&input, params), &input, burst, -3.0).expect("close")
    };
    let no_hold = close_start(0.0);
    let held = close_start(150.0);
    assert!((held as f64 - no_hold as f64 - ms(150.0) as f64).abs() <= ms(3.0) as f64);
}

#[test]
fn range_sets_the_closed_floor() {
    let input = sine(ms(500.0), 1000.0, 0.001);
    for range_db in [20.0, 40.0, 80.0] {
        let params = GateParams {
            threshold_db: -10.0,
            range_db,
            ..Default::default()
        };
        let output = run_gate(&input, params);
        assert!((gain_db_at(&output, &input, ms(400.0)) + f64::from(range_db)).abs() < 0.5);
    }
}

#[test]
fn params_apply_mid_stream() {
    let burst = 40 * BLOCK;
    let input = burst_then_floor(burst, ms(1500.0), 0.0005);
    let mut gate = NoiseGate::new(FS).expect("gate preparation");
    let mut params = GateParams {
        threshold_db: -40.0,
        release_ms: 500.0,
        hold_ms: 0.0,
        ..Default::default()
    };
    gate.set_params(params);
    let mut output = Vec::with_capacity(input.len());
    for (index, source) in input.chunks(BLOCK).enumerate() {
        if index * BLOCK == burst {
            params.release_ms = 5.0;
            gate.set_params(params);
        }
        let mut data = source.to_vec();
        gate.process(&mut PlanarBuffer::new(&mut data, 1, source.len()).expect("buffer"))
            .expect("gate processing");
        output.extend_from_slice(&data);
    }
    assert!(measured_tau_ms(&output, &input, burst) < 10.0);
}

#[test]
fn input_stage_applies_gain_before_gate_and_resets_on_reenable() {
    let mut stage = InputStage::new(FS, BLOCK).expect("input preparation");
    let controls = InputControls {
        input_gain: 0.01,
        gate_enabled: true,
        gate: GateParams {
            threshold_db: -40.0,
            hold_ms: 0.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut data = sine(BLOCK, 1000.0, 0.5);
    let original = data.clone();
    stage
        .process(
            &mut PlanarBuffer::new(&mut data, 1, BLOCK).expect("buffer"),
            controls,
        )
        .expect("input processing");
    assert!(data.iter().all(|sample| sample.abs() < 0.01));
    assert_ne!(data, original);

    let mut controls = controls;
    controls.input_gain = 1.0;
    for _ in 0..20 {
        let mut data = sine(BLOCK, 1000.0, 0.5);
        stage
            .process(
                &mut PlanarBuffer::new(&mut data, 1, BLOCK).expect("buffer"),
                controls,
            )
            .expect("input processing");
    }
    controls.gate_enabled = false;
    let mut bypassed = sine(BLOCK, 1000.0, 0.5);
    let original = bypassed.clone();
    stage
        .process(
            &mut PlanarBuffer::new(&mut bypassed, 1, BLOCK).expect("buffer"),
            controls,
        )
        .expect("gate bypass");
    assert_eq!(bypassed, original);

    controls.gate_enabled = true;
    let mut restarted = sine(BLOCK, 1000.0, 0.5);
    stage
        .process(
            &mut PlanarBuffer::new(&mut restarted, 1, BLOCK).expect("buffer"),
            controls,
        )
        .expect("gate restart");
    assert!(restarted[0].abs() < 0.001);
}

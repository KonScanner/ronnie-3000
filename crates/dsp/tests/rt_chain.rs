//! Atomic chain publication and callback failure tests.

use ronie_core::InputMode;
use ronie_dsp::{
    ChainError, LatencyReport, PlanarBuffer, PrepareSpec,
    lane::LaneSnapshot,
    prepare::PreparedChainChange,
    rt_chain::{ChainCommand, ChainControls, RtChain},
};
use std::sync::mpsc::TrySendError;

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

fn empty_snapshot() -> LaneSnapshot {
    LaneSnapshot {
        blocks: vec![],
        models: vec![],
    }
}

fn change(generation: u64) -> PreparedChainChange {
    PreparedChainChange::new(
        generation,
        spec(),
        empty_snapshot(),
        empty_snapshot(),
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("empty chain preparation")
}

#[test]
fn changes_apply_only_at_boundaries_and_stale_work_never_publishes() {
    let (mut chain, sender) = RtChain::prepare(change(1), 2).expect("real-time chain");
    assert_eq!(chain.active_generation(), 1);

    sender
        .try_send(ChainCommand::Replace(change(3)))
        .expect("queue generation three");
    assert_eq!(chain.active_generation(), 1);
    chain.apply_pending();
    assert_eq!(chain.active_generation(), 3);

    sender
        .try_send(ChainCommand::Replace(change(2)))
        .expect("queue stale generation");
    chain.apply_pending();
    assert_eq!(chain.active_generation(), 3);
    assert_eq!(chain.drain_retirements_off_thread(), 2);
}

#[test]
fn failed_candidate_preparation_leaves_the_current_chain_unchanged() {
    let (mut chain, _sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    let invalid = LaneSnapshot {
        blocks: vec![],
        models: vec![None],
    };
    let candidate = PreparedChainChange::new(
        2,
        spec(),
        invalid,
        empty_snapshot(),
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    );
    assert!(candidate.is_err());
    assert_eq!(chain.active_generation(), 1);

    let mut data = [0.0; 128];
    chain
        .process(&mut PlanarBuffer::new(&mut data, 2, 64).expect("audio buffer"))
        .expect("current chain remains valid");
    assert_eq!(data, [0.0; 128]);
}

#[test]
fn invalid_callbacks_do_not_mutate_audio_or_generation() {
    let (mut chain, _sender) = RtChain::prepare(change(7), 1).expect("real-time chain");
    let mut oversized = [0.25; 65];
    let error = chain
        .process(&mut PlanarBuffer::new(&mut oversized, 1, 65).expect("audio buffer"))
        .expect_err("oversized callback must fail");
    assert!(matches!(error, ChainError::InvalidLayout(_)));
    assert_eq!(oversized, [0.25; 65]);
    assert_eq!(chain.active_generation(), 7);
}

#[test]
fn non_48k_callbacks_cross_the_prepared_chain_boundary() {
    let mut host_spec = spec();
    host_spec.host_sample_rate = 44_100.0;
    let change = PreparedChainChange::new(
        1,
        host_spec,
        empty_snapshot(),
        empty_snapshot(),
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("44.1 kHz chain preparation");
    let (mut chain, _sender) = RtChain::prepare(change, 1).expect("real-time chain");
    let mut controls = chain.controls();
    controls.input.gate_enabled = false;
    chain.set_controls(controls);
    let mut data = [0.0; 64];
    data[0] = 1.0;
    chain
        .process(&mut PlanarBuffer::new(&mut data, 1, 64).expect("audio buffer"))
        .expect("resampled processing");
    assert!(data[..4].iter().all(|sample| sample.abs() < 1.0e-6));
    assert!(data.iter().any(|sample| sample.abs() > 1.0e-6));
}

#[test]
fn replacement_capacity_is_validated_before_audio_mutation() {
    let (mut chain, sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    let mut smaller_spec = spec();
    smaller_spec.max_host_frames = 32;
    let replacement = PreparedChainChange::new(
        2,
        smaller_spec,
        empty_snapshot(),
        empty_snapshot(),
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("smaller chain preparation");
    sender
        .try_send(ChainCommand::Replace(replacement))
        .expect("queue replacement");
    let mut data = [0.25; 64];
    let error = chain
        .process(&mut PlanarBuffer::new(&mut data, 1, 64).expect("audio buffer"))
        .expect_err("new capacity rejects the callback");
    assert!(matches!(error, ChainError::InvalidLayout(_)));
    assert_eq!(chain.active_generation(), 2);
    assert_eq!(data, [0.25; 64]);
}

fn unity_controls() -> ChainControls {
    let mut controls = ChainControls::default();
    controls.input.gate_enabled = false;
    controls
}

fn sine(frames: usize, offset: usize) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            let phase = (frame + offset) as f32 * 1_000.0 / 48_000.0;
            0.25 * (phase * std::f32::consts::TAU).sin()
        })
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Runs a stereo sine through the chain for `callbacks` buffers and returns the last output.
fn run_sine(chain: &mut RtChain, callbacks: usize) -> Vec<f32> {
    let mut output = Vec::new();
    for callback in 0..callbacks {
        output = sine(64, callback * 64);
        output.extend(sine(64, callback * 64));
        chain
            .process(&mut PlanarBuffer::new(&mut output, 2, 64).expect("audio buffer"))
            .expect("processing");
    }
    output
}

#[test]
fn queued_controls_apply_at_the_next_boundary_and_the_newest_wins() {
    let (mut chain, sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    chain.set_controls(unity_controls());
    let unity = rms(&run_sine(&mut chain, 64));

    let mut controls = unity_controls();
    controls.output.output_level = 0.0;
    sender.try_send_controls(controls).expect("first controls");
    controls.output.output_level = 0.75;
    sender.try_send_controls(controls).expect("newest controls");
    assert_eq!(chain.controls().output.output_level, 0.5);

    let boosted = rms(&run_sine(&mut chain, 64));
    assert_eq!(chain.controls().output.output_level, 0.75);
    let expected = 10.0_f32.powf(12.0 / 20.0);
    assert!(
        (boosted / unity / expected - 1.0).abs() < 0.05,
        "{boosted} against {unity}"
    );
}

#[test]
fn a_full_controls_queue_returns_the_snapshot_to_the_caller() {
    let (_chain, sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    let mut controls = unity_controls();
    let mut rejected = None;
    for _ in 0..64 {
        if let Err(TrySendError::Full(returned)) = sender.try_send_controls(controls) {
            rejected = Some(returned);
            break;
        }
    }
    controls.output.output_level = 0.5;
    assert_eq!(rejected, Some(controls));
}

#[test]
fn a_non_finite_input_sample_never_enters_the_chain() {
    let (mut chain, _sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    chain.set_controls(unity_controls());
    let mut data = sine(128, 0);
    data[3] = f32::NAN;
    data[70] = f32::INFINITY;
    chain
        .process(&mut PlanarBuffer::new(&mut data, 2, 64).expect("audio buffer"))
        .expect("processing");
    assert!(data.iter().all(|sample| sample.is_finite()));
    assert!(!chain.take_output_sanitized());
    assert!(rms(&run_sine(&mut chain, 8)) > 0.01);
}

#[test]
fn a_poisoned_output_is_repaired_and_the_output_stage_state_is_cleared() {
    let (mut chain, _sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    chain.set_controls(unity_controls());
    let before = rms(&run_sine(&mut chain, 32));

    let mut poison = unity_controls();
    poison.input.input_gain = f32::INFINITY;
    chain.set_controls(poison);
    let mut data = sine(128, 0);
    chain
        .process(&mut PlanarBuffer::new(&mut data, 2, 64).expect("audio buffer"))
        .expect("processing");
    assert!(data.iter().all(|sample| sample.is_finite()));
    assert!(chain.take_output_sanitized());
    assert!(!chain.take_output_sanitized());

    chain.set_controls(unity_controls());
    let after = run_sine(&mut chain, 32);
    assert!(after.iter().all(|sample| sample.is_finite()));
    assert!(
        !chain.take_output_sanitized(),
        "poison stayed in the output stage"
    );
    assert!(
        (rms(&after) / before - 1.0).abs() < 0.05,
        "audio must return, {} against {before}",
        rms(&after)
    );
}

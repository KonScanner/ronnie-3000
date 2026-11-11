//! Full-chain serial/worker differential tests over host rates and numeric inputs.

use ronie_core::{ChainBlock, InputMode, ModelId, Tone};
use ronie_dsp::{
    LatencyReport, PlanarBuffer, PrepareSpec, lane::LaneSnapshot, prepare::PreparedChainChange,
    rt::RtWorkerPool, rt_chain::RtChain,
};
use ronie_oracle::synthetic::synthetic_a2_document;
use serde_json::json;

fn change(spec: PrepareSpec) -> PreparedChainChange {
    let compiled = ronie_dsp::compile_model(&synthetic_a2_document(3).expect("synthetic document"))
        .expect("compiled model");
    let block = ChainBlock::tone(
        "model",
        Tone::from_value(json!({
            "id": 1,
            "title": "Model",
            "format": "nam",
            "models": [{"id": 1, "name": "Model"}]
        }))
        .expect("tone"),
        ModelId(1),
    );
    PreparedChainChange::new(
        1,
        spec,
        LaneSnapshot {
            blocks: vec![block],
            models: vec![Some(ronie_dsp::block::BlockModel::Nam(compiled))],
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
    .expect("chain preparation")
}

fn samples(frames: usize, callback: usize) -> Vec<f32> {
    let mut data = (0..frames * 2)
        .map(|index| ((index * 41 + callback * 17) % 257) as f32 / 128.0 - 1.0)
        .collect::<Vec<_>>();
    match callback {
        2 => data[0] = f32::NAN,
        3 => data[0] = f32::INFINITY,
        4 => data[0] = f32::NEG_INFINITY,
        5 => data[0] = f32::from_bits(1),
        _ => {}
    }
    data
}

#[test]
fn complete_streams_match_serial_bits_across_rates_and_non_divisible_callbacks() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for oversampling in [1, 8] {
            let spec = PrepareSpec {
                host_sample_rate: rate,
                max_host_frames: 64,
                oversampling,
                input_mode: InputMode::Stereo,
                stereo_input: true,
                stereo_output: true,
            };
            let (mut serial, _) = RtChain::prepare(change(spec), 1).expect("serial chain");
            let (mut parallel, _) = RtChain::prepare(change(spec), 1).expect("parallel chain");
            let pool = RtWorkerPool::prepare(4, 16).expect("worker pool");
            parallel.set_worker_pool(Some(pool));
            let mut controls = serial.controls();
            controls.input.gate_enabled = false;
            serial.set_controls(controls);
            parallel.set_controls(controls);
            for (callback, frames) in [31, 32, 47, 64, 33, 17, 64].into_iter().enumerate() {
                let mut expected = samples(frames, callback);
                let mut actual = expected.clone();
                serial
                    .process(
                        &mut PlanarBuffer::new(&mut expected, 2, frames).expect("serial buffer"),
                    )
                    .expect("serial processing");
                parallel
                    .process(
                        &mut PlanarBuffer::new(&mut actual, 2, frames).expect("parallel buffer"),
                    )
                    .expect("parallel processing");
                assert_eq!(
                    actual
                        .iter()
                        .map(|sample| sample.to_bits())
                        .collect::<Vec<_>>(),
                    expected
                        .iter()
                        .map(|sample| sample.to_bits())
                        .collect::<Vec<_>>(),
                    "rate {rate}, oversampling {oversampling}, callback {callback}"
                );
            }
            parallel.set_worker_pool(None);
        }
    }
}

#[test]
fn restored_generation_matches_a_fresh_chain_bit_for_bit() {
    let spec = PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 2,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    };
    let (mut restored, sender) = RtChain::prepare(change(spec), 1).expect("restored chain");
    let (mut fresh, _) = RtChain::prepare(change(spec), 1).expect("fresh chain");
    let mut controls = restored.controls();
    controls.input.gate_enabled = false;
    restored.set_controls(controls);
    fresh.set_controls(controls);
    let mut warmup = samples(64, 0);
    restored
        .process(&mut PlanarBuffer::new(&mut warmup, 2, 64).expect("warmup buffer"))
        .expect("warmup");
    let mut replacement = change(spec);
    replacement.generation = 2;
    sender
        .try_send(ronie_dsp::rt_chain::ChainCommand::Replace(replacement))
        .expect("queue restoration");

    let mut actual = samples(47, 1);
    let mut expected = actual.clone();
    restored
        .process(&mut PlanarBuffer::new(&mut actual, 2, 47).expect("restored buffer"))
        .expect("restored processing");
    fresh
        .process(&mut PlanarBuffer::new(&mut expected, 2, 47).expect("fresh buffer"))
        .expect("fresh processing");
    assert_eq!(restored.active_generation(), 2);
    assert_eq!(
        actual
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>()
    );
}

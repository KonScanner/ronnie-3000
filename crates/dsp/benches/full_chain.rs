//! Representative full-chain Criterion benchmarks.

#![allow(missing_docs)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ronie_core::{AudioDocument, ChainBlock, InputMode, ModelId, Tone};
use ronie_dsp::{
    LatencyReport, PlanarBuffer, PrepareSpec, block::BlockModel, compile_model, ir::compile_ir,
    lane::LaneSnapshot, prepare::PreparedChainChange, rt_chain::RtChain,
};
use ronie_oracle::synthetic::synthetic_a2_document;
use serde_json::json;

fn block(id: &str, title: &str, format: &str) -> ChainBlock {
    ChainBlock::tone(
        id,
        Tone::from_value(json!({
            "id": 1,
            "title": title,
            "format": format,
            "models": [{"id": 1, "name": title}]
        }))
        .expect("tone"),
        ModelId(1),
    )
}

fn full_chain(c: &mut Criterion) {
    let nam = BlockModel::Nam(
        compile_model(&synthetic_a2_document(8).expect("synthetic document"))
            .expect("compiled model"),
    );
    let ir = BlockModel::Ir(
        compile_ir(
            &AudioDocument {
                sample_rate: 48_000.0,
                channels: vec![vec![1.0]],
            },
            None,
        )
        .expect("compiled IR"),
    );
    let change = PreparedChainChange::new(
        1,
        PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        LaneSnapshot {
            blocks: vec![block("nam", "Model", "nam"), block("ir", "IR", "ir")],
            models: vec![Some(nam), Some(ir)],
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
    let (mut chain, _sender) = RtChain::prepare(change, 1).expect("real-time chain");
    let mut controls = chain.controls();
    controls.input.gate_enabled = false;
    chain.set_controls(controls);
    let mut buffer = vec![0.25_f32; 128];

    c.bench_function("nam_ir_full_chain_48k_stereo_64", |bencher| {
        bencher.iter(|| {
            chain
                .process(black_box(
                    &mut PlanarBuffer::new(&mut buffer, 2, 64).expect("audio buffer"),
                ))
                .expect("benchmark process");
        });
    });
}

/// A cab-length decaying kernel.
fn cab_kernel(seed: f32) -> Vec<f32> {
    (0..4_096)
        .map(|index| (index as f32 * seed).sin() * (-(index as f32) / 1_024.0).exp())
        .collect()
}

fn cab(seed: f32) -> std::sync::Arc<ronie_dsp::ir::CompiledIr> {
    compile_ir(
        &AudioDocument {
            sample_rate: 48_000.0,
            channels: vec![cab_kernel(seed)],
        },
        None,
    )
    .expect("compiled IR")
}

fn cab_chain(model: BlockModel) -> RtChain {
    let change = PreparedChainChange::new(
        1,
        PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        LaneSnapshot {
            blocks: vec![block("cab", "Cab", "ir")],
            models: vec![Some(model)],
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
    let (mut chain, _sender) = RtChain::prepare(change, 1).expect("real-time chain");
    let mut controls = chain.controls();
    controls.input.gate_enabled = false;
    chain.set_controls(controls);
    chain
}

fn cab_pair(c: &mut Criterion) {
    let a = cab(0.37);
    let b = cab(0.11);
    let cases = [
        ("cab_ir_4096_48k_stereo_64", BlockModel::Ir(a.clone())),
        (
            "cab_ir_pair_4096_48k_stereo_64",
            BlockModel::IrPair { a, b },
        ),
    ];
    for (name, model) in cases {
        let mut chain = cab_chain(model);
        let mut buffer = vec![0.25_f32; 128];
        c.bench_function(name, |bencher| {
            bencher.iter(|| {
                chain
                    .process(black_box(
                        &mut PlanarBuffer::new(&mut buffer, 2, 64).expect("audio buffer"),
                    ))
                    .expect("benchmark process");
            });
        });
    }
}

criterion_group!(benches, full_chain, cab_pair);
criterion_main!(benches);

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

criterion_group!(benches, full_chain);
criterion_main!(benches);

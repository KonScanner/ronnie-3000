//! Measures the time of one audio callback at 256 frames and 48 kHz, on this thread.
//!
//! Run with `cargo run --release -p ronie-app --example callback_cost`. The budget of one callback
//! is 5333 microseconds.

use std::time::{Duration, Instant};

use ronie_app::audio::{AudioBackendKind, AudioWorker, DeviceConfig};
use ronie_core::{AudioDocument, ChainBlock, InputMode, ModelId, Tone};
use ronie_dsp::{
    LatencyReport, PrepareSpec, block::BlockModel, ir::compile_ir, lane::LaneSnapshot,
    prepare::PreparedChainChange, rt_chain::ChainControls,
};

const FRAMES: usize = 256;
const RATE: u32 = 48_000;
const CALLBACKS: usize = 40_000;

fn identity_block(id: &str) -> ChainBlock {
    let tone = Tone::from_value(serde_json::json!({
        "id": 1,
        "title": "Identity IR",
        "format": "ir",
        "models": [{"id": 1, "name": "Identity"}],
    }))
    .expect("tone");
    ChainBlock::tone(id, tone, ModelId(1))
}

fn chain(stereo_input: bool) -> PreparedChainChange {
    let model = compile_ir(
        &AudioDocument {
            sample_rate: f64::from(RATE),
            channels: vec![vec![1.0]],
        },
        None,
    )
    .expect("identity IR");
    let blocks = vec![identity_block("first"), identity_block("second")];
    let models = blocks
        .iter()
        .map(|_| Some(BlockModel::Ir(model.clone())))
        .collect();
    PreparedChainChange::new(
        1,
        PrepareSpec {
            host_sample_rate: f64::from(RATE),
            max_host_frames: FRAMES,
            oversampling: 1,
            input_mode: if stereo_input {
                InputMode::Stereo
            } else {
                InputMode::Left
            },
            stereo_input,
            stereo_output: true,
        },
        LaneSnapshot { blocks, models },
        LaneSnapshot {
            blocks: Vec::new(),
            models: Vec::new(),
        },
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("chain")
}

fn config(selected_channels: usize) -> DeviceConfig {
    DeviceConfig {
        backend: AudioBackendKind::Dummy,
        sample_rate: Some(RATE),
        buffer_frames: Some(FRAMES as u32),
        input_channels: selected_channels,
        output_channels: 2,
        ..DeviceConfig::default()
    }
}

/// The mean and the 99.9th percentile of a callback, in microseconds.
fn measure(mut callback: impl FnMut()) -> (f64, f64) {
    for _ in 0..2_000 {
        callback();
    }
    let mut times = Vec::with_capacity(CALLBACKS);
    for _ in 0..CALLBACKS {
        let started = Instant::now();
        callback();
        times.push(started.elapsed());
    }
    times.sort_unstable();
    let total: Duration = times.iter().sum();
    let mean = total.as_secs_f64() * 1e6 / CALLBACKS as f64;
    let high = times[CALLBACKS * 999 / 1000].as_secs_f64() * 1e6;
    (mean, high)
}

fn report(name: &str, (mean, high): (f64, f64)) {
    println!("{name}: mean {mean:.2} us, p99.9 {high:.2} us, budget 5333 us");
}

/// One case with the layout that the host delivers.
fn case(name: &str, selected: usize, layout: usize) {
    let config = config(selected);
    let mut worker =
        AudioWorker::with_chain(&config, chain(selected == 2), ChainControls::default())
            .expect("worker");
    let input: Vec<f32> = (0..FRAMES * layout)
        .map(|index| 0.25 * (index as f32 * 0.03).sin())
        .collect();
    let mut output = vec![0.0_f32; FRAMES * 2];
    report(
        name,
        measure(|| {
            worker
                .process_interleaved(&input, layout, &mut output, 2)
                .expect("callback");
        }),
    );
}

/// The worst layout of a route: each device channel has a level, and a test tone plays.
fn case_with_tone(name: &str, layout: usize) {
    let mut config = config(1);
    config.input_route = [layout - 1, layout - 1];
    let mut worker =
        AudioWorker::with_chain(&config, chain(false), ChainControls::default()).expect("worker");
    let input: Vec<f32> = (0..FRAMES * layout)
        .map(|index| 0.25 * (index as f32 * 0.03).sin())
        .collect();
    let mut output = vec![0.0_f32; FRAMES * 2];
    let telemetry = worker.telemetry();
    report(
        name,
        measure(|| {
            // A new request keeps the tone in the callback for the whole run.
            telemetry.request_test_tone();
            worker
                .process_interleaved(&input, layout, &mut output, 2)
                .expect("callback");
        }),
    );
}

fn main() {
    case("mono input, 1 channel from the host", 1, 1);
    case("stereo input, 2 channels from the host", 2, 2);
    case("mono input, 2 channels from the host", 1, 2);
    case("mono input, 32 channels from the host", 1, 32);
    case_with_tone("mono input, 2 channels from the host, test tone", 2);
    case_with_tone("mono input, 32 channels from the host, test tone", 32);
}

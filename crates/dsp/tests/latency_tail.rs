//! Host bus, latency, and tail contract tests.

use ronie_core::{AudioDocument, ChainBlock, InputMode, ModelId, Tone};
use ronie_dsp::{
    LatencyReport, PlanarBuffer, PrepareSpec,
    block::BlockModel,
    host::{BusLayout, HostCapabilities},
    input::{InputStage, PitchWindow},
    ir::compile_ir,
    lane::LaneSnapshot,
    prepare::PreparedChainChange,
    rt_chain::RtChain,
};

fn spec(rate: f64) -> PrepareSpec {
    PrepareSpec {
        host_sample_rate: rate,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    }
}

fn chain(spec: PrepareSpec) -> RtChain {
    let empty = || LaneSnapshot {
        blocks: vec![],
        models: vec![],
    };
    let change = PreparedChainChange::new(
        1,
        spec,
        empty(),
        empty(),
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("chain preparation");
    RtChain::prepare(change, 1).expect("real-time chain").0
}

#[test]
fn reports_bus_layout_and_boundary_delay_from_the_active_snapshot() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        let chain = chain(spec(rate));
        let capabilities = HostCapabilities::from_spec(spec(rate));
        let report = chain.host_report();
        assert_eq!(report.bus_layout, BusLayout::Stereo);
        let expected = if rate == 48_000.0 { 0 } else { 12 };
        assert!(report.latency_samples >= expected);
        assert_eq!(report.tail_seconds, 0.0);
        assert_eq!(capabilities.max_host_frames, 64);
    }

    let mut mono_spec = spec(48_000.0);
    mono_spec.stereo_input = false;
    mono_spec.stereo_output = false;
    assert_eq!(chain(mono_spec).host_report().bus_layout, BusLayout::Mono);
}

#[test]
fn every_pitch_window_reports_delay_and_oversampling_does_not_change_it() {
    for window in [
        PitchWindow::Ms20,
        PitchWindow::Ms30,
        PitchWindow::Ms40,
        PitchWindow::Ms60,
    ] {
        let mut reports = Vec::new();
        for factor in [1, 2, 4, 8] {
            let mut oversampled = spec(48_000.0);
            oversampled.oversampling = factor;
            let mut chain = chain(oversampled);
            let mut controls = chain.controls();
            controls.input.pitch_enabled = true;
            controls.input.pitch.window = window;
            chain.set_controls(controls);
            reports.push(chain.host_report().latency_samples);
        }
        assert!(reports.windows(2).all(|pair| pair[0] == pair[1]));
        let input = InputStage::new(48_000.0, 64).expect("input stage");
        let mut controls = ronie_dsp::input::InputControls {
            pitch_enabled: true,
            ..Default::default()
        };
        controls.pitch.window = window;
        assert_eq!(reports[0], input.latency_samples(controls));
    }
}

#[test]
fn ir_tail_comes_from_the_prepared_kernel() {
    let block = ChainBlock::tone(
        "ir",
        Tone::from_value(serde_json::json!({
            "id": 1,
            "title": "IR",
            "format": "ir",
            "models": [{"id": 1, "name": "IR"}]
        }))
        .expect("tone"),
        ModelId(1),
    );
    let model = BlockModel::Ir(
        compile_ir(
            &AudioDocument {
                sample_rate: 48_000.0,
                channels: vec![
                    (0..4800)
                        .map(|index| match index {
                            0 => 1.0,
                            4799 => 0.01,
                            _ => 0.0,
                        })
                        .collect(),
                ],
            },
            None,
        )
        .expect("IR preparation"),
    );
    let change = PreparedChainChange::new(
        1,
        spec(48_000.0),
        LaneSnapshot {
            blocks: vec![block],
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
    let chain = RtChain::prepare(change, 1).expect("real-time chain").0;
    assert!((chain.host_report().tail_seconds - 0.1).abs() < 1.0e-9);
}

#[test]
fn maximum_and_over_promised_callbacks_keep_their_atomic_contract() {
    let mut large = spec(48_000.0);
    large.max_host_frames = 8192;
    let mut chain = chain(large);
    let mut data = vec![0.0; 8192 * 2];
    chain
        .process(&mut PlanarBuffer::new(&mut data, 2, 8192).expect("large buffer"))
        .expect("maximum callback");
    let mut oversized = vec![0.25; 8193];
    assert!(
        chain
            .process(&mut PlanarBuffer::new(&mut oversized, 1, 8193).expect("oversized buffer"))
            .is_err()
    );
    assert!(oversized.iter().all(|sample| *sample == 0.25));
}

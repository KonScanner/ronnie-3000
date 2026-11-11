//! Input routing, dual mono, and mono-output fold tests.

use ronie_core::{AudioDocument, ChainBlock, InputMode, ModelId, Tone};
use ronie_dsp::{
    PlanarBuffer, PrepareSpec,
    block::BlockModel,
    ir::compile_ir,
    lane::LaneSnapshot,
    routing::RoutingControls,
    topology::{LaneSet, RtTopology},
};
use serde_json::json;

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

fn model() -> BlockModel {
    BlockModel::Ir(
        compile_ir(
            &AudioDocument {
                sample_rate: 48_000.0,
                channels: vec![vec![1.0]],
            },
            None,
        )
        .expect("identity IR"),
    )
}

fn block(id: &str) -> ChainBlock {
    ChainBlock::tone(
        id,
        Tone::from_value(
            json!({"id": 1, "title": "IR", "format": "ir", "models": [{"id": 100, "name": "IR"}]}),
        )
        .expect("tone"),
        ModelId(100),
    )
}

fn snapshot(blocks: Vec<ChainBlock>) -> LaneSnapshot {
    LaneSnapshot {
        models: blocks
            .iter()
            .map(|block| if block.is_insert { None } else { Some(model()) })
            .collect(),
        blocks,
    }
}

fn lanes(stereo: bool, input_mode: InputMode) -> LaneSet {
    LaneSet::new(
        snapshot(vec![block("left")]),
        snapshot(vec![block("right")]),
        spec(),
        stereo,
        input_mode,
    )
    .expect("lanes")
}

fn process(lanes: &mut LaneSet, topology: &mut RtTopology, controls: RoutingControls) -> Vec<f32> {
    const SETTLED_CALLBACKS: usize = 48_000 / 64;
    let mut data = vec![0.0; 128];
    data[..64].fill(0.5);
    data[64..].fill(0.25);
    let mut output = Vec::new();
    let original = data.clone();
    for callback in 0..SETTLED_CALLBACKS + 2 {
        data.copy_from_slice(&original);
        let mut buffer = PlanarBuffer::new(&mut data, 2, 64).expect("input");
        topology
            .process(&mut buffer, lanes, controls)
            .expect("routing");
        if callback >= SETTLED_CALLBACKS {
            let (left, right) = buffer.as_slice().split_at(64);
            for frame in 0..64 {
                output.push(left[frame]);
                output.push(right[frame]);
            }
        }
    }
    output
}

#[test]
fn stereo_sum_and_channel_selection_follow_reference_gain() {
    let pad = 10.0_f32.powf(-18.0 / 20.0);
    let mut lanes = self::lanes(false, InputMode::Stereo);
    let mut topology = RtTopology::new(spec());
    let output = process(&mut lanes, &mut topology, RoutingControls::default());
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.375 * pad).abs() < 1.0e-6)
    );

    let mut lanes = self::lanes(false, InputMode::Left);
    let mut topology = RtTopology::new(spec());
    let output = process(&mut lanes, &mut topology, RoutingControls::default());
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.5 * pad).abs() < 1.0e-6)
    );

    let mut lanes = self::lanes(false, InputMode::Right);
    let mut topology = RtTopology::new(spec());
    let output = process(&mut lanes, &mut topology, RoutingControls::default());
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.25 * pad).abs() < 1.0e-6)
    );
}

#[test]
fn dual_mono_is_independent_and_folds_when_it_cannot_engage() {
    let pad = 10.0_f32.powf(-18.0 / 20.0);
    let mut lanes = self::lanes(false, InputMode::DualMono);
    let mut topology = RtTopology::new(spec());
    let output = process(&mut lanes, &mut topology, RoutingControls::default());
    for frame in 0..output.len() / 2 {
        assert!((output[frame * 2] - output[frame * 2 + 1]).abs() > 1.0e-6);
        assert!((output[frame * 2] - 0.5 * pad).abs() < 1.0e-6);
        assert!((output[frame * 2 + 1] - 0.25 * pad).abs() < 1.0e-6);
    }

    let mut lanes = self::lanes(false, InputMode::DualMono);
    let mut topology = RtTopology::new(spec());
    let controls = RoutingControls {
        stereo_output: false,
        ..Default::default()
    };
    let output = process(&mut lanes, &mut topology, controls);
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.375 * pad).abs() < 1.0e-6)
    );

    let mut lanes = self::lanes(false, InputMode::DualMono);
    let mut topology = RtTopology::new(spec());
    let controls = RoutingControls {
        stereo_input: false,
        ..Default::default()
    };
    let output = process(&mut lanes, &mut topology, controls);
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.5 * pad).abs() < 1.0e-6)
    );
}

#[test]
fn mono_output_fold_applies_balance_solo_and_polarity() {
    let pad = 10.0_f32.powf(-18.0 / 20.0);
    let mut lanes = self::lanes(true, InputMode::Stereo);
    let mut topology = RtTopology::new(spec());
    let controls = RoutingControls {
        stereo_output: false,
        balance_left: 0.5,
        balance_right: 0.25,
        invert_left: true,
        ..Default::default()
    };
    let output = process(&mut lanes, &mut topology, controls);
    let expected = 0.5 * (-0.5 * 0.5 * pad + 0.25 * 0.25 * pad);
    assert!(
        output
            .iter()
            .all(|sample| (*sample - expected).abs() < 1.0e-6)
    );

    let mut lanes = self::lanes(true, InputMode::Stereo);
    let mut topology = RtTopology::new(spec());
    let controls = RoutingControls {
        stereo_output: false,
        solo_right: true,
        balance_right: 0.25,
        ..Default::default()
    };
    let output = process(&mut lanes, &mut topology, controls);
    let expected = 0.5 * 0.25 * 0.25 * pad;
    assert!(
        output
            .iter()
            .all(|sample| (*sample - expected).abs() < 1.0e-6)
    );
}

#[test]
fn short_mono_host_callback_preserves_the_independent_right_lane() {
    let mut lanes = LaneSet::new(
        snapshot(vec![block("left")]),
        snapshot(vec![]),
        spec(),
        true,
        InputMode::Stereo,
    )
    .expect("lanes");
    let mut topology = RtTopology::new(spec());
    let controls = RoutingControls {
        stereo_output: false,
        ..Default::default()
    };
    let mut data = vec![0.5; 31];
    for _ in 0..80 {
        data.fill(0.5);
        let mut buffer = PlanarBuffer::new(&mut data, 1, 31).expect("mono input");
        topology
            .process(&mut buffer, &mut lanes, controls)
            .expect("short mono callback");
    }
    let pad = 10.0_f32.powf(-18.0 / 20.0);
    let expected = 0.5 * (0.5 * pad + 0.5);
    assert!(
        data.iter()
            .all(|sample| (*sample - expected).abs() < 1.0e-6)
    );
}

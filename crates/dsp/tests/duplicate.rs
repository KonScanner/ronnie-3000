//! Duplication, clipboard snapshots, independent histories, and undo tests.

use ronie_core::{AudioDocument, BlockId, ChainBlock, ChainSide, InputMode, ModelId, Tone};
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
    let mut block = ChainBlock::tone(
        id,
        Tone::from_value(
            json!({"id": 1, "title": "IR", "format": "ir", "models": [{"id": 100, "name": "IR"}]}),
        )
        .expect("tone"),
        ModelId(100),
    );
    block.params.input_gain = 0.3;
    block.params.output_gain = 0.6;
    block.params.mix = 0.7;
    block.params.normalize = false;
    block
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

fn lanes(blocks: Vec<ChainBlock>, stereo: bool) -> LaneSet {
    LaneSet::new(
        snapshot(blocks),
        snapshot(vec![]),
        spec(),
        stereo,
        InputMode::Stereo,
    )
    .expect("lanes")
}

fn process(lanes: &mut LaneSet, topology: &mut RtTopology) -> Vec<f32> {
    const SETTLED_CALLBACKS: usize = 48_000 / 64;
    let mut data = vec![0.25; 128];
    let mut output = Vec::new();
    let original = data.clone();
    for callback in 0..SETTLED_CALLBACKS + 2 {
        data.copy_from_slice(&original);
        let mut buffer = PlanarBuffer::new(&mut data, 2, 64).expect("input");
        topology
            .process(&mut buffer, lanes, RoutingControls::default())
            .expect("process");
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
fn duplicate_carries_settings_and_independent_history() {
    let mut lanes = self::lanes(vec![block("source")], true);
    let mut topology = RtTopology::new(spec());
    let clone = topology
        .duplicate_block(&mut lanes, &BlockId("source".into()), ChainSide::Right, 0)
        .expect("duplicate");
    assert_ne!(clone, BlockId("source".into()));
    assert_eq!(lanes.left.blocks.len(), 5);
    assert_eq!(lanes.right.blocks.len(), 5);
    assert_eq!(lanes.right.blocks[0].params.mix, 0.7);
    assert!(!lanes.right.blocks[0].params.normalize);
    assert!(lanes.right.blocks[1..].iter().all(|block| block.is_insert));

    let left = process(&mut lanes, &mut topology);
    assert_eq!(left[0], left[1]);
    assert!(topology.remove_block(&mut lanes, &BlockId("source".into())));
    let output = process(&mut lanes, &mut topology);
    assert!(
        output
            .iter()
            .skip(1)
            .step_by(2)
            .any(|sample| sample.abs() > 1.0e-6),
        "the duplicate must keep processing after its source is removed",
    );
}

#[test]
fn clipboard_is_a_snapshot_and_undo_redo_preserves_identity() {
    let mut lanes = self::lanes(vec![block("source")], true);
    let mut topology = RtTopology::new(spec());
    assert!(topology.copy_block(&lanes, &BlockId("source".into())));
    assert!(topology.can_paste_block());
    assert!(topology.remove_block(&mut lanes, &BlockId("source".into())));
    let pasted = topology
        .paste_block(&mut lanes, ChainSide::Left, 0)
        .expect("paste");
    assert_eq!(lanes.left.blocks[0].params.mix, 0.7);
    assert_eq!(lanes.left.blocks[0].params.input_gain, 0.3);
    assert!(!lanes.left.blocks[0].params.normalize);

    assert!(topology.undo(&mut lanes));
    assert!(topology.can_paste_block());
    assert!(topology.redo(&mut lanes));
    assert_eq!(lanes.left.blocks[0].id, pasted);
}

#[test]
fn bad_sources_and_mono_right_lane_are_rejected() {
    let mut lanes = self::lanes(vec![block("source"), ChainBlock::insert("insert")], false);
    let mut topology = RtTopology::new(spec());
    assert!(
        topology
            .duplicate_block(&mut lanes, &BlockId("insert".into()), ChainSide::Left, 0)
            .is_none()
    );
    assert!(
        topology
            .duplicate_block(&mut lanes, &BlockId("stale".into()), ChainSide::Left, 0)
            .is_none()
    );
    assert!(
        topology
            .duplicate_block(&mut lanes, &BlockId("source".into()), ChainSide::Right, 0)
            .is_none()
    );
    assert!(!topology.copy_block(&lanes, &BlockId("insert".into())));
    assert!(
        topology
            .paste_block(&mut lanes, ChainSide::Right, 0)
            .is_none()
    );
}

#[test]
fn duplicate_ids_across_lanes_are_rejected_during_preparation() {
    let result = LaneSet::new(
        snapshot(vec![block("same")]),
        snapshot(vec![block("same")]),
        spec(),
        true,
        InputMode::Stereo,
    );
    assert!(result.is_err());
}

#[test]
fn reordering_to_the_current_order_does_not_dip_audio() {
    let mut lanes = self::lanes(vec![block("source")], true);
    let mut topology = RtTopology::new(spec());
    let before = process(&mut lanes, &mut topology);
    let order: Vec<_> = lanes
        .left
        .blocks
        .iter()
        .map(|block| block.id.clone())
        .collect();
    assert!(topology.reorder(&mut lanes, &order));

    let mut data = vec![0.25; 128];
    let mut buffer = PlanarBuffer::new(&mut data, 2, 64).expect("input");
    topology
        .process(&mut buffer, &mut lanes, RoutingControls::default())
        .expect("process");
    let (left, right) = buffer.as_slice().split_at(64);
    for frame in 0..64 {
        assert_eq!(before[before.len() - 128 + frame * 2], left[frame]);
        assert_eq!(before[before.len() - 127 + frame * 2], right[frame]);
    }
}

#[test]
fn invalid_restore_does_not_replace_the_active_lanes() {
    let mut lanes = self::lanes(vec![block("source")], true);
    let invalid = ronie_dsp::topology::LaneSetSnapshot {
        left: snapshot(vec![block("same")]),
        right: snapshot(vec![block("same")]),
        ..lanes.snapshot()
    };

    assert!(lanes.restore(invalid, spec()).is_err());
    assert_eq!(lanes.left.blocks[0].id, BlockId("source".into()));
}

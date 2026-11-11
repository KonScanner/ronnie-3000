//! Branch scheduling, invalidation, alignment, and structural history tests.

use ronie_core::{AudioDocument, BlockId, ChainBlock, ChainSide, InputMode, ModelId, Tone};
use ronie_dsp::{
    PlanarBuffer, PrepareSpec,
    block::BlockModel,
    ir::compile_ir,
    lane::LaneSnapshot,
    routing::{RoutingControls, Topology},
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

fn lanes(left: Vec<ChainBlock>, right: Vec<ChainBlock>, stereo: bool) -> LaneSet {
    LaneSet::new(
        snapshot(left),
        snapshot(right),
        spec(),
        stereo,
        InputMode::Stereo,
    )
    .expect("lanes")
}

fn process(lanes: &mut LaneSet, topology: &mut RtTopology) -> (Vec<f32>, Vec<f32>) {
    const SETTLED_CALLBACKS: usize = 48_000 / 64;
    let mut data = vec![0.25; 128];
    let mut left_output = Vec::new();
    let mut right_output = Vec::new();
    let original = data.clone();
    for callback in 0..SETTLED_CALLBACKS + 2 {
        data.copy_from_slice(&original);
        let mut buffer = PlanarBuffer::new(&mut data, 2, 64).expect("input");
        topology
            .process(&mut buffer, lanes, RoutingControls::default())
            .expect("branch");
        if callback >= SETTLED_CALLBACKS {
            let (left, right) = buffer.as_slice().split_at(64);
            left_output.extend_from_slice(left);
            right_output.extend_from_slice(right);
        }
    }
    (left_output, right_output)
}

#[test]
fn branch_routes_trunk_then_restores_independent_lanes() {
    let mut lanes = self::lanes(vec![block("left")], vec![], true);
    let mut topology = RtTopology::new(spec());
    assert!(topology.set_branch(&mut lanes, ChainSide::Left, BlockId("left".into())));
    assert_eq!(lanes.input_mode, InputMode::Left);
    let (left, right) = process(&mut lanes, &mut topology);
    for (frame, (left, right)) in left.iter().zip(&right).enumerate() {
        assert!(
            (left - right).abs() < 1.0e-6,
            "frame {frame}: left={left} right={right}",
        );
    }

    assert!(topology.clear_branch(&mut lanes));
    assert_eq!(lanes.topology(), Topology::Stereo);
    assert!(lanes.right.blocks.iter().all(|block| block.is_insert));
    let (left, right) = process(&mut lanes, &mut topology);
    assert!(
        left.iter()
            .zip(&right)
            .any(|(left, right)| (left - right).abs() > 1.0e-4),
        "cleared output: left={:?} right={:?}",
        &left[..4],
        &right[..4],
    );
}

#[test]
fn moving_removing_and_swapping_invalidate_or_follow_taps() {
    let mut lanes = self::lanes(vec![block("left")], vec![block("right")], true);
    let mut topology = RtTopology::new(spec());
    assert!(topology.set_branch(&mut lanes, ChainSide::Left, BlockId("left".into())));
    assert!(topology.remove_block(&mut lanes, &BlockId("left".into())));
    assert!(lanes.branch.is_none());

    assert!(topology.undo(&mut lanes));
    assert!(lanes.branch.is_some());
    assert!(topology.move_block_to_chain(&mut lanes, &BlockId("left".into()), ChainSide::Right, 0));
    assert!(lanes.branch.is_none());

    assert!(topology.set_branch(&mut lanes, ChainSide::Right, BlockId("left".into())));
    assert!(topology.swap_chains(&mut lanes));
    assert_eq!(lanes.branch.as_ref().expect("branch").side, ChainSide::Left);
}

#[test]
fn no_op_branch_edit_does_not_dip_audio() {
    let mut lanes = self::lanes(vec![block("left")], vec![block("right")], true);
    let mut topology = RtTopology::new(spec());
    assert!(topology.set_branch(&mut lanes, ChainSide::Left, BlockId("left".into())));
    let first = process(&mut lanes, &mut topology);
    assert!(topology.set_branch(&mut lanes, ChainSide::Left, BlockId("left".into())));
    let second = process(&mut lanes, &mut topology);
    assert_eq!(first, second);
}

#[test]
fn dormant_branch_reengages_and_insert_layout_stays_aligned() {
    let mut lanes = self::lanes(
        vec![block("left"), block("after")],
        vec![block("right")],
        true,
    );
    let mut topology = RtTopology::new(spec());
    assert!(topology.set_branch(&mut lanes, ChainSide::Left, BlockId("left".into())));
    assert_eq!(lanes.left.blocks.len(), 5);
    assert_eq!(lanes.right.blocks.len(), 4);
    assert_eq!(
        lanes.topology(),
        Topology::Branched {
            trunk: ChainSide::Left,
            after_block: BlockId("left".into()),
        }
    );

    topology.set_stereo_enabled(&mut lanes, false);
    assert_eq!(lanes.topology(), Topology::Mono);
    topology.set_stereo_enabled(&mut lanes, true);
    assert!(matches!(lanes.topology(), Topology::Branched { .. }));
    assert_eq!(lanes.input_mode, InputMode::Left);
}

#[test]
fn invalid_taps_are_rejected_without_an_edit_fade() {
    let mut lanes = self::lanes(
        vec![block("left"), ChainBlock::insert("insert")],
        vec![block("right")],
        true,
    );
    let mut topology = RtTopology::new(spec());
    let before = process(&mut lanes, &mut topology);
    assert!(!topology.set_branch(&mut lanes, ChainSide::Left, BlockId("insert".into())));
    assert!(!topology.set_branch(&mut lanes, ChainSide::Left, BlockId("stale".into())));
    assert!(!topology.clear_branch(&mut lanes));
    let after = process(&mut lanes, &mut topology);
    assert_eq!(before, after);
}

#[test]
fn post_tap_blocks_stay_on_the_trunk_and_use_their_own_settings() {
    let mut prefix = block("prefix");
    prefix.params.mix = 0.0;
    let tap = block("tap");
    let mut suffix = block("suffix");
    suffix.params.mix = 0.0;
    let branch = block("branch");
    let mut lanes = self::lanes(vec![prefix, tap, suffix], vec![branch], true);
    let mut topology = RtTopology::new(spec());
    assert!(topology.set_branch(&mut lanes, ChainSide::Left, BlockId("tap".into())));

    let (left, right) = process(&mut lanes, &mut topology);
    let pad = 10.0_f32.powf(-18.0 / 20.0);
    assert!(
        left.iter()
            .all(|sample| (*sample - 0.25 * pad).abs() < 1.0e-6)
    );
    assert!(
        right
            .iter()
            .all(|sample| (*sample - 0.25 * pad * pad).abs() < 1.0e-6)
    );
}

#[test]
fn short_mono_host_callback_keeps_the_branch_tap() {
    let mut lanes = self::lanes(vec![block("left")], vec![], true);
    let mut topology = RtTopology::new(spec());
    assert!(topology.set_branch(&mut lanes, ChainSide::Left, BlockId("left".into())));
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
            .expect("short mono branch");
    }
    let pad = 10.0_f32.powf(-18.0 / 20.0);
    assert!(
        data.iter()
            .all(|sample| (*sample - 0.5 * pad).abs() < 1.0e-6)
    );
}

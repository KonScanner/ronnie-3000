//! Prepared topology allocation and invalid-callback tests.
#![allow(unsafe_code)]

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

thread_local! {
    static ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DEALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: every method forwards the original contract and only adds counters.
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the layout is forwarded unchanged.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        DEALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the pointer and layout are the allocator's original pair.
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn counts(operation: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    operation();
    (
        ALLOCATIONS.with(std::cell::Cell::get),
        DEALLOCATIONS.with(std::cell::Cell::get),
    )
}

#[test]
fn prepared_branched_and_short_mono_callbacks_do_not_allocate() {
    let model = BlockModel::Ir(
        compile_ir(
            &AudioDocument {
                sample_rate: 48_000.0,
                channels: vec![vec![1.0]],
            },
            None,
        )
        .expect("identity IR"),
    );
    let block = ChainBlock::tone(
        "left",
        Tone::from_value(json!({
            "id": 1,
            "title": "IR",
            "format": "ir",
            "models": [{"id": 100, "name": "IR"}]
        }))
        .expect("tone"),
        ModelId(100),
    );
    let spec = PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    };
    let mut lanes = LaneSet::new(
        LaneSnapshot {
            blocks: vec![block],
            models: vec![Some(model)],
        },
        LaneSnapshot {
            blocks: vec![],
            models: vec![],
        },
        spec,
        true,
        InputMode::Stereo,
    )
    .expect("lanes");
    let mut topology = RtTopology::new(spec);
    assert!(topology.set_branch(&mut lanes, ChainSide::Left, BlockId("left".into())));
    let mut stereo = [0.25; 128];
    let mut mono = [0.5; 31];
    let mut oversized = [0.25; 65];
    let controls = RoutingControls {
        stereo_output: false,
        ..Default::default()
    };
    let mut stereo_result = Ok(());
    let mut mono_result = Ok(());
    let mut invalid_result = Ok(());

    let counts = counts(|| {
        stereo_result = topology.process(
            &mut PlanarBuffer::new(&mut stereo, 2, 64).expect("stereo input"),
            &mut lanes,
            RoutingControls::default(),
        );
        mono_result = topology.process(
            &mut PlanarBuffer::new(&mut mono, 1, 31).expect("mono input"),
            &mut lanes,
            controls,
        );
        invalid_result = topology.process(
            &mut PlanarBuffer::new(&mut oversized, 1, 65).expect("oversized input"),
            &mut lanes,
            RoutingControls::default(),
        );
    });

    assert!(stereo_result.is_ok(), "{stereo_result:?}");
    assert!(mono_result.is_ok(), "{mono_result:?}");
    assert!(invalid_result.is_err());
    assert_eq!(counts, (0, 0));
}

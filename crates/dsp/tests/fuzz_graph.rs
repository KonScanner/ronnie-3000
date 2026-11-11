//! Property tests for bounded graph layouts and deterministic processing.
#![allow(unsafe_code)]

use proptest::prelude::*;
use ronie_core::{BlockId, ChainSide, InputMode};
use ronie_dsp::{
    LatencyReport, PlanarBuffer, PrepareSpec, lane::LaneSnapshot, prepare::PreparedChainChange,
    rt_chain::RtChain,
};

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

fn empty() -> LaneSnapshot {
    LaneSnapshot {
        blocks: vec![],
        models: vec![],
    }
}

fn change(spec: PrepareSpec) -> PreparedChainChange {
    let mut change = PreparedChainChange::new(
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
    .expect("empty chain preparation");
    assert!(!change.topology.set_branch(
        &mut change.blocks,
        ChainSide::Left,
        BlockId("missing".into())
    ));
    change
}

fn chain(spec: PrepareSpec) -> RtChain {
    let mut chain = RtChain::prepare(change(spec), 1)
        .expect("real-time chain")
        .0;
    let mut controls = chain.controls();
    controls.input.gate_enabled = false;
    chain.set_controls(controls);
    chain
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn random_graph_callbacks_are_bounded_deterministic_and_allocation_free(
        rate in prop::sample::select(vec![44_100.0, 48_000.0, 96_000.0]),
        oversampling in prop::sample::select(vec![1_u8, 2, 4, 8]),
        frames in 1_usize..=65,
        channels in 1_usize..=2,
        bits in any::<u32>(),
    ) {
        let spec = PrepareSpec {
            host_sample_rate: rate,
            max_host_frames: 64,
            oversampling,
            input_mode: InputMode::Stereo,
            stereo_input: channels == 2,
            stereo_output: channels == 2,
        };
        let mut first = chain(spec);
        let mut second = chain(spec);
        let mut actual = vec![f32::from_bits(bits); frames * channels];
        let mut expected = actual.clone();
        let first_result;
        let second_result;
        ALLOCATIONS.with(|count| count.set(0));
        DEALLOCATIONS.with(|count| count.set(0));
        {
            first_result = first.process(
                &mut PlanarBuffer::new(&mut actual, channels, frames).expect("first buffer"),
            );
            second_result = second.process(
                &mut PlanarBuffer::new(&mut expected, channels, frames).expect("second buffer"),
            );
        }
        assert_eq!(first_result.is_ok(), frames <= 64);
        assert_eq!(second_result.is_ok(), frames <= 64);
        assert!(
            actual
                .iter()
                .zip(&expected)
                .all(|(left, right)| left.to_bits() == right.to_bits())
        );
        assert_eq!(ALLOCATIONS.with(std::cell::Cell::get), 0);
        assert_eq!(DEALLOCATIONS.with(std::cell::Cell::get), 0);
    }
}

#[test]
fn invalid_snapshot_is_rejected_without_panicking() {
    let spec = PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    };
    let invalid = LaneSnapshot {
        blocks: vec![],
        models: vec![None],
    };
    assert!(
        PreparedChainChange::new(
            1,
            spec,
            invalid,
            empty(),
            false,
            LatencyReport {
                samples: 0,
                tail_seconds: 0.0,
            },
        )
        .is_err()
    );
}

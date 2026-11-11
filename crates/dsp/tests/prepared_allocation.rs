//! Full prepared-chain callback allocation tests.
#![allow(unsafe_code)]

use ronie_core::InputMode;
use ronie_dsp::{
    LatencyReport, PlanarBuffer, PrepareSpec,
    lane::LaneSnapshot,
    prepare::PreparedChainChange,
    rt_chain::{ChainCommand, RtChain},
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

fn counts(operation: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    operation();
    (
        ALLOCATIONS.with(std::cell::Cell::get),
        DEALLOCATIONS.with(std::cell::Cell::get),
    )
}

fn change(generation: u64) -> PreparedChainChange {
    change_at_rate(generation, 48_000.0)
}

fn change_at_rate(generation: u64, sample_rate: f64) -> PreparedChainChange {
    let empty = || LaneSnapshot {
        blocks: vec![],
        models: vec![],
    };
    PreparedChainChange::new(
        generation,
        PrepareSpec {
            host_sample_rate: sample_rate,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        empty(),
        empty(),
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("empty chain preparation")
}

#[test]
fn prepared_processing_publication_and_invalid_calls_do_not_allocate() {
    let (mut chain, sender) = RtChain::prepare(change(1), 2).expect("real-time chain");
    sender
        .try_send(ChainCommand::Replace(change(2)))
        .expect("queued change");
    let mut stereo = [0.0; 128];
    let mut mono = [0.0; 31];
    let mut oversized = [0.0; 65];
    let mut stereo_result = Ok(());
    let mut mono_result = Ok(());
    let mut invalid_result = Ok(());

    let counts = counts(|| {
        stereo_result =
            chain.process(&mut PlanarBuffer::new(&mut stereo, 2, 64).expect("stereo buffer"));
        mono_result = chain.process(&mut PlanarBuffer::new(&mut mono, 1, 31).expect("mono buffer"));
        invalid_result =
            chain.process(&mut PlanarBuffer::new(&mut oversized, 1, 65).expect("oversized buffer"));
        chain.apply_pending();
    });

    assert!(stereo_result.is_ok(), "{stereo_result:?}");
    assert!(mono_result.is_ok(), "{mono_result:?}");
    assert!(invalid_result.is_err());
    assert_eq!(chain.active_generation(), 2);
    assert_eq!(counts, (0, 0));
}

#[test]
fn resampled_prepared_callbacks_do_not_allocate() {
    let (mut chain, _sender) =
        RtChain::prepare(change_at_rate(1, 44_100.0), 1).expect("real-time chain");
    let mut stereo = [0.0; 128];
    let mut mono = [0.0; 31];
    let mut stereo_result = Ok(());
    let mut mono_result = Ok(());

    let counts = counts(|| {
        stereo_result =
            chain.process(&mut PlanarBuffer::new(&mut stereo, 2, 64).expect("stereo buffer"));
        mono_result = chain.process(&mut PlanarBuffer::new(&mut mono, 1, 31).expect("mono buffer"));
    });

    assert!(stereo_result.is_ok(), "{stereo_result:?}");
    assert!(mono_result.is_ok(), "{mono_result:?}");
    assert_eq!(counts, (0, 0));
}

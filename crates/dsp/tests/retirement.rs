//! Retirement capacity, deferred destruction, and stopped-audio tests.
#![allow(unsafe_code)]

use ronie_core::InputMode;
use ronie_dsp::{
    LatencyReport, PrepareSpec,
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
    PreparedChainChange::new(
        generation,
        PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        LaneSnapshot {
            blocks: vec![],
            models: vec![],
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
    .expect("empty chain preparation")
}

#[test]
fn full_capacity_rejects_control_work_and_callback_never_deallocates() {
    let (mut chain, sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    sender
        .try_send(ChainCommand::Replace(change(2)))
        .expect("one queued change");
    assert!(sender.try_send(ChainCommand::Replace(change(3))).is_err());

    let callback_counts = counts(|| chain.apply_pending());
    assert_eq!(chain.active_generation(), 2);
    assert_eq!(callback_counts, (0, 0));

    let retired = chain.drain_retirements_off_thread();
    assert_eq!(retired, 1);
    assert!(DEALLOCATIONS.with(std::cell::Cell::get) > 0);
    sender
        .try_send(ChainCommand::Replace(change(3)))
        .expect("capacity was released");
}

#[test]
fn stopped_audio_retires_pending_work_without_publishing_it() {
    let (mut chain, sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    sender
        .try_send(ChainCommand::Replace(change(2)))
        .expect("queued change");
    assert_eq!(chain.drain_retirements_off_thread(), 1);
    assert_eq!(chain.active_generation(), 1);
    sender
        .try_send(ChainCommand::Replace(change(3)))
        .expect("stopped queue drained");
}

#[test]
fn the_control_endpoint_destroys_retired_chains_while_the_callback_keeps_running() {
    let (mut chain, sender) = RtChain::prepare(change(1), 1).expect("real-time chain");
    sender
        .try_send(ChainCommand::Replace(change(2)))
        .expect("queued change");
    let callback = std::thread::spawn(move || {
        let callback_counts = counts(|| chain.apply_pending());
        (chain, callback_counts)
    });
    let (mut chain, callback_counts) = callback.join().expect("callback thread");
    assert_eq!(callback_counts, (0, 0));
    assert_eq!(chain.active_generation(), 2);

    let (_, control_deallocations) = counts(|| assert_eq!(sender.drain_retired(), 1));
    assert!(control_deallocations > 0);
    sender
        .try_send(ChainCommand::Replace(change(3)))
        .expect("the drain released capacity without callback help");
    chain.apply_pending();
    assert_eq!(chain.active_generation(), 3);
    assert_eq!(sender.drain_retired(), 1);
}

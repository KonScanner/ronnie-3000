//! Caller and worker allocation checks for prepared scheduling.
#![allow(unsafe_code)]

use std::{
    num::{NonZeroU8, NonZeroUsize},
    sync::atomic::{AtomicUsize, Ordering},
};

use ronie_dsp::{
    PhaseEngine, compile_model,
    rt::{Job, RtWorkerPool},
};
use ronie_oracle::synthetic::synthetic_a2_document;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

// SAFETY: every method forwards the original GlobalAlloc contract to System.
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the layout is forwarded unchanged.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        DEALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the pointer and layout are the allocator's original pair.
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn work(_context: *mut ()) -> bool {
    true
}

#[test]
fn prepared_fork_join_does_not_allocate_on_caller_or_workers() {
    let mut pool = RtWorkerPool::prepare(4, 16).expect("worker pool");
    let mut contexts = [0_usize; 16];
    let mut jobs = contexts
        .iter_mut()
        .map(|context| {
            // SAFETY: every context remains live until the join completes.
            unsafe { Job::new(work, context) }
        })
        .collect::<Vec<_>>();
    let compiled = compile_model(&synthetic_a2_document(3).expect("synthetic document"))
        .expect("compiled model");
    let mut engine = PhaseEngine::new(
        &compiled,
        NonZeroU8::new(8).expect("factor"),
        NonZeroU8::new(2).expect("voices"),
    )
    .expect("phase engine");
    engine
        .prepare(48_000.0, NonZeroUsize::new(64).expect("frames"), false)
        .expect("phase preparation");
    let mut audio = [0.25; 128];
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    pool.fork_join(&mut jobs).expect("jobs complete");
    engine
        .process_planar_with_pool(&mut audio, 2, true, &pool)
        .expect("sixteen phase jobs");
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0);
    assert_eq!(DEALLOCATIONS.load(Ordering::SeqCst), 0);
    pool.release();
}

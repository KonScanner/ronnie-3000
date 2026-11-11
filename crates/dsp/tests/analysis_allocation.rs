//! Prepared tuner and automatic measurement allocation tests.
#![allow(unsafe_code)]

use ronie_dsp::{
    PlanarBuffer,
    analysis::{AutoBalance, AutoOffset, AutoOffsetState, TunerDetector},
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

#[test]
fn prepared_analysis_callbacks_do_not_allocate() {
    const BLOCK: usize = 512;

    let mut tuner = TunerDetector::new(48_000.0).expect("tuner preparation");
    tuner.set_enabled(true);
    let input = [0.2; BLOCK];

    let mut balance = AutoBalance::new(48_000.0);
    balance.start();
    let right = [0.1; BLOCK];

    let mut offset = AutoOffset::new(48_000.0).expect("auto-offset preparation");
    offset.arm();
    let mut probe = [0.0; BLOCK];
    let mut host = [0.2; BLOCK * 2];
    let mut output_result = Ok(());
    let mut rendered = false;

    let counts = counts(|| {
        tuner.push_samples(&input);
        balance.process(&input, &right);
        while matches!(
            offset.state(),
            AutoOffsetState::FadeOut | AutoOffsetState::Probing | AutoOffsetState::Tail
        ) {
            rendered |= offset.render_probe_input(&mut probe);
            offset.capture_chain_outputs(&probe, &probe);
            output_result = offset.apply_output_gain(
                &mut PlanarBuffer::new(&mut host, 2, BLOCK).expect("host buffer"),
            );
        }
    });

    output_result.expect("auto-offset output processing");
    assert!(rendered);
    assert_eq!(offset.state(), AutoOffsetState::Captured);
    assert_eq!(counts, (0, 0));
    std::hint::black_box((tuner, balance, offset));
}

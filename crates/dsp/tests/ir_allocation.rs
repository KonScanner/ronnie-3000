//! Prepared IR allocation and failed-preparation atomicity tests.
#![allow(unsafe_code)]

use ronie_core::AudioDocument;
use ronie_dsp::{
    PlanarBuffer,
    ir::{PreparedIr, compile_ir},
    oversampling::ChainOversampler,
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
fn prepared_processing_and_invalid_calls_do_not_allocate() {
    let document = AudioDocument {
        sample_rate: 48_000.0,
        channels: vec![vec![1.0, 0.5, -0.25, 0.1]],
    };
    let compiled = compile_ir(&document, None).expect("compiled IR");
    let mut prepared = PreparedIr::new(compiled.clone(), false);
    prepared.prepare(8).expect("preparation");
    let mut control = PreparedIr::new(compiled, false);
    control.prepare(8).expect("control preparation");
    let mut valid = [0.25, -0.5, 0.75, 1.0];
    let mut invalid = [0.25; 3];
    let mut expected = valid;
    let mut valid_result = Ok(());
    let mut invalid_result = Ok(());
    let mut reprepare_result = Ok(());
    let mut valid_oversampler = ChainOversampler::new(1, 8).expect("oversampling");
    let mut invalid_oversampler = ChainOversampler::new(2, 8).expect("oversampling");
    let mut control_oversampler = ChainOversampler::new(1, 8).expect("oversampling");

    let counts = counts(|| {
        valid_result = prepared.process_planar(
            &mut PlanarBuffer::new(&mut valid, 1, 4).expect("valid"),
            &mut valid_oversampler,
        );
        invalid_result = prepared.process_planar(
            &mut PlanarBuffer::new(&mut invalid, 1, 3).expect("invalid"),
            &mut invalid_oversampler,
        );
        reprepare_result = prepared.prepare(0);
    });
    control
        .process_planar(
            &mut PlanarBuffer::new(&mut expected, 1, 4).expect("control"),
            &mut control_oversampler,
        )
        .expect("control process");

    assert!(valid_result.is_ok(), "{valid_result:?}");
    assert!(invalid_result.is_err());
    assert!(reprepare_result.is_err());
    assert_eq!(valid, expected);
    assert_eq!(counts, (0, 0));
}

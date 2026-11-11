//! Scalar A2 streaming and allocation tests.
#![allow(unsafe_code)]

use ronie_core::{DspError, NamDocument};
use ronie_dsp::{A2Engine, A2Model, CompiledA2, compile_a2};
use ronie_oracle::synthetic::synthetic_a2_document;
use serde_json as _;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    num::NonZeroUsize,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

static TEST_LOCK: Mutex<()> = Mutex::new(());
// Per thread, because the test harness also allocates on its own threads.
thread_local! {
    static ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DEALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: every method forwards the original GlobalAlloc contract to System;
// the counters are per thread and do not alter pointer ownership or layout.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the caller supplies a valid layout, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        DEALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the pointer and layout are the allocator's original pair.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn lock() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn assert_no_allocations(operation: impl FnOnce()) {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    operation();
    assert_eq!(ALLOCATIONS.with(std::cell::Cell::get), 0);
    assert_eq!(DEALLOCATIONS.with(std::cell::Cell::get), 0);
}

fn zero_document() -> NamDocument {
    let mut document = synthetic_a2_document(3).expect("valid synthetic document");
    document.weights.fill(0.0);
    let last = document.weights.len() - 1;
    document.weights[last] = 1.0;
    document
}

fn impulse_document() -> NamDocument {
    const KERNELS: [usize; 23] = [
        6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 15, 15, 6, 6, 6, 6, 6, 6, 6,
    ];
    let mut document = zero_document();
    document.weights[0] = 1.0;
    let mut cursor = 3;
    for kernel in KERNELS {
        document.weights[cursor + kernel - 1] = 1.0;
        cursor += kernel * 9 + 6;
        document.weights[cursor] = 1.0;
        cursor += 12;
    }
    let head_start = cursor;
    document.weights[head_start + 15] = 1.0;
    document.weights[head_start + 14] = 0.5;
    document.weights[head_start + 16 * 3 + 1] = 1.0;
    document
}

fn compiled(document: &NamDocument) -> Arc<CompiledA2> {
    Arc::new(compile_a2(document).expect("valid compiled model"))
}

fn prepared(document: &NamDocument, max_frames: usize) -> A2Model {
    let mut model = A2Model::new(compiled(document)).expect("valid model");
    model
        .prepare(
            48_000.0,
            NonZeroUsize::new(max_frames).expect("nonzero block size"),
            false,
        )
        .expect("valid preparation");
    model
}

#[test]
fn emits_zero_for_zero_weight_model() {
    let _guard = lock();
    let mut model = prepared(&zero_document(), 64);
    let input = [0.0, 1.0, -1.0];
    let mut output = [1.0; 3];

    model.process(&input, &mut output).expect("valid process");

    assert_eq!(output, [0.0; 3]);
    assert_eq!(model.max_frames(), 64);
}

#[test]
fn matches_hand_computed_impulse_output_bits() {
    let _guard = lock();
    let mut model = prepared(&impulse_document(), 64);
    let input = [1.0, 0.0, 0.0, 0.0];
    let mut output = [0.0; 4];

    model.process(&input, &mut output).expect("valid process");

    assert_eq!(output, [8_388_607.0, 4_194_303.5, 0.0, 0.0]);
    assert_eq!(output[0].to_bits(), 8_388_607.0_f64.to_bits());
}

#[test]
fn output_is_bit_identical_across_call_slicing() {
    let _guard = lock();
    let document = synthetic_a2_document(3).expect("valid synthetic document");
    let input: Vec<f64> = (0..2048)
        .map(|index| ((index * 37 % 257) as f64 - 128.0) / 128.0)
        .collect();

    let mut whole = prepared(&document, 2048);
    let mut expected = vec![0.0; input.len()];
    whole.process(&input, &mut expected).expect("whole render");

    for block_size in [1, 31, 64, 65, 256] {
        let mut model = prepared(&document, block_size);
        let mut actual = vec![0.0; input.len()];
        for (start, chunk) in input.chunks(block_size).enumerate() {
            let begin = start * block_size;
            let end = begin + chunk.len();
            model
                .process(chunk, &mut actual[begin..end])
                .expect("chunked render");
        }
        let expected_bits: Vec<u64> = expected.iter().map(|value| value.to_bits()).collect();
        let actual_bits: Vec<u64> = actual.iter().map(|value| value.to_bits()).collect();
        assert_eq!(actual_bits, expected_bits, "block size {block_size}");
    }
}

#[test]
fn propagates_nonfinite_inputs_and_preserves_denormal_zero_behavior() {
    let _guard = lock();
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut model = prepared(&zero_document(), 64);
        let mut output = [0.0];
        model
            .process(&[value], &mut output)
            .expect("nonfinite process");
        assert!(output[0].is_nan(), "input {value}");
    }

    let mut model = prepared(&zero_document(), 64);
    let mut output = [1.0];
    model
        .process(&[f64::from_bits(1)], &mut output)
        .expect("denormal process");
    assert_eq!(output[0].to_bits(), 0.0_f64.to_bits());
}

#[test]
fn rejects_invalid_calls_without_allocation_or_state_mutation() {
    let _guard = lock();
    let document = impulse_document();
    let model = compiled(&document);
    let mut engine = A2Engine::<3>::new(Arc::clone(&model)).expect("valid engine");
    let mut twin = A2Engine::<3>::new(model).expect("valid twin");
    let input = [1.0, 0.0, 0.0, 0.0];
    let mut output = [0.0; 4];

    assert_no_allocations(|| {
        assert!(matches!(
            engine.process(&input, &mut output),
            Err(DspError::NotPrepared)
        ));
    });

    for current in [&mut engine, &mut twin] {
        current
            .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
            .expect("valid preparation");
        current
            .process(&input, &mut output)
            .expect("initial render");
    }

    let oversized_input = vec![0.0; 65];
    let mut oversized_output = vec![42.0; 65];
    assert_no_allocations(|| {
        assert!(matches!(
            engine.process(&oversized_input, &mut oversized_output),
            Err(DspError::BlockTooLarge {
                max: 64,
                actual: 65
            })
        ));
    });
    assert!(oversized_output.iter().all(|value| *value == 42.0));

    let mut unequal_output = [9.0; 3];
    assert_no_allocations(|| {
        assert!(matches!(
            engine.process(&input, &mut unequal_output),
            Err(DspError::InvalidBufferLength {
                input: 4,
                output: 3
            })
        ));
    });
    assert_eq!(unequal_output, [9.0; 3]);

    let next_input = [0.0; 4];
    let mut actual = [0.0; 4];
    let mut expected = [0.0; 4];
    assert_no_allocations(|| {
        engine
            .process(&next_input, &mut actual)
            .expect("post-error render");
        twin.process(&next_input, &mut expected)
            .expect("twin render");
    });
    assert_eq!(
        actual.map(|value| value.to_bits()),
        expected.map(|value| value.to_bits())
    );
}

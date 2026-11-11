//! Allocation and randomized streaming tests.
#![allow(unsafe_code)]

use proptest::prelude::*;
use ronie_core::{NamArchitecture, NamDocument, NamMetadata, VersionSupport};
use ronie_dsp::{A2Model, ModelEngine, PhaseEngine, compile_model};
use ronie_oracle::synthetic::synthetic_a2_document;
use serde_json::{Value, json};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    num::{NonZeroU8, NonZeroUsize},
    sync::{Mutex, MutexGuard, PoisonError},
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

fn child_value(channels: usize) -> Value {
    let document = synthetic_a2_document(channels).expect("synthetic child");
    json!({
        "version": document.version,
        "architecture": "WaveNet",
        "config": document.config,
        "weights": document.weights,
        "sample_rate": 48_000.0,
        "metadata": {}
    })
}

fn container_with_thresholds(thresholds: [f64; 3]) -> NamDocument {
    let children = [child_value(3), child_value(8), child_value(8)];
    NamDocument {
        version: "0.7.0".to_owned(),
        support: VersionSupport::Yes,
        architecture: NamArchitecture::SlimmableContainer,
        sample_rate: Some(48_000.0),
        metadata: NamMetadata::default(),
        config: json!({"submodels": [
            {"max_value": thresholds[0], "model": children[0]},
            {"max_value": thresholds[1], "model": children[1]},
            {"max_value": thresholds[2], "model": children[2]}
        ]}),
        weights: vec![],
    }
}

#[test]
fn prepared_single_callbacks_do_not_allocate() {
    let _guard = lock();
    let document = synthetic_a2_document(3).expect("synthetic document");
    let mut engine = A2Model::new(std::sync::Arc::new(
        ronie_dsp::compile_a2(&document).expect("compiled model"),
    ))
    .expect("model engine");
    engine
        .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
        .expect("preparation");
    let input = [0.25; 64];
    let mut output = [0.0; 64];
    let mut unequal = [0.0; 63];
    let oversized_input = [0.0; 65];
    let mut oversized_output = [0.0; 65];

    assert_no_allocations(|| {
        engine.process(&input, &mut output).expect("valid process");
        assert!(engine.process(&input, &mut unequal).is_err());
        assert!(
            engine
                .process(&oversized_input, &mut oversized_output)
                .is_err()
        );
    });
}

#[test]
fn prepared_phase_callbacks_do_not_allocate() {
    let _guard = lock();
    let document = synthetic_a2_document(3).expect("synthetic document");
    let compiled = compile_model(&document).expect("compiled model");
    let mut engine = PhaseEngine::new(
        &compiled,
        NonZeroU8::new(2).expect("factor"),
        NonZeroU8::new(2).expect("voices"),
    )
    .expect("phase engine");
    engine
        .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
        .expect("preparation");
    let mut valid = [0.25_f32; 128];
    let mut invalid_channels = [0.0_f32; 6];
    let mut invalid_length = [0.0_f32; 3];
    let mut oversized = [0.0_f32; 65];

    assert_no_allocations(|| {
        engine
            .process_planar(&mut valid, 2, true)
            .expect("valid planar process");
        assert!(
            engine
                .process_planar(&mut invalid_channels, 3, false)
                .is_err()
        );
        assert!(
            engine
                .process_planar(&mut invalid_length, 2, false)
                .is_err()
        );
        assert!(engine.process_planar(&mut oversized, 1, false).is_err());
    });
}

#[test]
fn prepared_tier_changes_do_not_allocate() {
    let _guard = lock();
    let document = container_with_thresholds([0.5, 0.75, 1.0]);
    let compiled = compile_model(&document).expect("compiled container");
    let mut engine = ModelEngine::new(&compiled).expect("container engine");
    engine
        .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
        .expect("preparation");

    assert_no_allocations(|| {
        assert!(engine.set_slimmable_size(0.0).expect("lite tier"));
        assert!(engine.set_slimmable_size(1.0).expect("full tier"));
        assert!(engine.set_slimmable_size(0.0).expect("lite tier again"));
        assert!(!engine.set_slimmable_size(0.0).expect("unchanged tier"));
    });

    let mut unprepared = ModelEngine::new(&compiled).expect("unprepared engine");
    assert_no_allocations(|| {
        assert!(unprepared.set_slimmable_size(0.0).is_err());
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn random_block_slicing_matches_whole_render(
        block_sizes in prop::collection::vec(1_usize..=65, 1..12),
        seed in any::<u64>()
    ) {
        let _guard = lock();
        let document = synthetic_a2_document(3).expect("synthetic document");
        let compiled = std::sync::Arc::new(ronie_dsp::compile_a2(&document).expect("compiled model"));
        let input: Vec<f64> = (0..257)
            .map(|index| ((index * 37 % 257) as f64 - 128.0) / 128.0 + seed as f64 * 0.0)
            .collect();
        let mut whole = A2Model::new(std::sync::Arc::clone(&compiled)).expect("whole engine");
        whole.prepare(48_000.0, NonZeroUsize::new(257).expect("nonzero"), false).expect("whole preparation");
        let mut expected = vec![0.0; input.len()];
        whole.process(&input, &mut expected).expect("whole render");

        let mut chunked = A2Model::new(compiled).expect("chunked engine");
        chunked.prepare(48_000.0, NonZeroUsize::new(65).expect("nonzero"), false).expect("chunked preparation");
        let mut actual = vec![0.0; input.len()];
        let mut start = 0;
        for size in block_sizes {
            if start >= input.len() {
                break;
            }
            let end = (start + size).min(input.len());
            chunked.process(&input[start..end], &mut actual[start..end]).expect("chunked render");
            start = end;
        }
        while start < input.len() {
            let end = (start + 65).min(input.len());
            chunked.process(&input[start..end], &mut actual[start..end]).expect("remaining render");
            start = end;
        }
        let expected: Vec<u64> = expected.iter().map(|value| value.to_bits()).collect();
        let actual: Vec<u64> = actual.iter().map(|value| value.to_bits()).collect();
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn invalid_tiers_fail_without_panics(thresholds in any::<[f64; 3]>(), size in any::<f64>()) {
        let _guard = lock();
        let document = container_with_thresholds(thresholds);
        if let Ok(compiled) = compile_model(&document) {
            let mut engine = ModelEngine::new(&compiled).expect("valid compiled container");
            engine
                .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
                .expect("preparation");
            let _ = engine.set_slimmable_size(size);
        }
    }
}

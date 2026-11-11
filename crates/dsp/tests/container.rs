//! Slimmable container lifecycle tests.
#![allow(unsafe_code)]

use ronie_core::{DspError, NamArchitecture, NamDocument, NamMetadata, VersionSupport};
use ronie_dsp::{CompiledModel, ModelEngine, compile_model};
use ronie_oracle::synthetic::synthetic_a2_document;
use serde_json::{Value, json};
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

fn child_value(document: &NamDocument) -> Value {
    let mut value = json!({
        "version": document.version,
        "architecture": match document.architecture {
            NamArchitecture::WaveNet => "WaveNet",
            NamArchitecture::SlimmableContainer => "SlimmableContainer",
        },
        "config": document.config,
        "weights": document.weights,
        "metadata": {}
    });
    if let Some(sample_rate) = document.sample_rate {
        value["sample_rate"] = json!(sample_rate);
    }
    value
}

fn child(channels: usize, sample_rate: Option<f64>) -> Value {
    let mut document = synthetic_a2_document(channels).expect("valid synthetic child");
    document.sample_rate = sample_rate;
    child_value(&document)
}

fn container_document() -> NamDocument {
    container_document_with_children(vec![
        json!({"max_value": 0.5, "model": child(3, Some(48_000.0))}),
        json!({"max_value": 1.0, "model": child(8, Some(48_000.0))}),
    ])
}

fn container_document_with_children(submodels: Vec<Value>) -> NamDocument {
    NamDocument {
        version: "0.7.0".to_owned(),
        support: VersionSupport::Yes,
        architecture: NamArchitecture::SlimmableContainer,
        sample_rate: Some(48_000.0),
        metadata: NamMetadata::default(),
        config: json!({"submodels": submodels}),
        weights: vec![],
    }
}

fn prepared_engine(compiled: &CompiledModel, prewarm: bool) -> ModelEngine {
    let mut engine = ModelEngine::new(compiled).expect("valid model engine");
    engine
        .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), prewarm)
        .expect("valid preparation");
    engine
}

#[test]
fn compiles_container_and_enforces_tier_boundaries() {
    let _guard = lock();
    let document = container_document();
    let compiled = compile_model(&document).expect("valid container");
    let CompiledModel::Slimmable(container) = &compiled else {
        panic!("expected slimmable container");
    };
    assert_eq!(container.submodels().len(), 2);
    assert_eq!(container.expected_sample_rate(), 48_000.0);

    let mut engine = prepared_engine(&compiled, false);
    let ModelEngine::Slimmable(container) = &engine else {
        panic!("expected slimmable engine");
    };
    assert_eq!(container.active_index(), 1);
    assert_eq!(container.prepared_tiers(), 2);

    assert!(engine.set_slimmable_size(0.0).expect("valid switch"));
    assert_eq!(engine.active_index(), 0);
    assert!(!engine.set_slimmable_size(0.499_999).expect("same tier"));
    assert_eq!(engine.active_index(), 0);
    assert!(engine.set_slimmable_size(0.5).expect("valid switch"));
    assert_eq!(engine.active_index(), 1);
    assert!(!engine.set_slimmable_size(1.0).expect("same tier"));
}

#[test]
fn rejects_invalid_container_structure_and_sample_rates() {
    let _guard = lock();
    for document in [
        container_document_with_children(vec![]),
        container_document_with_children(vec![
            json!({"max_value": 0.7, "model": child(3, Some(48_000.0))}),
            json!({"max_value": 0.5, "model": child(8, Some(48_000.0))}),
            json!({"max_value": 1.0, "model": child(8, Some(48_000.0))}),
        ]),
        container_document_with_children(vec![
            json!({"max_value": 0.5, "model": child(3, Some(48_000.0))}),
            json!({"max_value": 0.7, "model": child(8, Some(48_000.0))}),
        ]),
        container_document_with_children(vec![
            json!({"max_value": 0.5, "model": child(3, Some(44_100.0))}),
            json!({"max_value": 1.0, "model": child(8, Some(48_000.0))}),
        ]),
    ] {
        assert!(compile_model(&document).is_err());
    }

    let mut unsupported = container_document();
    unsupported.config["submodels"][0]["model"]["architecture"] = json!("SlimmableContainer");
    assert!(matches!(
        compile_model(&unsupported),
        Err(DspError::InvalidContainer)
    ));

    let unknown = container_document_with_children(vec![
        json!({"max_value": 0.5, "model": child(3, None)}),
        json!({"max_value": 1.0, "model": child(8, None)}),
    ]);
    let mut unknown = unknown;
    unknown.sample_rate = None;
    let compiled = compile_model(&unknown).expect("unknown rates should be accepted");
    let CompiledModel::Slimmable(container) = &compiled else {
        panic!("expected slimmable container");
    };
    assert_eq!(container.expected_sample_rate(), -1.0);
}

#[test]
fn shares_compiled_children_and_keeps_independent_histories() {
    let _guard = lock();
    let document = container_document();
    let compiled = compile_model(&document).expect("valid container");
    let CompiledModel::Slimmable(container) = &compiled else {
        panic!("expected slimmable container");
    };
    let shared = Arc::clone(&container.submodels()[0].model);
    assert_eq!(Arc::strong_count(&shared), 2);

    let mut first = prepared_engine(&compiled, false);
    let mut second = prepared_engine(&compiled, false);
    assert_eq!(Arc::strong_count(&shared), 4);

    first.set_slimmable_size(0.0).expect("select lite");
    second.set_slimmable_size(0.0).expect("select lite");
    let input = [1.0, 0.0, 0.0, 0.0];
    let mut first_initial = [0.0; 4];
    let mut second_initial = [0.0; 4];
    first
        .process(&input, &mut first_initial)
        .expect("first render");
    second
        .process(&input, &mut second_initial)
        .expect("second render");
    assert_eq!(
        first_initial.map(|value| value.to_bits()),
        second_initial.map(|value| value.to_bits())
    );

    let mut first_later = [0.0; 4];
    first
        .process(&input, &mut first_later)
        .expect("later render");
    assert_ne!(
        first_initial.map(|value| value.to_bits()),
        first_later.map(|value| value.to_bits())
    );

    let mut second_independent = [0.0; 4];
    second
        .process(&input, &mut second_independent)
        .expect("independent render");
    assert_eq!(
        second_independent.map(|value| value.to_bits()),
        first_later.map(|value| value.to_bits())
    );
}

#[test]
fn tier_switch_is_atomic_and_allocation_free() {
    let _guard = lock();
    let document = container_document();
    let compiled = compile_model(&document).expect("valid container");
    let mut engine = prepared_engine(&compiled, false);
    let mut twin = prepared_engine(&compiled, false);
    let input = [1.0, 0.0, 0.0, 0.0];
    let mut initial = [0.0; 4];
    let mut twin_initial = [0.0; 4];
    engine
        .process(&input, &mut initial)
        .expect("initial render");
    twin.process(&input, &mut twin_initial)
        .expect("twin render");

    assert_no_allocations(|| {
        assert!(!engine.set_slimmable_size(1.0).expect("unchanged tier"));
    });
    let mut actual = [0.0; 4];
    let mut expected = [0.0; 4];
    engine
        .process(&input, &mut actual)
        .expect("continued render");
    twin.process(&input, &mut expected)
        .expect("twin continued render");
    assert_eq!(
        actual.map(|value| value.to_bits()),
        expected.map(|value| value.to_bits())
    );

    assert_no_allocations(|| {
        assert!(engine.set_slimmable_size(0.0).expect("changed tier"));
    });
    assert_eq!(engine.active_index(), 0);

    let mut fresh = prepared_engine(&compiled, false);
    fresh.set_slimmable_size(0.0).expect("fresh lite tier");
    let mut switched_output = [0.0; 4];
    let mut fresh_output = [0.0; 4];
    engine
        .process(&input, &mut switched_output)
        .expect("switched render");
    fresh
        .process(&input, &mut fresh_output)
        .expect("fresh render");
    assert_eq!(
        switched_output.map(|value| value.to_bits()),
        fresh_output.map(|value| value.to_bits())
    );

    let mut unprepared = ModelEngine::new(&compiled).expect("valid unprepared engine");
    assert!(matches!(
        unprepared.set_slimmable_size(0.0),
        Err(DspError::NotPrepared)
    ));
    assert_eq!(unprepared.active_index(), 1);
}

#[test]
fn single_model_rejects_size_change_before_preparation() {
    let _guard = lock();
    let document = ronie_oracle::synthetic::synthetic_a2_document(3).expect("synthetic document");
    let compiled = compile_model(&document).expect("compiled single model");
    let mut engine = ModelEngine::new(&compiled).expect("single engine");

    assert!(matches!(
        engine.set_slimmable_size(0.0),
        Err(DspError::NotPrepared)
    ));
    assert_eq!(engine.active_index(), 0);
}

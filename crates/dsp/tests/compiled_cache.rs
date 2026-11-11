//! Immutable compiled-model cache and independent-engine tests.

use std::{num::NonZeroUsize, sync::Arc};

use ronie_core::{
    ContentId, ContentPreparationKey, DspError, InstructionSet, ModelBytes, PreparationKey,
};
use ronie_dsp::{CompiledModelCache, ModelEngine};
use ronie_oracle::synthetic::synthetic_a2_document;

fn bytes() -> ModelBytes {
    let document = synthetic_a2_document(3).expect("synthetic document");
    let value = serde_json::json!({
        "version": document.version,
        "architecture": "WaveNet",
        "config": document.config,
        "weights": document.weights,
        "sample_rate": document.sample_rate,
        "metadata": {}
    });
    ModelBytes::from(serde_json::to_vec(&value).expect("model JSON"))
}

fn key(bytes: &ModelBytes) -> ContentPreparationKey {
    ContentPreparationKey {
        content: ContentId::from_bytes(bytes),
        preparation: PreparationKey {
            expected_sample_rate_bits: 48_000_f64.to_bits(),
            oversampling: 1,
            voices: 1,
            instruction_set: InstructionSet::Scalar,
        },
    }
}

#[test]
fn engines_share_compiled_weights_but_not_mutable_histories() {
    let bytes = bytes();
    let cache = CompiledModelCache::new(4);
    let key = key(&bytes);
    let first = cache.get_or_compile(&key, &bytes).expect("first compile");
    let second = cache.get_or_compile(&key, &bytes).expect("cache hit");
    assert!(Arc::ptr_eq(&first, &second));

    let mut left = ModelEngine::new(&first).expect("left engine");
    let mut right = ModelEngine::new(&second).expect("right engine");
    for engine in [&mut left, &mut right] {
        engine
            .prepare(48_000.0, NonZeroUsize::new(64).expect("block"), false)
            .expect("prepare");
    }
    let input: Vec<_> = (0..64)
        .map(|index| if index == 0 { 1.0 } else { 0.0 })
        .collect();
    let mut left_output = vec![0.0; 64];
    let mut right_output = vec![0.0; 64];
    left.process(&input, &mut left_output).expect("left render");
    right
        .process(&input, &mut right_output)
        .expect("right render");

    assert_eq!(left_output, right_output);
}

#[test]
fn preparation_key_changes_do_not_share_a_compiled_entry() {
    let bytes = bytes();
    let cache = CompiledModelCache::new(4);
    let first_key = key(&bytes);
    let mut second_key = key(&bytes);
    second_key.preparation.oversampling = 8;

    let first = cache.get_or_compile(&first_key, &bytes).expect("first");
    let second = cache.get_or_compile(&second_key, &bytes).expect("second");

    assert!(!Arc::ptr_eq(&first, &second));
}

#[test]
fn failed_compilation_can_be_retried_without_poisoning_good_entries() {
    let invalid = ModelBytes::from(
        br#"{"version":"0.7.0","architecture":"WaveNet","config":{},"weights":[]}"#.as_slice(),
    );
    let invalid_key = key(&invalid);
    let cache = CompiledModelCache::new(4);

    for _ in 0..2 {
        assert!(cache.get_or_compile(&invalid_key, &invalid).is_err());
    }

    let valid = bytes();
    let valid_key = key(&valid);
    let good = cache
        .get_or_compile(&valid_key, &valid)
        .expect("good compile");
    let mismatch = cache.get_or_compile(&valid_key, &invalid);
    assert!(matches!(mismatch, Err(DspError::ContentMismatch)));
    let cached = cache
        .get_or_compile(&valid_key, &valid)
        .expect("good entry remains");
    assert!(Arc::ptr_eq(&good, &cached));
}

#[test]
fn bounded_eviction_never_invalidates_live_compiled_users() {
    let bytes = bytes();
    let cache = CompiledModelCache::new(1);
    let first_key = key(&bytes);
    let first = cache.get_or_compile(&first_key, &bytes).expect("first");
    let mut second_key = key(&bytes);
    second_key.preparation.voices = 2;
    cache
        .get_or_compile(&second_key, &bytes)
        .expect("second evicts first");

    let mut engine = ModelEngine::new(&first).expect("live compiled model remains valid");
    engine
        .prepare(48_000.0, NonZeroUsize::new(64).expect("block"), false)
        .expect("prepare");
}

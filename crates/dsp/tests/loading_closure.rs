//! Official local loading, model compilation, and immutable-cache closure verification.

use std::sync::Arc;

use ronie_core::{
    ContentId, ContentPreparationKey, InstructionSet, ModelBytes, ModelId, PreparationKey,
    load_local_path,
};
use ronie_dsp::{CompiledModelCache, validate_a2_document};

#[test]
#[ignore = "requires the official source fixture directory"]
fn official_a2_loads_compiles_and_deduplicates_immutable_weights() {
    let Some(root) = std::env::var_os("RONIE_REFERENCE_ROOT") else {
        return;
    };
    let official = std::fs::read(std::path::PathBuf::from(root).join("test/files/a2-amp-test.nam"))
        .expect("official fixture");
    let temp = tempfile::tempdir().expect("temp dir");
    let input = temp.path().join("input");
    std::fs::create_dir(&input).expect("input directory");
    std::fs::write(input.join("amp.nam"), &official).expect("local fixture");
    let stash = ronie_core::LocalStash::new(temp.path().join("stash")).expect("stash");
    let loaded = load_local_path(&input, &stash, &|document| validate_a2_document(document))
        .expect("local load and A2 gate");
    let bytes = std::fs::read(&loaded.stashed[0].path).expect("stashed bytes");
    assert_eq!(loaded.tone.models[0].id, ModelId::from_local_bytes(&bytes));
    let key = ContentPreparationKey {
        content: ContentId::from_bytes(&bytes),
        preparation: PreparationKey {
            expected_sample_rate_bits: 48_000_f64.to_bits(),
            oversampling: 1,
            voices: 1,
            instruction_set: InstructionSet::Scalar,
        },
    };
    let cache = CompiledModelCache::new(2);

    let first = cache
        .get_or_compile(&key, &ModelBytes::from(bytes.clone()))
        .expect("compile");
    let second = cache
        .get_or_compile(&key, &ModelBytes::from(bytes))
        .expect("cache hit");

    assert!(Arc::ptr_eq(&first, &second));
}

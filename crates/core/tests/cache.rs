//! Per-block byte-cache identity, persistence selection, and eviction tests.

use std::sync::Arc;

use ronie_core::{
    BlockId, ByteCacheKey, ChainBlock, ModelByteCache, ModelBytes, ModelId, ModelNamespace, Tone,
};
use serde_json::json;

fn key(block: &str, id: i64, namespace: ModelNamespace) -> ByteCacheKey {
    ByteCacheKey {
        block_id: BlockId(block.into()),
        model_id: ModelId(id),
        namespace,
    }
}

fn tone() -> Tone {
    Tone::from_value(json!({
        "id": 1,
        "title": "Catalog",
        "format": "nam",
        "models": [{"id": 7, "name": "one"}, {"id": 8, "name": "two"}]
    }))
    .expect("tone")
}

#[test]
fn byte_cache_separates_blocks_and_local_catalog_namespaces() {
    let cache = ModelByteCache::new(1024);
    let first = key("one", 7, ModelNamespace::Catalog);
    let second = key("two", 7, ModelNamespace::Catalog);
    let local = key("one", 7, ModelNamespace::Local);
    cache
        .insert(first.clone(), ModelBytes::from(b"first".as_slice()))
        .expect("first");
    cache
        .insert(second.clone(), ModelBytes::from(b"second".as_slice()))
        .expect("second");
    cache
        .insert(local.clone(), ModelBytes::from(b"local".as_slice()))
        .expect("local");

    assert_eq!(cache.get(&first).expect("first").as_ref(), b"first");
    assert_eq!(cache.get(&second).expect("second").as_ref(), b"second");
    assert_eq!(cache.get(&local).expect("local").as_ref(), b"local");
}

#[test]
fn byte_cache_returns_the_same_allocation_and_evicts_only_map_ownership() {
    let cache = ModelByteCache::new(5);
    let first_key = key("one", 1, ModelNamespace::Catalog);
    let second_key = key("two", 2, ModelNamespace::Catalog);
    let bytes = ModelBytes::from(b"12345".as_slice());
    cache
        .insert(first_key.clone(), bytes.clone())
        .expect("first");
    let live = cache.get(&first_key).expect("live");
    assert!(Arc::ptr_eq(&bytes.0, &live.0));

    cache
        .insert(second_key, ModelBytes::from(b"67890".as_slice()))
        .expect("second");

    assert!(cache.get(&first_key).is_none());
    assert_eq!(live.as_ref(), b"12345");
}

#[test]
fn oversized_entries_are_rejected_without_evicting_good_bytes() {
    let cache = ModelByteCache::new(5);
    let good = key("good", 1, ModelNamespace::Catalog);
    let oversized = key("large", 2, ModelNamespace::Catalog);
    cache
        .insert(good.clone(), ModelBytes::from(b"good".as_slice()))
        .expect("good");

    let result = cache.insert(oversized, ModelBytes::from(b"too large".as_slice()));

    assert!(result.is_err());
    assert_eq!(cache.get(&good).expect("good").as_ref(), b"good");
}

#[test]
fn persistence_prunes_auditions_without_evicting_the_authoritative_cache() {
    let cache = ModelByteCache::new(1024);
    let active = key("block", 7, ModelNamespace::Catalog);
    let audition = key("block", 9, ModelNamespace::Catalog);
    cache
        .insert(active.clone(), ModelBytes::from(b"active".as_slice()))
        .expect("active");
    cache
        .insert(audition.clone(), ModelBytes::from(b"audition".as_slice()))
        .expect("audition");
    let block = ChainBlock::tone("block", tone(), ModelId(7));

    let retained = cache.retain_referenced(std::slice::from_ref(&block));

    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].model_id, ModelId(7));
    assert_eq!(retained[0].bytes.as_ref(), b"active");
    assert_eq!(
        cache
            .get(&audition)
            .expect("audition remains cached")
            .as_ref(),
        b"audition"
    );
}

#[test]
fn touch_local_marks_an_entry_recent_for_bounded_eviction() {
    let cache = ModelByteCache::new(10);
    let local = key("local", 1, ModelNamespace::Local);
    let catalog = key("catalog", 2, ModelNamespace::Catalog);
    cache
        .insert(local.clone(), ModelBytes::from(b"local".as_slice()))
        .expect("local");
    cache
        .insert(catalog.clone(), ModelBytes::from(b"other".as_slice()))
        .expect("catalog");

    cache.touch_local(&local).expect("touch local");
    cache
        .insert(
            key("new", 3, ModelNamespace::Catalog),
            ModelBytes::from(b"new".as_slice()),
        )
        .expect("new");

    assert!(cache.get(&local).is_some());
    assert!(cache.get(&catalog).is_none());
}

#[test]
fn persistence_retains_only_the_active_model_in_the_matching_namespace() {
    let cache = ModelByteCache::new(1024);
    let catalog = key("block", 7, ModelNamespace::Catalog);
    cache
        .insert(catalog.clone(), ModelBytes::from(b"catalog".as_slice()))
        .expect("catalog");
    cache
        .insert(
            key("block", 7, ModelNamespace::Local),
            ModelBytes::from(b"local".as_slice()),
        )
        .expect("local collision");
    cache
        .insert(
            key("block", 8, ModelNamespace::Catalog),
            ModelBytes::from(b"listed".as_slice()),
        )
        .expect("listed catalog model");
    let block = ChainBlock::tone("block", tone(), ModelId(7));

    let retained = cache.retain_referenced(std::slice::from_ref(&block));

    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].namespace, ModelNamespace::Catalog);
    assert_eq!(retained[0].model_id, ModelId(7));
    assert_eq!(retained[0].bytes.as_ref(), b"catalog");
}

#[test]
fn stale_restore_batch_cannot_replace_a_newer_generation() {
    let cache = ModelByteCache::new(1024);
    let key = key("block", 7, ModelNamespace::Catalog);
    let older = cache.begin_restore();
    let newer = cache.begin_restore();
    cache
        .publish_restore(
            newer,
            vec![(key.clone(), ModelBytes::from(b"new".as_slice()))],
        )
        .expect("newer restore");

    let result = cache.publish_restore(
        older,
        vec![(key.clone(), ModelBytes::from(b"old".as_slice()))],
    );

    assert!(matches!(result, Err(ronie_core::CacheError::Superseded)));
    assert_eq!(cache.get(&key).expect("new bytes").as_ref(), b"new");
}

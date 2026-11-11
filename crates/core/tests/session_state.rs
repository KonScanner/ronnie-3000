//! Session-state codec, migration, and offline restoration tests.

use ronie_core::{
    BlockId, ByteCacheKey, JuceCodec, JuceTree, JuceValue, ModelByteCache, ModelId, ModelNamespace,
    ModelSource, RestoreCaches, RestoreError, SessionSnapshot, StateCodec, StateError, Tone,
};
use serde_json::json;

/// A file URL of an absolute path on this system. A Windows path needs a drive.
fn file_url(path: &str) -> String {
    if cfg!(windows) {
        format!("file:///C:{path}")
    } else {
        format!("file://{path}")
    }
}

fn tree(
    identifier: &str,
    properties: Vec<(String, JuceValue)>,
    children: Vec<JuceTree>,
) -> JuceTree {
    JuceTree {
        identifier: identifier.into(),
        properties,
        children,
    }
}

fn string(value: &str) -> JuceValue {
    JuceValue::String(value.into())
}

fn tone(local: bool, with_url: bool) -> Tone {
    let first_url = with_url.then_some(if local {
        file_url("/tmp/first.nam")
    } else {
        "https://fixtures.local/first.nam".to_owned()
    });
    let second_url = with_url.then_some(if local {
        file_url("/tmp/second.nam")
    } else {
        "https://fixtures.local/second.nam".to_owned()
    });
    Tone::from_value(json!({
        "id": 1,
        "local": local,
        "title": "Fixture",
        "format": "nam",
        "models": [
            {"id": 100, "name": "first", "model_url": first_url},
            {"id": 101, "name": "second", "model_url": second_url}
        ],
        "futureToneField": {"retain": true}
    }))
    .expect("tone")
}

fn block(id: &str, tone: &Tone, active: i64, cached: &[(&str, i64, &[u8])]) -> JuceTree {
    let cache = tree(
        "ModelCache",
        Vec::new(),
        cached
            .iter()
            .map(|(_, model_id, bytes)| {
                tree(
                    "CachedModel",
                    vec![
                        ("modelId".into(), JuceValue::Int64(*model_id)),
                        ("data".into(), JuceValue::Binary(bytes.to_vec())),
                        ("futureCacheField".into(), string("retained")),
                    ],
                    vec![tree("FutureCacheChild", Vec::new(), Vec::new())],
                )
            })
            .collect(),
    );
    tree(
        "ChainBlock",
        vec![
            ("id".into(), string(id)),
            ("type".into(), string("NAM")),
            ("enabled".into(), JuceValue::Bool(true)),
            ("normalize".into(), JuceValue::Bool(true)),
            ("slimSize".into(), JuceValue::Double(0.0)),
            ("inputGain".into(), JuceValue::Double(0.5)),
            ("outputGain".into(), JuceValue::Double(0.5)),
            ("mix".into(), JuceValue::Double(1.0)),
            ("toneId".into(), JuceValue::Int64(1)),
            (
                "toneJson".into(),
                string(&tone.to_json().expect("tone JSON")),
            ),
            ("activeModelId".into(), JuceValue::Int64(active)),
            ("futureBlockField".into(), JuceValue::Int(7)),
        ],
        vec![
            tree(
                "Eq",
                vec![
                    ("enabled".into(), JuceValue::Bool(false)),
                    ("pre".into(), JuceValue::Bool(false)),
                ],
                Vec::new(),
            ),
            cache,
        ],
    )
}

fn state_bytes(schema: i32, block_tree: JuceTree, with_legacy: bool) -> Vec<u8> {
    let parameters = tree(
        "PARAMETERS",
        Vec::new(),
        if with_legacy {
            vec![
                tree(
                    "Param",
                    vec![("id".into(), string("transposeEnabled"))],
                    Vec::new(),
                ),
                tree(
                    "Param",
                    vec![("id".into(), string("pitchEnabled"))],
                    Vec::new(),
                ),
            ]
        } else {
            Vec::new()
        },
    );
    let snapshot = tree(
        "ChainSnapshot",
        vec![
            ("stereoEnabled".into(), JuceValue::Bool(false)),
            ("branchSide".into(), string("left")),
            ("branchAfterBlockId".into(), string("")),
            ("futureSnapshotField".into(), JuceValue::Int(9)),
        ],
        vec![
            tree("ChainBlocks", Vec::new(), vec![block_tree]),
            tree("RightChainBlocks", Vec::new(), Vec::new()),
        ],
    );
    let root = tree(
        "RonieState",
        vec![
            ("schemaVersion".into(), JuceValue::Int(schema)),
            ("inputMode".into(), string("dual")),
            ("futureRootField".into(), string("retained")),
        ],
        vec![parameters, snapshot],
    );
    let mut bytes = b"T3KB".to_vec();
    bytes.extend_from_slice(&JuceCodec::default().write_tree(&root).expect("state tree"));
    bytes
}

#[test]
fn current_state_round_trip_migrates_legacy_ids_and_preserves_unknown_fields() {
    let bytes = state_bytes(
        1,
        block(
            "block-1",
            &tone(false, true),
            100,
            &[("cached", 100, b"model")],
        ),
        true,
    );
    let codec = StateCodec::new();
    let document = codec.decode_session(&bytes).expect("decode current state");

    assert_eq!(
        document.snapshot.input_mode,
        ronie_core::InputMode::DualMono
    );
    assert_eq!(document.snapshot.editor_scale, 1.0);
    assert_eq!(document.snapshot.editor_extra_height, 36);
    assert_eq!(document.parameters.children.len(), 1);
    assert_eq!(
        document.parameters.children[0].properties[0].1,
        string("pitchEnabled")
    );
    let future = document
        .raw
        .properties
        .iter()
        .find(|(name, _)| name == "futureRootField")
        .map(|(_, value)| value);
    assert_eq!(future, Some(&string("retained")));
    assert_eq!(document.model_cache.len(), 1);

    let encoded = codec.encode_session(&document).expect("encode state");
    let decoded = codec.decode_session(&encoded).expect("decode round trip");
    assert_eq!(decoded.raw, document.raw);
    assert_eq!(decoded.snapshot.chain.left[0].id.0, "block-1");
}

#[test]
fn newer_schema_magic_and_truncation_are_rejected() {
    let bytes = state_bytes(2, block("block-1", &tone(false, true), 100, &[]), false);
    assert!(matches!(
        StateCodec::new().decode_session(&bytes),
        Err(StateError::UnsupportedSchema(2))
    ));
    assert!(matches!(
        StateCodec::new().decode_session(b"T3KX"),
        Err(StateError::InvalidMagic)
    ));
    assert!(StateCodec::new().decode_session(b"T3KB\0").is_err());
}

#[test]
fn local_restoration_reseeds_full_list_and_excludes_auditions() {
    let local = tone(true, true);
    let bytes = state_bytes(
        1,
        block(
            "local",
            &local,
            100,
            &[
                ("active", 100, b"first"),
                ("sibling", 101, b"second"),
                ("audition", 999, b"stale"),
            ],
        ),
        false,
    );
    let document = StateCodec::new().decode_session(&bytes).expect("document");
    let cache = ModelByteCache::new(1024);
    let current = document.snapshot.clone();

    let plan = ronie_core::restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: Some(&current),
        },
    )
    .expect("local restore");

    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(plan.jobs[0].model_id, ModelId(100));
    assert!(plan.jobs[0].bytes.is_some());
    for (id, expected) in [(100, b"first".as_slice()), (101, b"second".as_slice())] {
        let key = ByteCacheKey {
            block_id: BlockId("local".into()),
            model_id: ModelId(id),
            namespace: ModelNamespace::Local,
        };
        assert_eq!(cache.get(&key).expect("cached local").as_ref(), expected);
    }
    assert!(
        cache
            .get(&ByteCacheKey {
                block_id: BlockId("local".into()),
                model_id: ModelId(999),
                namespace: ModelNamespace::Local
            })
            .is_none()
    );
    assert!(plan.reconciled_block_ids.contains(&BlockId("local".into())));
}

#[test]
fn catalog_restoration_keeps_only_active_bytes() {
    let catalog = tone(false, true);
    let bytes = state_bytes(
        1,
        block(
            "catalog",
            &catalog,
            999,
            &[
                ("listed", 100, b"listed"),
                ("active", 999, b"active"),
                ("audition", 555, b"stale"),
            ],
        ),
        false,
    );
    let document = StateCodec::new().decode_session(&bytes).expect("document");
    let cache = ModelByteCache::new(1024);

    let plan = ronie_core::restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: None,
        },
    )
    .expect("active restore");

    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(plan.jobs[0].model_id, ModelId(999));
    assert_eq!(
        plan.jobs[0].bytes.as_ref().expect("embedded").as_ref(),
        b"active"
    );
    assert!(
        cache
            .get(&ByteCacheKey {
                block_id: BlockId("catalog".into()),
                model_id: ModelId(999),
                namespace: ModelNamespace::Catalog
            })
            .is_some()
    );
    for id in [100, 555] {
        assert!(
            cache
                .get(&ByteCacheKey {
                    block_id: BlockId("catalog".into()),
                    model_id: ModelId(id),
                    namespace: ModelNamespace::Catalog
                })
                .is_none()
        );
    }
}

#[test]
fn absent_bytes_produce_a_retryable_source_job() {
    let bytes = state_bytes(1, block("retry", &tone(false, true), 100, &[]), false);
    let document = StateCodec::new().decode_session(&bytes).expect("document");
    let cache = ModelByteCache::new(1024);

    let plan = ronie_core::restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: None,
        },
    )
    .expect("retry plan");

    assert_eq!(plan.jobs.len(), 1);
    assert!(plan.jobs[0].bytes.is_none());
    assert!(matches!(
        plan.jobs[0].source,
        Some(ModelSource::Catalog { .. })
    ));
    assert!(plan.jobs[0].generation > 0);
}

#[test]
fn missing_source_and_bytes_leave_current_state_untouched() {
    let bytes = state_bytes(1, block("broken", &tone(false, false), 100, &[]), false);
    let document = StateCodec::new().decode_session(&bytes).expect("document");
    let cache = ModelByteCache::new(1024);
    let current = document.snapshot.clone();
    let before = current.clone();

    let result = ronie_core::restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: Some(&current),
        },
    );

    assert!(matches!(result, Err(RestoreError::MissingModel { .. })));
    assert_eq!(current, before);
    assert!(cache.retain_referenced(&current.chain.left).is_empty());
}

#[test]
fn failed_new_schema_does_not_mutate_the_current_snapshot() {
    let bytes = state_bytes(2, block("block-1", &tone(false, true), 100, &[]), false);
    let current = SessionSnapshot::from_ui_value(json!({})).expect("current");
    let before = current.clone();

    let result = StateCodec::new().decode_session(&bytes);

    assert!(result.is_err());
    assert_eq!(current, before);
}

#[test]
fn restoration_uses_existing_cache_bytes_before_declaring_a_missing_model() {
    let bytes = state_bytes(1, block("cached", &tone(false, false), 100, &[]), false);
    let document = StateCodec::new().decode_session(&bytes).expect("document");
    let cache = ModelByteCache::new(1024);
    let key = ByteCacheKey {
        block_id: BlockId("cached".into()),
        model_id: ModelId(100),
        namespace: ModelNamespace::Catalog,
    };
    cache
        .insert(key, ronie_core::ModelBytes::from(b"cached".as_slice()))
        .expect("seed cache");

    let plan = ronie_core::restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: None,
        },
    )
    .expect("offline cache restore");

    assert_eq!(
        plan.jobs[0].bytes.as_ref().expect("bytes").as_ref(),
        b"cached"
    );
}

#[test]
fn restoration_does_not_confuse_same_id_models_across_namespaces() {
    let bytes = state_bytes(
        1,
        block(
            "collision",
            &tone(false, true),
            100,
            &[("catalog", 100, b"catalog")],
        ),
        false,
    );
    let mut document = StateCodec::new().decode_session(&bytes).expect("document");
    document.model_cache.insert(
        0,
        ronie_core::ReferencedModelBytes {
            block_id: BlockId("collision".into()),
            model_id: ModelId(100),
            namespace: ModelNamespace::Local,
            bytes: ronie_core::ModelBytes::from(b"local".as_slice()),
        },
    );
    let cache = ModelByteCache::new(1024);

    let plan = ronie_core::restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: None,
        },
    )
    .expect("restore");

    assert_eq!(
        plan.jobs[0].bytes.as_ref().expect("bytes").as_ref(),
        b"catalog"
    );
    assert!(
        cache
            .get(&ByteCacheKey {
                block_id: BlockId("collision".into()),
                model_id: ModelId(100),
                namespace: ModelNamespace::Local
            })
            .is_none()
    );
}

#[test]
fn changed_active_model_requires_reconciliation() {
    let bytes = state_bytes(
        1,
        block("changed", &tone(false, true), 100, &[("new", 100, b"new")]),
        false,
    );
    let document = StateCodec::new().decode_session(&bytes).expect("document");
    let mut current = document.snapshot.clone();
    current
        .chain
        .left
        .iter_mut()
        .find(|block| block.id == BlockId("changed".into()))
        .expect("changed block")
        .active_model_id = Some(ModelId(101));
    let cache = ModelByteCache::new(1024);

    let plan = ronie_core::restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: Some(&current),
        },
    )
    .expect("restore");

    assert!(
        !plan
            .reconciled_block_ids
            .contains(&BlockId("changed".into()))
    );
}

#[test]
fn changed_active_bytes_require_reconciliation() {
    let bytes = state_bytes(
        1,
        block("changed", &tone(false, true), 100, &[("new", 100, b"new")]),
        false,
    );
    let document = StateCodec::new().decode_session(&bytes).expect("document");
    let current = document.snapshot.clone();
    let cache = ModelByteCache::new(1024);
    cache
        .insert(
            ByteCacheKey {
                block_id: BlockId("changed".into()),
                model_id: ModelId(100),
                namespace: ModelNamespace::Catalog,
            },
            ronie_core::ModelBytes::from(b"old".as_slice()),
        )
        .expect("old bytes");

    let plan = ronie_core::restore_offline(
        &document,
        RestoreCaches {
            bytes: &cache,
            current: Some(&current),
        },
    )
    .expect("restore");

    assert!(
        !plan
            .reconciled_block_ids
            .contains(&BlockId("changed".into()))
    );
}

#[test]
fn ui_session_reads_the_active_preset_object() {
    let snapshot = SessionSnapshot::from_ui_value(json!({
        "chain": [],
        "preset": {"id": "factory-1", "name": "Crunch Rhythm"}
    }))
    .expect("UI session");
    let active = snapshot.active_preset.expect("active preset");
    assert_eq!(active.id, "factory-1");
    assert_eq!(active.name, "Crunch Rhythm");
}

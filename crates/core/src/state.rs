use crate::{
    BlockId, ByteCacheKey, JuceCodec, JuceTree, JuceValue, ModelByteCache, ModelBytes, ModelId,
    ModelNamespace, ModelSource, ReferencedModelBytes, RestoreError, SessionSnapshot, StateError,
    ToneError, migrate_param_ids, references_model,
};

const STATE_MAGIC: &[u8; 4] = b"T3KB";
const STATE_SCHEMA_VERSION: i64 = 1;

/// Referenced embedded model bytes retained by a session document.
pub type ModelCache = Vec<ReferencedModelBytes>;

/// Complete session with raw and typed projections.
#[derive(Debug, Clone)]
pub struct SessionDocument {
    /// Raw migrated root tree, including unknown fields.
    pub raw: JuceTree,
    /// Typed session and chain snapshot.
    pub snapshot: SessionSnapshot,
    /// Parameter state after legacy migration.
    pub parameters: JuceTree,
    /// MIDI state after legacy migration.
    pub midi: JuceTree,
    /// Referenced embedded model bytes only.
    pub model_cache: ModelCache,
}

/// Bounded `T3KB` schema-version-1 codec.
#[derive(Debug, Clone)]
pub struct StateCodec {
    codec: JuceCodec,
}

impl Default for StateCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl StateCodec {
    /// Creates a codec with the default binary limits.
    #[must_use]
    pub fn new() -> Self {
        Self {
            codec: JuceCodec::default(),
        }
    }

    /// Decodes magic, schema, raw trees, typed snapshots, migration, and referenced bytes.
    ///
    /// # Errors
    ///
    /// Returns magic, schema, binary codec, tone, or chain errors.
    pub fn decode_session(&self, input: &[u8]) -> Result<SessionDocument, StateError> {
        if input.len() < STATE_MAGIC.len() || &input[..STATE_MAGIC.len()] != STATE_MAGIC {
            return Err(StateError::InvalidMagic);
        }
        let mut raw = self.codec.read_tree(&input[STATE_MAGIC.len()..])?;
        let schema = int_property(&raw, "schemaVersion").unwrap_or(STATE_SCHEMA_VERSION);
        if schema > STATE_SCHEMA_VERSION {
            return Err(StateError::UnsupportedSchema(schema));
        }
        let snapshot = SessionSnapshot::from_juce_tree(&raw)?;
        let mut parameters = child(&raw, "PARAMETERS")
            .cloned()
            .unwrap_or_else(|| empty_tree("PARAMETERS"));
        let mut midi = child(&raw, "MidiMappings")
            .cloned()
            .unwrap_or_else(|| empty_tree("MidiMappings"));
        let _ = migrate_param_ids(&mut parameters, "id");
        let _ = migrate_param_ids(&mut midi, "targetId");
        replace_child(&mut raw, "PARAMETERS", &parameters);
        replace_child(&mut raw, "MidiMappings", &midi);
        let model_cache = collect_model_cache(&raw, &snapshot)?;
        Ok(SessionDocument {
            raw,
            snapshot,
            parameters,
            midi,
            model_cache,
        })
    }

    /// Encodes a session with only model bytes still referenced by its typed chain.
    ///
    /// # Errors
    ///
    /// Returns binary codec, cache-selection, or tone errors.
    pub fn encode_session(&self, document: &SessionDocument) -> Result<Vec<u8>, StateError> {
        let mut raw = document.raw.clone();
        set_property(
            &mut raw,
            "schemaVersion",
            JuceValue::Int(STATE_SCHEMA_VERSION as i32),
        );
        rewrite_model_caches(&mut raw, document)?;
        let mut output = STATE_MAGIC.to_vec();
        output.extend_from_slice(&self.codec.write_tree(&raw)?);
        Ok(output)
    }
}

/// Existing cache and current snapshot used for restoration reconciliation.
#[derive(Debug, Clone, Copy)]
pub struct RestoreCaches<'a> {
    /// Authoritative per-block byte cache to re-seed.
    pub bytes: &'a ModelByteCache,
    /// Current snapshot whose matching block identities can retain prepared engines.
    pub current: Option<&'a SessionSnapshot>,
}

/// One cache-first background preparation job.
#[derive(Debug, Clone)]
pub struct PreparationJob {
    /// Unique generation within this restore plan.
    pub generation: u64,
    /// Destination block.
    pub block_id: BlockId,
    /// Active model to prepare.
    pub model_id: ModelId,
    /// Retry source when embedded bytes are absent.
    pub source: Option<ModelSource>,
    /// Embedded bytes, present for offline preparation.
    pub bytes: Option<ModelBytes>,
}

/// Reconciled snapshot and all work required to restore it.
#[derive(Debug, Clone)]
pub struct RestorePlan {
    /// Restored typed snapshot.
    pub snapshot: SessionSnapshot,
    /// Active-model preparation jobs.
    pub jobs: Vec<PreparationJob>,
    /// Blocks whose identities match the current snapshot and need no forced recompile.
    pub reconciled_block_ids: Vec<BlockId>,
}

/// Builds a cache-first restoration plan without mutating the current snapshot or cache.
/// Byte publication happens only after every active model has bytes or a retry source.
///
/// # Errors
///
/// Returns [`RestoreError::MissingModel`] for an unrecoverable model, or a cache error.
pub fn restore_offline(
    document: &SessionDocument,
    caches: RestoreCaches<'_>,
) -> Result<RestorePlan, RestoreError> {
    let restore_generation = caches.bytes.begin_restore();
    let mut jobs = Vec::new();
    let mut pending = Vec::new();
    for block in document
        .snapshot
        .chain
        .left
        .iter()
        .chain(&document.snapshot.chain.right)
        .filter(|block| !block.is_insert)
    {
        let Some(active) = block.active_model_id else {
            continue;
        };
        let source = block.tone.as_ref().and_then(|tone| {
            tone.models
                .iter()
                .find(|model| model.id == active)
                .and_then(|model| model.source.clone())
        });
        let namespace = crate::model_namespace(block, active);
        let key = ByteCacheKey {
            block_id: block.id.clone(),
            model_id: active,
            namespace,
        };
        let bytes = document
            .model_cache
            .iter()
            .find(|entry| {
                entry.block_id == block.id
                    && entry.model_id == active
                    && entry.namespace == namespace
            })
            .map(|entry| entry.bytes.clone())
            .or_else(|| caches.bytes.get(&key));
        if bytes.is_none() && source.is_none() {
            return Err(RestoreError::MissingModel {
                block_id: block.id.clone(),
                model_id: active,
            });
        }
        jobs.push(PreparationJob {
            generation: jobs.len() as u64 + 1,
            block_id: block.id.clone(),
            model_id: active,
            source,
            bytes,
        });
    }
    for block in document
        .snapshot
        .chain
        .left
        .iter()
        .chain(&document.snapshot.chain.right)
    {
        for entry in &document.model_cache {
            if entry.block_id == block.id
                && entry.namespace == crate::model_namespace(block, entry.model_id)
                && references_model(block, entry.model_id)
            {
                pending.push((
                    ByteCacheKey {
                        block_id: entry.block_id.clone(),
                        model_id: entry.model_id,
                        namespace: entry.namespace,
                    },
                    entry.bytes.clone(),
                ));
            }
        }
    }
    let reconciled_block_ids = caches.current.map_or_else(Vec::new, |current| {
        document
            .snapshot
            .chain
            .left
            .iter()
            .chain(&document.snapshot.chain.right)
            .filter(|block| {
                current
                    .chain
                    .left
                    .iter()
                    .chain(&current.chain.right)
                    .any(|existing| {
                        if existing.id != block.id
                            || existing.is_insert != block.is_insert
                            || existing.active_model_id != block.active_model_id
                            || existing.tone != block.tone
                            || existing.params != block.params
                        {
                            return false;
                        }
                        let Some(active) = block.active_model_id else {
                            return true;
                        };
                        let key = ByteCacheKey {
                            block_id: block.id.clone(),
                            model_id: active,
                            namespace: crate::model_namespace(block, active),
                        };
                        let restored = jobs
                            .iter()
                            .find(|job| job.block_id == block.id)
                            .and_then(|job| job.bytes.as_ref());
                        match (restored, caches.bytes.get(&key)) {
                            (Some(restored), Some(current)) => {
                                restored.as_ref() == current.as_ref()
                            }
                            (Some(_), None) => true,
                            (None, _) => false,
                        }
                    })
            })
            .map(|block| block.id.clone())
            .collect()
    });
    caches.bytes.publish_restore(restore_generation, pending)?;
    Ok(RestorePlan {
        snapshot: document.snapshot.clone(),
        jobs,
        reconciled_block_ids,
    })
}

fn collect_model_cache(
    raw: &JuceTree,
    snapshot: &SessionSnapshot,
) -> Result<ModelCache, StateError> {
    let mut entries = Vec::new();
    for block in snapshot
        .chain
        .left
        .iter()
        .chain(&snapshot.chain.right)
        .filter(|block| !block.is_insert)
    {
        let Some(raw_block) = find_raw_block(raw, &block.id) else {
            continue;
        };
        let Some(cache) = child(raw_block, "ModelCache") else {
            continue;
        };
        let namespace = if block.tone.as_ref().is_some_and(|tone| tone.local) {
            ModelNamespace::Local
        } else {
            ModelNamespace::Catalog
        };
        for cached in &cache.children {
            if cached.identifier != "CachedModel" {
                continue;
            }
            let Some(model_id) = int_property(cached, "modelId").map(ModelId) else {
                continue;
            };
            if !references_model(block, model_id) {
                continue;
            }
            let Some(data) = property(cached, "data").and_then(|value| match value {
                JuceValue::Binary(data) => Some(data),
                _ => None,
            }) else {
                continue;
            };
            entries.push(ReferencedModelBytes {
                block_id: block.id.clone(),
                model_id,
                namespace,
                bytes: ModelBytes::from(data.clone()),
            });
        }
    }
    Ok(entries)
}

fn rewrite_model_caches(raw: &mut JuceTree, document: &SessionDocument) -> Result<(), StateError> {
    for block in document
        .snapshot
        .chain
        .left
        .iter()
        .chain(&document.snapshot.chain.right)
        .filter(|block| !block.is_insert)
    {
        let Some(raw_block) = find_raw_block_mut(raw, &block.id) else {
            continue;
        };
        if !raw_block
            .children
            .iter()
            .any(|child| child.identifier == "ModelCache")
        {
            raw_block.children.push(JuceTree {
                identifier: "ModelCache".into(),
                properties: Vec::new(),
                children: Vec::new(),
            });
        }
        let Some(cache) = raw_block
            .children
            .iter_mut()
            .find(|child| child.identifier == "ModelCache")
        else {
            continue;
        };
        let previous = std::mem::take(&mut cache.children);
        let mut matched = std::collections::HashSet::new();
        for mut cached in previous {
            if cached.identifier != "CachedModel" {
                cache.children.push(cached);
                continue;
            }
            let Some(model_id) = int_property(&cached, "modelId").map(ModelId) else {
                continue;
            };
            if matched.contains(&model_id) {
                continue;
            }
            let Some(entry) = document.model_cache.iter().find(|entry| {
                entry.block_id == block.id
                    && entry.model_id == model_id
                    && entry.namespace == crate::model_namespace(block, model_id)
                    && references_model(block, model_id)
            }) else {
                continue;
            };
            set_property(&mut cached, "modelId", JuceValue::Int64(model_id.0));
            set_property(&mut cached, "data", JuceValue::Binary(entry.bytes.to_vec()));
            cache.children.push(cached);
            matched.insert(model_id);
        }
        for entry in &document.model_cache {
            if entry.block_id != block.id
                || entry.namespace != crate::model_namespace(block, entry.model_id)
                || !references_model(block, entry.model_id)
                || matched.contains(&entry.model_id)
            {
                continue;
            }
            cache.children.push(JuceTree {
                identifier: "CachedModel".into(),
                properties: vec![
                    ("modelId".into(), JuceValue::Int64(entry.model_id.0)),
                    ("data".into(), JuceValue::Binary(entry.bytes.to_vec())),
                ],
                children: Vec::new(),
            });
            matched.insert(entry.model_id);
        }
    }
    Ok(())
}

fn find_raw_block<'a>(raw: &'a JuceTree, id: &BlockId) -> Option<&'a JuceTree> {
    child(raw, "ChainSnapshot")?
        .children
        .iter()
        .filter(|lane| lane.identifier == "ChainBlocks" || lane.identifier == "RightChainBlocks")
        .flat_map(|lane| &lane.children)
        .find(|block| string_property(block, "id") == Some(id.0.as_str()))
}

fn find_raw_block_mut<'a>(raw: &'a mut JuceTree, id: &BlockId) -> Option<&'a mut JuceTree> {
    raw.children
        .iter_mut()
        .find(|child| child.identifier == "ChainSnapshot")?
        .children
        .iter_mut()
        .filter(|lane| lane.identifier == "ChainBlocks" || lane.identifier == "RightChainBlocks")
        .flat_map(|lane| &mut lane.children)
        .find(|block| string_property(block, "id") == Some(id.0.as_str()))
}

fn replace_child(raw: &mut JuceTree, name: &str, replacement: &JuceTree) {
    if let Some(child) = raw
        .children
        .iter_mut()
        .find(|child| child.identifier == name)
    {
        *child = replacement.clone();
    } else {
        raw.children.push(replacement.clone());
    }
}

fn child<'a>(tree: &'a JuceTree, name: &str) -> Option<&'a JuceTree> {
    tree.children.iter().find(|child| child.identifier == name)
}

fn property<'a>(tree: &'a JuceTree, name: &str) -> Option<&'a JuceValue> {
    tree.properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn string_property<'a>(tree: &'a JuceTree, name: &str) -> Option<&'a str> {
    match property(tree, name) {
        Some(JuceValue::String(value)) => Some(value),
        _ => None,
    }
}

fn int_property(tree: &JuceTree, name: &str) -> Option<i64> {
    match property(tree, name) {
        Some(JuceValue::Int(value)) => Some(i64::from(*value)),
        Some(JuceValue::Int64(value)) => Some(*value),
        Some(JuceValue::Double(value)) => Some(*value as i64),
        Some(JuceValue::String(value)) => value.parse().ok(),
        _ => None,
    }
}

fn set_property(tree: &mut JuceTree, name: &str, value: JuceValue) {
    if let Some(existing) = tree
        .properties
        .iter_mut()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
    {
        *existing = value;
    } else {
        tree.properties.push((name.into(), value));
    }
}

fn empty_tree(identifier: &str) -> JuceTree {
    JuceTree {
        identifier: identifier.into(),
        properties: Vec::new(),
        children: Vec::new(),
    }
}

impl From<ToneError> for StateError {
    fn from(value: ToneError) -> Self {
        Self::Tone(value)
    }
}

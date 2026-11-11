use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use sha2::{Digest, Sha256};

use crate::{BlockId, CacheError, ChainBlock, ModelBytes, ModelId, ModelSource, references_model};

/// Byte-cache namespace, preventing local/catalog ID collisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelNamespace {
    /// Remote catalog model.
    Catalog,
    /// Local stash model.
    Local,
}

impl ModelNamespace {
    /// Derives the namespace from a model source.
    #[must_use]
    pub const fn from_source(source: &ModelSource) -> Self {
        match source {
            ModelSource::Catalog { .. } => Self::Catalog,
            ModelSource::Local { .. } => Self::Local,
        }
    }
}

/// Authoritative per-block/model identity for immutable bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ByteCacheKey {
    /// Block that owns the byte entry.
    pub block_id: BlockId,
    /// Model identity within that block.
    pub model_id: ModelId,
    /// Local or catalog namespace.
    pub namespace: ModelNamespace,
}

/// Bytes selected for session or preset persistence.
#[derive(Debug, Clone)]
pub struct ReferencedModelBytes {
    /// Referencing block.
    pub block_id: BlockId,
    /// Referenced model.
    pub model_id: ModelId,
    /// Source namespace.
    pub namespace: ModelNamespace,
    /// Shared immutable bytes.
    pub bytes: ModelBytes,
}

#[derive(Debug)]
struct ByteEntry {
    bytes: ModelBytes,
    last_used: u64,
}

#[derive(Debug, Default)]
struct ByteState {
    entries: HashMap<ByteCacheKey, ByteEntry>,
    total_bytes: usize,
    restore_generation: u64,
}

#[derive(Debug)]
struct ByteInner {
    state: RwLock<ByteState>,
    max_bytes: usize,
    clock: AtomicU64,
}

/// Bounded, process-local authoritative byte cache.
#[derive(Debug, Clone)]
pub struct ModelByteCache {
    inner: Arc<ByteInner>,
}

impl ModelByteCache {
    /// Creates a cache with a strict total byte bound for map-owned entries.
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self {
            inner: Arc::new(ByteInner {
                state: RwLock::new(ByteState::default()),
                max_bytes,
                clock: AtomicU64::new(0),
            }),
        }
    }

    /// Returns shared bytes and marks the entry as recently used.
    #[must_use]
    pub fn get(&self, key: &ByteCacheKey) -> Option<ModelBytes> {
        let tick = self.tick();
        let mut state = self.write();
        let entry = state.entries.get_mut(key)?;
        entry.last_used = tick;
        Some(entry.bytes.clone())
    }

    /// Inserts bytes and evicts least-recently-used map entries to meet the bound.
    /// Live `Arc` clones remain valid after eviction.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::EntryTooLarge`] without changing existing entries
    /// when one value exceeds the cache's total capacity.
    pub fn insert(&self, key: ByteCacheKey, bytes: ModelBytes) -> Result<(), CacheError> {
        if bytes.len() > self.inner.max_bytes {
            return Err(CacheError::EntryTooLarge {
                actual: bytes.len(),
                max: self.inner.max_bytes,
            });
        }
        let tick = self.tick();
        let mut state = self.write();
        if let Some(previous) = state.entries.get(&key) {
            state.total_bytes = state.total_bytes.saturating_sub(previous.bytes.len());
        }
        state.total_bytes = state.total_bytes.saturating_add(bytes.len());
        state.entries.insert(
            key,
            ByteEntry {
                bytes,
                last_used: tick,
            },
        );
        while state.total_bytes > self.inner.max_bytes {
            let Some(oldest) = state
                .entries
                .iter()
                .min_by_key(|(key, entry)| (entry.last_used, (*key).clone()))
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = state.entries.remove(&oldest) {
                state.total_bytes = state.total_bytes.saturating_sub(entry.bytes.len());
            }
        }
        Ok(())
    }

    /// Validates and publishes a complete batch under one cache write lock.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::EntryTooLarge`] without changing the cache when
    /// any entry exceeds total capacity.
    pub fn insert_batch(&self, entries: Vec<(ByteCacheKey, ModelBytes)>) -> Result<(), CacheError> {
        validate_batch(&entries, self.inner.max_bytes)?;
        let tick = self.tick();
        let mut state = self.write();
        for (key, bytes) in entries {
            insert_locked(&mut state, key, bytes, tick, self.inner.max_bytes);
        }
        Ok(())
    }

    /// Starts a new restore generation, excluding older batches from later publication.
    pub fn begin_restore(&self) -> u64 {
        let mut state = self.write();
        state.restore_generation = state.restore_generation.wrapping_add(1);
        state.restore_generation
    }

    /// Publishes one complete restore batch if its generation is still current.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Superseded`] or a size error without changing entries.
    pub fn publish_restore(
        &self,
        generation: u64,
        entries: Vec<(ByteCacheKey, ModelBytes)>,
    ) -> Result<(), CacheError> {
        validate_batch(&entries, self.inner.max_bytes)?;
        let tick = self.tick();
        let mut state = self.write();
        if state.restore_generation != generation {
            return Err(CacheError::Superseded);
        }
        for (key, bytes) in entries {
            insert_locked(&mut state, key, bytes, tick, self.inner.max_bytes);
        }
        Ok(())
    }

    /// Selects referenced persistence bytes without evicting in-memory auditions.
    #[must_use]
    pub fn retain_referenced(&self, blocks: &[ChainBlock]) -> Vec<ReferencedModelBytes> {
        let state = self.read();
        let mut retained: Vec<_> = state
            .entries
            .iter()
            .filter(|(key, _)| {
                blocks.iter().any(|block| {
                    block.id == key.block_id
                        && key.namespace == crate::model_namespace(block, key.model_id)
                        && references_model(block, key.model_id)
                })
            })
            .map(|(key, entry)| ReferencedModelBytes {
                block_id: key.block_id.clone(),
                model_id: key.model_id,
                namespace: key.namespace,
                bytes: entry.bytes.clone(),
            })
            .collect();
        retained.sort_by(|left, right| {
            (&left.block_id, left.model_id, left.namespace).cmp(&(
                &right.block_id,
                right.model_id,
                right.namespace,
            ))
        });
        retained
    }

    /// Marks a local entry recently used, preserving it across bounded eviction.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::WrongNamespace`] for catalog keys or missing local entries.
    pub fn touch_local(&self, key: &ByteCacheKey) -> Result<(), CacheError> {
        if key.namespace != ModelNamespace::Local {
            return Err(CacheError::WrongNamespace);
        }
        let tick = self.tick();
        let mut state = self.write();
        let entry = state
            .entries
            .get_mut(key)
            .ok_or(CacheError::WrongNamespace)?;
        entry.last_used = tick;
        Ok(())
    }

    fn tick(&self) -> u64 {
        self.inner.clock.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn read(&self) -> RwLockReadGuard<'_, ByteState> {
        self.inner
            .state
            .read()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, ByteState> {
        self.inner
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

fn validate_batch(
    entries: &[(ByteCacheKey, ModelBytes)],
    max_bytes: usize,
) -> Result<(), CacheError> {
    for (_, bytes) in entries {
        if bytes.len() > max_bytes {
            return Err(CacheError::EntryTooLarge {
                actual: bytes.len(),
                max: max_bytes,
            });
        }
    }
    Ok(())
}

fn insert_locked(
    state: &mut ByteState,
    key: ByteCacheKey,
    bytes: ModelBytes,
    tick: u64,
    max_bytes: usize,
) {
    if let Some(previous) = state.entries.get(&key) {
        state.total_bytes = state.total_bytes.saturating_sub(previous.bytes.len());
    }
    state.total_bytes = state.total_bytes.saturating_add(bytes.len());
    state.entries.insert(
        key,
        ByteEntry {
            bytes,
            last_used: tick,
        },
    );
    while state.total_bytes > max_bytes {
        let Some(oldest) = state
            .entries
            .iter()
            .min_by_key(|(key, entry)| (entry.last_used, (*key).clone()))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        if let Some(entry) = state.entries.remove(&oldest) {
            state.total_bytes = state.total_bytes.saturating_sub(entry.bytes.len());
        }
    }
}

/// Cryptographic identity of exact model bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentId {
    /// SHA-256 digest of the complete byte string.
    pub sha256: [u8; 32],
    /// Exact byte length.
    pub byte_len: u64,
}

impl ContentId {
    /// Computes content identity independently of stash names and model IDs.
    #[must_use]
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        Self {
            sha256: Sha256::digest(bytes).into(),
            byte_len: bytes.len() as u64,
        }
    }
}

/// CPU and preparation details that affect compiled-model identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InstructionSet {
    /// Portable scalar implementation.
    Scalar,
    /// x86-64 AVX2 implementation.
    Avx2,
    /// x86-64 AVX-512 implementation.
    Avx512,
    /// AArch64 NEON implementation.
    Neon,
}

/// Runtime preparation requirements for immutable compiled weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PreparationKey {
    /// Expected model sample rate as exact `f64` bits.
    pub expected_sample_rate_bits: u64,
    /// Oversampling factor.
    pub oversampling: u8,
    /// Independent voice count.
    pub voices: u8,
    /// Selected instruction set.
    pub instruction_set: InstructionSet,
}

/// Full content-and-preparation key for a compiled-model cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentPreparationKey {
    /// Exact byte identity.
    pub content: ContentId,
    /// Preparation requirements; tier and gain are deliberately excluded.
    pub preparation: PreparationKey,
}

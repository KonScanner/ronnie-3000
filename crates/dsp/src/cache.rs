use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use ronie_core::{ContentId, ContentPreparationKey, DspError, ModelBytes, parse_nam};

use crate::{CompiledModel, compile_model};

struct CacheEntry {
    model: Arc<CompiledModel>,
    last_used: u64,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<ContentPreparationKey, CacheEntry>,
    clock: u64,
}

/// Process-local cache of immutable compiled weights, keyed by content and preparation.
pub struct CompiledModelCache {
    state: Mutex<CacheState>,
    max_entries: usize,
}

impl CompiledModelCache {
    /// Creates a cache with an explicit entry bound.
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            state: Mutex::new(CacheState::default()),
            max_entries,
        }
    }

    /// Returns shared compiled weights or compiles them once for the full key.
    /// Mutable histories are created separately by [`crate::ModelEngine`].
    ///
    /// # Errors
    ///
    /// Returns content mismatch, parse/compile errors, or zero-capacity errors.
    /// Failed compilation does not remove an existing good entry.
    pub fn get_or_compile(
        &self,
        key: &ContentPreparationKey,
        bytes: &ModelBytes,
    ) -> Result<Arc<CompiledModel>, DspError> {
        if ContentId::from_bytes(bytes) != key.content {
            return Err(DspError::ContentMismatch);
        }
        if self.max_entries == 0 {
            return Err(DspError::CacheCapacity);
        }
        let mut state = self.lock();
        state.clock = state.clock.wrapping_add(1);
        let tick = state.clock;
        if let Some(entry) = state.entries.get_mut(key) {
            entry.last_used = tick;
            return Ok(Arc::clone(&entry.model));
        }

        let document = parse_nam(bytes)?;
        let model = Arc::new(compile_model(&document)?);
        if state.entries.len() >= self.max_entries {
            let oldest = state
                .entries
                .iter()
                .min_by_key(|(key, entry)| (entry.last_used, **key))
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                state.entries.remove(&oldest);
            }
        }
        state.entries.insert(
            *key,
            CacheEntry {
                model: Arc::clone(&model),
                last_used: tick,
            },
        );
        Ok(model)
    }

    fn lock(&self) -> MutexGuard<'_, CacheState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

pub use secrecy::SecretString;

use crate::{
    AcquireError, BlockId, ByteCacheKey, FetchError, ModelByteCache, ModelBytes, ModelId,
    ModelNamespace, ModelSource,
};

/// One model download request with an optional redacted bearer token.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// Local or catalog byte source.
    pub source: ModelSource,
    /// Optional secret bearer value.
    pub bearer: Option<SecretString>,
}

/// Asynchronous byte-source adapter implemented by catalog and local clients.
pub trait ModelFetcher {
    /// Fetches exact model bytes without exposing credentials in errors.
    fn fetch(
        &self,
        request: FetchRequest,
    ) -> impl Future<Output = Result<ModelBytes, FetchError>> + Send;
}

/// One generation-stamped acquisition request.
#[derive(Debug, Clone)]
pub struct AcquisitionRequest {
    /// Destination block.
    pub block_id: BlockId,
    /// Destination model identity.
    pub model_id: ModelId,
    /// Byte source.
    pub source: ModelSource,
    /// Optional secret bearer value.
    pub bearer: Option<SecretString>,
}

/// Successfully published model bytes and their generation.
#[derive(Debug, Clone)]
pub struct AcquiredModel {
    /// Monotonic request generation.
    pub generation: u64,
    /// Destination block.
    pub block_id: BlockId,
    /// Destination model.
    pub model_id: ModelId,
    /// Source namespace.
    pub namespace: ModelNamespace,
    /// Shared immutable bytes.
    pub bytes: ModelBytes,
}

#[derive(Default)]
struct CoordinatorState {
    latest: HashMap<BlockId, u64>,
    pending: HashSet<u64>,
    cancelled: HashSet<u64>,
}

/// Coordinates downloads so stale or cancelled completions cannot publish bytes.
pub struct AcquisitionCoordinator<F: ModelFetcher> {
    fetcher: F,
    cache: ModelByteCache,
    next_generation: AtomicU64,
    state: Arc<Mutex<CoordinatorState>>,
}

impl<F: ModelFetcher> AcquisitionCoordinator<F> {
    /// Creates a coordinator over an authoritative byte cache.
    #[must_use]
    pub fn new(fetcher: F, cache: ModelByteCache) -> Self {
        Self {
            fetcher,
            cache,
            next_generation: AtomicU64::new(1),
            state: Arc::new(Mutex::new(CoordinatorState::default())),
        }
    }

    /// Fetches and publishes one model if it is still the latest request for its block.
    ///
    /// # Errors
    ///
    /// Returns fetch/cache errors, or a superseded/cancelled outcome without publishing.
    pub async fn load(&self, request: AcquisitionRequest) -> Result<AcquiredModel, AcquireError> {
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let namespace = ModelNamespace::from_source(&request.source);
        {
            let mut state = self.lock();
            if state.cancelled.remove(&generation) {
                return Err(AcquireError::Cancelled { generation });
            }
            state.pending.insert(generation);
            state.latest.insert(request.block_id.clone(), generation);
        }

        let result = self
            .fetcher
            .fetch(FetchRequest {
                source: request.source.clone(),
                bearer: request.bearer,
            })
            .await;
        let mut state = self.lock();
        state.pending.remove(&generation);
        if state.cancelled.remove(&generation) {
            return Err(AcquireError::Cancelled { generation });
        }
        if state.latest.get(&request.block_id) != Some(&generation) {
            return Err(AcquireError::Superseded { generation });
        }
        let bytes = result?;
        self.cache.insert(
            ByteCacheKey {
                block_id: request.block_id.clone(),
                model_id: request.model_id,
                namespace,
            },
            bytes.clone(),
        )?;
        drop(state);
        Ok(AcquiredModel {
            generation,
            block_id: request.block_id,
            model_id: request.model_id,
            namespace,
            bytes,
        })
    }

    /// Marks one generation cancelled. Its later completion cannot publish bytes.
    pub fn cancel(&self, generation: u64) {
        let mut state = self.lock();
        if state.pending.contains(&generation)
            || generation >= self.next_generation.load(Ordering::SeqCst)
        {
            state.cancelled.insert(generation);
        }
    }

    fn lock(&self) -> MutexGuard<'_, CoordinatorState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

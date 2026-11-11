//! Generation-safe model acquisition tests.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use ronie_core::{
    AcquireError, AcquisitionCoordinator, AcquisitionRequest, BlockId, ByteCacheKey, FetchError,
    FetchRequest, ModelByteCache, ModelBytes, ModelFetcher, ModelId, ModelNamespace, ModelSource,
    SecretString,
};
use tokio::sync::Notify;
use url::Url;

#[derive(Clone)]
struct ControlledFetcher {
    calls: Arc<AtomicUsize>,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl ControlledFetcher {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        }
    }
}

impl ModelFetcher for ControlledFetcher {
    fn fetch(
        &self,
        _request: FetchRequest,
    ) -> impl Future<Output = Result<ModelBytes, FetchError>> + Send {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        async move {
            if call == 0 {
                self.started.notify_one();
                self.release.notified().await;
                Ok(ModelBytes::from(b"old".as_slice()))
            } else {
                Ok(ModelBytes::from(b"new".as_slice()))
            }
        }
    }
}

#[derive(Clone)]
struct FailingFetcher;

impl ModelFetcher for FailingFetcher {
    async fn fetch(&self, _request: FetchRequest) -> Result<ModelBytes, FetchError> {
        Err(FetchError::Failed("controlled failure"))
    }
}

fn request() -> AcquisitionRequest {
    AcquisitionRequest {
        block_id: BlockId("block".into()),
        model_id: ModelId(7),
        source: ModelSource::Catalog {
            url: Url::parse("https://fixtures.local/model.nam").expect("URL"),
        },
        bearer: Option::<SecretString>::None,
    }
}

fn cache_key() -> ByteCacheKey {
    ByteCacheKey {
        block_id: BlockId("block".into()),
        model_id: ModelId(7),
        namespace: ModelNamespace::Catalog,
    }
}

#[tokio::test]
async fn stale_completion_cannot_replace_a_newer_request() {
    let fetcher = ControlledFetcher::new();
    let cache = ModelByteCache::new(1024);
    let coordinator = Arc::new(AcquisitionCoordinator::new(fetcher.clone(), cache.clone()));
    let first = tokio::spawn({
        let coordinator = Arc::clone(&coordinator);
        async move { coordinator.load(request()).await }
    });
    fetcher.started.notified().await;

    let second = coordinator.load(request()).await.expect("newer load");
    assert_eq!(second.bytes.as_ref(), b"new");
    fetcher.release.notify_one();
    let first = first.await.expect("first task");

    assert!(matches!(first, Err(AcquireError::Superseded { .. })));
    assert_eq!(
        cache.get(&cache_key()).expect("cached new bytes").as_ref(),
        b"new"
    );
}

#[tokio::test]
async fn cancelled_completion_cannot_publish_bytes() {
    let fetcher = ControlledFetcher::new();
    let cache = ModelByteCache::new(1024);
    let coordinator = Arc::new(AcquisitionCoordinator::new(fetcher.clone(), cache.clone()));
    let first = tokio::spawn({
        let coordinator = Arc::clone(&coordinator);
        async move { coordinator.load(request()).await }
    });
    fetcher.started.notified().await;

    coordinator.cancel(1);
    fetcher.release.notify_one();
    let result = first.await.expect("task");

    assert!(matches!(
        result,
        Err(AcquireError::Cancelled { generation: 1 })
    ));
    assert!(cache.get(&cache_key()).is_none());
}

#[tokio::test]
async fn failed_replacement_leaves_previous_good_bytes_available() {
    let cache = ModelByteCache::new(1024);
    cache
        .insert(cache_key(), ModelBytes::from(b"good".as_slice()))
        .expect("good bytes");
    let coordinator = AcquisitionCoordinator::new(FailingFetcher, cache.clone());

    let result = coordinator.load(request()).await;

    assert!(matches!(
        result,
        Err(AcquireError::Fetch(FetchError::Failed(_)))
    ));
    assert_eq!(
        cache.get(&cache_key()).expect("good bytes remain").as_ref(),
        b"good"
    );
}

#[tokio::test]
async fn cancellation_survives_more_than_1024_unsettled_generations() {
    let fetcher = ControlledFetcher::new();
    let cache = ModelByteCache::new(1024);
    let coordinator = Arc::new(AcquisitionCoordinator::new(fetcher.clone(), cache));
    let first = tokio::spawn({
        let coordinator = Arc::clone(&coordinator);
        async move { coordinator.load(request()).await }
    });
    fetcher.started.notified().await;

    for generation in 1..=1025 {
        coordinator.cancel(generation);
    }
    fetcher.release.notify_one();
    let result = first.await.expect("task");

    assert!(matches!(
        result,
        Err(AcquireError::Cancelled { generation: 1 })
    ));
}

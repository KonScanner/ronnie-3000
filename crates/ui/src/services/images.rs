//! Bounded asynchronous artwork and avatar cache with UI-thread delivery.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Condvar, Mutex, MutexGuard, mpsc},
};

use eframe::egui::{ColorImage, TextureHandle};
use ronie_core::{DownloadClient, HttpTransport};
use tokio::runtime::Handle;
use url::Url;

/// Canonical artwork side in device pixels.
pub const ART_SIDE: u32 = 512;
/// Canonical avatar side in device pixels.
pub const AVATAR_SIDE: u32 = 64;
const BUDGET_BYTES: usize = 48 * 1024 * 1024;
const MAX_ENTRIES: usize = 512;
const WORKERS: usize = 4;

/// Synchronous fetch boundary implemented by HTTP and test fixtures.
pub trait ImageFetcher: Send + Sync + 'static {
    /// Returns a decoded source image or `None` for a 404/failure.
    fn fetch(&self, url: &str, side: u32) -> Option<ColorImage>;
}

type Callback = Box<dyn FnMut(Option<ColorImage>) + Send + 'static>;

struct Pending {
    url: String,
    callback: Callback,
}

struct Job {
    url: String,
    side: u32,
}

struct State {
    cache: BTreeMap<String, Option<ColorImage>>,
    order: VecDeque<String>,
    total_bytes: usize,
    queued: BTreeSet<String>,
    jobs: VecDeque<Job>,
    pending: BTreeMap<u64, Pending>,
    next_id: u64,
    active_jobs: usize,
    closed: bool,
}

impl State {
    fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
            order: VecDeque::new(),
            total_bytes: 0,
            queued: BTreeSet::new(),
            jobs: VecDeque::new(),
            pending: BTreeMap::new(),
            next_id: 1,
            active_jobs: 0,
            closed: false,
        }
    }

    fn insert(&mut self, url: String, image: Option<ColorImage>) {
        if self.cache.contains_key(&url) {
            return;
        }
        let bytes = image.as_ref().map_or(0, image_bytes);
        self.cache.insert(url.clone(), image);
        self.order.push_back(url);
        self.total_bytes += bytes;
        while (self.total_bytes > BUDGET_BYTES || self.order.len() > MAX_ENTRIES)
            && self.order.len() > 1
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(image) = self.cache.remove(&oldest) {
                self.total_bytes = self
                    .total_bytes
                    .saturating_sub(image.as_ref().map_or(0, image_bytes));
            }
        }
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    delivery_tx: mpsc::Sender<(u64, Option<ColorImage>)>,
    delivery_rx: Mutex<mpsc::Receiver<(u64, Option<ColorImage>)>>,
    fetcher: Arc<dyn ImageFetcher>,
}

/// Cloneable cache owner. Clones share cache, workers, and budget.
#[derive(Clone)]
pub struct ImageCache {
    shared: Arc<Shared>,
}

impl ImageCache {
    /// Creates a cache with four workers.
    #[must_use]
    pub fn new(fetcher: Arc<dyn ImageFetcher>) -> Self {
        let (delivery_tx, delivery_rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::new()),
            wake: Condvar::new(),
            delivery_tx,
            delivery_rx: Mutex::new(delivery_rx),
            fetcher,
        });
        for _ in 0..WORKERS {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || worker(shared));
        }
        Self { shared }
    }

    /// Returns a cached result, including `Some(None)` for a known failure.
    #[must_use]
    pub fn cached(&self, url: &str) -> Option<Option<ColorImage>> {
        let state = self.lock();
        if state.closed {
            return None;
        }
        state.cache.get(url).cloned()
    }

    /// Loads one URL, coalescing concurrent requests for the same resource.
    pub fn load(
        &self,
        url: &str,
        side: u32,
        callback: impl FnMut(Option<ColorImage>) + Send + 'static,
    ) -> ImageRequest {
        let mut callback = Box::new(callback) as Callback;
        if url.is_empty() || side == 0 {
            callback(None);
            return ImageRequest::detached();
        }
        if let Some(image) = self.cached(url) {
            callback(image);
            return ImageRequest::detached();
        }
        let mut state = self.lock();
        if state.closed {
            return ImageRequest::detached();
        }
        let id = state.next_id;
        state.next_id += 1;
        state.pending.insert(
            id,
            Pending {
                url: url.to_owned(),
                callback,
            },
        );
        if state.queued.insert(url.to_owned()) {
            state.jobs.push_back(Job {
                url: url.to_owned(),
                side,
            });
            self.shared.wake.notify_one();
        }
        ImageRequest {
            id,
            shared: Arc::clone(&self.shared),
        }
    }

    /// Delivers completed results on the caller's UI thread.
    pub fn poll(&self) {
        loop {
            let received = self
                .shared
                .delivery_rx
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .try_recv();
            let Ok((id, image)) = received else {
                break;
            };
            let callback = {
                let mut state = self.lock();
                if state.closed {
                    None
                } else {
                    state.pending.remove(&id).map(|pending| pending.callback)
                }
            };
            if let Some(mut callback) = callback {
                callback(image);
            }
        }
    }

    /// Whether no fetch remains active or queued.
    #[must_use]
    pub fn idle(&self) -> bool {
        let state = self.lock();
        state.jobs.is_empty() && state.active_jobs == 0 && state.queued.is_empty()
    }

    /// Current decoded byte budget.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.lock().total_bytes
    }

    /// Current cache entry count, including failed entries.
    #[must_use]
    pub fn entries(&self) -> usize {
        self.lock().cache.len()
    }

    /// Cancels callbacks and prevents all later cache writes.
    pub fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.pending.clear();
        state.jobs.clear();
        state.queued.clear();
        self.shared.wake.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// Fetcher for the four workers of an [`ImageCache`] over the download client of a service.
///
/// Each fetch blocks its worker thread, and the decode runs on that thread and not on a runtime
/// thread. The origin policy of the client checks each address, and an image request has no
/// token. Each failure is `None`, so the caller shows its placeholder.
pub struct HttpImageFetcher<T: HttpTransport> {
    downloads: DownloadClient<T>,
    runtime: Handle,
}

impl<T: HttpTransport> HttpImageFetcher<T> {
    /// Creates a fetcher that runs its requests on `runtime`.
    #[must_use]
    pub const fn new(downloads: DownloadClient<T>, runtime: Handle) -> Self {
        Self { downloads, runtime }
    }
}

impl<T: HttpTransport + Send + Sync + 'static> ImageFetcher for HttpImageFetcher<T> {
    fn fetch(&self, url: &str, _side: u32) -> Option<ColorImage> {
        let url = Url::parse(url).ok()?;
        let image = self
            .runtime
            .block_on(self.downloads.download_image(&url))
            .ok()??;
        let size = [
            usize::try_from(image.width).ok()?,
            usize::try_from(image.height).ok()?,
        ];
        Some(ColorImage::from_rgba_unmultiplied(size, &image.rgba))
    }
}

type Answer<K> = (K, String, Option<ColorImage>);

/// The textures of one view, with one image for each key.
///
/// A key asks one time for each address, whatever the answer is. A failed image therefore stays a
/// placeholder and is not fetched in each frame, also after the cache forgets the failure.
pub struct RemoteImages<K> {
    cache: ImageCache,
    asked: BTreeMap<K, String>,
    requests: BTreeMap<K, ImageRequest>,
    textures: BTreeMap<K, TextureHandle>,
    answers: Arc<Mutex<Vec<Answer<K>>>>,
}

impl<K: Ord + Clone + Send + 'static> RemoteImages<K> {
    /// Creates a view with no images.
    #[must_use]
    pub fn new(cache: ImageCache) -> Self {
        Self {
            cache,
            asked: BTreeMap::new(),
            requests: BTreeMap::new(),
            textures: BTreeMap::new(),
            answers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Asks for the image of `key`. A new address drops the texture of the old address.
    pub fn request(&mut self, key: K, url: &str, side: u32) {
        if self.asked.get(&key).is_some_and(|asked| asked == url) {
            return;
        }
        self.textures.remove(&key);
        self.asked.insert(key.clone(), url.to_owned());
        let answers = Arc::clone(&self.answers);
        let (answer_key, answer_url) = (key.clone(), url.to_owned());
        let request = self.cache.load(url, side, move |image| {
            answers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((answer_key.clone(), answer_url.clone(), image));
        });
        self.requests.insert(key, request);
    }

    /// Takes the finished images on the UI thread, and returns the new textures.
    pub fn poll(&mut self, context: &eframe::egui::Context) -> Vec<(K, TextureHandle)> {
        self.cache.poll();
        let answers = std::mem::take(
            &mut *self
                .answers
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let mut delivered = Vec::new();
        for (key, url, image) in answers {
            if self.asked.get(&key) != Some(&url) {
                continue;
            }
            self.requests.remove(&key);
            if let Some(image) = image {
                let texture = context.load_texture(
                    "remote-image",
                    image,
                    eframe::egui::TextureOptions::LINEAR,
                );
                self.textures.insert(key.clone(), texture.clone());
                delivered.push((key, texture));
            }
        }
        delivered
    }

    /// Current texture of one key.
    #[must_use]
    pub fn texture(&self, key: &K) -> Option<&TextureHandle> {
        self.textures.get(key)
    }

    /// Drops the keys that `keep` refuses. A later request for one of them uses the cache.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.asked.retain(|key, _| keep(key));
        self.requests.retain(|key, _| keep(key));
        self.textures.retain(|key, _| keep(key));
    }

    /// Stops the images in flight, and a later request for one of them asks again.
    pub fn stop(&mut self) {
        for key in self.requests.keys() {
            self.asked.remove(key);
        }
        self.requests.clear();
        self.answers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    /// Whether an image is in flight.
    #[must_use]
    pub fn busy(&self) -> bool {
        !self.requests.is_empty()
    }
}

/// Handle for one pending requester.
pub struct ImageRequest {
    id: u64,
    shared: Arc<Shared>,
}

impl ImageRequest {
    fn detached() -> Self {
        Self {
            id: 0,
            shared: Arc::new(Shared {
                state: Mutex::new(State::new()),
                wake: Condvar::new(),
                delivery_tx: mpsc::channel().0,
                delivery_rx: Mutex::new(mpsc::channel().1),
                fetcher: Arc::new(NullFetcher),
            }),
        }
    }

    /// Cancels only this requester, leaving other waiters for the same URL active.
    pub fn cancel(self) {}
}

impl Drop for ImageRequest {
    fn drop(&mut self) {
        if self.id == 0 {
            return;
        }
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.pending.remove(&self.id);
    }
}

struct NullFetcher;
impl ImageFetcher for NullFetcher {
    fn fetch(&self, _url: &str, _side: u32) -> Option<ColorImage> {
        None
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        let job = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            loop {
                if let Some(job) = state.jobs.pop_front() {
                    state.active_jobs += 1;
                    break job;
                }
                if state.closed {
                    return;
                }
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
        };
        // A runtime that shuts down during a fetch and a decoder that fails can both panic. The
        // worker must continue, and the requester gets its placeholder.
        let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            shared
                .fetcher
                .fetch(&job.url, job.side)
                .map(|image| cover_image(image, job.side))
        }))
        .unwrap_or(None);
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active_jobs = state.active_jobs.saturating_sub(1);
        state.queued.remove(&job.url);
        if !state.closed {
            state.insert(job.url.clone(), image.clone());
            let waiters: Vec<u64> = state
                .pending
                .iter()
                .filter(|(_, pending)| pending.url == job.url)
                .map(|(id, _)| *id)
                .collect();
            for id in waiters {
                let _ = shared.delivery_tx.send((id, image.clone()));
            }
        }
        shared.wake.notify_all();
    }
}

fn image_bytes(image: &ColorImage) -> usize {
    image.width() * image.height() * 4
}

fn cover_image(source: ColorImage, side: u32) -> ColorImage {
    let side = (side as usize).min(source.width()).min(source.height());
    if side == 0 {
        return ColorImage::default();
    }
    if source.width() == side * 2 && source.height() == side * 2 {
        let average = |a: u8, b: u8, c: u8, d: u8| {
            ((u32::from(a) + u32::from(b) + u32::from(c) + u32::from(d) + 2) / 4) as u8
        };
        let mut pixels = Vec::with_capacity(side * side);
        for y in 0..side {
            let top = &source.pixels[y * source.width() * 2..][..source.width()];
            let bottom = &source.pixels[(y * 2 + 1) * source.width()..][..source.width()];
            for (top, bottom) in top
                .as_chunks::<2>()
                .0
                .iter()
                .zip(bottom.as_chunks::<2>().0.iter())
            {
                let [a, b] = [top[0], bottom[0]];
                let [c, d] = [top[1], bottom[1]];
                pixels.push(eframe::egui::Color32::from_rgba_unmultiplied(
                    average(a.r(), b.r(), c.r(), d.r()),
                    average(a.g(), b.g(), c.g(), d.g()),
                    average(a.b(), b.b(), c.b(), d.b()),
                    average(a.a(), b.a(), c.a(), d.a()),
                ));
            }
        }
        return ColorImage::new([side, side], pixels);
    }
    super::bitmap::cover(&source, side as u32, side as u32)
}

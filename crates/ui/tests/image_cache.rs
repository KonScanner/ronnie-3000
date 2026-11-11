//! Bounded asynchronous artwork cache, fallback, and cancellation tests.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use eframe::egui::{self, Color32};
use ronie_core::{DownloadClient, HttpError, HttpResponse, OriginPolicy, RecordedTransport};
use ronie_ui::{
    default::{ART_SIDE, AVATAR_SIDE, ImageCache, ImageFetcher},
    services::{HttpImageFetcher, RemoteImages},
};
use url::Url;

#[derive(Default)]
struct ScriptedFetcher {
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
    fail: bool,
    delay_ms: u64,
}

impl ScriptedFetcher {
    fn with_delay(delay_ms: u64) -> Self {
        Self {
            delay_ms,
            ..Default::default()
        }
    }
}

impl ImageFetcher for ScriptedFetcher {
    fn fetch(&self, url: &str, _side: u32) -> Option<eframe::egui::ColorImage> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(self.delay_ms));
        self.active.fetch_sub(1, Ordering::SeqCst);
        if self.fail {
            return None;
        }
        let side = if url.contains("avatar") { 32 } else { 1024 };
        Some(eframe::egui::ColorImage::new(
            [side, side],
            vec![Color32::WHITE; side * side],
        ))
    }
}

fn load(
    cache: &ImageCache,
    url: &str,
    side: u32,
) -> (
    ronie_ui::default::ImageRequest,
    Arc<Mutex<Vec<Option<eframe::egui::ColorImage>>>>,
) {
    let results = Arc::new(Mutex::new(Vec::new()));
    let output = Arc::clone(&results);
    let request = cache.load(url, side, move |image| {
        output.lock().expect("results").push(image);
    });
    (request, results)
}

fn idle(cache: &ImageCache) {
    // A shared CI runner decodes the large test images slowly in a debug build, so the limit is
    // generous. A passing test still returns as soon as the workers are idle.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if cache.idle() {
            cache.poll();
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("image workers did not become idle");
}

#[test]
fn art_and_avatar_are_cover_cropped_without_upscaling() {
    let cache = ImageCache::new(Arc::new(ScriptedFetcher::default()));
    let (_art_request, art) = load(&cache, "https://img.example/amp", ART_SIDE);
    let (_avatar_request, avatar) = load(&cache, "https://img.example/avatar", AVATAR_SIDE);
    idle(&cache);
    assert_eq!(
        art.lock().expect("art")[0]
            .as_ref()
            .expect("art image")
            .size,
        [512, 512]
    );
    assert_eq!(
        avatar.lock().expect("avatar")[0]
            .as_ref()
            .expect("avatar image")
            .size,
        [32, 32]
    );
}

#[test]
fn failed_image_is_a_zero_byte_fallback_entry() {
    let cache = ImageCache::new(Arc::new(ScriptedFetcher {
        fail: true,
        ..Default::default()
    }));
    let (_request, result) = load(&cache, "https://img.example/missing", ART_SIDE);
    idle(&cache);
    assert!(result.lock().expect("result")[0].is_none());
    assert!(
        cache
            .cached("https://img.example/missing")
            .expect("cached")
            .is_none()
    );
    assert_eq!(cache.total_bytes(), 0);
    assert_eq!(cache.entries(), 1);
}

#[test]
fn duplicate_urls_share_one_fetch_and_all_waiters_resolve() {
    let fetcher = Arc::new(ScriptedFetcher::default());
    let cache = ImageCache::new(fetcher.clone());
    let (_first_request, first) = load(&cache, "https://img.example/shared", ART_SIDE);
    let (_second_request, second) = load(&cache, "https://img.example/shared", ART_SIDE);
    idle(&cache);
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
    assert_eq!(first.lock().expect("first").len(), 1);
    assert_eq!(second.lock().expect("second").len(), 1);
}

#[test]
fn decoded_budget_and_entry_count_evict_oldest_first() {
    let cache = ImageCache::new(Arc::new(ScriptedFetcher::default()));
    let mut requests = Vec::new();
    for index in 0..60 {
        requests.push(load(
            &cache,
            &format!("https://img.example/{index}"),
            ART_SIDE,
        ));
    }
    idle(&cache);
    assert!(
        cache.total_bytes() <= 48 * 1024 * 1024,
        "bytes {}",
        cache.total_bytes()
    );
    assert!(cache.entries() <= 48);
    assert!(cache.entries() > 0);
    assert!(cache.cached("https://img.example/0").is_none());
}

#[test]
fn worker_pool_never_exceeds_four_active_fetches() {
    let fetcher = Arc::new(ScriptedFetcher::with_delay(20));
    let cache = ImageCache::new(fetcher.clone());
    let mut requests = Vec::new();
    for index in 0..20 {
        requests.push(load(
            &cache,
            &format!("https://img.example/{index}"),
            ART_SIDE,
        ));
    }
    idle(&cache);
    assert!(fetcher.peak.load(Ordering::SeqCst) <= 4);
}

#[test]
fn close_cancels_callbacks_and_late_results_do_not_enter_cache() {
    let cache = ImageCache::new(Arc::new(ScriptedFetcher::with_delay(40)));
    let (_request, result) = load(&cache, "https://img.example/slow", ART_SIDE);
    cache.close();
    std::thread::sleep(Duration::from_millis(80));
    cache.poll();
    assert!(result.lock().expect("result").is_empty());
    assert!(cache.cached("https://img.example/slow").is_none());
    let (_late_request, late) = load(&cache, "https://img.example/late", ART_SIDE);
    cache.poll();
    assert!(late.lock().expect("late").is_empty());
}

#[test]
fn cancelling_one_requester_keeps_the_other_waiter() {
    let cache = ImageCache::new(Arc::new(ScriptedFetcher::with_delay(20)));
    let (request, cancelled) = load(&cache, "https://img.example/shared", ART_SIDE);
    let (_other_request, other) = load(&cache, "https://img.example/shared", ART_SIDE);
    request.cancel();
    idle(&cache);
    assert!(cancelled.lock().expect("cancelled").is_empty());
    assert_eq!(other.lock().expect("other").len(), 1);
}

#[test]
fn cache_can_be_cloned_for_shared_browser_consumers() {
    let cache = ImageCache::new(Arc::new(ScriptedFetcher::default()));
    let clone = cache.clone();
    let (_request, result) = load(&clone, "https://img.example/shared", ART_SIDE);
    idle(&cache);
    assert!(cache.cached("https://img.example/shared").is_some());
    assert_eq!(result.lock().expect("result").len(), 1);
}

#[test]
fn failed_entries_also_have_a_hard_entry_bound() {
    let cache = ImageCache::new(Arc::new(ScriptedFetcher {
        fail: true,
        ..Default::default()
    }));
    let mut requests = BTreeMap::new();
    for index in 0..520 {
        requests.insert(
            index,
            load(&cache, &format!("https://img.example/{index}"), ART_SIDE),
        );
    }
    idle(&cache);
    assert!(cache.entries() <= 512);
}

struct GradientFetcher;

impl ImageFetcher for GradientFetcher {
    fn fetch(&self, _url: &str, _side: u32) -> Option<eframe::egui::ColorImage> {
        Some(eframe::egui::ColorImage::new(
            [2, 2],
            vec![
                Color32::from_gray(0),
                Color32::from_gray(64),
                Color32::from_gray(128),
                Color32::from_gray(255),
            ],
        ))
    }
}

#[test]
fn cache_cover_averages_all_source_pixels() {
    let cache = ImageCache::new(Arc::new(GradientFetcher));
    let (_request, result) = load(&cache, "https://img.example/gradient", 1);
    idle(&cache);
    let image = result.lock().expect("result")[0]
        .as_ref()
        .expect("gradient image")
        .clone();
    assert_eq!(image.pixels, vec![Color32::from_gray(112)]);
}

struct WideFetcher;

impl ImageFetcher for WideFetcher {
    fn fetch(&self, _url: &str, _side: u32) -> Option<eframe::egui::ColorImage> {
        let mut image = eframe::egui::ColorImage::new([4, 2], vec![Color32::BLACK; 8]);
        for y in 0..2 {
            for x in 1..3 {
                image.pixels[y * 4 + x] = Color32::WHITE;
            }
        }
        Some(image)
    }
}

#[test]
fn cache_cover_crops_from_the_center() {
    let cache = ImageCache::new(Arc::new(WideFetcher));
    let (_request, result) = load(&cache, "https://img.example/wide", 2);
    idle(&cache);
    let image = result.lock().expect("result")[0]
        .as_ref()
        .expect("wide image")
        .clone();
    assert_eq!(image.pixels, vec![Color32::WHITE; 4]);
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("png");
    bytes.into_inner()
}

/// A fetcher over recorded answers, and the runtime that it needs.
fn http_fetcher(
    responses: Vec<Result<HttpResponse, HttpError>>,
) -> (
    HttpImageFetcher<RecordedTransport>,
    RecordedTransport,
    tokio::runtime::Runtime,
) {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let transport = RecordedTransport::new(responses);
    let origin = Url::parse("https://api.example").expect("origin");
    let policy = OriginPolicy::new(
        origin,
        [Url::parse("https://images.example").expect("public images")],
    );
    let fetcher = HttpImageFetcher::new(
        DownloadClient::new(transport.clone(), policy),
        runtime.handle().clone(),
    );
    (fetcher, transport, runtime)
}

#[test]
fn an_http_image_is_decoded_on_the_calling_thread_with_no_token() {
    let (fetcher, transport, _runtime) = http_fetcher(vec![Ok(HttpResponse::new(200, png(3, 2)))]);

    let image = fetcher
        .fetch("https://images.example/art.png", ART_SIDE)
        .expect("image");

    assert_eq!(image.size, [3, 2]);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("authorization")),
        "an image request has no token"
    );
}

#[test]
fn an_http_404_or_failure_falls_back_with_no_retry() {
    for (name, response) in [
        ("404", Ok(HttpResponse::new(404, Vec::new()))),
        ("500", Ok(HttpResponse::new(500, Vec::new()))),
        (
            "not an image",
            Ok(HttpResponse::new(200, b"not an image".to_vec())),
        ),
        ("no answer", Err(HttpError::Transport)),
        ("too large", Err(HttpError::ResponseTooLarge { max: 1 })),
    ] {
        let (fetcher, transport, _runtime) = http_fetcher(vec![response]);

        assert!(
            fetcher
                .fetch("https://images.example/art.png", ART_SIDE)
                .is_none(),
            "{name}"
        );
        assert_eq!(transport.requests().len(), 1, "{name}");
    }
}

#[test]
fn an_address_that_is_not_approved_is_refused_before_transmission() {
    for url in [
        "http://images.example/art.png",
        "https://elsewhere.example/art.png",
        "https://user:secret@images.example/art.png",
        "file:///tmp/art.png",
        "not a url",
    ] {
        let (fetcher, transport, _runtime) = http_fetcher(Vec::new());

        assert!(fetcher.fetch(url, ART_SIDE).is_none(), "{url}");
        assert!(transport.requests().is_empty(), "{url}");
    }
}

#[test]
fn an_image_past_the_decode_limit_falls_back() {
    let (fetcher, _transport, _runtime) =
        http_fetcher(vec![Ok(HttpResponse::new(200, png(8193, 1)))]);

    assert!(
        fetcher
            .fetch("https://images.example/wide.png", ART_SIDE)
            .is_none()
    );
}

#[test]
fn http_images_stay_inside_the_side_and_the_budget_of_the_cache() {
    let (fetcher, _transport, _runtime) =
        http_fetcher(vec![Ok(HttpResponse::new(200, png(2048, 1024)))]);
    let cache = ImageCache::new(Arc::new(fetcher));

    let (_request, result) = load(&cache, "https://images.example/art.png", ART_SIDE);
    idle(&cache);

    let side = ART_SIDE as usize;
    assert_eq!(
        result.lock().expect("result")[0]
            .as_ref()
            .expect("image")
            .size,
        [side, side]
    );
    assert_eq!(cache.total_bytes(), side * side * 4);
    cache.close();
}

struct PanickingFetcher;

impl ImageFetcher for PanickingFetcher {
    fn fetch(&self, _url: &str, _side: u32) -> Option<eframe::egui::ColorImage> {
        panic!("a fetch failure");
    }
}

#[test]
fn a_fetch_that_panics_falls_back_and_the_worker_continues() {
    let cache = ImageCache::new(Arc::new(PanickingFetcher));
    for index in 0..8 {
        let url = format!("https://img.example/{index}");
        let (_request, result) = load(&cache, &url, ART_SIDE);
        idle(&cache);
        assert!(result.lock().expect("result")[0].is_none(), "{url}");
    }
    assert_eq!(cache.entries(), 8);
}

fn scripted_cache() -> (ImageCache, Arc<ScriptedFetcher>) {
    let fetcher = Arc::new(ScriptedFetcher::default());
    (ImageCache::new(fetcher.clone()), fetcher)
}

fn delivered<K: Ord + Clone + Send + 'static>(
    images: &mut RemoteImages<K>,
    context: &egui::Context,
) -> Vec<K> {
    let mut keys = Vec::new();
    for _ in 0..400 {
        keys.extend(images.poll(context).into_iter().map(|(key, _)| key));
        if !images.busy() {
            return keys;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the images did not arrive");
}

#[test]
fn a_view_receives_one_texture_for_each_key_and_keeps_it() {
    let (cache, fetcher) = scripted_cache();
    let context = egui::Context::default();
    let mut images = RemoteImages::new(cache);

    images.request(1, "https://img.example/art", ART_SIDE);
    images.request(1, "https://img.example/art", ART_SIDE);
    images.request(2, "https://img.example/art", ART_SIDE);

    let keys = delivered(&mut images, &context);
    assert_eq!(keys.len(), 2);
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1, "one shared fetch");
    let side = ART_SIDE as usize;
    assert_eq!(images.texture(&1).expect("texture").size(), [side, side]);
    assert!(images.poll(&context).is_empty());
    assert!(images.texture(&2).is_some());
}

#[test]
fn a_failed_image_stays_a_placeholder_and_is_not_asked_for_again() {
    let fetcher = Arc::new(ScriptedFetcher {
        fail: true,
        ..Default::default()
    });
    let context = egui::Context::default();
    let mut images = RemoteImages::new(ImageCache::new(fetcher.clone()));

    images.request(1, "https://img.example/missing", ART_SIDE);
    assert!(delivered(&mut images, &context).is_empty());
    for _ in 0..50 {
        images.request(1, "https://img.example/missing", ART_SIDE);
        images.poll(&context);
    }
    std::thread::sleep(Duration::from_millis(20));

    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
    assert!(images.texture(&1).is_none());
    assert!(!images.busy());
}

#[test]
fn a_failed_image_stays_failed_after_the_cache_forgets_it() {
    let fetcher = Arc::new(ScriptedFetcher {
        fail: true,
        ..Default::default()
    });
    let cache = ImageCache::new(fetcher.clone());
    let context = egui::Context::default();
    let mut images = RemoteImages::new(cache.clone());
    images.request(0, "https://img.example/missing", ART_SIDE);
    assert!(delivered(&mut images, &context).is_empty());

    // More than the entry limit of the cache.
    for index in 0..600 {
        let url = format!("https://img.example/{index}");
        drop(cache.load(&url, ART_SIDE, |_| {}));
        if index % 16 == 0 {
            idle(&cache);
        }
    }
    idle(&cache);
    let calls = fetcher.calls.load(Ordering::SeqCst);
    assert!(cache.cached("https://img.example/missing").is_none());
    images.request(0, "https://img.example/missing", ART_SIDE);
    std::thread::sleep(Duration::from_millis(20));

    assert_eq!(fetcher.calls.load(Ordering::SeqCst), calls);
}

#[test]
fn a_new_address_for_a_key_drops_the_old_texture_and_ignores_the_old_answer() {
    let fetcher = Arc::new(ScriptedFetcher::with_delay(30));
    let context = egui::Context::default();
    let mut images = RemoteImages::new(ImageCache::new(fetcher));

    images.request(1, "https://img.example/old", ART_SIDE);
    images.request(1, "https://img.example/new", AVATAR_SIDE);
    let keys = delivered(&mut images, &context);
    assert_eq!(keys, [1]);

    images.request(1, "https://img.example/newer", ART_SIDE);
    assert!(
        images.texture(&1).is_none(),
        "the old picture is gone at once"
    );
}

#[test]
fn retain_drops_the_textures_of_other_keys_and_a_later_request_uses_the_cache() {
    let (cache, fetcher) = scripted_cache();
    let context = egui::Context::default();
    let mut images = RemoteImages::new(cache);
    images.request(1, "https://img.example/one", ART_SIDE);
    images.request(2, "https://img.example/two", ART_SIDE);
    assert_eq!(delivered(&mut images, &context).len(), 2);

    images.retain(|key| *key == 2);

    assert!(images.texture(&1).is_none());
    assert!(images.texture(&2).is_some());
    images.request(1, "https://img.example/one", ART_SIDE);
    assert_eq!(delivered(&mut images, &context), [1]);
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 2, "no second fetch");
}

#[test]
fn stop_cancels_the_work_and_a_later_request_asks_again() {
    let fetcher = Arc::new(ScriptedFetcher::with_delay(20));
    let context = egui::Context::default();
    let cache = ImageCache::new(fetcher);
    let mut images = RemoteImages::new(cache.clone());
    images.request(1, "https://img.example/one", ART_SIDE);

    images.stop();

    assert!(!images.busy());
    idle(&cache);
    assert!(images.poll(&context).is_empty());
    images.request(1, "https://img.example/one", ART_SIDE);
    assert_eq!(delivered(&mut images, &context), [1]);
}

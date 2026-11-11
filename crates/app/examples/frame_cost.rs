//! Measures the CPU cost of one real UI frame on the real standalone backend and a dummy device.
//!
//! Run an optimized build with no window:
//!
//! ```text
//! cargo run --release -p ronie-app --example frame_cost
//! ```
//!
//! Other modes: `--actions MODEL.nam` times edits with a real model, and `--blur` times the backdrop
//! blur.
//!
//! This tool uses a temporary settings directory, a dummy device, and a fake service. It opens no
//! audio device and uses no network. With `--window`, it opens a real GL window, so it runs only on
//! a private display, as `script/check-ui.sh` does.

use std::{
    collections::BTreeMap,
    io::Cursor,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use eframe::egui;
use ronie_app::{
    audio::{AudioBackendKind, DummyBackend},
    standalone_backend::StandaloneBackend,
};
use ronie_core::{
    BlockId, ChainSnapshot, HttpError, HttpRequest, HttpResponse, HttpTransport, InputMode,
    ModelBytes, ModelId, OAuthConfig, OriginPolicy, PREFERENCES_SETTINGS, SettingsStore, Tone,
    UI_PREFERENCES_SETTINGS,
    params::{ParameterId, ParameterValue},
};
use ronie_dsp::PrepareSpec;
use ronie_plugin::{ProcessorBackend, RonieProcessor};
use ronie_ui::{
    backend::{
        ChainFault, ChainViewState, GesturePhase, HostControl, MeterSnapshot, UiAction, UiBackend,
        UiError, UiReceipt,
    },
    default::{BlockView, ChromeController, MidiMapSnapshot, SystemDeviceState},
    fonts::Fonts,
    services::community::CommunityService,
};
use url::Url;

const ORIGIN: &str = "https://api.example";
/// Tones on one page of the service, as the real catalog sends them.
const PAGE_SIZE: usize = 20;
/// Models of one tone. A real tone often has many models, and each one is in its raw document.
const MODELS_PER_TONE: usize = 24;
const WARM_FRAMES: usize = 90;
const MEASURED_FRAMES: usize = 400;
/// The window of the real app, in points.
const WINDOW: egui::Vec2 = egui::vec2(1024.0, 614.0);

/// A service that answers from memory, so that images take the real worker and decode path.
#[derive(Clone)]
struct FakeService {
    artwork: Arc<Vec<u8>>,
    page: Arc<Vec<u8>>,
}

impl HttpTransport for FakeService {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let path = request.url.path();
        let body = if path.starts_with("/img/") {
            self.artwork.as_ref().clone()
        } else if path.starts_with("/api/v1/tones") {
            self.page.as_ref().clone()
        } else {
            return Ok(HttpResponse {
                status: 404,
                body: Vec::new(),
            });
        };
        Ok(HttpResponse { status: 200, body })
    }
}

/// A photo-like image with fine detail, at twice the side that the app keeps.
fn artwork_jpeg() -> Vec<u8> {
    let side = 1024_u32;
    let image = image::RgbImage::from_fn(side, side, |x, y| {
        let wave = |a: u32, b: u32| (((x * a) ^ (y * b)) % 255) as u8;
        image::Rgb([wave(3, 5), wave(7, 2), wave(1, 9)])
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .expect("encoding to memory");
    bytes
}

fn tone_value(id: i64) -> serde_json::Value {
    let models: Vec<_> = (0..MODELS_PER_TONE as i64)
        .map(|index| {
            serde_json::json!({
                "id": id * 100 + index,
                "name": format!("AC30-6 TB Brl {index}"),
                "model_url": format!("https://models.example/{id}/{index}.wav"),
                "architecture": "WaveNet",
                "size_bytes": 1_234_567,
            })
        })
        .collect();
    serde_json::json!({
        "id": id,
        "title": format!("'02 Vox AC30/6 Top Boost {id}"),
        "description": "A long description of the amplifier and the capture. ".repeat(20),
        "gear": "amp",
        "format": "ir",
        "models_count": MODELS_PER_TONE,
        "a2_models_count": MODELS_PER_TONE,
        "downloads_count": 5246,
        "favorites_count": 320,
        "published_at": "2026-06-01T12:00:00Z",
        "images": [format!("{ORIGIN}/img/art-{id}.jpg")],
        "user": {
            "username": format!("creator{id}"),
            "display_name": format!("Creator {id}"),
            "is_verified": true,
            "avatar_url": format!("{ORIGIN}/img/avatar-{id}.jpg"),
        },
        "models": models,
        "makes": [{"id": 11, "name": "Vox"}, {"id": 12, "name": "AC30/6 Top Boost"}],
        "tags": [{"id": 1, "name": "rock"}, {"id": 2, "name": "british"}],
    })
}

fn impulse_response() -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36_u32 + 8).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&8_u32.to_le_bytes());
    for sample in [16_000_i16, 0, 0, 0] {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Counts and times each call that the UI makes on the real backend.
struct CountingBackend {
    inner: Arc<StandaloneBackend>,
    calls: Mutex<BTreeMap<&'static str, (u64, Duration)>>,
    /// The chain snapshot of the last view, to count each new one.
    last_snapshot: Mutex<Option<Arc<ChainSnapshot>>>,
}

/// The name of the count of new chain snapshots. A new snapshot is a copy of the chain.
const NEW_SNAPSHOT: &str = "new chain snapshot";

impl CountingBackend {
    fn timed<R>(&self, name: &'static str, call: impl FnOnce(&StandaloneBackend) -> R) -> R {
        let started = Instant::now();
        let result = call(&self.inner);
        let spent = started.elapsed();
        let mut calls = self.calls.lock().expect("no panic holds this lock");
        let entry = calls.entry(name).or_default();
        entry.0 += 1;
        entry.1 += spent;
        result
    }

    fn take(&self) -> BTreeMap<&'static str, (u64, Duration)> {
        std::mem::take(&mut *self.calls.lock().expect("no panic holds this lock"))
    }

    /// Counts a view whose chain snapshot is not the snapshot of the last view.
    fn count_snapshot(&self, chain: &Arc<ChainSnapshot>) {
        let mut last = self.last_snapshot.lock().expect("no panic holds this lock");
        if last.as_ref().is_some_and(|known| Arc::ptr_eq(known, chain)) {
            return;
        }
        *last = Some(Arc::clone(chain));
        drop(last);
        let mut calls = self.calls.lock().expect("no panic holds this lock");
        calls.entry(NEW_SNAPSHOT).or_default().0 += 1;
    }
}

impl UiBackend for CountingBackend {
    fn chain_state(&self, known_revision: u64) -> ChainViewState {
        let view = self.timed("chain_state", |inner| inner.chain_state(known_revision));
        self.count_snapshot(&view.chain);
        view
    }
    fn meter_levels(&self) -> MeterSnapshot {
        self.timed("meter_levels", UiBackend::meter_levels)
    }
    fn spectrum_bins(&self, block_id: &BlockId) -> [f32; 64] {
        self.timed("spectrum_bins", |inner| inner.spectrum_bins(block_id))
    }
    fn tuner_reading(&self) -> ronie_dsp::analysis::TunerReading {
        self.timed("tuner_reading", UiBackend::tuner_reading)
    }
    fn supports(&self, control: HostControl) -> bool {
        self.timed("supports", |inner| inner.supports(control))
    }
    fn diagnostic_log(&self) -> Option<String> {
        self.timed("diagnostic_log", UiBackend::diagnostic_log)
    }
    fn device_settings(&self) -> Option<SystemDeviceState> {
        self.timed("device_settings", UiBackend::device_settings)
    }
    fn chain_faults(&self) -> Vec<ChainFault> {
        self.timed("chain_faults", UiBackend::chain_faults)
    }
    fn midi_map(&self) -> MidiMapSnapshot {
        self.timed("midi_map", UiBackend::midi_map)
    }
    fn calibration_draft(&self) -> Option<String> {
        self.timed("calibration_draft", UiBackend::calibration_draft)
    }
    fn preference(&self, key: &str) -> Option<bool> {
        self.timed("preference", |inner| inner.preference(key))
    }
    fn dispatch(&self, action: UiAction) -> Result<UiReceipt, UiError> {
        self.timed("dispatch", |inner| inner.dispatch(action))
    }
    fn validate_model(
        &self,
        tone: &Tone,
        model_id: ModelId,
        bytes: &ModelBytes,
    ) -> Result<(), UiError> {
        self.inner.validate_model(tone, model_id, bytes)
    }
    fn settle_audio(&self) -> Result<(), UiError> {
        self.inner.settle_audio()
    }
    fn can_paste_block(&self) -> bool {
        self.timed("can_paste_block", UiBackend::can_paste_block)
    }
    fn parameter(&self, id: ParameterId) -> Option<ParameterValue> {
        self.timed("parameter", |inner| inner.parameter(id))
    }
    fn set_parameter(
        &self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<(), UiError> {
        self.timed("set_parameter", |inner| {
            inner.set_parameter(id, value, gesture)
        })
    }
}

/// The real backend with a loaded chain, the real controller, and the fake service.
struct Bench {
    backend: Arc<StandaloneBackend>,
    counting: Arc<CountingBackend>,
    chrome: ChromeController,
    blocks: Vec<BlockId>,
    phase: f32,
    _root: tempfile::TempDir,
}

impl Bench {
    fn new(tones: usize) -> Self {
        let root = tempfile::tempdir().expect("temporary root");
        let processor = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        }));
        // A dummy device with the timing of a real stream: each compile runs on the chain worker.
        let backend = Arc::new(
            StandaloneBackend::with_audio_backend(
                processor,
                &root.path().join(PREFERENCES_SETTINGS),
                Arc::new(|_| Box::new(DummyBackend::connected())),
                AudioBackendKind::Dummy,
            )
            .expect("dummy backend"),
        );
        backend.settle_audio().expect("first open");
        let mut blocks = Vec::new();
        for id in 0..tones {
            let target = backend
                .chain_state(0)
                .chain
                .left
                .iter()
                .find(|block| block.is_insert)
                .expect("an empty slot")
                .id
                .clone();
            let tone = Tone::from_value(tone_value(id as i64 + 1)).expect("tone");
            let model_id = tone.models[0].id;
            backend
                .dispatch(UiAction::LoadToneWithBytes {
                    target_insert_id: target.clone(),
                    tone,
                    model_id,
                    bytes: impulse_response().into(),
                })
                .expect("load");
            backend.settle_audio().expect("settle");
            blocks.push(target);
        }
        let counting = Arc::new(CountingBackend {
            inner: backend.clone(),
            calls: Mutex::default(),
            last_snapshot: Mutex::default(),
        });
        let page = serde_json::json!({
            "data": (1..=PAGE_SIZE as i64).map(|id| tone_value(id + 100)).collect::<Vec<_>>(),
            "page": 1,
            "page_size": PAGE_SIZE,
            "total": PAGE_SIZE * 3,
            "total_pages": 3,
        });
        let transport = FakeService {
            artwork: Arc::new(artwork_jpeg()),
            page: Arc::new(serde_json::to_vec(&page).expect("page")),
        };
        let origin = Url::parse(ORIGIN).expect("origin");
        let community = CommunityService::new(
            OAuthConfig::new(origin.clone(), "public-client"),
            OriginPolicy::new(
                origin,
                [Url::parse("https://models.example").expect("origin")],
            ),
            transport,
            SettingsStore::standalone(root.path(), UI_PREFERENCES_SETTINGS),
        );
        let mut chrome =
            ChromeController::with_community(counting.clone(), Vec::new(), false, true, community);
        chrome.set_standalone(true);
        Self {
            backend,
            counting,
            chrome,
            blocks,
            phase: 0.0,
            _root: root,
        }
    }

    /// Gives the meters and the analysis real signal, as the audio thread does.
    fn play_signal(&mut self) {
        let step = std::f32::consts::TAU * 110.0 / 48_000.0;
        let frames = self
            .backend
            .device_settings()
            .expect("device")
            .banner
            .buffer_size as usize;
        let input: Vec<f32> = (0..frames)
            .map(|_| {
                self.phase = (self.phase + step) % std::f32::consts::TAU;
                0.25 * self.phase.sin()
            })
            .collect();
        for _ in 0..3 {
            self.backend.render_dummy(&input).expect("render");
        }
    }

    fn open(&mut self, page: Page) {
        self.chrome.go_to_main();
        match page {
            Page::Chain | Page::ChainPointer | Page::KnobDrag => {}
            Page::Browser => {
                let insert = self
                    .backend
                    .chain_state(0)
                    .chain
                    .left
                    .iter()
                    .find(|block| block.is_insert)
                    .expect("an empty slot")
                    .id
                    .clone();
                self.chrome.gallery_mut().activate(&insert.0);
            }
            Page::Block => {
                self.chrome.open_detail(&self.blocks[0].0).expect("detail");
            }
            Page::Eq => {
                self.chrome.open_detail(&self.blocks[0].0).expect("detail");
                self.chrome
                    .detail_mut()
                    .expect("detail")
                    .set_view(BlockView::Eq);
            }
            Page::Tuner => self.chrome.set_tuner(true),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Chain,
    ChainPointer,
    KnobDrag,
    Browser,
    Block,
    Eq,
    Tuner,
}

impl Page {
    const ALL: [Self; 7] = [
        Self::Chain,
        Self::ChainPointer,
        Self::KnobDrag,
        Self::Browser,
        Self::Block,
        Self::Eq,
        Self::Tuner,
    ];
}

/// The pointer events of one frame of a page that has a moving pointer.
fn pointer_events(page: Page, frame: usize) -> Vec<egui::Event> {
    if page != Page::ChainPointer {
        return Vec::new();
    }
    let x = 120.0 + (frame % 600) as f32;
    vec![egui::Event::PointerMoved(egui::pos2(x, 260.0))]
}

fn raw_input(started: Instant, pixels_per_point: f32, events: Vec<egui::Event>) -> egui::RawInput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW)),
        time: Some(started.elapsed().as_secs_f64()),
        predicted_dt: 1.0 / 60.0,
        events,
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(pixels_per_point);
    input
}

/// Sorted samples in milliseconds.
struct Samples(Vec<f64>);

impl Samples {
    fn of(mut values: Vec<f64>) -> Self {
        values.sort_by(f64::total_cmp);
        Self(values)
    }
    fn mean(&self) -> f64 {
        self.0.iter().sum::<f64>() / self.0.len().max(1) as f64
    }
    fn at(&self, fraction: f64) -> f64 {
        let last = self.0.len().saturating_sub(1);
        self.0
            .get((last as f64 * fraction).round() as usize)
            .copied()
            .unwrap_or_default()
    }
}

fn measure(bench: &mut Bench, page: Page, pixels_per_point: f32, accesskit: bool) {
    let context = egui::Context::default();
    if accesskit {
        context.enable_accesskit();
    }
    Fonts::install(&context);
    bench.open(page);
    let started = Instant::now();
    let mut logic_ms = Vec::with_capacity(MEASURED_FRAMES);
    let mut pass_ms = Vec::with_capacity(MEASURED_FRAMES);
    let mut tessellate_ms = Vec::with_capacity(MEASURED_FRAMES);
    let mut total_ms = Vec::with_capacity(MEASURED_FRAMES);
    let mut shown_device = None;
    let mut shown_faults = Vec::new();
    let mut primitives = 0;
    let mut vertices = 0;
    let parameter = ParameterId("inputLevel");
    for frame in 0..WARM_FRAMES + MEASURED_FRAMES {
        bench.play_signal();
        if frame == WARM_FRAMES {
            let _ = bench.counting.take();
        }
        let begin = Instant::now();
        bench.chrome.poll_auth();
        bench.chrome.poll_community();
        ronie_ui::shell::sync_device_banner(
            bench.counting.as_ref(),
            &mut bench.chrome,
            &mut shown_device,
            0,
        );
        ronie_ui::shell::sync_chain_faults(
            bench.counting.as_ref(),
            &mut bench.chrome,
            &mut shown_faults,
            0,
        );
        let after_logic = Instant::now();
        if page == Page::KnobDrag {
            // A knob drag writes one value in each frame, on the UI thread.
            let value = 0.25 + 0.5 * ((frame % 100) as f32 / 100.0);
            let _ = bench.counting.set_parameter(
                parameter,
                ParameterValue(value),
                GesturePhase::Change,
            );
        }
        let chrome = &mut bench.chrome;
        let mut output = context.run_ui(
            raw_input(started, pixels_per_point, pointer_events(page, frame)),
            |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                    .show(ui, |ui| ronie_ui::default::show(ui, chrome));
            },
        );
        let after_pass = Instant::now();
        let shapes = std::mem::take(&mut output.shapes);
        let clipped = context.tessellate(shapes, output.pixels_per_point);
        let after_tessellate = Instant::now();
        output.textures_delta.clear();
        if frame >= WARM_FRAMES {
            logic_ms.push((after_logic - begin).as_secs_f64() * 1e3);
            pass_ms.push((after_pass - after_logic).as_secs_f64() * 1e3);
            tessellate_ms.push((after_tessellate - after_pass).as_secs_f64() * 1e3);
            total_ms.push((after_tessellate - begin).as_secs_f64() * 1e3);
            primitives = clipped.len();
            vertices = clipped
                .iter()
                .map(|clipped| match &clipped.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => mesh.vertices.len(),
                    egui::epaint::Primitive::Callback(_) => 0,
                })
                .sum();
        }
    }
    let (logic, pass, tessellate, total) = (
        Samples::of(logic_ms),
        Samples::of(pass_ms),
        Samples::of(tessellate_ms),
        Samples::of(total_ms),
    );
    println!(
        "| {page:?} | {pixels_per_point} | {accesskit} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {primitives} | {vertices} |",
        logic.mean(),
        pass.mean(),
        tessellate.mean(),
        total.mean(),
        total.at(0.95),
        total.at(1.0),
    );
    let mut calls = bench.counting.take();
    let snapshots = calls.remove(NEW_SNAPSHOT).map_or(0, |(count, _)| count);
    let mut calls: Vec<_> = calls.into_iter().collect();
    calls.sort_by_key(|(_, (_, spent))| std::cmp::Reverse(*spent));
    let frames = MEASURED_FRAMES as f64;
    let text: Vec<String> = calls
        .iter()
        .take(6)
        .map(|(name, (count, spent))| {
            format!(
                "{name} {:.1}/frame {:.1} us/frame",
                *count as f64 / frames,
                spent.as_secs_f64() * 1e6 / frames
            )
        })
        .collect();
    eprintln!("  {page:?} calls: {}", text.join("; "));
    eprintln!(
        "  {page:?} new chain snapshots: {:.2}/frame",
        snapshots as f64 / frames
    );
}

/// Times each edit on the calling thread, as a click on the UI thread runs it, with real models.
fn measure_actions(model: &std::path::Path, count: usize) {
    let bench = Bench::new(0);
    let bytes = std::fs::read(model).expect("a model file");
    let mut value = tone_value(1);
    value["format"] = serde_json::json!("nam");
    let backend = bench.backend.clone();
    let start = Instant::now();
    let mut blocks = Vec::new();
    for _ in 0..count {
        let target = backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .find(|block| block.is_insert)
            .expect("an empty slot")
            .id
            .clone();
        let tone = Tone::from_value(value.clone()).expect("tone");
        let model_id = tone.models[0].id;
        let begin = Instant::now();
        backend
            .dispatch(UiAction::LoadToneWithBytes {
                target_insert_id: target.clone(),
                tone,
                model_id,
                bytes: bytes.clone().into(),
            })
            .expect("load");
        let dispatched = begin.elapsed();
        let begin = Instant::now();
        backend.settle_audio().expect("settle");
        println!(
            "| LoadToneWithBytes {} | {:.2} | settle off the UI thread {:.2} ms |",
            blocks.len() + 1,
            dispatched.as_secs_f64() * 1e3,
            begin.elapsed().as_secs_f64() * 1e3,
        );
        blocks.push(target);
    }
    eprintln!("loaded in {:?}", start.elapsed());
    let timed = |name: &str, action: UiAction| {
        let begin = Instant::now();
        let result = backend.dispatch(action);
        println!(
            "| {name} | {:.2} | {} |",
            begin.elapsed().as_secs_f64() * 1e3,
            result.map_or_else(|error| error.to_string(), |_| "true".to_owned())
        );
    };
    timed("RemoveBlock", UiAction::RemoveBlock(blocks[0].clone()));
    timed("Undo", UiAction::Undo);
    timed("Redo", UiAction::Redo);
    timed("Undo", UiAction::Undo);
    timed("SetStereoMode on", UiAction::SetStereoMode(true));
    timed("SetStereoMode off", UiAction::SetStereoMode(false));
    timed(
        "DuplicateBlock",
        UiAction::DuplicateBlock(blocks[0].clone()),
    );
    timed("Undo", UiAction::Undo);
    // A preset load restores every model of the preset. Another structure comes first, so that
    // the load needs a new chain.
    backend.settle_audio().expect("settle");
    timed(
        "SavePreset",
        UiAction::SavePreset {
            name: "Bench".to_owned(),
        },
    );
    let preset = backend
        .chain_state(0)
        .session
        .active_preset
        .clone()
        .expect("the saved preset is active")
        .id;
    timed("RemoveBlock", UiAction::RemoveBlock(blocks[0].clone()));
    backend.settle_audio().expect("settle");
    timed(
        &format!("LoadPreset with {count} models"),
        UiAction::LoadPreset { id: preset },
    );
    let begin = Instant::now();
    backend.settle_audio().expect("settle");
    println!(
        "| settle after LoadPreset | {:.2} | off the UI thread |",
        begin.elapsed().as_secs_f64() * 1e3
    );
    let begin = Instant::now();
    let enabled = !backend
        .chain_state(0)
        .chain
        .block(&blocks[0])
        .expect("block")
        .params
        .enabled;
    println!(
        "| chain_state with {count} models | {:.2} | {enabled} |",
        begin.elapsed().as_secs_f64() * 1e3
    );

    // How long a frame waits for the backend while a compile runs on another thread.
    let worker_backend = backend.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_stop = stop.clone();
    let target = blocks[0].clone();
    let worker = std::thread::spawn(move || {
        let mut loads: i64 = 0;
        while !worker_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let active = worker_backend
                .chain_state(0)
                .chain
                .block(&target)
                .and_then(|block| block.active_model_id)
                .expect("an active model");
            let model_id = ModelId(5_000 + loads);
            let mut document = value.clone();
            document["models"] = serde_json::json!([{
                "id": model_id.0,
                "name": "Next",
                "model_url": "https://models.example/next.nam",
            }]);
            let tone = Tone::from_value(document).expect("tone");
            worker_backend
                .dispatch(UiAction::SelectModelWithBytes {
                    block_id: target.clone(),
                    tone,
                    model_id,
                    replaces: Some(active),
                    bytes: bytes.clone().into(),
                })
                .expect("select");
            worker_backend.settle_audio().expect("settle");
            loads += 1;
        }
        loads
    });
    let mut chain_ms = Vec::new();
    let mut meter_ms = Vec::new();
    let begin = Instant::now();
    while begin.elapsed() < Duration::from_secs(4) {
        let started = Instant::now();
        let _ = backend.chain_state(0);
        chain_ms.push(started.elapsed().as_secs_f64() * 1e3);
        let started = Instant::now();
        let _ = backend.meter_levels();
        meter_ms.push(started.elapsed().as_secs_f64() * 1e3);
        std::thread::sleep(Duration::from_millis(1));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let loads = worker.join().expect("worker");
    let (chain, meter) = (Samples::of(chain_ms), Samples::of(meter_ms));
    println!(
        "| chain_state during {loads} model changes | mean {:.3} | p99 {:.3} max {:.3} |",
        chain.mean(),
        chain.at(0.99),
        chain.at(1.0)
    );
    println!(
        "| meter_levels during {loads} model changes | mean {:.3} | p99 {:.3} max {:.3} |",
        meter.mean(),
        meter.at(0.99),
        meter.at(1.0)
    );
}

/// Runs the real GL frame loop with the options of the real shell.
mod window {
    use super::{Bench, Duration, Fonts, Instant, Page, PathBuf, Samples, egui, pointer_events};

    pub(super) struct Options {
        pub vsync: bool,
        pub capture: Option<PathBuf>,
        pub seconds: f32,
        pub periodic_capture: bool,
    }

    struct App {
        bench: Bench,
        pages: Vec<Page>,
        options: Options,
        page_started: Instant,
        last_begin: Option<Instant>,
        frame: usize,
        ui_ms: Vec<f64>,
        interval_ms: Vec<f64>,
        cpu_ms: Vec<f64>,
        hover: bool,
        started: Instant,
        modal_capture: ronie_ui::default::ModalCapture,
        backdrop_tx: std::sync::mpsc::Sender<std::sync::Arc<egui::ColorImage>>,
        backdrop_rx: std::sync::mpsc::Receiver<std::sync::Arc<egui::ColorImage>>,
        capture_ms: Vec<f64>,
    }

    impl App {
        fn report(&mut self, page: Page) {
            let ui = Samples::of(std::mem::take(&mut self.ui_ms));
            let interval = Samples::of(std::mem::take(&mut self.interval_ms));
            let cpu = Samples::of(std::mem::take(&mut self.cpu_ms));
            let capture = Samples::of(std::mem::take(&mut self.capture_ms));
            println!(
                "| {page:?} | {} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {} | {:.2} | {} |",
                self.options.vsync,
                self.options.periodic_capture,
                ui.mean(),
                cpu.mean(),
                cpu.at(0.95),
                cpu.at(1.0),
                interval.mean(),
                interval.at(0.5),
                interval.at(0.95),
                interval.at(1.0),
                interval.0.len(),
                capture.mean(),
                capture.0.len(),
            );
        }
    }

    impl eframe::App for App {
        fn raw_input_hook(&mut self, _context: &egui::Context, input: &mut egui::RawInput) {
            let Some(page) = self.pages.first() else {
                return;
            };
            if self.hover {
                input
                    .events
                    .push(egui::Event::PointerMoved(egui::pos2(226.0, 240.0)));
            } else {
                input.events.extend(pointer_events(*page, self.frame));
            }
        }

        fn logic(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
            self.bench.play_signal();
            self.bench.chrome.poll_auth();
            self.bench.chrome.poll_community();
            if let Some(cpu) = frame.info().cpu_usage
                && self.frame > 30
            {
                self.cpu_ms.push(f64::from(cpu) * 1e3);
            }
            context.request_repaint();
        }

        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            let begin = Instant::now();
            let context = ui.ctx().clone();
            if self.options.periodic_capture
                && self
                    .modal_capture
                    .request_due(self.started.elapsed().as_millis() as i64, false)
            {
                let sender = self.backdrop_tx.clone();
                context.request_screenshot(move |image| {
                    let _ = sender.send(image);
                });
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ui, |ui| ronie_ui::default::show(ui, &mut self.bench.chrome));
            if let Ok(image) = self.backdrop_rx.try_recv()
                && self.modal_capture.complete(false)
            {
                let blur = Instant::now();
                let blurred = ronie_ui::default::ModalScrim::blur_snapshot(&image);
                let texture =
                    context.load_texture("modal-backdrop", blurred, egui::TextureOptions::LINEAR);
                self.bench.chrome.set_modal_backdrop(Some(texture));
                self.capture_ms.push(blur.elapsed().as_secs_f64() * 1e3);
            }
            if self.frame > 30 {
                self.ui_ms.push(begin.elapsed().as_secs_f64() * 1e3);
                if let Some(last) = self.last_begin {
                    self.interval_ms.push((begin - last).as_secs_f64() * 1e3);
                }
            }
            self.last_begin = Some(begin);
            self.frame += 1;
            if let Some(path) = self.options.capture.clone()
                && self.page_started.elapsed() > Duration::from_secs_f32(self.options.seconds)
            {
                context.request_screenshot(move |image| save(&path, &image));
                self.options.capture = None;
                self.page_started = Instant::now();
                return;
            }
            if !self.pages.is_empty()
                && self.page_started.elapsed() > Duration::from_secs_f32(self.options.seconds)
            {
                let page = self.pages.remove(0);
                self.report(page);
                match self.pages.first() {
                    Some(next) => {
                        self.bench.open(*next);
                        self.frame = 0;
                        self.last_begin = None;
                        self.page_started = Instant::now();
                    }
                    None => context.send_viewport_cmd(egui::ViewportCommand::Close),
                }
            }
        }
    }

    fn save(path: &std::path::Path, image: &egui::ColorImage) {
        let bytes: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect();
        let buffer = image::RgbaImage::from_raw(image.width() as u32, image.height() as u32, bytes)
            .expect("a screenshot has a consistent size");
        buffer.save(path).expect("the path can be written");
    }

    pub(super) fn run(bench: Bench, pages: Vec<Page>, options: Options, hover: bool) {
        let vsync = options.vsync;
        let mut native = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("frame cost")
                .with_inner_size([1024.0, 614.0]),
            renderer: eframe::Renderer::Glow,
            event_loop_builder: Some(Box::new(|builder| {
                #[cfg(target_os = "linux")]
                {
                    use winit::platform::x11::EventLoopBuilderExtX11;
                    builder.with_x11();
                }
                // Another operating system keeps the default event loop.
                #[cfg(not(target_os = "linux"))]
                let _ = builder;
            })),
            ..Default::default()
        };
        native.glow_options.vsync = vsync;
        eframe::run_native(
            "frame cost",
            native,
            Box::new(move |creation| {
                Fonts::install(&creation.egui_ctx);
                creation.egui_ctx.enable_accesskit();
                let mut bench = bench;
                bench.open(pages[0]);
                let (backdrop_tx, backdrop_rx) = std::sync::mpsc::channel();
                Ok(Box::new(App {
                    bench,
                    pages,
                    options,
                    page_started: Instant::now(),
                    last_begin: None,
                    frame: 0,
                    ui_ms: Vec::new(),
                    interval_ms: Vec::new(),
                    cpu_ms: Vec::new(),
                    hover,
                    started: Instant::now(),
                    modal_capture: ronie_ui::default::ModalCapture::default(),
                    backdrop_tx,
                    backdrop_rx,
                    capture_ms: Vec::new(),
                }))
            }),
        )
        .expect("a window on the private display");
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| arguments.iter().any(|argument| argument == flag);
    let value = |flag: &str| {
        arguments
            .iter()
            .position(|argument| argument == flag)
            .and_then(|index| arguments.get(index + 1))
    };
    if let Some(model) = value("--actions") {
        let count = value("--tones").map_or(3, |tones| tones.parse().expect("a tone count"));
        println!("| Edit | ms on the calling thread | result |");
        println!("|---|---|---|");
        measure_actions(std::path::Path::new(model), count);
        return;
    }
    if has("--blur") {
        println!("| Window pixels | blur ms |");
        println!("|---|---|");
        for size in [[1024, 614], [2048, 1228], [3840, 2160]] {
            let image =
                egui::ColorImage::new(size, vec![egui::Color32::from_gray(64); size[0] * size[1]]);
            let begin = Instant::now();
            let _ = ronie_ui::default::ModalScrim::blur_snapshot(&image);
            println!(
                "| {}x{} | {:.1} |",
                size[0],
                size[1],
                begin.elapsed().as_secs_f64() * 1e3
            );
        }
        return;
    }
    let tones = value("--tones").map_or(3, |tones| tones.parse().expect("a tone count"));
    let mut bench = Bench::new(tones);
    // The images and the catalog arrive from worker threads.
    if has("--window") {
        assert!(
            std::env::var_os("WAYLAND_DISPLAY").is_none()
                && std::env::var_os("RONIE_PRIVATE_DISPLAY").is_some(),
            "--window runs only on a private display: see the header of this file"
        );
        let pages = match value("--page").map(String::as_str) {
            Some("browser") => vec![Page::Browser],
            Some("chain") => vec![Page::Chain],
            _ => Page::ALL.to_vec(),
        };
        println!(
            "| Page | vsync | periodic capture | ui ms | eframe frame ms | p95 | max | interval ms | p50 | p95 | max | frames | blur ms | captures |"
        );
        println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
        window::run(
            bench,
            pages,
            window::Options {
                vsync: has("--vsync"),
                capture: value("--capture").map(PathBuf::from),
                seconds: value("--seconds").map_or(3.0, |value| value.parse().expect("seconds")),
                periodic_capture: has("--periodic-capture"),
            },
            has("--hover"),
        );
        return;
    }
    println!(
        "| Page | scale | accesskit | logic ms | pass ms | tessellate ms | total ms | p95 | max | primitives | vertices |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let pixels: f32 = value("--scale").map_or(2.0, |value| value.parse().expect("a scale"));
    for page in Page::ALL {
        measure(&mut bench, page, pixels, true);
    }
    if has("--compare") {
        for page in Page::ALL {
            measure(&mut bench, page, pixels, false);
        }
        for page in Page::ALL {
            measure(&mut bench, page, 1.0, true);
        }
    }
}

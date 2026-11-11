//! Recorded community answers through the real processor and the dummy audio device.
//!
//! No test here contacts a service or opens a device. The transport answers by path.

use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    net::{Ipv4Addr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ronie_app::standalone_backend::{StandaloneBackend, standalone_processor};
use ronie_core::{
    BlockId, ChainBlock, ChainSide, HttpError, HttpRequest, HttpResponse, HttpTransport, ModelId,
    OAuthConfig, OAuthFlow, OAuthTokens, OriginPolicy, PREFERENCES_SETTINGS, Pkce,
    RecordedTransport, SecretString, SessionStore, SettingsStore, StateCodec, Tone, ToneId,
    UI_PREFERENCES_SETTINGS,
    params::{ParameterId, ParameterValue},
};
use ronie_ui::{
    backend::{GesturePhase, MockBackend, UiAction, UiBackend},
    default::ChromeController,
    driver::UiDriver,
    services::{LoadPhase, community::CommunityService},
    testbed::load_scenarios,
};
use url::Url;

const FRAMES: usize = 256;
const MODELS: &str = "/api/v1/models";

#[derive(Clone, Default)]
struct PathTransport(Arc<Mutex<BTreeMap<String, VecDeque<HttpResponse>>>>);

impl PathTransport {
    fn respond(&self, path: &str, response: HttpResponse) -> &Self {
        self.0
            .lock()
            .expect("routes")
            .entry(path.to_owned())
            .or_default()
            .push_back(response);
        self
    }

    /// Answers one tone load: the tone, its one model, and the model file.
    fn offer(&self, tone: i64, format: &str, model: i64, bytes: &[u8]) {
        let file = format!("/files/{model}");
        self.respond(
            &format!("/api/v1/tones/{tone}"),
            HttpResponse::json(
                200,
                serde_json::json!({"id": tone, "title": format!("Tone {tone}"), "format": format, "models": []}),
            ),
        )
        .respond(
            MODELS,
            HttpResponse::json(
                200,
                serde_json::json!({
                    "data": [{
                        "id": model,
                        "name": "Model",
                        "model_url": format!("https://models.example{file}"),
                    }],
                    "page": 1,
                    "page_size": 1,
                    "total": 1,
                    "total_pages": 1,
                }),
            ),
        )
        .respond(&file, HttpResponse::new(200, bytes.to_vec()));
    }
}

impl HttpTransport for PathTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.0
            .lock()
            .expect("routes")
            .get_mut(request.url.path())
            .and_then(VecDeque::pop_front)
            .ok_or(HttpError::Transport)
    }
}

fn origin() -> Url {
    Url::parse("https://api.example").expect("origin")
}

fn settings(root: &Path) -> SettingsStore {
    SettingsStore::standalone(root, UI_PREFERENCES_SETTINGS)
}

fn sign_in(root: &Path) {
    SessionStore::new(
        settings(root),
        OAuthFlow::new(
            OAuthConfig::new(origin(), "public-client"),
            RecordedTransport::new(Vec::new()),
        ),
    )
    .set_tokens(OAuthTokens {
        access: SecretString::from("stored-access"),
        refresh: SecretString::from("stored-refresh"),
        expires_at_ms: i64::MAX,
    })
    .expect("tokens");
}

fn backend(root: &Path) -> Arc<StandaloneBackend> {
    Arc::new(
        StandaloneBackend::dummy(
            standalone_processor(root).expect("processor"),
            &root.join(PREFERENCES_SETTINGS),
        )
        .expect("dummy backend"),
    )
}

fn synthetic_nam() -> Vec<u8> {
    ronie_oracle::synthetic::synthetic_a2_json(8).expect("synthetic NAM")
}

/// A two-tap comb, which an amplitude measurement at 1 kHz can tell from the dry signal.
fn synthetic_ir() -> Vec<u8> {
    let samples = [16_000_i16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16_000];
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36_u32 + samples.len() as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// The settled level of a 1 kHz sine at the left output.
fn level(backend: &StandaloneBackend) -> f32 {
    let mut output = Vec::new();
    let mut offset = 0;
    for _ in 0..48_000 / FRAMES / 4 + 1 {
        let input: Vec<f32> = (0..FRAMES)
            .map(|frame| {
                let phase = (offset + frame) as f32 / 48.0;
                0.25 * (std::f32::consts::TAU * phase).sin()
            })
            .collect();
        offset += FRAMES;
        output = backend.render_dummy(&input).expect("render");
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    let left: Vec<f32> = output.iter().step_by(2).copied().collect();
    (left.iter().map(|sample| sample * sample).sum::<f32>() / left.len() as f32).sqrt()
}

fn assert_differs(actual: f32, other: f32, context: &str) {
    assert!(
        (actual / other - 1.0).abs() > 0.05,
        "{context}: {actual} against {other}"
    );
}

fn assert_close(actual: f32, expected: f32, context: &str) {
    assert!(
        (actual / expected - 1.0).abs() < 0.05,
        "{context}: {actual} against {expected}"
    );
}

fn catalog_row(id: i64) -> Tone {
    Tone::from_value(serde_json::json!({"id": id, "title": format!("Tone {id}"), "models": []}))
        .expect("row")
}

fn first_insert(backend: &StandaloneBackend) -> BlockId {
    backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|block| block.is_insert)
        .expect("insert")
        .id
        .clone()
}

fn tone_blocks(backend: &StandaloneBackend) -> Vec<ChainBlock> {
    backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .filter(|block| !block.is_insert)
        .cloned()
        .collect()
}

struct Rig {
    _directory: tempfile::TempDir,
    root: PathBuf,
    transport: PathTransport,
    backend: Arc<StandaloneBackend>,
    chrome: ChromeController,
}

impl Rig {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("root");
        let root = directory.path().to_path_buf();
        sign_in(&root);
        let transport = PathTransport::default();
        let backend = backend(&root);
        let chrome = ChromeController::with_community(
            backend.clone(),
            Vec::new(),
            false,
            true,
            CommunityService::new(
                OAuthConfig::new(origin(), "public-client"),
                OriginPolicy::new(
                    origin(),
                    [Url::parse("https://models.example").expect("models")],
                ),
                transport.clone(),
                settings(&root),
            ),
        );
        Self {
            _directory: directory,
            root,
            transport,
            backend,
            chrome,
        }
    }

    fn pick(&mut self, id: i64) -> LoadPhase {
        let target = first_insert(&self.backend);
        self.chrome
            .gallery_mut()
            .select_tone(ChainSide::Left, target);
        self.chrome.sync_browser();
        self.chrome.pick_tone(catalog_row(id));
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.chrome.load_flow_phase() == LoadPhase::Loading {
            assert!(Instant::now() < deadline, "the load did not finish");
            self.chrome.poll_community();
            std::thread::sleep(Duration::from_millis(2));
        }
        let phase = self.chrome.load_flow_phase();
        self.chrome.sync_browser();
        phase
    }

    fn generation(&self) -> u64 {
        self.backend
            .audio_status()
            .expect("status")
            .published_generation
    }
}

#[test]
fn recorded_models_reach_the_running_chain_and_restore_with_no_network() {
    let mut rig = Rig::new();
    let (nam, ir) = (synthetic_nam(), synthetic_ir());
    rig.transport.offer(6, "ir", 8, &ir);
    rig.transport.offer(5, "nam", 7, &nam);
    let dry = level(&rig.backend);
    let first = rig.generation();

    assert_eq!(rig.pick(6), LoadPhase::Ready);
    let with_ir = level(&rig.backend);
    let status = rig.backend.audio_status().expect("status");
    assert!(status.published_generation > first);
    assert_eq!(status.active_generation, status.published_generation);
    assert_eq!(status.chain_error, None);
    assert!(status.unavailable_blocks.is_empty());
    assert_differs(with_ir, dry, "the IR must process the signal");

    assert_eq!(rig.pick(5), LoadPhase::Ready);
    let with_both = level(&rig.backend);
    assert!(
        rig.backend
            .audio_status()
            .expect("status")
            .unavailable_blocks
            .is_empty()
    );
    assert_differs(with_both, with_ir, "the NAM must process the signal");
    assert_eq!(tone_blocks(&rig.backend).len(), 2);

    let Rig {
        root,
        _directory,
        backend: running,
        chrome,
        ..
    } = rig;
    drop(chrome);
    drop(running);

    let stored = StateCodec::new()
        .decode_session(&std::fs::read(root.join("session.state")).expect("saved session"))
        .expect("session");
    let mut embedded: Vec<(i64, Vec<u8>)> = stored
        .model_cache
        .iter()
        .map(|entry| (entry.model_id.0, entry.bytes.to_vec()))
        .collect();
    embedded.sort();
    assert_eq!(embedded, [(7, nam), (8, ir)]);

    // This launch has no transport at all.
    let restarted = backend(&root);
    let blocks = tone_blocks(&restarted);
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.tone.as_ref().expect("tone").id)
            .collect::<Vec<_>>(),
        [ToneId(6), ToneId(5)]
    );
    assert!(
        blocks
            .iter()
            .all(|block| block.tone.as_ref().expect("tone").models.len() == 1),
        "a catalog tone persists only its active model"
    );
    assert_close(level(&restarted), with_both, "the restored chain");
    let status = restarted.audio_status().expect("status");
    assert!(status.unavailable_blocks.is_empty());
    assert_eq!(status.chain_error, None);
}

#[test]
fn bytes_that_cannot_compile_keep_the_running_chain_and_the_session() {
    let mut rig = Rig::new();
    rig.transport.offer(6, "ir", 8, &synthetic_ir());
    rig.transport.offer(5, "nam", 7, b"{\"not\": \"a model\"}");
    assert_eq!(rig.pick(6), LoadPhase::Ready);
    let good = level(&rig.backend);
    let generation = rig.generation();
    let revision = rig.backend.chain_state(0).revision;

    assert_eq!(rig.pick(5), LoadPhase::Failed);

    assert_eq!(rig.generation(), generation);
    assert_eq!(rig.backend.chain_state(0).revision, revision);
    assert_close(level(&rig.backend), good, "the chain that kept running");
    assert!(rig.chrome.session().signed_in());
    assert!(rig.chrome.toast().is_some());
}

#[test]
fn a_corrupt_nam_or_wav_keeps_the_running_chain_and_the_saved_session() {
    let nam = synthetic_nam();
    let wav = synthetic_ir();
    let cases: [(&str, &[u8]); 4] = [
        ("nam", &nam[..nam.len() / 2]),
        ("nam", b"\xff\xfe not text"),
        ("ir", &wav[..wav.len() / 2]),
        ("ir", b"RIFF\x04\x00\x00\x00WAVE"),
    ];
    for (format, corrupt) in cases {
        let mut rig = Rig::new();
        rig.transport.offer(6, "ir", 8, &wav);
        rig.transport.offer(5, format, 7, corrupt);
        assert_eq!(rig.pick(6), LoadPhase::Ready);
        let good = level(&rig.backend);
        let generation = rig.generation();
        let revision = rig.backend.chain_state(0).revision;

        assert_eq!(rig.pick(5), LoadPhase::Failed, "{format}");

        assert_eq!(rig.generation(), generation, "{format}");
        assert_eq!(rig.backend.chain_state(0).revision, revision, "{format}");
        assert_close(level(&rig.backend), good, "the chain that kept running");
        assert!(rig.chrome.session().signed_in(), "{format}");
        assert!(rig.chrome.toast().is_some(), "{format}");

        let Rig {
            root,
            backend,
            chrome,
            _directory,
            ..
        } = rig;
        drop(chrome);
        drop(backend);
        let saved = StateCodec::new()
            .decode_session(&std::fs::read(root.join("session.state")).expect("saved session"))
            .expect("session");
        assert_eq!(saved.model_cache.len(), 1, "{format}");
        assert_eq!(
            saved.model_cache[0].bytes.as_ref(),
            wav.as_slice(),
            "{format}"
        );
    }
}

#[test]
fn a_download_that_fails_keeps_the_running_chain_and_can_retry() {
    let mut rig = Rig::new();
    rig.transport.offer(6, "ir", 8, &synthetic_ir());
    assert_eq!(rig.pick(6), LoadPhase::Ready);
    let good = level(&rig.backend);
    let generation = rig.generation();
    // The transport has no answer for the next tone.
    assert_eq!(rig.pick(5), LoadPhase::Failed);
    assert_eq!(rig.generation(), generation);
    assert_close(level(&rig.backend), good, "the chain that kept running");
    assert!(rig.chrome.session().signed_in());

    rig.transport.offer(5, "nam", 7, &synthetic_nam());
    rig.chrome.retry_load();
    let deadline = Instant::now() + Duration::from_secs(30);
    while rig.chrome.load_flow_phase() == LoadPhase::Loading {
        assert!(Instant::now() < deadline, "the retry did not finish");
        rig.chrome.poll_community();
        std::thread::sleep(Duration::from_millis(2));
    }

    assert_eq!(rig.chrome.load_flow_phase(), LoadPhase::Ready);
    assert!(rig.generation() > generation);
}

#[test]
fn a_late_completion_after_a_close_changes_no_chain() {
    let mut rig = Rig::new();
    let revision = rig.backend.chain_state(0).revision;
    let target = first_insert(&rig.backend);
    rig.chrome
        .gallery_mut()
        .select_tone(ChainSide::Left, target);
    rig.chrome.sync_browser();
    let ticket = rig
        .chrome
        .load_flow_mut()
        .begin_remote_load(catalog_row(5))
        .expect("load");
    let generation = rig.generation();

    rig.chrome.go_to_main();
    let tone = Tone::from_value(serde_json::json!({
        "id": 5,
        "title": "Late",
        "models": [{"id": 7, "name": "Late", "model_url": "https://models.example/files/7"}],
    }))
    .expect("tone");
    let published = ticket
        .publish(tone, ModelId(7), synthetic_nam().into())
        .expect("a late completion is not an error");

    assert!(!published);
    assert_eq!(rig.backend.chain_state(0).revision, revision);
    assert_eq!(rig.generation(), generation);
    assert!(tone_blocks(&rig.backend).is_empty());
}

#[test]
fn a_caller_edit_during_a_pending_chain_does_not_compile_and_is_not_lost() {
    let directory = tempfile::tempdir().expect("root");
    let backend = backend(directory.path());
    let dry = level(&backend);
    let target = first_insert(&backend);
    let generation = backend.audio_status().expect("status").published_generation;
    let tone = Tone::from_value(serde_json::json!({
        "id": 6,
        "title": "Remote",
        "format": "ir",
        "models": [{"id": 8, "name": "Cab", "model_url": "https://models.example/files/8"}],
    }))
    .expect("tone");

    backend
        .dispatch(UiAction::LoadToneWithBytes {
            target_insert_id: target,
            tone,
            model_id: ModelId(8),
            bytes: synthetic_ir().into(),
        })
        .expect("publish");
    let same = |backend: &StandaloneBackend| {
        backend.audio_status().expect("status").published_generation == generation
    };
    assert!(same(&backend), "the dispatch does not compile");
    backend
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.5),
            GesturePhase::End,
        )
        .expect("edit");
    backend
        .dispatch(UiAction::SetStereoMode(false))
        .expect("structure edit");
    assert!(
        same(&backend),
        "an edit does not compile for the pending chain"
    );

    backend.settle_audio().expect("settle");

    assert!(!same(&backend));
    assert_differs(level(&backend), dry, "the IR must process the signal");
}

#[test]
fn a_model_switch_compiles_only_when_the_audio_settles() {
    let mut rig = Rig::new();
    rig.transport.offer(6, "ir", 8, &synthetic_ir());
    assert_eq!(rig.pick(6), LoadPhase::Ready);
    let block = tone_blocks(&rig.backend)[0].id.clone();
    let tone = Tone::from_value(serde_json::json!({
        "id": 6,
        "title": "Tone 6",
        "format": "ir",
        "models": [{"id": 9, "name": "Other", "model_url": "https://models.example/files/9"}],
    }))
    .expect("tone");
    let before = rig.generation();

    rig.backend
        .dispatch(UiAction::SelectModelWithBytes {
            block_id: block,
            tone,
            model_id: ModelId(9),
            replaces: Some(ModelId(8)),
            bytes: synthetic_ir().into(),
        })
        .expect("publish");

    assert_eq!(rig.generation(), before, "the dispatch does not compile");
    rig.backend.settle_audio().expect("settle");
    assert!(rig.generation() > before);
    assert!(
        rig.backend
            .audio_status()
            .expect("status")
            .unavailable_blocks
            .is_empty()
    );
}

#[test]
fn a_recorded_model_switch_reaches_the_running_chain_and_restores_with_no_network() {
    let mut rig = Rig::new();
    let quiet = synthetic_ir();
    let mut loud = quiet.clone();
    // The second impulse response has a stronger first sample.
    let data = loud.len() - 26;
    loud[data..data + 2].copy_from_slice(&32_000_i16.to_le_bytes());
    rig.transport.offer(6, "ir", 8, &quiet);
    assert_eq!(rig.pick(6), LoadPhase::Ready);
    let first = level(&rig.backend);
    let block = tone_blocks(&rig.backend)[0].id.clone();

    rig.transport
        .respond(
            MODELS,
            HttpResponse::json(
                200,
                serde_json::json!({
                    "data": [
                        {"id": 8, "name": "Quiet", "model_url": "https://models.example/files/8"},
                        {"id": 9, "name": "Loud", "model_url": "https://models.example/files/9"},
                    ],
                    "page": 1,
                    "page_size": 300,
                    "total": 2,
                    "total_pages": 1,
                }),
            ),
        )
        .respond("/files/9", HttpResponse::new(200, loud));
    rig.chrome.open_detail(&block.0).expect("detail");
    let deadline = Instant::now() + Duration::from_secs(30);
    while rig
        .chrome
        .detail()
        .is_some_and(|detail| detail.model_select().options().len() < 2)
    {
        assert!(Instant::now() < deadline, "the list did not arrive");
        rig.chrome.poll_community();
        std::thread::sleep(Duration::from_millis(2));
    }

    let session = rig.chrome.session();
    rig.chrome
        .detail_mut()
        .expect("detail")
        .select_model(ModelId(9), session)
        .expect("selection");
    rig.chrome.poll_community();
    while rig
        .chrome
        .detail()
        .is_some_and(|detail| detail.model_select().target().is_some())
    {
        assert!(Instant::now() < deadline, "the switch did not finish");
        rig.chrome.poll_community();
        std::thread::sleep(Duration::from_millis(2));
    }

    let switched = level(&rig.backend);
    assert_differs(switched, first, "the new bytes must process the signal");
    let status = rig.backend.audio_status().expect("status");
    assert!(status.unavailable_blocks.is_empty());
    assert_eq!(status.chain_error, None);
    assert_eq!(
        tone_blocks(&rig.backend)[0].active_model_id,
        Some(ModelId(9))
    );

    let Rig {
        root,
        backend,
        chrome,
        _directory,
        ..
    } = rig;
    drop(chrome);
    drop(backend);
    let restarted = crate::backend(&root);
    let blocks = tone_blocks(&restarted);
    assert_eq!(blocks[0].active_model_id, Some(ModelId(9)));
    assert_eq!(blocks[0].tone.as_ref().expect("tone").models.len(), 1);
    assert_close(level(&restarted), switched, "the restored chain");
    assert!(
        restarted
            .audio_status()
            .expect("status")
            .unavailable_blocks
            .is_empty()
    );
}

#[test]
fn a_compile_that_holds_the_backend_does_not_lose_the_session_at_exit() {
    let directory = tempfile::tempdir().expect("root");
    let root = directory.path();
    sign_in(root);
    let processor = standalone_processor(root).expect("processor");
    let at_exit = processor.clone();
    let backend = Arc::new(
        StandaloneBackend::dummy(processor, &root.join(PREFERENCES_SETTINGS)).expect("backend"),
    );
    let mut chrome = ChromeController::with_community(
        backend.clone(),
        Vec::new(),
        false,
        true,
        CommunityService::new(
            OAuthConfig::new(origin(), "public-client"),
            OriginPolicy::new(origin(), []),
            PathTransport::default(),
            settings(root),
        ),
    );
    backend
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.75),
            GesturePhase::End,
        )
        .expect("edit");
    let target = first_insert(&backend);
    chrome.gallery_mut().select_tone(ChainSide::Left, target);
    chrome.sync_browser();
    // A compile on a blocking thread holds its ticket, which holds the backend, for as long as the
    // compile runs. This one runs until the end of this test.
    let ticket = chrome
        .load_flow_mut()
        .begin_remote_load(catalog_row(5))
        .expect("load");
    let (release, compile_ends) = std::sync::mpsc::channel::<()>();
    let compile = std::thread::spawn(move || {
        let _ = compile_ends.recv();
        drop(ticket);
    });

    drop(chrome);
    drop(backend);
    // The compile still runs here, so a wait for it would never end.
    let (saved, save_ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || saved.send(at_exit.save_at_exit()));
    save_ended
        .recv_timeout(Duration::from_secs(30))
        .expect("the exit did not wait for the compile")
        .expect("save");

    assert!(root.join("session.state").is_file(), "the session is lost");
    release.send(()).expect("the compile runs");
    compile.join().expect("compile");
    assert_eq!(
        crate::backend(root).parameter(ParameterId("inputLevel")),
        Some(ParameterValue(0.75)),
        "the edit before the exit survives"
    );
}

// The complete journey, through real frames with no window. Each value below is fake.

const FAKE_KEY: &str = "fake-publishable-key";
const FAKE_CODE: &str = "fake-authorization-code";
const FAKE_ACCESS: &str = "fake-access-token";
const FAKE_REFRESH: &str = "fake-refresh-token";
const FAKE_ROTATED_ACCESS: &str = "fake-rotated-access-token";
const FAKE_ROTATED_REFRESH: &str = "fake-rotated-refresh-token";
const TOKEN: &str = "/api/v1/oauth/token";
const USER: &str = "/api/v1/user";
const SEARCH: &str = "/api/v1/tones/search";

/// A transport that keeps each raw request, so that the test can prove that a secret was real.
#[derive(Clone, Default)]
struct RecordingTransport {
    routes: PathTransport,
    raw: Arc<Mutex<Vec<HttpRequest>>>,
}

impl RecordingTransport {
    fn raw(&self) -> Vec<HttpRequest> {
        self.raw.lock().expect("requests").clone()
    }
}

impl HttpTransport for RecordingTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.raw.lock().expect("requests").push(request.clone());
        self.routes.execute(request).await
    }
}

fn fake_service(
    root: &Path,
    transport: RecordingTransport,
) -> CommunityService<RecordingTransport> {
    CommunityService::new(
        OAuthConfig::new(origin(), FAKE_KEY),
        OriginPolicy::new(
            origin(),
            [Url::parse("https://models.example").expect("models")],
        ),
        transport,
        settings(root),
    )
}

fn token_answer(access: &str, refresh: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        serde_json::json!({"access_token": access, "refresh_token": refresh, "expires_in": 3600}),
    )
}

fn search_answer(rows: &[(i64, &str)]) -> HttpResponse {
    let data: Vec<serde_json::Value> = rows
        .iter()
        .map(|(id, title)| serde_json::json!({"id": id, "title": title, "format": "nam", "models": []}))
        .collect();
    HttpResponse::json(
        200,
        serde_json::json!({"data": data, "page": 1, "page_size": 12, "total": rows.len(), "total_pages": 1}),
    )
}

/// A driver that needs no effect from a mock, because this test reads the real backend.
fn driver_over(chrome: ChromeController) -> UiDriver {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-empty")
        .expect("scenario");
    UiDriver::new(chrome, MockBackend::from_scenario(&scenario))
}

fn frames_until(driver: &mut UiDriver, what: &str, mut done: impl FnMut(&mut UiDriver) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(driver) {
        assert!(Instant::now() < deadline, "timed out: {what}");
        driver.frame();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Plays the system browser after the service accepted the sign-in.
fn browser_returns_with_a_code(authorize_url: &str) {
    let authorize = Url::parse(authorize_url).expect("authorize URL");
    let field = |name: &str| {
        authorize
            .query_pairs()
            .find(|(key, _)| key == name)
            .expect("query field")
            .1
            .into_owned()
    };
    let redirect = Url::parse(&field("redirect_uri")).expect("redirect URI");
    assert_eq!(redirect.host_str(), Some("localhost"));
    let port = redirect.port().expect("redirect port");
    let mut callback = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("callback socket");
    write!(
        callback,
        "GET /?code={FAKE_CODE}&state={} HTTP/1.1\r\nHost: x\r\n\r\n",
        field("state")
    )
    .expect("callback request");
    let mut page = String::new();
    callback.read_to_string(&mut page).expect("callback page");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
}

fn form_field(request: &HttpRequest, name: &str) -> Option<String> {
    url::form_urlencoded::parse(request.body.as_deref().unwrap_or_default())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn bearer_of(request: &HttpRequest) -> Option<&str> {
    request
        .headers
        .iter()
        .find(|(name, _)| name == "Authorization")
        .map(|(_, value)| value.as_str())
}

/// The public shape of one request: its method, its address, and whether a bearer went with it.
fn public_line(request: &HttpRequest) -> String {
    let query = request
        .url
        .query()
        .map_or_else(String::new, |query| format!("?{query}"));
    format!(
        "{:?} {}{}{query}{}",
        request.method,
        request.url.origin().ascii_serialization(),
        request.url.path(),
        if bearer_of(request).is_some() {
            " bearer"
        } else {
            ""
        }
    )
}

fn files_below(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory).expect("directory") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            files.extend(files_below(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn the_complete_journey_reaches_audio_and_restores_with_no_network_and_no_secret() {
    let directory = tempfile::tempdir().expect("root");
    let root = directory.path().to_path_buf();
    let nam = synthetic_nam();
    let transport = RecordingTransport::default();
    transport
        .routes
        .respond(TOKEN, token_answer(FAKE_ACCESS, FAKE_REFRESH))
        .respond(
            USER,
            HttpResponse::json(200, serde_json::json!({"username": "player"})),
        )
        .respond(SEARCH, search_answer(&[(4, "Clean Rig"), (5, "Fuzz Rig")]))
        .respond(SEARCH, search_answer(&[(5, "Fuzz Rig")]))
        // The access token is out of date at the first catalog read of the tone.
        .respond("/api/v1/tones/5", HttpResponse::new(401, Vec::new()))
        .respond(
            TOKEN,
            token_answer(FAKE_ROTATED_ACCESS, FAKE_ROTATED_REFRESH),
        );
    transport.routes.offer(5, "nam", 7, &nam);
    let running = backend(&root);
    let dry = level(&running);
    let first_generation = running.audio_status().expect("status").published_generation;
    let chrome = ChromeController::with_community(
        running.clone(),
        Vec::new(),
        false,
        true,
        fake_service(&root, transport.clone()),
    );
    let mut driver = driver_over(chrome);
    assert!(!driver.chrome().session().signed_in());

    // Sign-in, through the system browser.
    driver.chrome_mut().sign_in().expect("sign-in");
    let mut opened = Vec::new();
    frames_until(&mut driver, "the authorize page", |driver| {
        opened.extend(driver.interaction().opened_urls);
        !opened.is_empty()
    });
    assert_eq!(opened.len(), 1);
    assert!(opened[0].starts_with("https://api.example/api/v1/oauth/authorize?"));
    browser_returns_with_a_code(&opened[0]);
    frames_until(&mut driver, "the signed-in user", |driver| {
        driver.chrome().session().signed_in() && driver.chrome().public_user().is_some()
    });

    // Search, and the tone that the search finds.
    let _ = driver.click_first("Add Tone").expect("open the browser");
    let target = driver
        .chrome_mut()
        .load_flow_mut()
        .pending_target()
        .expect("the insert of the click")
        .block_id
        .clone();
    frames_until(&mut driver, "the first results", |driver| {
        driver.is_visible("Clean Rig")
    });
    let _ = driver.fill("Search tones", "fuzz").expect("type");
    frames_until(&mut driver, "the search results", |driver| {
        !driver.is_visible("Clean Rig") && driver.is_visible("Fuzz Rig")
    });
    let _ = driver.click("Fuzz Rig").expect("the card");
    frames_until(&mut driver, "the load", |driver| {
        driver.chrome().load_flow_phase() != LoadPhase::Loading
    });
    assert_eq!(driver.chrome().load_flow_phase(), LoadPhase::Ready);
    driver.settle();

    // Preparation: the running chain now holds the model.
    let with_model = level(&running);
    assert_differs(with_model, dry, "the model must process the signal");
    let status = running.audio_status().expect("status");
    assert!(status.published_generation > first_generation);
    assert_eq!(status.active_generation, status.published_generation);
    assert!(status.unavailable_blocks.is_empty());
    assert_eq!(status.chain_error, None);
    let blocks = tone_blocks(&running);
    assert_eq!(blocks.len(), 1);
    let block = blocks[0].clone();
    assert_eq!(block.id.0, target, "the model landed on the chosen insert");
    assert_eq!(block.active_model_id, Some(ModelId(7)));
    assert_eq!(block.tone.as_ref().expect("tone").id, ToneId(5));

    // The public requests, in order.
    let raw = transport.raw();
    let lines: Vec<String> = raw.iter().map(public_line).collect();
    assert_eq!(
        lines,
        [
            "Post https://api.example/api/v1/oauth/token",
            "Get https://api.example/api/v1/user bearer",
            "Get https://api.example/api/v1/tones/search?page=1&page_size=12 bearer",
            "Get https://api.example/api/v1/tones/search?page=1&page_size=12&query=fuzz bearer",
            "Get https://api.example/api/v1/tones/5 bearer",
            "Post https://api.example/api/v1/oauth/token",
            "Get https://api.example/api/v1/tones/5 bearer",
            "Get https://api.example/api/v1/models?tone_id=5&page=1&page_size=1&architecture=2 bearer",
            "Get https://models.example/files/7",
        ]
    );

    // Each secret was real, so that the checks of the evidence below can fail.
    let exchange = &raw[0];
    assert_eq!(
        form_field(exchange, "grant_type").as_deref(),
        Some("authorization_code")
    );
    assert_eq!(form_field(exchange, "code").as_deref(), Some(FAKE_CODE));
    assert_eq!(form_field(exchange, "client_id").as_deref(), Some(FAKE_KEY));
    let verifier = form_field(exchange, "code_verifier").expect("verifier");
    let authorize = Url::parse(&opened[0]).expect("authorize URL");
    assert!(
        authorize
            .query_pairs()
            .any(|(key, value)| key == "code_challenge" && value == Pkce::challenge_for(&verifier)),
        "the verifier belongs to the page that the browser opened"
    );
    assert_eq!(
        form_field(&raw[5], "refresh_token").as_deref(),
        Some(FAKE_REFRESH)
    );
    assert_eq!(
        bearer_of(&raw[1]),
        Some(format!("Bearer {FAKE_ACCESS}").as_str())
    );
    assert_eq!(
        bearer_of(&raw[6]),
        Some(format!("Bearer {FAKE_ROTATED_ACCESS}").as_str())
    );

    // Save, with the close of the window.
    let toast = driver.chrome().visible_toast().map(str::to_owned);
    let public_user = driver.chrome().public_user().cloned();
    drop(driver);
    drop(running);
    let saved = StateCodec::new()
        .decode_session(&std::fs::read(root.join("session.state")).expect("saved session"))
        .expect("session");
    assert_eq!(saved.model_cache.len(), 1);
    assert_eq!(saved.model_cache[0].block_id, block.id);
    assert_eq!(saved.model_cache[0].model_id, ModelId(7));
    assert_eq!(saved.model_cache[0].bytes.as_ref(), nam.as_slice());

    // Restart with a transport that answers nothing.
    let offline = RecordingTransport::default();
    let restarted = backend(&root);
    let mut chrome = ChromeController::with_community(
        restarted.clone(),
        Vec::new(),
        false,
        true,
        fake_service(&root, offline.clone()),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while offline.raw().is_empty() {
        assert!(Instant::now() < deadline, "the user refresh did not start");
        chrome.poll_community();
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..50 {
        chrome.poll_community();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(chrome.session().signed_in(), "offline is not a sign-out");
    assert_eq!(
        offline.raw().iter().map(public_line).collect::<Vec<_>>(),
        ["Get https://api.example/api/v1/user bearer"],
        "a restored chain asks the network for nothing"
    );
    let restored_blocks = tone_blocks(&restarted);
    assert_eq!(restored_blocks.len(), 1);
    assert_eq!(restored_blocks[0].id, block.id);
    assert_eq!(restored_blocks[0].active_model_id, Some(ModelId(7)));
    let restored_status = restarted.audio_status().expect("status");
    assert!(restored_status.unavailable_blocks.is_empty());
    assert_eq!(restored_status.chain_error, None);
    assert_close(level(&restarted), with_model, "the restored chain");
    drop(chrome);
    drop(restarted);
    let resaved = StateCodec::new()
        .decode_session(&std::fs::read(root.join("session.state")).expect("saved session"))
        .expect("session");
    assert_eq!(resaved.model_cache[0].bytes.as_ref(), nam.as_slice());

    // The evidence holds each public result, and each request in its formatted form.
    let offline_raw = offline.raw();
    let formatted = |requests: &[HttpRequest]| -> Vec<String> {
        requests
            .iter()
            .map(|request| format!("{request:?}"))
            .collect()
    };
    let evidence = serde_json::json!({
        "stages": ["sign-in", "search", "tone", "models", "download", "prepare", "save", "restart"],
        "requests": lines,
        "formatted_requests": formatted(&raw),
        "offline_requests": offline_raw.iter().map(public_line).collect::<Vec<_>>(),
        "formatted_offline_requests": formatted(&offline_raw),
        "selected_model_id": block.active_model_id.expect("model").0,
        "prepared_block_id": block.id.0,
        "restored_model_bytes": resaved.model_cache[0].bytes.len(),
        "audio_status": format!("{status:?}"),
        "restored_audio_status": format!("{restored_status:?}"),
        "public_user": public_user,
        "toast": toast,
    })
    .to_string();
    let bearer = format!("Bearer {FAKE_ACCESS}");
    let rotated_bearer = format!("Bearer {FAKE_ROTATED_ACCESS}");
    let secrets = [
        FAKE_KEY,
        FAKE_CODE,
        FAKE_ACCESS,
        FAKE_REFRESH,
        FAKE_ROTATED_ACCESS,
        FAKE_ROTATED_REFRESH,
        verifier.as_str(),
        bearer.as_str(),
        rotated_bearer.as_str(),
    ];
    for secret in secrets {
        assert!(!evidence.contains(secret), "the evidence holds a secret");
    }
    println!("{evidence}");

    // The token store is the one file that holds a token, and only its owner can read it.
    let token_store = settings(&root).path().to_path_buf();
    let mut checked = 0;
    for file in files_below(&root) {
        if file == token_store {
            // Only Unix has a mode for each file.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&file)
                    .expect("metadata")
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o077, 0, "the token store is private");
            }
            continue;
        }
        let bytes = std::fs::read(&file).expect("file");
        let text = String::from_utf8_lossy(&bytes);
        for secret in secrets {
            assert!(!text.contains(secret), "a saved file holds a secret");
        }
        checked += 1;
    }
    assert!(checked > 0);
}

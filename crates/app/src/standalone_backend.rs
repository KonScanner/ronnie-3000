//! Standalone UI adapter joining processor, audio manager, and persistent device settings.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use ronie_core::{
    BlockId, InputMode, LocalStash, ModelBytes, ModelId, PREFERENCES_SETTINGS, SettingsStore, Tone,
    params::{ParameterId, ParameterValue},
};
use ronie_dsp::PrepareSpec;
use ronie_plugin::{ProcessorBackend, RonieProcessor};
use ronie_ui::{
    backend::{
        ChainFault, ChainViewState, DeviceAction, GesturePhase, HostControl, MeterSnapshot,
        NavigationAction, UiAction, UiBackend, UiError, UiReceipt,
    },
    default::{MidiMapSnapshot, SystemDeviceState},
    session_edit::check_device_config,
};

use crate::{
    audio::{
        AudioBackendFactory, AudioBackendKind, AudioEngine, AudioEngineStatus, DummyBackend,
        LiveSpectrum, LiveTuner, new_backend,
    },
    audio_settings::StandaloneAudioSettings,
    diagnostics::DiagnosticLog,
    file_manager::open_in_file_manager,
};

/// Builds the production processor and all persistence rooted at one standalone config directory.
///
/// The preparation values here are only a start. The audio engine replaces them with the final
/// stream layout before it prepares a chain.
///
/// # Errors
/// Returns stash, settings, or session restoration errors.
pub fn standalone_processor(root: &Path) -> Result<ProcessorBackend, UiError> {
    let stash = LocalStash::new(root.join("stash")).map_err(|error| {
        UiError::Control(format!(
            "creating local model stash {}: {error}",
            root.join("stash").display()
        ))
    })?;
    let processor = RonieProcessor::with_stash(
        PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            // The engine sets the oversampling of each live chain from OPTIMIZE and the
            // oversampling setting, as it sets the rate and the callback.
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        },
        stash,
    );
    ProcessorBackend::with_settings(
        processor,
        SettingsStore::standalone(root, PREFERENCES_SETTINGS),
    )
}

/// What opens a folder of the app for the user. A test or a dummy stream must not start a program
/// on the desktop.
pub type FolderOpener = Arc<dyn Fn(&Path) -> Result<(), UiError> + Send + Sync>;

/// UI backend with real device-manager mutations and persistent standalone settings.
///
/// This adapter holds no audio protocol and no thread. It tells [`AudioEngine`] about each change,
/// and the engine does the slow work on its own threads.
pub struct StandaloneBackend {
    processor: ProcessorBackend,
    engine: AudioEngine,
    /// Only the tuner page reads this, so no other work waits for it.
    tuner: Mutex<LiveTuner>,
    /// Only the EQ page reads this, so no other work waits for it.
    spectrum: Mutex<LiveSpectrum>,
    open_folder: FolderOpener,
    presets: PathBuf,
    log: DiagnosticLog,
}

impl StandaloneBackend {
    /// Creates a deterministic standalone backend without touching host audio.
    ///
    /// # Errors
    ///
    /// Returns settings or dummy-stream errors.
    pub fn dummy(processor: ProcessorBackend, path: &Path) -> Result<Self, UiError> {
        Self::with_dummy_device(processor, path, DummyBackend::connected())
    }

    /// Creates a deterministic standalone backend on a dummy device that the caller also controls.
    ///
    /// # Errors
    ///
    /// Returns settings or dummy-stream errors.
    pub fn with_dummy_device(
        processor: ProcessorBackend,
        path: &Path,
        device: DummyBackend,
    ) -> Result<Self, UiError> {
        let settings = load_settings(path)?;
        let session = processor.clone();
        Self::around(processor, path, Arc::new(|_: &Path| Ok(())), |log| {
            AudioEngine::with_dummy_device(session, settings, device, log)
        })
    }

    /// Creates a real CPAL standalone backend. This touches no device: a worker thread finds the
    /// devices and opens the stream, and open failures remain visible for recovery.
    ///
    /// `driver` comes from the command line, and it wins over the saved driver.
    ///
    /// # Errors
    ///
    /// Returns settings errors, or an error when no engine thread can start.
    pub fn real(
        processor: ProcessorBackend,
        path: &Path,
        driver: Option<AudioBackendKind>,
    ) -> Result<Self, UiError> {
        let settings = load_settings(path)?;
        let kind = match driver.unwrap_or(settings.config().backend) {
            AudioBackendKind::Dummy => AudioBackendKind::System,
            other => other,
        };
        let session = processor.clone();
        Self::around(processor, path, Arc::new(open_in_file_manager), |log| {
            AudioEngine::deferred(session, settings, Arc::new(new_backend), kind, log)
        })
    }

    /// Creates a backend on a backend factory, with the same worker thread as a real backend.
    ///
    /// # Errors
    ///
    /// Returns settings errors, or an error when no engine thread can start.
    pub fn with_audio_backend(
        processor: ProcessorBackend,
        path: &Path,
        make_backend: AudioBackendFactory,
        kind: AudioBackendKind,
    ) -> Result<Self, UiError> {
        let settings = load_settings(path)?;
        let session = processor.clone();
        Self::around(processor, path, Arc::new(|_: &Path| Ok(())), |log| {
            AudioEngine::deferred(session, settings, make_backend, kind, log)
        })
    }

    /// Changes how long device work can run, for a test of a stuck job.
    ///
    /// # Errors
    ///
    /// Returns an error when the engine lock is poisoned.
    pub fn set_device_work_limit(&self, limit: Duration) -> Result<(), UiError> {
        self.engine.set_work_limit(limit)
    }

    fn around(
        processor: ProcessorBackend,
        settings_path: &Path,
        open_folder: FolderOpener,
        engine: impl FnOnce(DiagnosticLog) -> Result<AudioEngine, UiError>,
    ) -> Result<Self, UiError> {
        let root = settings_path.parent().unwrap_or_else(|| Path::new("."));
        let log = DiagnosticLog::start(&root.join("logs"));
        Ok(Self {
            processor,
            engine: engine(log.clone())?,
            tuner: Mutex::default(),
            spectrum: Mutex::default(),
            open_folder,
            presets: root.join("presets"),
            log,
        })
    }

    /// Replaces what opens a folder of the app.
    #[must_use]
    pub fn with_folder_opener(mut self, open_folder: FolderOpener) -> Self {
        self.open_folder = open_folder;
        self
    }

    /// Carries out a request that only the host can do. A click calls this, never a frame.
    fn carry_out(&self, request: &NavigationAction) -> Result<(), UiError> {
        let folder = match request {
            NavigationAction::OpenPresetsFolder => &self.presets,
            NavigationAction::RevealLogs => self.log.directory(),
            // The view reads the text, and no adapter can open a privacy page.
            NavigationAction::CopyLogs | NavigationAction::OpenMicrophonePrivacy => return Ok(()),
        };
        std::fs::create_dir_all(folder)
            .map_err(|error| UiError::Control(format!("creating a folder: {error}")))?;
        (self.open_folder)(folder)
    }

    /// Renders explicit input through the restored production dummy chain, as one callback.
    ///
    /// The callback keeps the old chain while the engine compiles a new one. Call
    /// [`UiBackend::settle_audio`] first to render the newest session.
    ///
    /// # Errors
    ///
    /// Returns stream-layout or processing errors.
    pub fn render_dummy(&self, input: &[f32]) -> Result<Vec<f32>, UiError> {
        self.engine.render_dummy(input)
    }

    /// Renders all channels of the dummy device, as a host gives them.
    ///
    /// # Errors
    ///
    /// Returns stream-layout or processing errors.
    pub fn render_dummy_capture(&self, capture: &[f32]) -> Result<Vec<f32>, UiError> {
        self.engine.render_dummy_capture(capture)
    }

    /// Current observable audio engine state.
    ///
    /// # Errors
    ///
    /// Returns an error when the engine lock is poisoned.
    pub fn audio_status(&self) -> Result<AudioEngineStatus, UiError> {
        self.engine.status()
    }
}

fn load_settings(path: &Path) -> Result<StandaloneAudioSettings, UiError> {
    StandaloneAudioSettings::load(path).map_err(|error| UiError::Control(error.to_string()))
}

impl UiBackend for StandaloneBackend {
    fn chain_state(&self, known_revision: u64) -> ChainViewState {
        self.processor.chain_state(known_revision)
    }

    fn meter_levels(&self) -> MeterSnapshot {
        self.engine.meter_snapshot()
    }

    fn tuner_reading(&self) -> ronie_dsp::analysis::TunerReading {
        match (self.engine.telemetry(), self.tuner.lock()) {
            (Some(stream), Ok(mut tuner)) => tuner.read(&stream),
            _ => self.processor.tuner_reading(),
        }
    }

    fn spectrum_bins(&self, block_id: &BlockId) -> [f32; 64] {
        match (self.engine.spectrum_tap(block_id), self.spectrum.lock()) {
            (Some(tap), Ok(mut spectrum)) => spectrum.read(&tap),
            _ => self.processor.spectrum_bins(block_id),
        }
    }

    fn device_settings(&self) -> Option<SystemDeviceState> {
        self.engine.device_state()
    }

    fn input_channel_levels(&self) -> Vec<f32> {
        self.engine.capture_levels_db()
    }

    fn refresh_devices(&self) {
        self.engine.request_device_list();
    }

    fn chain_faults(&self) -> Vec<ChainFault> {
        self.engine.chain_faults()
    }

    fn midi_map(&self) -> MidiMapSnapshot {
        self.processor.midi_map()
    }

    fn calibration_draft(&self) -> Option<String> {
        self.processor.calibration_draft()
    }

    fn preference(&self, key: &str) -> Option<bool> {
        self.processor.preference(key)
    }

    fn text_preference(&self, key: &str) -> Option<String> {
        self.processor.text_preference(key)
    }

    fn supports(&self, control: HostControl) -> bool {
        // The callback of each stream mixes this tone.
        control == HostControl::TestTone || self.processor.supports(control)
    }

    fn diagnostic_log(&self) -> Option<String> {
        self.log.recent()
    }

    fn dispatch(&self, action: UiAction) -> Result<UiReceipt, UiError> {
        if let UiAction::Navigate(request) = &action {
            self.carry_out(request)?;
        }
        if let UiAction::Device(device_action) = &action {
            // The session editor refuses this configuration later. The engine must not open a
            // stream for it before that refusal.
            if let DeviceAction::Configure(config) = device_action {
                check_device_config(config)?;
            }
            self.engine.request_device(device_action)?;
        }
        // A new structure only wakes the chain worker of the engine, so this returns at once.
        // A load flow holds its lock here and calls `settle_audio` next.
        let for_settle = matches!(
            action,
            UiAction::LoadToneWithBytes { .. } | UiAction::SelectModelWithBytes { .. }
        );
        let edit = || self.processor.dispatch(action);
        if for_settle {
            self.engine.edit_session_for_settle(edit)
        } else {
            self.engine.edit_session(edit)
        }
    }

    fn validate_model(
        &self,
        tone: &Tone,
        model_id: ModelId,
        bytes: &ModelBytes,
    ) -> Result<(), UiError> {
        self.processor.validate_model(tone, model_id, bytes)
    }

    fn settle_audio(&self) -> Result<(), UiError> {
        self.engine.settle()
    }

    fn can_paste_block(&self) -> bool {
        self.processor.can_paste_block()
    }

    fn parameter(&self, id: ParameterId) -> Option<ParameterValue> {
        self.processor.parameter(id)
    }

    fn set_parameter(
        &self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<(), UiError> {
        self.engine
            .edit_session(|| self.processor.set_parameter(id, value, gesture))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    use crate::audio::PrepareChain;

    use super::*;

    /// A limit for what must happen at once, so that a stall fails and does not hang the test.
    const LIMIT: Duration = Duration::from_secs(20);

    fn ir_tone() -> Tone {
        Tone::from_value(serde_json::json!({
            "id": 6,
            "title": "Remote",
            "format": "ir",
            "models": [{"id": 8, "name": "Cab", "model_url": "https://models.example/8.wav"}],
        }))
        .expect("tone")
    }

    fn ir_bytes() -> ModelBytes {
        let samples = [16_000_i16, 0, 0, 16_000];
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
        bytes.into()
    }

    fn processor() -> ProcessorBackend {
        ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        }))
    }

    /// A backend with a model that gave a non-finite output, which asks for a repair.
    ///
    /// The repair compile runs through `prepare`.
    fn backend_with_a_bad_model(
        prepare: PrepareChain,
    ) -> (Arc<StandaloneBackend>, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("tempdir");
        let backend = Arc::new(
            StandaloneBackend::dummy(processor(), &directory.path().join(PREFERENCES_SETTINGS))
                .expect("backend"),
        );
        let target = backend.chain_state(0).chain.left[0].id.clone();
        backend
            .dispatch(UiAction::LoadToneWithBytes {
                target_insert_id: target.clone(),
                tone: ir_tone(),
                model_id: ModelId(8),
                bytes: ir_bytes(),
            })
            .expect("load");
        backend.settle_audio().expect("settle");
        backend.engine.set_prepare_for_test(prepare);
        let block = backend
            .chain_state(0)
            .chain
            .block(&target)
            .cloned()
            .expect("block");
        let mut params = block.params;
        params.mix = f64::NAN;
        backend
            .dispatch(UiAction::SetBlockParams {
                block_id: target,
                params,
            })
            .expect("poison");
        render(&backend);
        (backend, directory)
    }

    fn render(backend: &StandaloneBackend) {
        let frames = backend.engine.audio_frames_for_test();
        backend.render_dummy(&vec![0.25; frames]).expect("render");
    }

    /// Runs one call on another thread, and fails if the call does not return.
    fn returns_at_once<T: Send + 'static>(
        what: &str,
        call: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (answer, returned) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer.send(call());
        });
        returned
            .recv_timeout(LIMIT)
            .unwrap_or_else(|_| panic!("{what} waited for the compile"))
    }

    fn published(backend: &StandaloneBackend) -> u64 {
        backend.audio_status().expect("status").published_generation
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + LIMIT;
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_frame_returns_while_a_repair_compile_is_held_open() {
        let (started_tx, started) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let compiles = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&compiles);
        let (backend, _directory) =
            backend_with_a_bad_model(Arc::new(move |plan: &ronie_plugin::SessionChainPlan| {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = started_tx.send(std::thread::current().id());
                let _ = gate.lock().expect("gate").recv_timeout(LIMIT);
                plan.prepare()
            }));
        let first = published(&backend);

        let frame = Arc::clone(&backend);
        let frame_thread = returns_at_once("the frame", move || {
            let device = frame.device_settings();
            (device.is_some(), std::thread::current().id())
        });
        assert!(frame_thread.0);
        let compile_thread = started
            .recv_timeout(LIMIT)
            .expect("the compile must run on the chain worker");
        assert_ne!(compile_thread, frame_thread.1, "the frame compiled");

        // Everything that a frame or an edit does stays free while the compile is open.
        for _ in 0..50 {
            let frame = Arc::clone(&backend);
            returns_at_once("a later frame", move || {
                let _ = frame.device_settings();
                let _ = frame.chain_faults();
                let _ = frame.meter_levels();
            });
        }
        let edit = Arc::clone(&backend);
        returns_at_once("an edit", move || {
            edit.set_parameter(
                ParameterId("outputLevel"),
                ParameterValue(0.5),
                GesturePhase::End,
            )
            .expect("edit");
        });
        assert_eq!(
            published(&backend),
            first,
            "no chain before the compile ends"
        );
        assert_eq!(compiles.load(Ordering::SeqCst), 1, "one compile at a time");
        assert!(
            !backend.chain_faults().is_empty(),
            "the fault shows during the repair"
        );

        drop(release);
        wait_for("the repair must reach the stream", || {
            published(&backend) > first
        });
    }

    #[test]
    fn a_compile_that_panics_does_not_stop_later_repairs() {
        let compiles = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&compiles);
        let (backend, _directory) =
            backend_with_a_bad_model(Arc::new(move |plan: &ronie_plugin::SessionChainPlan| {
                assert!(
                    counted.fetch_add(1, Ordering::SeqCst) > 0,
                    "this panic is the test"
                );
                plan.prepare()
            }));
        let first = published(&backend);

        let _ = backend.device_settings();
        wait_for("the first compile must run", || {
            compiles.load(Ordering::SeqCst) == 1 && !backend.engine.repair_wanted_for_test()
        });
        backend.engine.age_repair_for_test();
        render(&backend);

        wait_for("a later repair must reach the stream", || {
            let _ = backend.device_settings();
            published(&backend) > first
        });
    }

    #[test]
    fn a_poisoned_engine_lock_does_not_discard_the_restore_request() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        let processor = processor();
        processor
            .dispatch(UiAction::SavePreset {
                name: "Pending".to_owned(),
            })
            .expect("save preset");
        let id = processor
            .chain_state(0)
            .session
            .active_preset
            .clone()
            .expect("active preset")
            .id;
        let backend = StandaloneBackend::dummy(processor.clone(), &root.join(PREFERENCES_SETTINGS))
            .expect("backend");
        processor
            .dispatch(UiAction::LoadPreset { id })
            .expect("prepare preset restore");
        backend.engine.poison_for_test();

        assert!(
            backend
                .set_parameter(
                    ParameterId("outputLevel"),
                    ParameterValue(0.5),
                    GesturePhase::End,
                )
                .is_err()
        );
        assert!(
            matches!(
                backend
                    .processor
                    .take_restore()
                    .expect("read retained restore"),
                Some(ronie_plugin::SessionRestore::Compile)
            ),
            "the request must remain owned by the processor after lock failure"
        );
    }
}

//! Adapter from the control-side processor to the host-neutral UI backend.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use ronie_core::{
    InputMode, ModelBytes, ModelId, PresetStore, PropertiesFile, SettingsStore, Tone,
    params::{ParameterCatalog, ParameterId, ParameterValue},
};
use ronie_dsp::PrepareSpec;
use ronie_ui::backend::{
    ChainViewState, GesturePhase, HostControl, MeterSnapshot, UiAction, UiError, UiReceipt,
};

use crate::{
    audio_session::{AudioSessionState, SessionChain, SessionRestore, StreamFormat},
    processor::{RonieProcessor, SessionChainPlan},
};

pub use ronie_ui::backend::UiBackend;

/// Thread-safe control-state adapter; the active real-time graph remains callback-owned.
#[derive(Debug, Clone)]
pub struct ProcessorBackend {
    inner: Arc<BackendInner>,
}

#[derive(Debug)]
struct BackendInner {
    processor: Mutex<RonieProcessor>,
    settings: Option<SettingsStore>,
    /// The settings file as the backend read it one time at its start.
    ///
    /// A preference with no value in the processor comes from this copy, so a frame never reads
    /// the file. A key that the file did not have stays absent. A value that another process
    /// stores after the start is not seen until the next start; a value that this backend stores
    /// goes into the processor and is seen at once.
    stored: PropertiesFile,
    /// Held from the acceptance of an action until its setting is stored, so that two settings
    /// writes store their values in the order of their actions. It holds no data.
    settings_order: Mutex<()>,
    session_path: Option<std::path::PathBuf>,
}

impl ProcessorBackend {
    /// Wraps one control-side processor without machine persistence.
    #[must_use]
    pub fn new(processor: RonieProcessor) -> Self {
        Self {
            inner: Arc::new(BackendInner {
                processor: Mutex::new(processor),
                settings: None,
                stored: PropertiesFile::default(),
                settings_order: Mutex::new(()),
                session_path: None,
            }),
        }
    }

    /// Wraps a processor and seeds machine defaults before any host restore.
    ///
    /// # Errors
    /// Returns settings or parameter restoration errors.
    pub fn with_settings(
        processor: RonieProcessor,
        settings: SettingsStore,
    ) -> Result<Self, UiError> {
        let root = settings
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let processor = processor.with_preset_store(Arc::new(PresetStore::new(
            root.join("presets"),
            root.join("system"),
        )));
        let mut backend = Self {
            inner: Arc::new(BackendInner {
                processor: Mutex::new(processor),
                settings: Some(settings),
                stored: PropertiesFile::default(),
                settings_order: Mutex::new(()),
                session_path: None,
            }),
        };
        let stored = backend.seed_machine_defaults()?;
        let session_path = root.join("session.state");
        backend.restore_session(&session_path)?;
        let inner = Arc::get_mut(&mut backend.inner)
            .ok_or_else(|| UiError::Control("standalone backend is already shared".to_owned()))?;
        inner.session_path = Some(session_path);
        inner.stored = stored;
        Ok(backend)
    }

    /// Takes what the latest session or preset restore left for the audio side.
    ///
    /// A saved session at launch can leave a prepared chain. A preset load leaves
    /// [`SessionRestore::Compile`], because it compiles no model.
    ///
    /// # Errors
    /// Returns an error when the control-state mutex is poisoned.
    pub fn take_restore(&self) -> Result<Option<SessionRestore>, UiError> {
        Ok(self.inner.lock()?.take_restore())
    }

    /// Records the final stream layout that later chain preparation must target.
    ///
    /// # Errors
    /// Returns an error when the control-state mutex is poisoned.
    pub fn set_stream_format(&self, format: StreamFormat) -> Result<(), UiError> {
        self.inner.lock()?.set_stream_format(format);
        Ok(())
    }

    /// Captures the session state that a running stream must reproduce.
    ///
    /// # Errors
    /// Returns an error when the control-state mutex is poisoned.
    pub fn audio_state(&self) -> Result<AudioSessionState, UiError> {
        Ok(self.inner.lock()?.audio_state())
    }

    /// Records the final stream layout, and copies what one chain for that layout reads, in one
    /// lock of the processor.
    ///
    /// The plan holds the session state of that same lock, so a caller can record what the chain
    /// reproduces before [`SessionChainPlan::prepare`] compiles it with no lock.
    ///
    /// # Errors
    /// Returns an error when the control-state mutex is poisoned.
    pub fn chain_plan(&self, format: StreamFormat) -> Result<SessionChainPlan, UiError> {
        let mut processor = self.inner.lock()?;
        processor.set_stream_format(format);
        Ok(processor.session_chain_plan())
    }

    /// Prepares a complete new chain from the current session on the calling thread.
    ///
    /// The lock covers only a copy of the session, so a frame does not wait for the compile.
    ///
    /// # Errors
    /// Returns a mutex error or a chain preparation error.
    pub fn prepare_session_chain(&self) -> Result<SessionChain, UiError> {
        let plan = self.inner.lock()?.session_chain_plan();
        plan.prepare()
    }

    /// Saves the session for a process that is about to exit, and releases this reference.
    ///
    /// The session also saves when the last reference drops. A compile on a blocking thread can
    /// hold a reference, and a process exit does not wait for that thread, so that drop can never
    /// run. This call does not wait for the compile, because a compile holds no lock.
    ///
    /// # Errors
    /// Returns a lock or a file error. With no other reference, the drop reports the error instead.
    pub fn save_at_exit(self) -> Result<(), UiError> {
        match Arc::try_unwrap(self.inner) {
            Ok(last) => {
                drop(last);
                Ok(())
            }
            Err(shared) => shared.save_session(),
        }
    }

    /// Seeds the machine defaults, and returns the settings file that it read for them.
    fn seed_machine_defaults(&self) -> Result<PropertiesFile, UiError> {
        let Some(settings) = &self.inner.settings else {
            return Ok(PropertiesFile::default());
        };
        let properties = settings
            .read()
            .map_err(|error| UiError::Control(error.to_string()))?;
        let mut processor = self.inner.lock()?;
        for spec in ParameterCatalog::v1()
            .iter()
            .filter(|spec| spec.scope.machine_default)
        {
            if let Some(value) = properties
                .get(spec.id.0)
                .and_then(|value| value.parse::<f32>().ok())
            {
                processor.set_parameter(spec.id, ParameterValue(value))?;
            }
        }
        if properties.get("multiCore").is_some() {
            let enabled = properties.get_bool("multiCore", true);
            processor.dispatch(UiAction::SetMultiCore(enabled))?;
        }
        if let Some(value) = properties
            .get("namSlimSizeDefault")
            .and_then(|value| value.parse::<f32>().ok())
        {
            processor.dispatch(UiAction::SetNamFullSize(value >= 0.5))?;
        }
        // The machine defaults are the start state and not an edit, so no undo step holds them.
        processor.clear_history();
        drop(processor);
        Ok(properties)
    }

    fn restore_session(&self, path: &Path) -> Result<(), UiError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(UiError::Control(format!(
                    "reading standalone session {}: {error}",
                    path.display()
                )));
            }
        };
        self.inner.lock()?.restore_session_bytes(&bytes)
    }
}

impl BackendInner {
    fn save_session(&self) -> Result<(), UiError> {
        let Some(path) = &self.session_path else {
            return Ok(());
        };
        let bytes = self.lock()?.session_bytes()?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|error| {
            UiError::Control(format!(
                "creating session directory {}: {error}",
                parent.display()
            ))
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| UiError::Control(error.to_string()))?;
        std::io::Write::write_all(&mut temporary, &bytes)
            .map_err(|error| UiError::Control(error.to_string()))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| UiError::Control(error.to_string()))?;
        temporary.persist(path).map_err(|error| {
            UiError::Control(format!(
                "replacing standalone session {}: {}",
                path.display(),
                error.error
            ))
        })?;
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, RonieProcessor>, UiError> {
        self.processor
            .lock()
            .map_err(|error| UiError::Control(error.to_string()))
    }
}

/// The machine setting that an action stores after the processor accepts it.
enum SettingWrite {
    /// A key with a value that the action itself gives.
    Value { key: String, value: String },
    /// The current value of a machine-default parameter.
    MachineDefault(ParameterId),
}

impl SettingWrite {
    fn for_action(action: &UiAction) -> Option<Self> {
        let bool_text = |value: bool| if value { "true" } else { "false" }.to_owned();
        match action {
            UiAction::PersistMachineDefault(id) => Some(Self::MachineDefault(*id)),
            UiAction::SetMultiCore(enabled) => Some(Self::Value {
                key: "multiCore".to_owned(),
                value: bool_text(*enabled),
            }),
            UiAction::SetNamFullSize(full) => Some(Self::Value {
                key: "namSlimSizeDefault".to_owned(),
                value: if *full { "1.0" } else { "0.0" }.to_owned(),
            }),
            UiAction::SetPreference { key, value } => Some(Self::Value {
                key: key.clone(),
                value: bool_text(*value),
            }),
            UiAction::SetTextPreference { key, value } => Some(Self::Value {
                key: key.clone(),
                value: value.clone(),
            }),
            _ => None,
        }
    }

    fn resolve(self, processor: &RonieProcessor) -> Result<(String, String), UiError> {
        match self {
            Self::Value { key, value } => Ok((key, value)),
            Self::MachineDefault(id) => {
                let value = processor
                    .parameter(id)
                    .ok_or(UiError::UnknownParameter(id))?;
                Ok((id.0.to_owned(), value.0.to_string()))
            }
        }
    }
}

impl Drop for BackendInner {
    fn drop(&mut self) {
        if let Err(error) = self.save_session() {
            eprintln!("Ronnie could not save the standalone session: {error}");
        }
    }
}

impl UiBackend for ProcessorBackend {
    fn chain_state(&self, known_revision: u64) -> ChainViewState {
        match self.inner.lock() {
            Ok(processor) => processor.chain_state(known_revision),
            // This processor only gives a view and prepares no chain. The audio engine sets the
            // oversampling of each live chain from OPTIMIZE and the oversampling setting.
            Err(_) => RonieProcessor::new(PrepareSpec {
                host_sample_rate: 48_000.0,
                max_host_frames: 64,
                oversampling: 1,
                input_mode: InputMode::Stereo,
                stereo_input: false,
                stereo_output: true,
            })
            .chain_state(known_revision),
        }
    }

    fn meter_levels(&self) -> MeterSnapshot {
        self.inner.lock().map_or_else(
            |_| MeterSnapshot::default(),
            |processor| processor.meter_levels(),
        )
    }

    fn supports(&self, control: HostControl) -> bool {
        // No real adapter can carry out these controls yet, so that no real view offers them.
        match control {
            HostControl::TestTone | HostControl::AutoBalance => false,
        }
    }

    fn midi_map(&self) -> ronie_ui::default::MidiMapSnapshot {
        self.inner.lock().map_or_else(
            |_| ronie_ui::default::MidiMapSnapshot::default(),
            |processor| processor.midi_map(),
        )
    }

    fn calibration_draft(&self) -> Option<String> {
        self.inner.lock().ok()?.calibration_draft()
    }

    fn preference(&self, key: &str) -> Option<bool> {
        if let Some(value) = self.inner.lock().ok()?.preference(key) {
            return Some(value);
        }
        // The copy from the start holds each stored value, so no frame reads the file.
        let stored = &self.inner.stored;
        stored.get(key).map(|_| stored.get_bool(key, false))
    }

    fn text_preference(&self, key: &str) -> Option<String> {
        if let Some(value) = self.inner.lock().ok()?.text_preference(key) {
            return Some(value);
        }
        // The copy from the start holds each stored value, so no frame reads the file.
        self.inner.stored.get(key).map(str::to_owned)
    }

    fn dispatch(&self, action: UiAction) -> Result<UiReceipt, UiError> {
        let write = self
            .inner
            .settings
            .as_ref()
            .and_then(|_| SettingWrite::for_action(&action));
        let (receipt, pending) = {
            let mut processor = self.inner.lock()?;
            // The processor validates the action. Only an accepted action reaches the settings.
            let (revision, effect) = processor.dispatch(action)?;
            let pending = write.map(|write| {
                // The order lock is taken before the processor lock is released, so that the
                // next action cannot store its setting first. The order lock holds no data, so
                // a poisoned lock still keeps the order.
                let order = self
                    .inner
                    .settings_order
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                (write.resolve(&processor), order)
            });
            (UiReceipt { revision, effect }, pending)
        };
        // The file write holds no processor lock, so a frame does not wait for it.
        if let (Some(settings), Some((setting, _order))) = (&self.inner.settings, pending) {
            let stored = setting.and_then(|(key, value)| {
                settings
                    .merge_key(&key, &value)
                    .map_err(|error| UiError::Control(error.to_string()))
            });
            // The processor accepted the action, so a failed write does not refuse it.
            if let Err(error) = stored {
                eprintln!("Ronnie could not store a machine setting: {error}");
            }
        }
        Ok(receipt)
    }

    fn validate_model(
        &self,
        tone: &Tone,
        model_id: ModelId,
        bytes: &ModelBytes,
    ) -> Result<(), UiError> {
        crate::persistence::validate_model(tone, model_id, bytes)
    }

    fn can_paste_block(&self) -> bool {
        self.inner
            .lock()
            .is_ok_and(|processor| processor.can_paste_block())
    }

    fn parameter(&self, id: ParameterId) -> Option<ParameterValue> {
        self.inner.lock().ok()?.parameter(id)
    }

    fn set_parameter(
        &self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<(), UiError> {
        self.inner.lock()?.edit_parameter(id, value, gesture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_preparation_needs_no_lock_of_the_processor() {
        let backend = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        }));
        let plan = backend
            .inner
            .lock()
            .expect("processor")
            .session_chain_plan();

        // A frame takes this lock. A preparation that also took it would never end here.
        let _frame = backend.inner.lock().expect("processor");
        assert!(plan.prepare().expect("chain").unavailable.is_empty());
    }

    #[test]
    fn a_plan_compiles_the_state_of_its_own_lock_and_not_a_later_edit() {
        let backend = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        }));
        let format = StreamFormat {
            sample_rate: 44_100.0,
            max_frames: 256,
            stereo_input: false,
            stereo_output: true,
            oversampling: 2,
        };
        let stereo = backend.chain_state(0).chain.stereo_enabled;
        let plan = backend.chain_plan(format).expect("plan");
        assert_eq!(plan.state(), &backend.audio_state().expect("state"));

        // An edit and its undo can come before the compile. The chain still has the planned state.
        backend
            .dispatch(UiAction::SetStereoMode(!stereo))
            .expect("structure edit");
        assert_ne!(plan.state(), &backend.audio_state().expect("state"));
        let chain = plan.prepare().expect("chain");

        assert_eq!(chain.change.generation, plan.generation());
        assert_eq!(chain.change.blocks.snapshot().stereo_enabled, stereo);
        assert_eq!(chain.change.spec.host_sample_rate, format.sample_rate);
        assert_eq!(chain.change.spec.max_host_frames, format.max_frames);
        assert_eq!(chain.change.spec.oversampling, format.oversampling);
    }

    fn settings_backend(root: &Path) -> ProcessorBackend {
        ProcessorBackend::with_settings(
            RonieProcessor::new(PrepareSpec {
                host_sample_rate: 48_000.0,
                max_host_frames: 128,
                oversampling: 1,
                input_mode: InputMode::Left,
                stereo_input: false,
                stereo_output: true,
            }),
            SettingsStore::standalone(root, ronie_core::PREFERENCES_SETTINGS),
        )
        .expect("settings backend")
    }

    #[test]
    fn a_refused_action_stores_no_setting() {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let backend = settings_backend(directory.path());
        let poisoner = backend.clone();
        // A panic that holds the lock poisons it, so the processor refuses every action.
        let _ = std::thread::spawn(move || {
            let _processor = poisoner.inner.lock().expect("processor");
            panic!("poison the processor lock");
        })
        .join();

        for action in [
            UiAction::SetMultiCore(false),
            UiAction::SetNamFullSize(false),
            UiAction::SetPreference {
                key: "t3k.showPitchControl".to_owned(),
                value: true,
            },
            UiAction::PersistMachineDefault(ParameterId("inputCalibrationLevel")),
        ] {
            assert!(backend.dispatch(action).is_err());
        }

        let properties =
            SettingsStore::standalone(directory.path(), ronie_core::PREFERENCES_SETTINGS)
                .read()
                .expect("stored properties");
        for key in [
            "multiCore",
            "namSlimSizeDefault",
            "t3k.showPitchControl",
            "inputCalibrationLevel",
        ] {
            assert_eq!(properties.get(key), None, "a refused action stored {key}");
        }
    }

    #[test]
    fn a_failed_setting_write_still_accepts_the_action() {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let backend = settings_backend(directory.path());
        let settings =
            SettingsStore::standalone(directory.path(), ronie_core::PREFERENCES_SETTINGS);
        let lock = settings.path().with_extension("lock");
        let _ = std::fs::remove_file(&lock);
        // A folder in place of the lock file makes each settings write fail.
        std::fs::create_dir(&lock).expect("lock folder");
        let before = backend.chain_state(0);

        let receipt = backend
            .dispatch(UiAction::SetMultiCore(!before.multi_core))
            .expect("the processor accepted the action");

        let after = backend.chain_state(0);
        assert_eq!(after.multi_core, !before.multi_core);
        assert!(after.revision > before.revision);
        assert_eq!(receipt.revision, after.revision);
        let properties = settings.read().expect("stored properties");
        assert_eq!(properties.get("multiCore"), None);
    }

    #[test]
    fn a_started_backend_has_no_history() {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let settings =
            SettingsStore::standalone(directory.path(), ronie_core::PREFERENCES_SETTINGS);
        for (key, value) in [
            ("multiCore", "false"),
            ("namSlimSizeDefault", "1.0"),
            ("inputCalibrationLevel", "6"),
        ] {
            settings.merge_key(key, value).expect("stored setting");
        }

        let first = settings_backend(directory.path());
        let view = first.chain_state(0);
        assert!(!view.multi_core && view.nam_full_size);
        assert!(!view.can_undo && !view.can_redo, "a first launch");
        first.save_at_exit().expect("save");

        let second = settings_backend(directory.path());
        let view = second.chain_state(0);
        assert!(!view.multi_core && view.nam_full_size);
        assert!(
            !view.can_undo && !view.can_redo,
            "a launch with a saved session"
        );
    }

    #[test]
    fn a_text_preference_stays_across_a_restart() {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let first = settings_backend(directory.path());
        assert_eq!(first.text_preference("pitchInstrument"), None);

        first
            .dispatch(UiAction::SetTextPreference {
                key: "pitchInstrument".to_owned(),
                value: "bass5".to_owned(),
            })
            .expect("text preference");
        assert_eq!(
            first.text_preference("pitchInstrument").as_deref(),
            Some("bass5")
        );
        let view = first.chain_state(0);
        assert!(!view.can_undo, "a preference is not an undo step");
        drop(first);

        let properties =
            SettingsStore::standalone(directory.path(), ronie_core::PREFERENCES_SETTINGS)
                .read()
                .expect("stored properties");
        assert_eq!(properties.get("pitchInstrument"), Some("bass5"));
        let second = settings_backend(directory.path());
        assert_eq!(
            second.text_preference("pitchInstrument").as_deref(),
            Some("bass5")
        );
    }

    #[test]
    fn a_preference_read_does_not_read_the_settings_file() {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let settings =
            SettingsStore::standalone(directory.path(), ronie_core::PREFERENCES_SETTINGS);
        settings
            .merge_key("t3k.showGateControl", "true")
            .expect("stored preference");
        settings
            .merge_key("pitchInstrument", "bass5")
            .expect("stored text preference");
        let backend = settings_backend(directory.path());

        // A file read after the start would find these values. The backend must not read them.
        settings
            .merge_key("missing.key", "true")
            .expect("later preference");
        settings
            .merge_key("missing.text", "later")
            .expect("later text preference");
        for _ in 0..100 {
            assert_eq!(backend.preference("missing.key"), None);
            assert_eq!(backend.text_preference("missing.text"), None);
        }
        // A file that cannot be read no longer has an effect on a read.
        let path = settings.path().to_path_buf();
        std::fs::remove_file(&path).expect("remove settings");
        std::fs::create_dir(&path).expect("folder in place of the settings file");
        for _ in 0..100 {
            assert_eq!(backend.preference("missing.key"), None);
        }

        // The values from before the start stay.
        assert_eq!(backend.preference("t3k.showGateControl"), Some(true));
        assert_eq!(
            backend.text_preference("pitchInstrument").as_deref(),
            Some("bass5")
        );
        // A value that this backend sets is seen at once.
        backend
            .dispatch(UiAction::SetPreference {
                key: "missing.key".to_owned(),
                value: false,
            })
            .expect("preference");
        assert_eq!(backend.preference("missing.key"), Some(false));
    }

    #[test]
    fn a_save_at_exit_saves_with_no_other_reference() {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let backend = settings_backend(directory.path());

        backend.save_at_exit().expect("save");

        assert!(directory.path().join("session.state").is_file());
    }

    #[test]
    fn a_save_at_exit_saves_while_a_compile_holds_a_reference() {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let backend = settings_backend(directory.path());
        let compile = backend.clone();

        backend.save_at_exit().expect("save");

        assert!(directory.path().join("session.state").is_file());
        std::fs::remove_file(directory.path().join("session.state")).expect("remove");
        drop(compile);
        assert!(
            directory.path().join("session.state").is_file(),
            "a compile that finishes before the exit still saves its own result"
        );
    }

    #[test]
    fn concurrent_final_clones_cannot_both_skip_the_session_save() {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let root = directory.path();
        let settings = SettingsStore::standalone(root, ronie_core::PREFERENCES_SETTINGS);
        let first = ProcessorBackend::with_settings(
            RonieProcessor::new(PrepareSpec {
                host_sample_rate: 48_000.0,
                max_host_frames: 128,
                oversampling: 1,
                input_mode: InputMode::Left,
                stereo_input: false,
                stereo_output: true,
            }),
            settings,
        )
        .expect("settings backend");
        first
            .set_parameter(
                ParameterId("inputLevel"),
                ParameterValue(0.75),
                GesturePhase::End,
            )
            .expect("edit parameter");
        let second = first.clone();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            for backend in [first, second] {
                let barrier = std::sync::Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    drop(backend);
                });
            }
        });
        assert!(root.join("session.state").is_file());
    }
}

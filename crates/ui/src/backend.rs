//! Host-neutral UI actions, receipts, and a deterministic mock backend.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use ronie_core::{
    BlockId, BlockParams, ChainBlock, ChainBranch, ChainSide, ChainSnapshot, EqParams, InputMode,
    LocalStash, MidiMapping, MidiSource, ModelBytes, ModelId, PresetInfo, SessionSnapshot, Tone,
    params::{ParameterCatalog, ParameterId, ParameterValue},
};
use ronie_dsp::PrepareSpec;
use thiserror::Error;

use crate::session_edit::{
    SessionEditor, SessionFrame, SessionSeed, SessionStorage, move_in_order,
};

/// Chain and host state consumed by any renderer without toolkit-specific types.
///
/// The chain and the session are shared. Each view of one unchanged session holds the same
/// snapshots, so a view costs no copy of the chain. A change makes new snapshots and never
/// changes a shared one, so a view that a caller keeps stays as it was.
#[derive(Debug, Clone)]
pub struct ChainViewState {
    /// Current chain revision.
    pub revision: u64,
    /// Whether the caller's known revision is stale.
    pub changed: bool,
    /// Immutable chain snapshot.
    pub chain: Arc<ChainSnapshot>,
    /// Immutable session-only state.
    pub session: Arc<SessionSnapshot>,
    /// Immutable audio preparation contract.
    pub preparation: PrepareSpec,
    /// Whether undo is available.
    pub can_undo: bool,
    /// Whether redo is available.
    pub can_redo: bool,
    /// Whether multicore processing is enabled.
    pub multi_core: bool,
    /// Whether full-size NAM models are selected for new blocks.
    pub nam_full_size: bool,
}

/// One block's current input and output levels in dB.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockMeter {
    /// Stable block identity.
    pub block_id: BlockId,
    /// Input level in dB.
    pub input_db: f32,
    /// Output level in dB.
    pub output_db: f32,
}

/// A single polled meter snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterSnapshot {
    /// Input levels by channel in dB.
    pub input_db: [f32; 2],
    /// Output levels by channel in dB.
    pub output_db: [f32; 2],
    /// Per-block levels.
    pub blocks: Vec<BlockMeter>,
    /// Audio callback load from zero to one.
    pub cpu: f32,
    /// Stereo correlation from -1 to 1.
    pub correlation: f32,
}

impl Default for MeterSnapshot {
    fn default() -> Self {
        Self {
            input_db: [-60.0, -60.0],
            output_db: [-60.0, -60.0],
            blocks: Vec::new(),
            cpu: 0.0,
            correlation: 1.0,
        }
    }
}

/// Automation gesture phase for a continuous parameter edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GesturePhase {
    /// Begin one coalesced edit.
    Begin,
    /// Continue an existing edit.
    Change,
    /// Commit one coalesced edit.
    End,
    /// Cancel and restore the value from before the gesture.
    Cancel,
}

/// Audio device and MIDI configuration supplied through the UI boundary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioDeviceConfig {
    /// Requested host sample rate.
    pub sample_rate: Option<f64>,
    /// Requested callback size.
    pub buffer_frames: Option<u32>,
    /// Identity of the selected input row.
    pub input: Option<String>,
    /// Identity of the selected output row.
    pub output: Option<String>,
    /// Enabled MIDI input names.
    pub midi_inputs: Vec<String>,
}

/// Requests that only the host can carry out.
///
/// The UI has nothing else to do for these controls, so this request is their only outcome. No
/// adapter carries them out yet.
#[derive(Debug, Clone, PartialEq)]
pub enum NavigationAction {
    /// Open the operating system's microphone privacy page.
    OpenMicrophonePrivacy,
    /// Open the user preset folder.
    OpenPresetsFolder,
    /// Copy recent diagnostic logs.
    CopyLogs,
    /// Reveal the diagnostic log file.
    RevealLogs,
}

/// A control that a view shows only when the backend can carry it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostControl {
    /// The test tone button of the output device row.
    TestTone,
    /// The automatic balance button of the faceplate.
    AutoBalance,
}

/// Standalone device actions.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceAction {
    /// Apply a new device configuration.
    Configure(AudioDeviceConfig),
    /// Cycle to the next input routing mode.
    CycleInputMode,
    /// Select an available audio backend by name.
    SelectBackend(String),
    /// Enable or disable input monitoring.
    SetHearYourself(bool),
    /// Replace selected input channels in selection order.
    SetInputChannels(Vec<usize>),
    /// Select an output channel pair.
    SetOutputPair(i32),
    /// Enable or disable one MIDI input.
    SetMidiInputEnabled {
        /// Stable MIDI input identity.
        id: String,
        /// New enablement.
        enabled: bool,
    },
    /// Play a short output test tone.
    PlayTestTone,
    /// Reopen the current device.
    Restart,
    /// Open the vendor control panel.
    OpenControlPanel,
    /// Open Bluetooth MIDI pairing.
    OpenBluetoothMidi,
}

/// MIDI mapping, learning, and UI-draft actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiAction {
    /// Set the channel filter, with zero for omni.
    SetChannel(u8),
    /// Arm one target for learning.
    StartLearn(String),
    /// Cancel the current learn request.
    CancelLearn,
    /// Set a typed UI-only draft.
    SetDraft(Option<String>),
    /// Set or replace one target mapping.
    SetMapping {
        /// Target identity.
        target_id: String,
        /// MIDI event family.
        source: MidiSource,
        /// Controller or note number.
        number: u8,
    },
    /// Remove one target mapping.
    RemoveMapping(String),
}

/// Every chain, preset, device, and host action that views send to the audio side.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    /// Change a parameter through the same boundary as direct parameter writes.
    SetParameter {
        /// Parameter identity.
        id: ParameterId,
        /// New real-unit value.
        value: ParameterValue,
        /// Gesture phase.
        gesture: GesturePhase,
    },
    /// Enable or disable the second chain.
    SetStereoMode(bool),
    /// Swap complete left and right lanes, including branch and polarity state.
    SwapChains,
    /// Select input routing.
    SetInputMode(InputMode),
    /// Enable or disable multicore processing.
    SetMultiCore(bool),
    /// Select full or slim models for new blocks.
    SetNamFullSize(bool),
    /// Undo one control edit.
    Undo,
    /// Redo one control edit.
    Redo,
    /// Restore the empty default chain.
    ResetToDefault,
    /// Select the active lane.
    SelectSide(ChainSide),
    /// Set or clear the branch point.
    SetBranch(Option<ChainBranch>),
    /// Load one tone into an insert.
    LoadTone {
        /// Destination insert.
        target_insert_id: BlockId,
        /// Tone metadata.
        tone: Tone,
        /// Selected model.
        model_id: ModelId,
    },
    /// Load one catalog tone with bytes that a background worker downloaded.
    ///
    /// This changes the session and stores the bytes. The running audio chain follows when the
    /// same thread calls [`UiBackend::settle_audio`], so a caller can hold a lock across this
    /// dispatch without holding it across a compile.
    LoadToneWithBytes {
        /// Destination insert.
        target_insert_id: BlockId,
        /// Tone metadata, which contains the selected model.
        tone: Tone,
        /// Selected model.
        model_id: ModelId,
        /// Complete model bytes.
        bytes: ModelBytes,
    },
    /// Replace a block with an open insert.
    RemoveBlock(BlockId),
    /// Duplicate a block after itself.
    DuplicateBlock(BlockId),
    /// Duplicate a block into an exact lane and index.
    DuplicateBlockToLane {
        /// Source block.
        block_id: BlockId,
        /// Destination lane.
        target_side: ChainSide,
        /// Destination zero-based index.
        target_index: usize,
    },
    /// Move a block within its lane.
    MoveBlock {
        /// Block to move.
        block_id: BlockId,
        /// New zero-based index.
        target_index: usize,
    },
    /// Copy a block into the clipboard.
    CopyBlock(BlockId),
    /// Paste the clipboard into one lane at an index.
    PasteBlock {
        /// Destination lane.
        side: ChainSide,
        /// Destination zero-based index.
        index: usize,
    },
    /// Move a block to any lane and index.
    MoveBlockToLane {
        /// Block to move.
        block_id: BlockId,
        /// Destination lane.
        target_side: ChainSide,
        /// Destination zero-based index.
        target_index: usize,
    },
    /// Place a block that was just added, without an undo step of its own.
    PlaceAddedBlock {
        /// The added block.
        block_id: BlockId,
        /// Destination lane.
        side: ChainSide,
        /// Destination zero-based index, clamped before the trailing insert.
        index: usize,
    },
    /// Replace the tone of a tone block in place, as one undo step. The block keeps its id, its
    /// place, and its params, and loses the second IR and the skin of the old tone.
    ReplaceTone {
        /// The tone block to replace.
        target_block_id: BlockId,
        /// Tone metadata.
        tone: Tone,
        /// Selected model.
        model_id: ModelId,
        /// Complete model bytes that a background worker downloaded, or `None` for a tone whose
        /// bytes the backend already holds. With bytes, the running audio chain follows when the
        /// same thread calls [`UiBackend::settle_audio`], as for [`UiAction::LoadToneWithBytes`].
        bytes: Option<ModelBytes>,
    },
    /// Replace the tone of a tone block in place with a local file, as [`UiAction::ReplaceTone`]
    /// does.
    ReplaceWithFile {
        /// The tone block to replace.
        target_block_id: BlockId,
        /// User-selected file or directory.
        path: PathBuf,
    },
    /// Load and stash a local file or folder into one insert.
    DropFile {
        /// Destination insert.
        target_insert_id: BlockId,
        /// User-selected file or directory.
        path: PathBuf,
    },
    /// Retry a failed active-model load.
    RetryModelLoad(BlockId),
    /// Replace a block's processing parameters.
    SetBlockParams {
        /// Block to edit.
        block_id: BlockId,
        /// New processing parameters.
        params: BlockParams,
    },
    /// Replace a block's processing parameters during a knob gesture.
    ///
    /// One gesture is one undo step, and a gesture that changes nothing is none. An action that
    /// removes the block closes its gesture, and a later end or cancel for that block is accepted
    /// with no change.
    SetBlockParamsGesture {
        /// Block to edit.
        block_id: BlockId,
        /// New processing parameters.
        params: BlockParams,
        /// Coalesced edit phase.
        gesture: GesturePhase,
    },
    /// Replace a block's EQ settings.
    ///
    /// The gesture rules are those of [`Self::SetBlockParamsGesture`].
    SetEq {
        /// Block to edit.
        block_id: BlockId,
        /// New EQ settings.
        params: EqParams,
        /// Coalesced edit phase.
        gesture: GesturePhase,
    },
    /// Select another model for a block.
    SelectModel {
        /// Block to edit.
        block_id: BlockId,
        /// New active model.
        model_id: ModelId,
    },
    /// Select another model of a catalog tone with bytes that a background worker downloaded.
    ///
    /// This changes the session and stores the bytes. The running audio chain follows when the
    /// same thread calls [`UiBackend::settle_audio`], so a caller can hold a lock across this
    /// dispatch without holding it across a compile.
    ///
    /// The action fails with no change unless the block still holds this tone on `replaces`, so
    /// a late completion cannot overwrite a newer change of the block.
    SelectModelWithBytes {
        /// Block to edit.
        block_id: BlockId,
        /// Stored tone, which contains the new model and no other catalog model.
        tone: Tone,
        /// New active model.
        model_id: ModelId,
        /// Active model when the switch started.
        replaces: Option<ModelId>,
        /// Complete model bytes.
        bytes: ModelBytes,
    },
    /// Set or clear the second IR of a cab block: another model of its tone, blended with the
    /// active model. `None` removes it and resets the blend, the levels and the invert, so the
    /// block plays its active model alone.
    ///
    /// The model must be an IR of the block's tone other than the active model.
    SelectSecondModel {
        /// Block to edit.
        block_id: BlockId,
        /// New second IR, or `None` to remove it.
        model_id: Option<ModelId>,
    },
    /// Set the second IR of a cab block to a catalog model with bytes that a background worker
    /// downloaded. The model joins the block's stored tone, which otherwise holds only the active
    /// model.
    ///
    /// The rules are those of [`Self::SelectModelWithBytes`], for the second IR: the action
    /// fails with no change unless the block still holds this tone with `replaces` as its
    /// second IR.
    SelectSecondModelWithBytes {
        /// Block to edit.
        block_id: BlockId,
        /// Stored tone that contains the new model.
        tone: Tone,
        /// New second IR.
        model_id: ModelId,
        /// Second IR when the switch started.
        replaces: Option<ModelId>,
        /// Complete model bytes.
        bytes: ModelBytes,
    },
    /// Choose the skin a block's render uses, or `None` for Auto: the skin of the gear family
    /// the tone identifies. One undo step; the audio chain is not touched.
    SetBlockSkin {
        /// Block to edit.
        block_id: BlockId,
        /// Skin identity, such as `"british"`, or `None` for Auto.
        skin: Option<String>,
    },
    /// Save or update a user preset.
    SavePreset {
        /// Preset display name.
        name: String,
    },
    /// Load a preset.
    LoadPreset {
        /// Preset identity.
        id: String,
    },
    /// Rename a user preset.
    RenamePreset {
        /// Preset identity.
        id: String,
        /// New display name.
        name: String,
    },
    /// Delete a user preset.
    DeletePreset {
        /// Preset identity.
        id: String,
    },
    /// Reorder a preset within its section.
    MovePreset {
        /// Preset identity.
        id: String,
        /// Relative movement.
        delta: i32,
    },
    /// Ask the host to carry out a request.
    Navigate(NavigationAction),
    /// Execute a device action.
    Device(DeviceAction),
    /// Execute a MIDI mapping or learn action.
    Midi(MidiAction),
    /// Persist a parameter as a machine default after an explicit user edit.
    PersistMachineDefault(ParameterId),
    /// Set a Boolean UI preference.
    SetPreference {
        /// Preference key.
        key: String,
        /// New value.
        value: bool,
    },
    /// Set a text UI preference, such as one choice of a list.
    ///
    /// A backend with machine settings stores the value, so it stays across restarts.
    SetTextPreference {
        /// Preference key.
        key: String,
        /// New value.
        value: String,
    },
    /// Retain or clear the calibration text draft.
    SetCalibrationDraft(Option<String>),
}

/// Backend-effect categories asserted by scenario tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEffect {
    /// No state effect.
    None,
    /// Chain or session changed.
    ChainChanged,
    /// A preset was saved or updated.
    PresetSaved,
    /// A parameter changed.
    ParameterChanged,
    /// A host request or a UI-only value was accepted.
    Navigation,
    /// Device configuration changed.
    DeviceChanged,
}

/// One accepted backend action and its resulting revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiReceipt {
    /// Revision after the action.
    pub revision: u64,
    /// Verified effect category.
    pub effect: UiEffect,
}

/// Backend failure contract shared by real and mock processors.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UiError {
    /// A drive or action named an unknown target.
    #[error("unknown UI target: {0}")]
    UnknownTarget(String),
    /// A parameter ID is outside the versioned catalog.
    #[error("unknown parameter: {0:?}")]
    UnknownParameter(ParameterId),
    /// A scenario's click could not play in the real UI.
    #[error("{scenario}: cannot click {target:?}: {reason}")]
    DriveFailed {
        /// Scenario identity.
        scenario: String,
        /// Fixture click target.
        target: String,
        /// Why the real UI did not take the click.
        reason: String,
    },
    /// A scenario's required backend effect was not observed.
    #[error("missing backend effect: expected {expected:?}, got {actual:?}")]
    MissingBackendEffect {
        /// Required effect.
        expected: UiEffect,
        /// Observed effect.
        actual: UiEffect,
    },
    /// An action is invalid in the current state.
    #[error("invalid UI action: {0}")]
    InvalidAction(String),
    /// Control-state access or storage failed.
    #[error("UI backend failure: {0}")]
    Control(String),
}

/// Why the running audio chain does not play what the session shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainFaultKind {
    /// The block has no model bytes, so it runs as bypass.
    MissingModel,
    /// The model bytes of the block cannot compile, so it runs as bypass.
    ModelRejected,
    /// The chain made non-finite samples, and the callback replaced them with silence.
    NonFiniteOutput,
    /// No chain can be prepared for the session, so the previous chain continues to run.
    ChainNotPrepared,
    /// The block failed during a callback, so it runs as bypass until a new chain replaces it.
    ProcessingFailed,
}

impl ChainFaultKind {
    const fn slug(self) -> &'static str {
        match self {
            Self::MissingModel => "missing-model",
            Self::ModelRejected => "model-rejected",
            Self::NonFiniteOutput => "non-finite-output",
            Self::ChainNotPrepared => "chain-not-prepared",
            Self::ProcessingFailed => "processing-failed",
        }
    }
}

/// One audio chain fault, as data that holds no reason text from a model or a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainFault {
    /// What is wrong.
    pub kind: ChainFaultKind,
    /// Tone title on the block, or `None` when the fault has no block or the block has no tone.
    pub title: Option<String>,
    /// Identity of this fault. A different block, tone, or model is a different fault.
    pub identity: String,
}

impl ChainFault {
    /// A fault on one block, with the identity of its current tone and model.
    #[must_use]
    pub fn for_block(kind: ChainFaultKind, block: &ChainBlock) -> Self {
        let tone = block.tone.as_ref();
        let tone_id = tone.map_or(-1, |tone| tone.id.0);
        let model_id = block.active_model_id.map_or(-1, |model| model.0);
        Self {
            kind,
            title: tone.map(|tone| tone.title.clone()),
            identity: format!("{}:{}:{}:{}", kind.slug(), block.id.0, tone_id, model_id),
        }
    }

    /// A fault that no single block owns.
    #[must_use]
    pub fn for_chain(kind: ChainFaultKind) -> Self {
        Self {
            kind,
            title: None,
            identity: kind.slug().to_owned(),
        }
    }
}

/// Host-neutral interface consumed by every renderer and implemented by real and mock processors.
pub trait UiBackend: Send + Sync {
    /// Returns an immutable chain snapshot and whether it differs from `known_revision`.
    fn chain_state(&self, known_revision: u64) -> ChainViewState;
    /// Returns one coherent meter snapshot.
    fn meter_levels(&self) -> MeterSnapshot;
    /// Returns the latest 64-bin spectrum for one block, or the silence floor.
    fn spectrum_bins(&self, _block_id: &BlockId) -> [f32; 64] {
        [-100.0; 64]
    }
    /// Returns the latest raw-input tuner reading, or silence when unavailable.
    fn tuner_reading(&self) -> ronie_dsp::analysis::TunerReading {
        ronie_dsp::analysis::TunerReading {
            frequency_hz: 0.0,
            confidence: 0.0,
            level_db: -60.0,
        }
    }
    /// Whether this backend can carry out one control. A view hides a control that it cannot.
    ///
    /// The default is yes, because the visual scenarios show each control.
    fn supports(&self, _control: HostControl) -> bool {
        true
    }
    /// Returns the recent text of the diagnostic log, when this backend keeps one.
    ///
    /// A click calls this, never a frame, because it can read a file.
    fn diagnostic_log(&self) -> Option<String> {
        None
    }
    /// Returns standalone audio and MIDI state when a device manager is attached.
    fn device_settings(&self) -> Option<crate::default::SystemDeviceState> {
        None
    }
    /// Returns the level of each channel of the input device in dB, for the settings page.
    ///
    /// A frame of the settings page calls this, so an implementation must answer from state that
    /// it already holds. A backend with no such levels gives those of the two chain inputs.
    fn input_channel_levels(&self) -> Vec<f32> {
        self.meter_levels().input_db.to_vec()
    }
    /// Asks for a new look at the devices while the settings page is visible.
    ///
    /// A frame calls this, so an implementation must not wait for a device.
    fn refresh_devices(&self) {}
    /// Returns the current audio chain faults, worst first, or none without an audio engine.
    ///
    /// The shell calls this on each pass of its logic, so an implementation must answer from
    /// state that it already holds.
    fn chain_faults(&self) -> Vec<ChainFault> {
        Vec::new()
    }
    /// Returns shared MIDI mapping and learn state.
    fn midi_map(&self) -> crate::default::MidiMapSnapshot {
        crate::default::MidiMapSnapshot {
            mappings: self.chain_state(0).session.midi_mappings.clone(),
            ..Default::default()
        }
    }
    /// Returns the UI-only calibration draft.
    fn calibration_draft(&self) -> Option<String> {
        None
    }
    /// Reads one Boolean UI preference, or `None` when it has no explicit value.
    fn preference(&self, _key: &str) -> Option<bool> {
        None
    }
    /// Reads one text UI preference, or `None` when it has no explicit value.
    fn text_preference(&self, _key: &str) -> Option<String> {
        None
    }
    /// Executes one typed action.
    ///
    /// # Errors
    /// Returns a typed target, state, or storage error.
    fn dispatch(&self, action: UiAction) -> Result<UiReceipt, UiError>;
    /// Compiles untrusted model bytes without any change, and reports whether they can load.
    ///
    /// A compile takes time, so only a background thread calls this. An implementation must not
    /// hold a lock that a frame needs.
    ///
    /// # Errors
    /// Returns an error for bytes that cannot become a model.
    fn validate_model(
        &self,
        _tone: &Tone,
        _model_id: ModelId,
        _bytes: &ModelBytes,
    ) -> Result<(), UiError> {
        Ok(())
    }
    /// Brings the running audio chain up to date after [`UiAction::LoadToneWithBytes`] or
    /// [`UiAction::SelectModelWithBytes`].
    ///
    /// A compile takes time, so only a background thread calls this. An implementation must not
    /// hold a lock that a frame needs while it compiles.
    ///
    /// # Errors
    /// Returns an error when the control state is unavailable.
    fn settle_audio(&self) -> Result<(), UiError> {
        Ok(())
    }
    /// Whether the block clipboard currently contains a pasteable tone.
    fn can_paste_block(&self) -> bool {
        false
    }
    /// Reads one catalog parameter or returns `None` for an unknown ID.
    fn parameter(&self, id: ParameterId) -> Option<ParameterValue>;
    /// Writes one catalog parameter with gesture metadata.
    ///
    /// # Errors
    /// Returns an unknown-parameter, range, or backend error.
    fn set_parameter(
        &self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<(), UiError>;
}

/// Declarative scenario interactions used by the neutral testbed.
#[derive(Debug, Clone, PartialEq)]
pub enum UiDrive {
    /// Wait for asynchronous work.
    Wait {
        /// Duration in milliseconds.
        ms: u64,
    },
    /// Click a named control after scrolling it into view.
    Click {
        /// Control or card name.
        target: String,
    },
    /// Replace a field's text.
    Fill {
        /// Field name.
        target: String,
        /// New text.
        text: String,
    },
    /// Replace and submit text.
    Submit {
        /// Field name.
        target: String,
        /// New text.
        text: String,
    },
    /// Scroll a target to a logical position.
    Scroll {
        /// Target or region name.
        target: String,
        /// Logical position.
        position: f32,
    },
    /// Open plugin settings.
    OpenSettings,
    /// Open system/audio settings.
    OpenSystemSettings,
    /// Scroll settings to a heading.
    ScrollSettings {
        /// Heading text.
        heading: String,
    },
    /// Remove pointer hover.
    Unhover,
    /// Hover a target after scrolling it into view.
    Hover {
        /// Target name.
        target: String,
    },
    /// Clear a filter chip.
    ClearFilter {
        /// Chip name.
        target: String,
    },
}

/// Mock event log entry used to prove effect and interaction ordering.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// A target was scrolled into view before interaction.
    Scrolled {
        /// Target name.
        target: String,
        /// Logical position.
        position: f32,
    },
    /// An effect was accepted.
    Effect(UiEffect),
}

/// The MIDI channel, learn target, and mappings of a fixture. The editor applies the limits.
fn midi_map_from_ui(value: &serde_json::Value) -> Option<(u8, String, Vec<MidiMapping>)> {
    let object = value.as_object()?;
    let mappings = object
        .get("mappings")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|mapping| {
                    Some(MidiMapping {
                        target_id: mapping["targetId"].as_str()?.to_owned(),
                        source: MidiSource::from_persisted(
                            mapping["source"].as_str().unwrap_or("cc"),
                        ),
                        number: u8::try_from(mapping["number"].as_u64()?).ok()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let channel = object
        .get("channel")
        .and_then(serde_json::Value::as_u64)
        .and_then(|channel| u8::try_from(channel).ok())
        .unwrap_or(0);
    let learn_target = object
        .get("learnTargetId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some((channel, learn_target, mappings))
}

/// The storage stubs of the mock. It has no preset store and no model byte cache, so each
/// preset request succeeds and each set of bytes is kept nowhere.
///
/// It keeps the order of the presets that it saved, so that a move of one of them at an edge
/// changes nothing, as in a real store. The preset list of a fixture is in the UI, so a move of a
/// preset that the mock did not save always moves.
#[derive(Debug)]
struct MockStorage {
    stash: Option<LocalStash>,
    saved_presets: Vec<String>,
}

impl SessionStorage for MockStorage {
    fn keep_model_bytes(
        &mut self,
        _block_id: &BlockId,
        _model_id: ModelId,
        _bytes: ModelBytes,
    ) -> Result<(), UiError> {
        Ok(())
    }

    fn share_model_bytes(&mut self, _source: &ChainBlock, _copy: &BlockId) {}

    fn load_local(&mut self, path: &std::path::Path) -> Result<Tone, UiError> {
        let stash = self
            .stash
            .as_ref()
            .ok_or_else(|| UiError::Control("local stash is unavailable".to_owned()))?;
        ronie_core::load_local_path(path, stash, &ronie_dsp::validate_a2_document)
            .map(|loaded| loaded.tone)
            .map_err(|error| UiError::Control(error.to_string()))
    }

    fn save_preset(
        &mut self,
        proposed: PresetInfo,
        _current: &SessionFrame,
    ) -> Result<PresetInfo, UiError> {
        if !self.saved_presets.contains(&proposed.id) {
            self.saved_presets.push(proposed.id.clone());
        }
        Ok(proposed)
    }

    fn load_preset(
        &mut self,
        id: &str,
        current: &SessionFrame,
        _preparation: PrepareSpec,
    ) -> Result<(PresetInfo, SessionFrame), UiError> {
        let info = PresetInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            factory: false,
        };
        Ok((info, current.clone()))
    }

    fn rename_preset(&mut self, _id: &str, _name: &str) -> Result<(), UiError> {
        Ok(())
    }

    fn delete_preset(&mut self, id: &str) -> Result<(), UiError> {
        self.saved_presets.retain(|known| known != id);
        Ok(())
    }

    fn move_preset(&mut self, id: &str, delta: i32) -> Result<bool, UiError> {
        Ok(move_in_order(&mut self.saved_presets, id, delta).unwrap_or(true))
    }
}

#[derive(Debug)]
struct MockState {
    editor: SessionEditor,
    storage: MockStorage,
    tuner: ronie_dsp::analysis::TunerReading,
    events: Vec<UiEvent>,
    actions: Vec<UiAction>,
    effect_override: Option<UiEffect>,
    device_settings: Option<crate::default::SystemDeviceState>,
    chain_faults: Vec<ChainFault>,
    /// Whether a tone load is refused, as a backend refuses a model it cannot read.
    refuse_tone_loads: bool,
}

impl MockState {
    /// Carries out one action in the editor, and then in the device stub.
    fn dispatch(&mut self, action: UiAction) -> Result<UiReceipt, UiError> {
        self.actions.push(action.clone());
        if self.refuse_tone_loads
            && matches!(
                action,
                UiAction::LoadTone { .. } | UiAction::ReplaceTone { bytes: None, .. }
            )
        {
            return Err(UiError::Control("the model file is missing".to_owned()));
        }
        let device = match &action {
            UiAction::Device(device) => Some(device.clone()),
            _ => None,
        };
        let receipt = self.editor.apply(action, &mut self.storage)?;
        if let (Some(action), Some(device)) = (device, &mut self.device_settings) {
            configure_device_stub(device, action);
        }
        let effect = self.effect_override.unwrap_or(receipt.effect);
        self.events.push(UiEvent::Effect(effect));
        Ok(UiReceipt {
            revision: receipt.revision,
            effect,
        })
    }
}

/// Shows a device action in the standalone device snapshot of a fixture.
fn configure_device_stub(device: &mut crate::default::SystemDeviceState, action: DeviceAction) {
    match action {
        DeviceAction::Configure(config) => device.apply_config(&config),
        DeviceAction::SelectBackend(backend) => device.banner.current_type = backend,
        DeviceAction::SetHearYourself(enabled) => device.banner.hear_yourself = enabled,
        DeviceAction::SetInputChannels(channels) => {
            for channel in &mut device.input_channels {
                channel.active = channels.contains(&channel.index);
            }
        }
        DeviceAction::SetOutputPair(pair) => device.active_output_pair = pair,
        DeviceAction::SetMidiInputEnabled { id, enabled } => {
            if let Some(input) = device.midi_inputs.iter_mut().find(|input| input.id == id) {
                input.enabled = enabled;
            }
        }
        DeviceAction::Restart => device.banner.device_open = true,
        DeviceAction::CycleInputMode
        | DeviceAction::PlayTestTone
        | DeviceAction::OpenControlPanel
        | DeviceAction::OpenBluetoothMidi => {}
    }
}

fn real_parameter(id: &'static str, normalized: f32) -> ParameterValue {
    ParameterValue(
        ParameterCatalog::v1()
            .iter()
            .find(|spec| spec.id == ParameterId(id))
            .map_or(normalized, |spec| spec.value_at(normalized)),
    )
}

/// The parameter values of the sliders, toggles, and combo boxes of a fixture.
fn fixture_parameters(fixture: &serde_json::Value) -> BTreeMap<ParameterId, ParameterValue> {
    let mut parameters = BTreeMap::new();
    for (id, default) in [
        ("inputLevel", 0.5),
        ("outputLevel", 0.5),
        ("outputBalance", 0.5),
        ("spreadOffset", 0.62),
        ("spreadWobble", 0.25),
        ("spreadCrossover", 0.5),
        ("alignOffset", 0.5),
        ("alignWobble", 0.25),
        ("alignCrossover", 0.5),
        ("chainPanLeft", 0.0),
        ("chainPanRight", 1.0),
        ("toneBass", 0.5),
        ("toneMid", 0.55),
        ("toneTreble", 0.45),
        ("gateThreshold", 0.35),
        ("inputCalibrationLevel", 0.5),
        ("gateRelease", 0.5),
        ("gateHold", 0.1),
        ("gateRange", 1.0),
        ("pitchSemitones", 0.5),
        ("pitchTonality", 1.0),
        ("pitchWindow", 1.0 / 3.0),
    ] {
        let normalized = fixture["sliders"][id].as_f64().unwrap_or(default) as f32;
        parameters.insert(ParameterId(id), real_parameter(id, normalized));
    }
    for (id, default) in [
        ("spreadEnabled", false),
        ("spreadWobbleEnabled", true),
        ("spreadCrossoverEnabled", true),
        ("spreadDiffuseEnabled", true),
        ("alignEnabled", false),
        ("alignWobbleEnabled", false),
        ("alignCrossoverEnabled", false),
        ("alignDiffuseEnabled", false),
        ("chainPanLinked", true),
        ("chainSoloLeft", false),
        ("chainSoloRight", false),
        ("chainInvertLeft", false),
        ("chainInvertRight", false),
        ("gateEnabled", true),
        ("toneEqEnabled", true),
        ("pitchEnabled", false),
        ("pitchStep", true),
        ("calibrateInput", false),
        ("osEnabled", false),
    ] {
        let value = &fixture["toggles"][id];
        let enabled = value
            .as_bool()
            .or_else(|| value.as_f64().map(|value| value != 0.0))
            .unwrap_or(default);
        parameters.insert(ParameterId(id), ParameterValue(f32::from(enabled)));
    }
    parameters.insert(
        ParameterId("osFactor"),
        ParameterValue(fixture["comboBoxes"]["osFactor"].as_f64().unwrap_or(0.0) as f32),
    );
    parameters
}

fn fixture_meters(scenario: &crate::testbed::UiScenario) -> MeterSnapshot {
    let value = &scenario.state["meters"];
    let channels = |key: &str, default: [f32; 2]| {
        let values = value[key].as_array();
        [
            values
                .and_then(|values| values.first())
                .and_then(serde_json::Value::as_f64)
                .map_or(default[0], |value| value as f32),
            values
                .and_then(|values| values.get(1))
                .and_then(serde_json::Value::as_f64)
                .map_or(default[1], |value| value as f32),
        ]
    };
    let mut blocks = value["blocks"]
        .as_object()
        .map(|objects| {
            objects
                .iter()
                .map(|(id, levels)| BlockMeter {
                    block_id: BlockId(id.clone()),
                    input_db: levels["in"].as_f64().unwrap_or(-60.0) as f32,
                    output_db: levels["out"].as_f64().unwrap_or(-60.0) as f32,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !value.is_object()
        && let Ok(session) = SessionSnapshot::from_ui_value(scenario.state["chain"].clone())
    {
        for block in session.chain.left.iter().chain(&session.chain.right) {
            if !block.is_insert {
                blocks.push(BlockMeter {
                    block_id: block.id.clone(),
                    input_db: -22.0,
                    output_db: -16.0,
                });
            }
        }
    }
    MeterSnapshot {
        input_db: channels("input", [-18.0, -21.0]),
        output_db: channels("output", [-12.0, -14.0]),
        blocks,
        cpu: value["cpu"].as_f64().unwrap_or(0.11) as f32,
        correlation: value["correlation"].as_f64().unwrap_or(0.85) as f32,
    }
}

/// Deterministic backend implementing the complete scenario surface.
#[derive(Debug, Clone)]
pub struct MockBackend {
    state: Arc<Mutex<MockState>>,
    meters: MeterSnapshot,
}

impl MockBackend {
    /// Builds a mock from neutral scenario state.
    #[must_use]
    pub fn from_scenario(scenario: &crate::testbed::UiScenario) -> Self {
        let fixture = &scenario.state;
        let mut session =
            SessionSnapshot::from_ui_value(fixture["chain"].clone()).unwrap_or_else(|_| {
                SessionSnapshot {
                    chain: ChainSnapshot::empty(),
                    input_mode: InputMode::Stereo,
                    editor_scale: 1.0,
                    editor_extra_height: 36,
                    midi_mappings: Vec::new(),
                    active_preset: None,
                }
            });
        session.chain.revision = fixture["chain"]["revision"].as_u64().unwrap_or(0);
        // A restore marks a second IR whose bytes it did not find. A session never stores the
        // mark, so a scenario names it beside the block.
        for block in fixture["chain"]["chain"].as_array().into_iter().flatten() {
            if block["irBMissing"].as_bool() == Some(true)
                && let Some(id) = block["blockId"].as_str()
                && let Some(block) = session.chain.block_mut(&BlockId(id.to_owned()))
            {
                block.ir_b_missing = block.ir_b.is_some();
            }
        }
        let preparation = PrepareSpec {
            host_sample_rate: fixture["chain"]["sampleRate"].as_f64().unwrap_or(48_000.0),
            max_host_frames: 64,
            oversampling: 1,
            input_mode: session.input_mode,
            stereo_input: session.chain.stereo_input,
            stereo_output: session.chain.stereo_output,
        };
        let (midi_channel, midi_learn_target) = match midi_map_from_ui(&fixture["midiMap"]) {
            Some((channel, learn_target, mappings)) => {
                session.midi_mappings = mappings;
                (channel, learn_target)
            }
            None => (0, String::new()),
        };
        let mut editor = SessionEditor::seeded(SessionSeed {
            session,
            preparation,
            parameters: fixture_parameters(fixture),
            midi_channel,
            midi_learn_target,
            can_undo: fixture["chain"]["canUndo"].as_bool().unwrap_or(false),
            can_redo: fixture["chain"]["canRedo"].as_bool().unwrap_or(false),
        });
        // The text and Boolean preferences that the machine settings of a real backend would hold.
        for (key, value) in fixture["textPreferences"].as_object().into_iter().flatten() {
            if let Some(value) = value.as_str() {
                editor.seed_text_preference(key.clone(), value);
            }
        }
        for (key, value) in fixture["preferences"].as_object().into_iter().flatten() {
            if let Some(value) = value.as_bool() {
                editor.seed_preference(key.clone(), value);
            }
        }
        Self {
            state: Arc::new(Mutex::new(MockState {
                editor,
                storage: MockStorage {
                    stash: LocalStash::new(std::env::temp_dir().join("ronie-mock-stash")).ok(),
                    saved_presets: Vec::new(),
                },
                tuner: ronie_dsp::analysis::TunerReading {
                    frequency_hz: fixture["tuner"]["frequency"].as_f64().unwrap_or(0.0),
                    confidence: fixture["tuner"]["confidence"].as_f64().unwrap_or(0.0),
                    level_db: fixture["tuner"]["level"].as_f64().unwrap_or(-60.0),
                },
                events: Vec::new(),
                actions: Vec::new(),
                effect_override: None,
                device_settings: crate::default::SystemDeviceState::from_ui_value(
                    &fixture["device"],
                ),
                chain_faults: Vec::new(),
                refuse_tone_loads: false,
            })),
            meters: fixture_meters(scenario),
        }
    }

    /// Forces every accepted action to report a different effect, for negative tests.
    pub fn set_receipt_effect_override(&mut self, effect: UiEffect) {
        if let Ok(mut state) = self.state.lock() {
            state.effect_override = Some(effect);
        }
    }

    /// Returns the ordered effect and scroll log.
    #[must_use]
    pub fn events(&self) -> Vec<UiEvent> {
        self.state
            .lock()
            .map_or_else(|_| Vec::new(), |state| state.events.clone())
    }

    /// Returns the typed action log in dispatch order.
    #[must_use]
    pub fn actions(&self) -> Vec<UiAction> {
        self.state
            .lock()
            .map_or_else(|_| Vec::new(), |state| state.actions.clone())
    }

    /// Records a scroll of a scenario, which the UI owns and the mock only logs.
    pub fn record_scroll(&self, target: &str, position: f32) {
        if let Ok(mut state) = self.state.lock() {
            state.events.push(UiEvent::Scrolled {
                target: target.to_owned(),
                position,
            });
        }
    }

    /// Replaces the standalone device snapshot used by settings and banner tests.
    pub fn set_device_settings(&self, device: Option<crate::default::SystemDeviceState>) {
        if let Ok(mut state) = self.state.lock() {
            state.device_settings = device;
        }
    }

    /// Makes each later tone load fail, or succeed again, for tests of a refused load.
    pub fn refuse_tone_loads(&self, refuse: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.refuse_tone_loads = refuse;
        }
    }

    /// Replaces the audio chain faults used by banner tests.
    pub fn set_chain_faults(&self, faults: Vec<ChainFault>) {
        if let Ok(mut state) = self.state.lock() {
            state.chain_faults = faults;
        }
    }

    /// Reads a Boolean UI preference from the deterministic store.
    #[must_use]
    pub fn preference(&self, key: &str) -> Option<bool> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.editor.preference(key))
    }

    /// Reads a text UI preference from the deterministic store.
    #[must_use]
    pub fn text_preference(&self, key: &str) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.editor.text_preference(key).map(str::to_owned))
    }

    fn lock(&self) -> Result<MutexGuard<'_, MockState>, UiError> {
        self.state
            .lock()
            .map_err(|error| UiError::Control(error.to_string()))
    }
}

impl UiBackend for MockBackend {
    fn chain_state(&self, known_revision: u64) -> ChainViewState {
        match self.state.lock() {
            Ok(state) => state.editor.view(known_revision),
            Err(_) => SessionEditor::new(PrepareSpec {
                host_sample_rate: 48_000.0,
                max_host_frames: 64,
                oversampling: 1,
                input_mode: InputMode::Stereo,
                stereo_input: false,
                stereo_output: true,
            })
            .view(known_revision),
        }
    }

    fn meter_levels(&self) -> MeterSnapshot {
        self.meters.clone()
    }

    fn spectrum_bins(&self, _block_id: &BlockId) -> [f32; 64] {
        std::array::from_fn(|index| {
            let x = index as f32 / 63.0;
            let hump = -18.0 - 55.0 * (x - 0.28).powi(2) * 4.0;
            let wiggle = 4.0 * (index as f32 * 1.7).sin() * (-x * 1.5).exp();
            (hump + wiggle).clamp(-100.0, -8.0)
        })
    }

    fn device_settings(&self) -> Option<crate::default::SystemDeviceState> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.device_settings.clone())
    }

    fn chain_faults(&self) -> Vec<ChainFault> {
        self.state
            .lock()
            .map(|state| state.chain_faults.clone())
            .unwrap_or_default()
    }

    fn midi_map(&self) -> crate::default::MidiMapSnapshot {
        self.state.lock().map_or_else(
            |_| crate::default::MidiMapSnapshot::default(),
            |state| state.editor.midi_map(),
        )
    }

    fn calibration_draft(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.editor.calibration_draft())
    }

    fn preference(&self, key: &str) -> Option<bool> {
        MockBackend::preference(self, key)
    }

    fn text_preference(&self, key: &str) -> Option<String> {
        MockBackend::text_preference(self, key)
    }

    fn dispatch(&self, action: UiAction) -> Result<UiReceipt, UiError> {
        self.lock()?.dispatch(action)
    }

    fn tuner_reading(&self) -> ronie_dsp::analysis::TunerReading {
        self.state.lock().map_or(
            ronie_dsp::analysis::TunerReading {
                frequency_hz: 0.0,
                confidence: 0.0,
                level_db: -60.0,
            },
            |state| state.tuner,
        )
    }

    fn can_paste_block(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.editor.can_paste())
    }

    fn parameter(&self, id: ParameterId) -> Option<ParameterValue> {
        self.state.lock().ok()?.editor.parameter(id)
    }

    fn set_parameter(
        &self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<(), UiError> {
        self.lock()?
            .dispatch(UiAction::SetParameter { id, value, gesture })
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock() -> MockBackend {
        MockBackend::from_scenario(&crate::testbed::UiScenario {
            id: "mock".to_owned(),
            section: crate::testbed::UiSection::Main,
            state: serde_json::json!({}),
            drives: Vec::new(),
            viewport: Default::default(),
        })
    }

    #[test]
    fn a_direct_parameter_write_is_logged_as_an_action() {
        let mut backend = mock();
        backend.set_receipt_effect_override(UiEffect::None);
        backend
            .set_parameter(
                ParameterId("outputLevel"),
                ParameterValue(0.25),
                GesturePhase::End,
            )
            .expect("write");
        assert_eq!(
            backend.actions(),
            vec![UiAction::SetParameter {
                id: ParameterId("outputLevel"),
                value: ParameterValue(0.25),
                gesture: GesturePhase::End,
            }]
        );
        assert_eq!(backend.events(), vec![UiEvent::Effect(UiEffect::None)]);
    }

    #[test]
    fn a_fixture_slider_uses_the_mapping_of_its_parameter() {
        assert_eq!(real_parameter("gateRelease", 0.0), ParameterValue(5.0));
        assert_eq!(
            real_parameter("spreadCrossover", 0.25),
            ParameterValue(0.25)
        );
        assert_eq!(real_parameter("toneBass", 0.5), ParameterValue(5.0));
    }
}

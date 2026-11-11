//! The session edit rules that each backend shares.
//!
//! [`SessionEditor`] owns the session snapshot, the parameters, the undo and redo history, the
//! open knob gesture, the block clipboard, and the MIDI learn state. Each backend sends its
//! actions here, so that the real processor and the mock follow one copy of the rules. A backend
//! keeps only its own input and output, and lends its storage through [`SessionStorage`].

use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    path::Path,
    sync::{Arc, OnceLock},
};

use ronie_core::{
    ActivePreset, BlockId, BlockParams, ChainBlock, ChainBranch, ChainSide, ChainSnapshot,
    EqParams, InputMode, MidiMapping, ModelBytes, ModelId, ModelSource, PresetInfo,
    SessionSnapshot, Tone,
    params::{ParameterCatalog, ParameterId, ParameterValue},
};
use ronie_dsp::PrepareSpec;

use crate::{
    backend::{
        AudioDeviceConfig, ChainViewState, DeviceAction, GesturePhase, MidiAction, UiAction,
        UiEffect, UiError, UiReceipt,
    },
    default::MidiMapSnapshot,
};

/// Everything that one undo step restores.
#[derive(Debug, Clone)]
pub struct SessionFrame {
    /// The chain and the session-only state.
    pub session: SessionSnapshot,
    /// The value of each catalog parameter.
    pub parameters: BTreeMap<ParameterId, ParameterValue>,
    /// Whether multicore processing is enabled.
    pub multi_core: bool,
    /// Whether full-size models are selected for new blocks.
    pub nam_full_size: bool,
}

/// What a session edit needs from storage outside the session.
///
/// The editor calls a method only after each check of the action passes, and it changes the
/// session only after the method succeeds. So a refused action changes neither.
pub trait SessionStorage {
    /// Keeps the downloaded bytes of one catalog model of a block.
    ///
    /// # Errors
    /// Returns an error when the bytes cannot be kept.
    fn keep_model_bytes(
        &mut self,
        block_id: &BlockId,
        model_id: ModelId,
        bytes: ModelBytes,
    ) -> Result<(), UiError>;

    /// Gives the new copy of a block the model bytes of its source.
    fn share_model_bytes(&mut self, source: &ChainBlock, copy: &BlockId);

    /// Reads and stashes a local file or folder as a tone.
    ///
    /// # Errors
    /// Returns an error when the path holds no usable tone.
    fn load_local(&mut self, path: &Path) -> Result<Tone, UiError>;

    /// Saves `current` as a user preset, and returns the preset identity.
    ///
    /// `proposed` holds the trimmed name, and the identity for a backend with no preset store.
    ///
    /// # Errors
    /// Returns an encoding or storage error.
    fn save_preset(
        &mut self,
        proposed: PresetInfo,
        current: &SessionFrame,
    ) -> Result<PresetInfo, UiError>;

    /// Reads one preset, and returns its identity and the session that it restores over
    /// `current`. A success commits the restore, because the editor then always accepts it.
    ///
    /// # Errors
    /// Returns an unknown-target error, or a decoding, preparation, or storage error.
    fn load_preset(
        &mut self,
        id: &str,
        current: &SessionFrame,
        preparation: PrepareSpec,
    ) -> Result<(PresetInfo, SessionFrame), UiError>;

    /// Renames one user preset to the trimmed `name`.
    ///
    /// # Errors
    /// Returns an unknown-target or storage error.
    fn rename_preset(&mut self, id: &str, name: &str) -> Result<(), UiError>;

    /// Deletes one user preset.
    ///
    /// # Errors
    /// Returns an unknown-target or storage error.
    fn delete_preset(&mut self, id: &str) -> Result<(), UiError>;

    /// Moves one preset by `delta` places, and returns false when its place does not change.
    ///
    /// # Errors
    /// Returns an unknown-target or storage error.
    fn move_preset(&mut self, id: &str, delta: i32) -> Result<bool, UiError>;
}

/// The state that a backend starts from instead of an empty session, such as a scenario fixture.
#[derive(Debug, Clone)]
pub struct SessionSeed {
    /// The session, used as it is. The chain is not normalized.
    pub session: SessionSnapshot,
    /// The preparation contract.
    pub preparation: PrepareSpec,
    /// Values that replace catalog defaults. An ID outside the catalog is ignored.
    pub parameters: BTreeMap<ParameterId, ParameterValue>,
    /// The MIDI channel filter. Zero is omni.
    pub midi_channel: u8,
    /// The target that is armed for learning, or empty.
    pub midi_learn_target: String,
    /// Whether the history starts with one undo step.
    pub can_undo: bool,
    /// Whether the history starts with one redo step.
    pub can_redo: bool,
}

/// The most undo steps that the history keeps. A new step above the limit drops the oldest one.
///
/// The app stays open for hours, and each step holds a copy of the session.
pub const MAX_UNDO_STEPS: usize = 200;

/// One open knob or EQ gesture. Everything that it changes is one undo step.
#[derive(Debug, Default)]
struct OpenGesture {
    /// The value of each parameter from before its first write in this gesture.
    parameters: Vec<(ParameterId, ParameterValue)>,
    /// The settings of each block from before its first write in this gesture.
    blocks: Vec<(BlockId, BlockParams)>,
    /// Whether the last undo step holds the state from before this gesture.
    step_taken: bool,
    /// The index of each undo step that this gesture took. Another action between two writes
    /// of the gesture makes the gesture take one more step.
    steps: Vec<usize>,
    /// Whether the history dropped the first step of this gesture, because it was the oldest
    /// step at the limit. Each step that the history still holds is then newer than the start
    /// of this gesture.
    first_step_dropped: bool,
}

impl OpenGesture {
    /// Puts each value from before this gesture back into `frame`. A block that `frame` does not
    /// hold is skipped.
    fn restore_into(&self, frame: &mut SessionFrame) {
        frame.parameters.extend(self.parameters.iter().copied());
        for (id, params) in &self.blocks {
            if let Some(block) = frame.session.chain.block_mut(id) {
                block.params = params.clone();
            }
        }
    }

    /// Whether this gesture changed a block that `chain` does not hold as a tone now.
    fn lost_a_block(&self, chain: &ChainSnapshot) -> bool {
        self.blocks
            .iter()
            .any(|(id, _)| chain.block(id).is_none_or(|block| block.is_insert))
    }
}

/// The current session, and the shared snapshots that each view of it holds.
///
/// Each mutable access clears the snapshots, and the next view builds them again. So a view
/// never shows an old session, and each view of an unchanged session shares one copy of it.
#[derive(Debug)]
struct CurrentFrame {
    frame: SessionFrame,
    shared: OnceLock<SharedSession>,
}

/// The snapshots that each view of one unchanged session shares.
#[derive(Debug)]
struct SharedSession {
    chain: Arc<ChainSnapshot>,
    session: Arc<SessionSnapshot>,
}

impl CurrentFrame {
    fn new(frame: SessionFrame) -> Self {
        Self {
            frame,
            shared: OnceLock::new(),
        }
    }

    /// The shared snapshots of the session. Only the first call after a change copies it.
    fn shared(&self) -> &SharedSession {
        self.shared.get_or_init(|| SharedSession {
            chain: Arc::new(self.frame.session.chain.clone()),
            session: Arc::new(self.frame.session.clone()),
        })
    }
}

impl Deref for CurrentFrame {
    type Target = SessionFrame;

    fn deref(&self) -> &SessionFrame {
        &self.frame
    }
}

impl DerefMut for CurrentFrame {
    /// Clears the shared snapshots, because the caller can change the session.
    fn deref_mut(&mut self) -> &mut SessionFrame {
        self.shared.take();
        &mut self.frame
    }
}

/// The session state of one backend and the rules that change it.
///
/// Each accepted action raises the revision, and a refused action changes nothing. The one
/// exception is a preset move that does not change the place of the preset, such as a move at the
/// edge of its section: it changes nothing, is accepted, and has no effect.
#[derive(Debug)]
pub struct SessionEditor {
    current: CurrentFrame,
    preparation: PrepareSpec,
    undo: Vec<SessionFrame>,
    redo: Vec<SessionFrame>,
    open_gesture: Option<OpenGesture>,
    clipboard: Option<ChainBlock>,
    midi_channel: u8,
    midi_learn_target: String,
    midi_draft: Option<String>,
    calibration_draft: Option<String>,
    preferences: BTreeMap<String, bool>,
    text_preferences: BTreeMap<String, String>,
}

impl SessionEditor {
    /// Starts an empty session with each catalog default, prepared for `preparation`.
    #[must_use]
    pub fn new(preparation: PrepareSpec) -> Self {
        Self {
            current: CurrentFrame::new(SessionFrame {
                session: empty_session(preparation.input_mode, preparation),
                parameters: catalog_defaults(),
                multi_core: true,
                nam_full_size: false,
            }),
            preparation,
            undo: Vec::new(),
            redo: Vec::new(),
            open_gesture: None,
            clipboard: None,
            midi_channel: 0,
            midi_learn_target: String::new(),
            midi_draft: None,
            calibration_draft: None,
            preferences: BTreeMap::new(),
            text_preferences: BTreeMap::new(),
        }
    }

    /// Starts from `seed` instead of an empty session.
    ///
    /// A seed has no earlier state, so its undo or redo step holds the seed session itself. A
    /// MIDI channel or mapping outside the limits is dropped, as an edit would refuse it.
    #[must_use]
    pub fn seeded(seed: SessionSeed) -> Self {
        let mut editor = Self::new(seed.preparation);
        editor.current.session = seed.session;
        editor
            .current
            .session
            .midi_mappings
            .retain(|mapping| mapping.number <= MAX_MIDI_NUMBER);
        for (id, value) in seed.parameters {
            if let Some(known) = editor.current.parameters.get_mut(&id) {
                *known = value;
            }
        }
        if seed.midi_channel <= MAX_MIDI_CHANNEL {
            editor.midi_channel = seed.midi_channel;
        }
        editor.midi_learn_target = seed.midi_learn_target;
        if seed.can_undo {
            editor.undo.push(editor.current.clone());
        }
        if seed.can_redo {
            editor.redo.push(editor.current.clone());
        }
        editor
    }

    /// The current session.
    #[must_use]
    pub fn current(&self) -> &SessionFrame {
        &self.current
    }

    /// The preparation contract that the next chain must meet.
    #[must_use]
    pub fn preparation(&self) -> PrepareSpec {
        self.preparation
    }

    /// An immutable view of the chain, and whether it differs from `known_revision`.
    ///
    /// Each view of an unchanged session shares one chain and one session snapshot, so only the
    /// first view after a change copies the chain. The other fields are read again each time.
    #[must_use]
    pub fn view(&self, known_revision: u64) -> ChainViewState {
        let shared = self.current.shared();
        let revision = shared.chain.revision;
        ChainViewState {
            revision,
            changed: revision != known_revision,
            chain: Arc::clone(&shared.chain),
            session: Arc::clone(&shared.session),
            preparation: self.preparation,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            multi_core: self.current.multi_core,
            nam_full_size: self.current.nam_full_size,
        }
    }

    /// Reads one parameter, or `None` for an ID outside the catalog.
    #[must_use]
    pub fn parameter(&self, id: ParameterId) -> Option<ParameterValue> {
        self.current.parameters.get(&id).copied()
    }

    /// Whether the block clipboard holds a tone.
    #[must_use]
    pub fn can_paste(&self) -> bool {
        self.clipboard.is_some()
    }

    /// The MIDI mappings and the learn state.
    #[must_use]
    pub fn midi_map(&self) -> MidiMapSnapshot {
        MidiMapSnapshot {
            channel: self.midi_channel,
            learn_target_id: self.midi_learn_target.clone(),
            mappings: self.current.session.midi_mappings.clone(),
            cc_draft: self.midi_draft.clone(),
        }
    }

    /// The UI-only calibration draft.
    #[must_use]
    pub fn calibration_draft(&self) -> Option<String> {
        self.calibration_draft.clone()
    }

    /// Reads one Boolean UI preference, or `None` when it has no value.
    #[must_use]
    pub fn preference(&self, key: &str) -> Option<bool> {
        self.preferences.get(key).copied()
    }

    /// Reads one text UI preference, or `None` when it has no value.
    #[must_use]
    pub fn text_preference(&self, key: &str) -> Option<&str> {
        self.text_preferences.get(key).map(String::as_str)
    }

    /// Sets one text UI preference of the start state. The revision does not change.
    ///
    /// A backend calls this for a stored value before the first frame, as a fixture does.
    pub fn seed_text_preference(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.text_preferences.insert(key.into(), value.into());
    }

    /// Sets one Boolean UI preference of the start state. The revision does not change.
    ///
    /// A backend calls this for a stored value before the first frame, as a fixture does.
    pub fn seed_preference(&mut self, key: impl Into<String>, value: bool) {
        self.preferences.insert(key.into(), value);
    }

    /// Records the final stream layout. The session revision does not change.
    pub fn set_stream_format(
        &mut self,
        sample_rate: f64,
        max_frames: usize,
        stereo_input: bool,
        stereo_output: bool,
    ) {
        self.preparation.host_sample_rate = sample_rate;
        self.preparation.max_host_frames = max_frames;
        self.preparation.stereo_input = stereo_input;
        self.preparation.stereo_output = stereo_output;
        self.current.session.chain.stereo_input = stereo_input;
        self.current.session.chain.stereo_output = stereo_output;
    }

    /// Records the oversampling of the next chain: 1, 2, 4, or 8. The session revision does not
    /// change.
    pub fn set_oversampling(&mut self, factor: u8) {
        self.preparation.oversampling = factor;
    }

    /// Replaces the session with a saved one. This is not an undo step.
    ///
    /// A loaded session starts with an empty history, because no earlier step belongs to it.
    pub fn load_saved_session(&mut self, frame: SessionFrame, midi_channel: u8) {
        self.restore(frame);
        self.clear_history();
        self.midi_channel = midi_channel;
        self.next_revision();
    }

    /// Removes each undo and redo step and each open gesture. The session does not change.
    ///
    /// A backend calls this after it sets its start state, such as the machine defaults.
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open_gesture = None;
    }

    /// Carries out one action, and returns the new revision and the effect.
    ///
    /// Each arm calls one method, which checks the action, changes the session, and raises the
    /// revision by its own rule.
    ///
    /// # Errors
    /// Returns an unknown-target, unknown-parameter, or invalid-action error, or an error of
    /// `storage`. A refused action changes nothing.
    pub fn apply(
        &mut self,
        action: UiAction,
        storage: &mut impl SessionStorage,
    ) -> Result<UiReceipt, UiError> {
        let effect = match action {
            UiAction::SetParameter { id, value, gesture } => {
                self.edit_parameter(id, value, gesture)
            }
            UiAction::SetStereoMode(enabled) => self.set_session_flag(|editor| {
                editor.current.session.chain.stereo_enabled = enabled;
            }),
            UiAction::SwapChains => self.swap_chains(),
            UiAction::SetInputMode(mode) => {
                self.set_session_flag(|editor| editor.set_input_mode(mode))
            }
            UiAction::SetMultiCore(enabled) => {
                self.set_session_flag(|editor| editor.current.multi_core = enabled)
            }
            UiAction::SetNamFullSize(enabled) => {
                self.set_session_flag(|editor| editor.current.nam_full_size = enabled)
            }
            UiAction::Undo => self.undo(),
            UiAction::Redo => self.redo(),
            UiAction::ResetToDefault => self.reset(),
            UiAction::SelectSide(side) => self.set_session_flag(|editor| {
                editor.current.session.chain.active_side = side;
            }),
            UiAction::SetBranch(branch) => self.set_branch(branch),
            UiAction::LoadTone {
                target_insert_id,
                tone,
                model_id,
            } => self.load_tone(&target_insert_id, tone, model_id),
            UiAction::LoadToneWithBytes {
                target_insert_id,
                tone,
                model_id,
                bytes,
            } => self.load_tone_with_bytes(&target_insert_id, tone, model_id, bytes, storage),
            UiAction::DropFile {
                target_insert_id,
                path,
            } => self.drop_file(&target_insert_id, &path, storage),
            UiAction::RemoveBlock(id) => self.remove_block(id),
            UiAction::MoveBlock {
                block_id,
                target_index,
            } => self.move_block(block_id, target_index),
            UiAction::MoveBlockToLane {
                block_id,
                target_side,
                target_index,
            } => self.move_block_to_lane(&block_id, target_side, target_index),
            UiAction::RetryModelLoad(block_id) => self.retry_model_load(&block_id),
            UiAction::SetBlockParamsGesture {
                block_id,
                params,
                gesture,
            } => self.edit_block_params_gesture(block_id, params, gesture),
            UiAction::SetBlockParams { block_id, params } => {
                self.set_block_params(&block_id, params)
            }
            UiAction::SetEq {
                block_id,
                params,
                gesture,
            } => self.edit_eq(block_id, params, gesture),
            UiAction::SelectModel { block_id, model_id } => self.select_model(&block_id, model_id),
            UiAction::SelectModelWithBytes {
                block_id,
                tone,
                model_id,
                replaces,
                bytes,
            } => self.select_model_with_bytes(&block_id, tone, model_id, replaces, bytes, storage),
            UiAction::DuplicateBlock(id) => self.duplicate_block(&id, storage),
            UiAction::DuplicateBlockToLane {
                block_id,
                target_side,
                target_index,
            } => self.duplicate_block_to_lane(&block_id, target_side, target_index, storage),
            UiAction::CopyBlock(id) => self.copy_block(&id),
            UiAction::PasteBlock { side, index } => self.paste_block(side, index, storage),
            UiAction::Device(action) => self.device(action),
            UiAction::Midi(action) => self.apply_midi(action),
            UiAction::SavePreset { name } => self.save_preset(&name, storage),
            UiAction::LoadPreset { id } => self.load_preset(&id, storage),
            UiAction::RenamePreset { id, name } => self.rename_preset(&id, &name, storage),
            UiAction::DeletePreset { id } => self.delete_preset(&id, storage),
            UiAction::MovePreset { id, delta } => self.move_preset(&id, delta, storage),
            UiAction::Navigate(_) => self.ui_value(UiEffect::Navigation, |_| Ok(())),
            UiAction::PersistMachineDefault(id) => {
                self.ui_value(UiEffect::ParameterChanged, |_| check_machine_default(id))
            }
            UiAction::SetPreference { key, value } => {
                self.ui_value(UiEffect::Navigation, |editor| {
                    editor.preferences.insert(key, value);
                    Ok(())
                })
            }
            UiAction::SetTextPreference { key, value } => {
                self.ui_value(UiEffect::Navigation, |editor| {
                    editor.text_preferences.insert(key, value);
                    Ok(())
                })
            }
            UiAction::SetCalibrationDraft(draft) => self.ui_value(UiEffect::Navigation, |editor| {
                editor.calibration_draft = draft;
                Ok(())
            }),
        }?;
        self.close_gesture_of_lost_block();
        Ok(self.receipt(effect))
    }

    /// Closes the open gesture when a block that it changed is gone, and keeps its undo step.
    ///
    /// A later write of the gesture cannot reach that block, so the gesture must not take the
    /// writes of other targets into its undo step. Each action that removes a block, such as a
    /// remove or the normalize of a chain, comes through [`Self::apply`], so the open gesture
    /// never names a lost block.
    fn close_gesture_of_lost_block(&mut self) {
        let chain = &self.current.session.chain;
        if self
            .open_gesture
            .as_ref()
            .is_some_and(|open| open.lost_a_block(chain))
        {
            self.open_gesture = None;
        }
    }

    /// Changes one session value as one undo step.
    fn set_session_flag(&mut self, change: impl FnOnce(&mut Self)) -> Result<UiEffect, UiError> {
        self.push_history();
        change(self);
        Ok(self.chain_changed())
    }

    /// Changes one value that is not session state, and that no undo step holds.
    fn ui_value(
        &mut self,
        effect: UiEffect,
        change: impl FnOnce(&mut Self) -> Result<(), UiError>,
    ) -> Result<UiEffect, UiError> {
        change(self)?;
        self.next_revision();
        Ok(effect)
    }

    fn undo(&mut self) -> Result<UiEffect, UiError> {
        let previous = self
            .undo
            .pop()
            .ok_or(UiError::InvalidAction("nothing to undo".to_owned()))?;
        self.redo.push(self.current.clone());
        self.restore(previous);
        Ok(self.chain_changed())
    }

    fn redo(&mut self) -> Result<UiEffect, UiError> {
        let next = self
            .redo
            .pop()
            .ok_or(UiError::InvalidAction("nothing to redo".to_owned()))?;
        self.undo.push(self.current.clone());
        self.drop_oldest_steps();
        self.restore(next);
        Ok(self.chain_changed())
    }

    fn reset(&mut self) -> Result<UiEffect, UiError> {
        self.push_history();
        // An open gesture holds values from before the reset, so a later cancel must not find it.
        self.open_gesture = None;
        let revision = self.revision() + 1;
        self.current.session = empty_session(InputMode::Stereo, self.preparation);
        self.current.session.chain.revision = revision;
        self.preparation.input_mode = InputMode::Stereo;
        self.current.parameters = catalog_defaults();
        // A reset keeps the default chain exactly, so it skips the normalize of a change.
        Ok(UiEffect::ChainChanged)
    }

    fn set_branch(&mut self, branch: Option<ChainBranch>) -> Result<UiEffect, UiError> {
        if let Some(candidate) = &branch {
            let chain = &self.current.session.chain;
            if !chain.stereo_enabled {
                return Err(UiError::InvalidAction(
                    "branching requires stereo mode".to_owned(),
                ));
            }
            if !chain.has_tap(candidate) {
                return Err(UiError::UnknownTarget(candidate.after_block_id.0.clone()));
            }
        }
        self.push_history();
        self.current.session.chain.branch = branch;
        Ok(self.chain_changed())
    }

    /// Replaces a block with a new tone block, as one undo step.
    fn load_tone(
        &mut self,
        target: &BlockId,
        tone: Tone,
        model_id: ModelId,
    ) -> Result<UiEffect, UiError> {
        let id = target.0.clone();
        self.change_block(target, |block| {
            *block = ChainBlock::tone(id, tone, model_id);
        })?;
        Ok(self.chain_changed())
    }

    fn load_tone_with_bytes(
        &mut self,
        target: &BlockId,
        tone: Tone,
        model_id: ModelId,
        bytes: ModelBytes,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        self.block(target)?;
        check_downloaded_model(&tone, model_id, &bytes)?;
        storage.keep_model_bytes(target, model_id, bytes)?;
        self.load_tone(target, tone, model_id)
    }

    fn drop_file(
        &mut self,
        target: &BlockId,
        path: &Path,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        self.block(target)?;
        let tone = storage.load_local(path)?;
        let model_id = tone
            .models
            .first()
            .ok_or_else(|| UiError::Control("local load has no models".to_owned()))?
            .id;
        self.load_tone(target, tone, model_id)
    }

    fn remove_block(&mut self, id: BlockId) -> Result<UiEffect, UiError> {
        self.tone_block(&id)?;
        let before = self.current.clone();
        // The check above makes this false result unreachable; it stays as a guard. A false
        // result changes nothing, so the undo step comes after the guard.
        if !self.current.session.chain.remove_tone(&id) {
            return Err(UiError::UnknownTarget(id.0));
        }
        self.push_frame(before);
        Ok(self.chain_changed())
    }

    fn move_block(&mut self, block_id: BlockId, target_index: usize) -> Result<UiEffect, UiError> {
        let side = self.current.session.chain.active_side;
        let index = lane(&self.current.session.chain, side)
            .iter()
            .position(|block| block.id == block_id)
            .ok_or(UiError::UnknownTarget(block_id.0))?;
        self.push_history();
        let lane = lane_mut(&mut self.current.session.chain, side);
        let block = lane.remove(index);
        lane.insert(target_index.min(lane.len()), block);
        Ok(self.chain_changed())
    }

    fn move_block_to_lane(
        &mut self,
        block_id: &BlockId,
        target_side: ChainSide,
        target_index: usize,
    ) -> Result<UiEffect, UiError> {
        let (source_side, source_index) = self.position(block_id)?;
        if source_side != target_side && self.block(block_id)?.is_insert {
            return Err(UiError::InvalidAction(
                "insert slots stay in their lane".to_owned(),
            ));
        }
        self.check_lane(target_side)?;
        self.push_history();
        let chain = &mut self.current.session.chain;
        let block = lane_mut(chain, source_side).remove(source_index);
        let lane = lane_mut(chain, target_side);
        lane.insert(target_index.min(lane.len()), block);
        Ok(self.chain_changed())
    }

    fn retry_model_load(&mut self, block_id: &BlockId) -> Result<UiEffect, UiError> {
        if !self
            .current
            .session
            .chain
            .block(block_id)
            .is_some_and(|block| !block.is_insert && block.load_failed)
        {
            return Err(UiError::InvalidAction(
                "block is not waiting for a model retry".to_owned(),
            ));
        }
        self.change_block(block_id, |block| {
            block.load_failed = false;
            block.model_loading = true;
        })?;
        Ok(self.chain_changed())
    }

    /// Writes the settings of one block, inside a gesture or as one undo step of its own.
    ///
    /// The rules are those of [`Self::edit_parameter`]. An end or a cancel for a block that is
    /// gone changes nothing and is accepted: the action that removed the block closed its
    /// gesture and kept its undo step. A cancel for a block that the open gesture did not change
    /// is refused, so that it cannot restore the values of another target.
    fn edit_block_params_gesture(
        &mut self,
        block_id: BlockId,
        params: BlockParams,
        gesture: GesturePhase,
    ) -> Result<UiEffect, UiError> {
        if matches!(gesture, GesturePhase::End | GesturePhase::Cancel)
            && self.tone_block(&block_id).is_err()
        {
            return Ok(UiEffect::None);
        }
        let current = self.block(&block_id)?.params.clone();
        if gesture == GesturePhase::Cancel {
            if self
                .open_gesture
                .as_ref()
                .is_some_and(|open| !open.blocks.iter().any(|(known, _)| *known == block_id))
            {
                return Err(UiError::InvalidAction(
                    "the open gesture did not change this block".to_owned(),
                ));
            }
            // The cancel raises the revision itself.
            self.cancel_gesture()?;
            return Ok(UiEffect::ChainChanged);
        }
        self.step_for_edit(gesture, current != params);
        if let Some(open) = &mut self.open_gesture
            && !open.blocks.iter().any(|(known, _)| *known == block_id)
        {
            open.blocks.push((block_id.clone(), current));
        }
        self.block_mut(&block_id)?.params = params;
        self.close_gesture_after(gesture);
        Ok(self.chain_changed())
    }

    fn set_block_params(
        &mut self,
        block_id: &BlockId,
        params: BlockParams,
    ) -> Result<UiEffect, UiError> {
        self.change_block(block_id, |block| block.params = params)?;
        Ok(self.chain_changed())
    }

    /// Writes the EQ settings of one block by the gesture rule of a block knob.
    fn edit_eq(
        &mut self,
        block_id: BlockId,
        params: EqParams,
        gesture: GesturePhase,
    ) -> Result<UiEffect, UiError> {
        // A missing block gets default settings here, and the gesture rule then decides.
        let mut next = self
            .block(&block_id)
            .map(|block| block.params.clone())
            .unwrap_or_default();
        next.eq = params;
        self.edit_block_params_gesture(block_id, next, gesture)
    }

    fn select_model(&mut self, block_id: &BlockId, model_id: ModelId) -> Result<UiEffect, UiError> {
        self.change_block(block_id, |block| block.active_model_id = Some(model_id))?;
        Ok(self.chain_changed())
    }

    fn select_model_with_bytes(
        &mut self,
        block_id: &BlockId,
        tone: Tone,
        model_id: ModelId,
        replaces: Option<ModelId>,
        bytes: ModelBytes,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        let held = self.block(block_id)?;
        let current = held.active_model_id == replaces
            && held
                .tone
                .as_ref()
                .is_some_and(|held| held.id == tone.id && !held.local);
        if !current {
            return Err(UiError::InvalidAction(
                "the block no longer holds this model".to_owned(),
            ));
        }
        check_downloaded_model(&tone, model_id, &bytes)?;
        storage.keep_model_bytes(block_id, model_id, bytes)?;
        self.change_block(block_id, |block| {
            block.tone = Some(tone);
            block.active_model_id = Some(model_id);
            block.loaded = true;
            block.model_loading = false;
            block.load_failed = false;
        })?;
        Ok(self.chain_changed())
    }

    fn duplicate_block(
        &mut self,
        id: &BlockId,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        let (side, index) = self.position(id)?;
        let source = self.tone_block(id)?.clone();
        self.copy_into_lane(source, "copy", side, index + 1, storage)
    }

    fn duplicate_block_to_lane(
        &mut self,
        id: &BlockId,
        side: ChainSide,
        index: usize,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        let source = self.tone_block(id)?.clone();
        self.check_lane(side)?;
        self.copy_into_lane(source, "copy", side, index, storage)
    }

    fn copy_block(&mut self, id: &BlockId) -> Result<UiEffect, UiError> {
        self.clipboard = Some(self.tone_block(id)?.clone());
        Ok(self.chain_changed())
    }

    fn paste_block(
        &mut self,
        side: ChainSide,
        index: usize,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        let source = self
            .clipboard
            .clone()
            .ok_or_else(|| UiError::InvalidAction("clipboard is empty".to_owned()))?;
        self.check_lane(side)?;
        self.copy_into_lane(source, "paste", side, index, storage)
    }

    fn device(&mut self, action: DeviceAction) -> Result<UiEffect, UiError> {
        match action {
            DeviceAction::CycleInputMode => {
                self.push_history();
                let next = match self.current.session.input_mode {
                    InputMode::Stereo => InputMode::Left,
                    InputMode::Left => InputMode::Right,
                    InputMode::Right => InputMode::DualMono,
                    InputMode::DualMono => InputMode::Stereo,
                };
                self.set_input_mode(next);
            }
            // The device configuration is machine state, not session state, so it takes no undo
            // step.
            DeviceAction::Configure(config) => {
                check_device_config(&config)?;
                if let Some(rate) = config.sample_rate {
                    self.preparation.host_sample_rate = rate;
                }
                if let Some(frames) = config.buffer_frames {
                    self.preparation.max_host_frames = frames as usize;
                }
            }
            DeviceAction::SelectBackend(_)
            | DeviceAction::SetMidiInputEnabled { .. }
            | DeviceAction::SetHearYourself(_)
            | DeviceAction::SetInputChannels(_)
            | DeviceAction::SetOutputPair(_)
            | DeviceAction::PlayTestTone
            | DeviceAction::Restart
            | DeviceAction::OpenControlPanel
            | DeviceAction::OpenBluetoothMidi => {}
        }
        self.next_revision();
        Ok(UiEffect::DeviceChanged)
    }

    fn save_preset(
        &mut self,
        name: &str,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        let name = preset_name(name)?;
        let proposed = PresetInfo {
            id: format!("user:{}", name.to_lowercase().replace(' ', "-")),
            name: name.to_owned(),
            factory: false,
        };
        let info = storage.save_preset(proposed, &self.current)?;
        self.push_history();
        self.current.session.active_preset = Some(ActivePreset {
            id: info.id,
            name: info.name,
        });
        self.next_revision();
        Ok(UiEffect::PresetSaved)
    }

    fn load_preset(
        &mut self,
        id: &str,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        let (info, frame) = storage.load_preset(id, &self.current, self.preparation)?;
        self.push_history();
        self.restore(frame);
        self.current.session.active_preset = Some(ActivePreset {
            id: info.id,
            name: info.name,
        });
        Ok(self.chain_changed())
    }

    fn rename_preset(
        &mut self,
        id: &str,
        name: &str,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        let name = preset_name(name)?;
        storage.rename_preset(id, name)?;
        // A rename is no undo step, so each step shows the new name, as the store does.
        for frame in self.frames_mut() {
            if let Some(active) = &mut frame.session.active_preset
                && active.id == id
            {
                name.clone_into(&mut active.name);
            }
        }
        self.next_revision();
        Ok(UiEffect::PresetSaved)
    }

    fn delete_preset(
        &mut self,
        id: &str,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        storage.delete_preset(id)?;
        // A delete is no undo step, so no step can make the deleted preset active again.
        for frame in self.frames_mut() {
            let active = &mut frame.session.active_preset;
            if active.as_ref().is_some_and(|active| active.id == id) {
                *active = None;
            }
        }
        self.next_revision();
        Ok(UiEffect::PresetSaved)
    }

    fn move_preset(
        &mut self,
        id: &str,
        delta: i32,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        if delta == 0 {
            return Err(UiError::InvalidAction("preset cannot be moved".to_owned()));
        }
        if !storage.move_preset(id, delta)? {
            // The preset keeps its place, so nothing changes and nothing is saved.
            return Ok(UiEffect::None);
        }
        self.next_revision();
        Ok(UiEffect::PresetSaved)
    }

    fn push_history(&mut self) {
        let frame = self.current.clone();
        self.push_frame(frame);
    }

    /// The current session, and each undo and redo step.
    fn frames_mut(&mut self) -> impl Iterator<Item = &mut SessionFrame> {
        std::iter::once(&mut *self.current)
            .chain(self.undo.iter_mut())
            .chain(self.redo.iter_mut())
    }

    /// Takes one undo step that holds `frame`, the state from before a change.
    fn push_frame(&mut self, frame: SessionFrame) {
        self.undo.push(frame);
        self.drop_oldest_steps();
        self.redo.clear();
        // The last step is not the step of an open gesture now, so that its next change needs one.
        if let Some(open) = &mut self.open_gesture {
            open.step_taken = false;
        }
    }

    /// Drops the oldest undo steps above [`MAX_UNDO_STEPS`].
    ///
    /// The open gesture keeps the index of each of its steps, so each index moves down by the
    /// number of dropped steps, and the index of a dropped step goes.
    fn drop_oldest_steps(&mut self) {
        let excess = self.undo.len().saturating_sub(MAX_UNDO_STEPS);
        if excess == 0 {
            return;
        }
        self.undo.drain(..excess);
        if let Some(open) = &mut self.open_gesture {
            let before = open.steps.len();
            open.steps.retain(|step| *step >= excess);
            if open.steps.len() < before {
                open.first_step_dropped = true;
            }
            for step in &mut open.steps {
                *step -= excess;
            }
        }
    }

    /// Checks that a block exists, takes one undo step, and then changes the block.
    fn change_block(
        &mut self,
        id: &BlockId,
        change: impl FnOnce(&mut ChainBlock),
    ) -> Result<(), UiError> {
        self.block(id)?;
        self.push_history();
        change(self.block_mut(id)?);
        Ok(())
    }

    /// Normalizes the chain and raises the revision.
    fn next_revision(&mut self) {
        let chain = &mut self.current.session.chain;
        chain.normalize();
        chain.revision += 1;
    }

    /// Replaces the session with `frame`, and keeps the revision.
    fn restore(&mut self, frame: SessionFrame) {
        // A revision only goes forward, so that a caller sees this change as a change.
        let revision = self.revision();
        *self.current = frame;
        self.current.session.chain.revision = revision;
        self.preparation.input_mode = self.current.session.input_mode;
        // The stream layout is not an undo step, so a frame can hold an old one.
        let chain = &mut self.current.session.chain;
        chain.stereo_input = self.preparation.stereo_input;
        chain.stereo_output = self.preparation.stereo_output;
        self.open_gesture = None;
    }

    /// Inserts a copy of `source` with a new identity at `index` of a lane, as one undo step.
    fn copy_into_lane(
        &mut self,
        source: ChainBlock,
        kind: &str,
        side: ChainSide,
        index: usize,
        storage: &mut impl SessionStorage,
    ) -> Result<UiEffect, UiError> {
        // A restored session can hold a copy from an earlier launch, whose revision started
        // again from zero. So the suffix goes up until no block has the identity.
        let mut suffix = self.revision() + 1;
        let id = loop {
            let id = BlockId(format!("{}-{kind}-{suffix}", source.id.0));
            if self.current.session.chain.block(&id).is_none() {
                break id;
            }
            suffix += 1;
        };
        self.push_history();
        storage.share_model_bytes(&source, &id);
        let mut copy = source;
        copy.id = id;
        let lane = lane_mut(&mut self.current.session.chain, side);
        lane.insert(index.min(lane.len()), copy);
        Ok(self.chain_changed())
    }

    fn revision(&self) -> u64 {
        self.current.session.chain.revision
    }

    fn receipt(&self, effect: UiEffect) -> UiReceipt {
        UiReceipt {
            revision: self.revision(),
            effect,
        }
    }

    fn chain_changed(&mut self) -> UiEffect {
        self.next_revision();
        UiEffect::ChainChanged
    }

    fn set_input_mode(&mut self, mode: InputMode) {
        self.current.session.input_mode = mode;
        self.preparation.input_mode = mode;
    }

    fn block(&self, id: &BlockId) -> Result<&ChainBlock, UiError> {
        self.current
            .session
            .chain
            .block(id)
            .ok_or_else(|| UiError::UnknownTarget(id.0.clone()))
    }

    fn block_mut(&mut self, id: &BlockId) -> Result<&mut ChainBlock, UiError> {
        self.current
            .session
            .chain
            .block_mut(id)
            .ok_or_else(|| UiError::UnknownTarget(id.0.clone()))
    }

    /// A block that holds a tone. An open slot is an unknown target here.
    fn tone_block(&self, id: &BlockId) -> Result<&ChainBlock, UiError> {
        self.block(id)
            .ok()
            .filter(|block| !block.is_insert)
            .ok_or_else(|| UiError::UnknownTarget(id.0.clone()))
    }

    /// The lane and index of a block.
    fn position(&self, id: &BlockId) -> Result<(ChainSide, usize), UiError> {
        let chain = &self.current.session.chain;
        [ChainSide::Left, ChainSide::Right]
            .into_iter()
            .find_map(|side| {
                lane(chain, side)
                    .iter()
                    .position(|block| &block.id == id)
                    .map(|index| (side, index))
            })
            .ok_or_else(|| UiError::UnknownTarget(id.0.clone()))
    }

    /// Refuses the right lane in mono.
    fn check_lane(&self, side: ChainSide) -> Result<(), UiError> {
        if side == ChainSide::Right && !self.current.session.chain.stereo_enabled {
            return Err(UiError::InvalidAction(
                "right lane requires stereo mode".to_owned(),
            ));
        }
        Ok(())
    }

    fn swap_chains(&mut self) -> Result<UiEffect, UiError> {
        if !self.current.session.chain.stereo_enabled {
            return Err(UiError::InvalidAction(
                "lane swap requires stereo mode".to_owned(),
            ));
        }
        self.push_history();
        let chain = &mut self.current.session.chain;
        std::mem::swap(&mut chain.left, &mut chain.right);
        if let Some(branch) = &mut chain.branch {
            branch.side = other_side(branch.side);
        }
        let left_id = ParameterId("chainInvertLeft");
        let right_id = ParameterId("chainInvertRight");
        let parameters = &mut self.current.parameters;
        let left = parameters
            .get(&left_id)
            .copied()
            .unwrap_or(ParameterValue(0.0));
        let right = parameters
            .get(&right_id)
            .copied()
            .unwrap_or(ParameterValue(0.0));
        if left != right {
            parameters.insert(left_id, right);
            parameters.insert(right_id, left);
        }
        Ok(self.chain_changed())
    }

    fn apply_midi(&mut self, action: MidiAction) -> Result<UiEffect, UiError> {
        match action {
            MidiAction::SetChannel(channel) => {
                if channel > MAX_MIDI_CHANNEL {
                    return Err(UiError::InvalidAction(
                        "MIDI channel must be 0 through 16".to_owned(),
                    ));
                }
                self.midi_channel = channel;
            }
            MidiAction::StartLearn(target) => {
                if self.midi_learn_target != target {
                    self.midi_draft = None;
                }
                self.midi_learn_target = target;
            }
            MidiAction::CancelLearn => {
                self.midi_learn_target.clear();
                self.midi_draft = None;
            }
            MidiAction::SetDraft(draft) => self.midi_draft = draft,
            MidiAction::SetMapping {
                target_id,
                source,
                number,
            } => {
                if number > MAX_MIDI_NUMBER {
                    return Err(UiError::InvalidAction(
                        "MIDI number must be between 0 and 127".to_owned(),
                    ));
                }
                let mapping = MidiMapping {
                    target_id: target_id.clone(),
                    source,
                    number,
                };
                self.push_history();
                let mappings = &mut self.current.session.midi_mappings;
                if let Some(existing) = mappings
                    .iter_mut()
                    .find(|mapping| mapping.target_id == target_id)
                {
                    *existing = mapping;
                } else {
                    mappings.push(mapping);
                }
                self.midi_learn_target.clear();
                self.midi_draft = None;
            }
            MidiAction::RemoveMapping(target) => {
                self.push_history();
                self.current
                    .session
                    .midi_mappings
                    .retain(|mapping| mapping.target_id != target);
            }
        }
        Ok(self.chain_changed())
    }

    /// Validates and writes one parameter, inside a gesture or as one undo step of its own.
    ///
    /// [`GesturePhase::Begin`] opens a gesture. While a gesture is open, each write joins it, also
    /// for another parameter, because one drag of a linked knob writes two parameters. The first
    /// write that changes a value takes the undo step, so a gesture that changes nothing takes
    /// none. [`GesturePhase::End`] closes the gesture, and with no open gesture it is one step,
    /// the same as [`GesturePhase::Change`]. [`GesturePhase::Cancel`] restores each value from
    /// before the gesture.
    fn edit_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<UiEffect, UiError> {
        let spec = ParameterCatalog::v1()
            .iter()
            .find(|spec| spec.id == id)
            .ok_or(UiError::UnknownParameter(id))?;
        if gesture == GesturePhase::Cancel {
            self.cancel_gesture()?;
            return Ok(UiEffect::ParameterChanged);
        }
        if !value.0.is_finite() {
            return Err(UiError::InvalidAction(
                "parameter value is not finite".to_owned(),
            ));
        }
        let mut next = value.0.clamp(spec.min, spec.max);
        if !spec.choices.is_empty() {
            next = next.round();
        } else if let Some(interval) = spec.interval.filter(|interval| *interval > 0.0) {
            next = (next / interval).round() * interval;
        }
        let next = ParameterValue(next);
        let current = self.parameter(id);
        self.step_for_edit(gesture, current != Some(next));
        if let (Some(open), Some(current)) = (&mut self.open_gesture, current)
            && !open.parameters.iter().any(|(known, _)| *known == id)
        {
            open.parameters.push((id, current));
        }
        self.current.parameters.insert(id, next);
        self.close_gesture_after(gesture);
        self.next_revision();
        Ok(UiEffect::ParameterChanged)
    }

    /// Takes the undo step that one edit needs. See [`Self::edit_parameter`] for the rules.
    fn step_for_edit(&mut self, gesture: GesturePhase, changes: bool) {
        if gesture == GesturePhase::Begin {
            self.open_gesture = Some(OpenGesture::default());
        }
        match &self.open_gesture {
            None => self.push_history(),
            Some(open) if changes && !open.step_taken => {
                self.push_history();
                let step = self.undo.len() - 1;
                if let Some(open) = &mut self.open_gesture {
                    open.step_taken = true;
                    open.steps.push(step);
                }
            }
            Some(_) => {}
        }
    }

    fn close_gesture_after(&mut self, gesture: GesturePhase) {
        if gesture == GesturePhase::End {
            self.open_gesture = None;
        }
    }

    /// Restores what the open gesture changed, and removes each undo step that it took.
    ///
    /// The step of another action during the gesture stays. That step holds a value of the
    /// gesture, so it gets the value from before the gesture, and an undo of it cannot show the
    /// cancelled value again.
    fn cancel_gesture(&mut self) -> Result<(), UiError> {
        let Some(open) = self.open_gesture.take() else {
            return Err(UiError::InvalidAction("no gesture to cancel".to_owned()));
        };
        // With its first step dropped, each step that the history holds is newer than the
        // gesture, so each one gets the values from before the gesture.
        let first = if open.first_step_dropped {
            Some(0)
        } else {
            open.steps.first().copied()
        };
        if let Some(first) = first {
            for frame in &mut self.undo[first..] {
                open.restore_into(frame);
            }
            for &step in open.steps.iter().rev() {
                self.undo.remove(step);
            }
        }
        open.restore_into(&mut self.current);
        self.next_revision();
        Ok(())
    }
}

/// Refuses bytes that cannot belong to one catalog model of `tone`.
fn check_downloaded_model(
    tone: &Tone,
    model_id: ModelId,
    bytes: &ModelBytes,
) -> Result<(), UiError> {
    let from_catalog = !tone.local
        && tone.models.iter().any(|model| {
            model.id == model_id && matches!(model.source, Some(ModelSource::Catalog { .. }))
        });
    if !from_catalog || bytes.is_empty() {
        return Err(UiError::InvalidAction(
            "downloaded bytes need a catalog model".to_owned(),
        ));
    }
    Ok(())
}

/// Moves `id` by `delta` places in `order`, and stops at each end.
///
/// Returns `None` when `order` does not hold `id`, and `Some(false)` when the place does not
/// change. A backend with no preset store uses this, so that it agrees with the store at an edge.
#[must_use]
pub fn move_in_order(order: &mut Vec<String>, id: &str, delta: i32) -> Option<bool> {
    let index = order.iter().position(|known| known == id)?;
    // `position` found `id`, so `order` is not empty.
    let last = order.len() - 1;
    let target = if delta < 0 {
        index.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        index.saturating_add(delta as usize).min(last)
    };
    if target == index {
        return Some(false);
    }
    let moving = order.remove(index);
    order.insert(target, moving);
    Some(true)
}

/// Refuses a device configuration with a sample rate or a buffer size that no device can use.
///
/// # Errors
/// Returns an invalid-action error for a sample rate that is not a positive number, or for a
/// buffer size of zero frames.
pub fn check_device_config(config: &AudioDeviceConfig) -> Result<(), UiError> {
    if config
        .sample_rate
        .is_some_and(|rate| !rate.is_finite() || rate <= 0.0)
    {
        return Err(UiError::InvalidAction(
            "sample rate must be a positive number".to_owned(),
        ));
    }
    if config.buffer_frames == Some(0) {
        return Err(UiError::InvalidAction(
            "buffer size must be more than zero frames".to_owned(),
        ));
    }
    Ok(())
}

/// The trimmed preset name, which must not be empty.
fn preset_name(name: &str) -> Result<&str, UiError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(UiError::InvalidAction("preset name is empty".to_owned()));
    }
    Ok(trimmed)
}

/// The highest MIDI channel filter. Zero is omni.
const MAX_MIDI_CHANNEL: u8 = 16;
/// The highest MIDI controller or note number.
const MAX_MIDI_NUMBER: u8 = 127;

/// Refuses a parameter that is not a machine default.
///
/// # Errors
/// Returns an unknown-parameter error for an ID outside the catalog, or an invalid-action error.
fn check_machine_default(id: ParameterId) -> Result<(), UiError> {
    let spec = ParameterCatalog::v1()
        .iter()
        .find(|spec| spec.id == id)
        .ok_or(UiError::UnknownParameter(id))?;
    if !spec.scope.machine_default {
        return Err(UiError::InvalidAction(format!(
            "parameter {} is not a machine default",
            id.0
        )));
    }
    Ok(())
}

fn catalog_defaults() -> BTreeMap<ParameterId, ParameterValue> {
    ParameterCatalog::v1()
        .iter()
        .map(|spec| (spec.id, ParameterValue(spec.default)))
        .collect()
}

/// The default session, with the stereo layout of `preparation`.
fn empty_session(input_mode: InputMode, preparation: PrepareSpec) -> SessionSnapshot {
    let mut chain = ChainSnapshot::empty();
    chain.stereo_input = preparation.stereo_input;
    chain.stereo_output = preparation.stereo_output;
    SessionSnapshot {
        chain,
        input_mode,
        editor_scale: 1.0,
        editor_extra_height: 36,
        midi_mappings: Vec::new(),
        active_preset: None,
    }
}

fn other_side(side: ChainSide) -> ChainSide {
    match side {
        ChainSide::Left => ChainSide::Right,
        ChainSide::Right => ChainSide::Left,
    }
}

fn lane(chain: &ChainSnapshot, side: ChainSide) -> &Vec<ChainBlock> {
    match side {
        ChainSide::Left => &chain.left,
        ChainSide::Right => &chain.right,
    }
}

fn lane_mut(chain: &mut ChainSnapshot, side: ChainSide) -> &mut Vec<ChainBlock> {
    match side {
        ChainSide::Left => &mut chain.left,
        ChainSide::Right => &mut chain.right,
    }
}

#[cfg(test)]
mod tests {
    use ronie_core::MidiSource;

    use super::*;

    /// Storage that keeps a log of its calls, and refuses each call when `refuse` is set.
    ///
    /// A preset move changes the place of the preset, unless `edge` is set.
    #[derive(Debug, Default)]
    struct FakeStorage {
        calls: Vec<String>,
        refuse: bool,
        edge: bool,
    }

    impl FakeStorage {
        fn call(&mut self, name: &str) -> Result<(), UiError> {
            self.calls.push(name.to_owned());
            if self.refuse {
                return Err(UiError::Control(format!("{name} failed")));
            }
            Ok(())
        }
    }

    impl SessionStorage for FakeStorage {
        fn keep_model_bytes(
            &mut self,
            _block_id: &BlockId,
            _model_id: ModelId,
            _bytes: ModelBytes,
        ) -> Result<(), UiError> {
            self.call("keep")
        }

        fn share_model_bytes(&mut self, _source: &ChainBlock, copy: &BlockId) {
            self.calls.push(format!("share {}", copy.0));
        }

        fn load_local(&mut self, _path: &Path) -> Result<Tone, UiError> {
            self.call("local")?;
            Ok(tone(5))
        }

        fn save_preset(
            &mut self,
            proposed: PresetInfo,
            _current: &SessionFrame,
        ) -> Result<PresetInfo, UiError> {
            self.call("save")?;
            Ok(proposed)
        }

        fn load_preset(
            &mut self,
            id: &str,
            current: &SessionFrame,
            _preparation: PrepareSpec,
        ) -> Result<(PresetInfo, SessionFrame), UiError> {
            self.call("load")?;
            let mut frame = current.clone();
            frame.multi_core = false;
            let info = PresetInfo {
                id: id.to_owned(),
                name: "Loaded".to_owned(),
                factory: false,
            };
            Ok((info, frame))
        }

        fn rename_preset(&mut self, _id: &str, _name: &str) -> Result<(), UiError> {
            self.call("rename")
        }

        fn delete_preset(&mut self, _id: &str) -> Result<(), UiError> {
            self.call("delete")
        }

        fn move_preset(&mut self, _id: &str, _delta: i32) -> Result<bool, UiError> {
            self.call("move")?;
            Ok(!self.edge)
        }
    }

    trait ApplyAlone {
        fn apply_alone(&mut self, action: UiAction) -> Result<UiReceipt, UiError>;
    }

    impl ApplyAlone for SessionEditor {
        fn apply_alone(&mut self, action: UiAction) -> Result<UiReceipt, UiError> {
            self.apply(action, &mut FakeStorage::default())
        }
    }

    fn tone(id: i64) -> Tone {
        Tone::from_value(serde_json::json!({
            "id": id,
            "title": format!("Tone {id}"),
            "format": "ir",
            "models": [
                {"id": id * 10, "name": "First", "model_url": "https://models.example/first.wav"},
            ],
        }))
        .expect("tone")
    }

    fn bytes() -> ModelBytes {
        b"downloaded".to_vec().into()
    }

    /// Loads a tone into the first slot of the left lane.
    fn load(editor: &mut SessionEditor) -> BlockId {
        let slot = editor.view(0).chain.left[0].id.clone();
        editor
            .apply_alone(UiAction::LoadTone {
                target_insert_id: slot.clone(),
                tone: tone(1),
                model_id: ModelId(10),
            })
            .expect("load");
        slot
    }

    fn editor() -> SessionEditor {
        SessionEditor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        })
    }

    fn write(editor: &mut SessionEditor, value: f32, gesture: GesturePhase) -> UiReceipt {
        editor
            .apply_alone(UiAction::SetParameter {
                id: ParameterId("outputLevel"),
                value: ParameterValue(value),
                gesture,
            })
            .expect("write")
    }

    fn level(editor: &SessionEditor) -> f32 {
        editor
            .parameter(ParameterId("outputLevel"))
            .expect("catalog parameter")
            .0
    }

    #[test]
    fn a_new_session_holds_each_catalog_default_and_the_preparation_layout() {
        let editor = editor();
        for spec in ParameterCatalog::v1().iter() {
            assert_eq!(
                editor.parameter(spec.id),
                Some(ParameterValue(spec.default))
            );
        }
        let view = editor.view(0);
        assert_eq!(view.session.input_mode, InputMode::Left);
        assert!(view.chain.stereo_output);
        assert!(!view.can_undo && !view.can_redo);
    }

    #[test]
    fn a_write_is_clamped_and_raises_the_revision() {
        let mut editor = editor();
        let receipt = write(&mut editor, 50.0, GesturePhase::End);
        assert_eq!(level(&editor), 1.0);
        assert_eq!(receipt.effect, UiEffect::ParameterChanged);
        assert_eq!(receipt.revision, editor.view(0).revision);
        assert!(receipt.revision > 0);
    }

    #[test]
    fn a_refused_write_changes_nothing() {
        let mut editor = editor();
        let before = editor.view(0).revision;
        let refused = editor.apply_alone(UiAction::SetParameter {
            id: ParameterId("outputLevel"),
            value: ParameterValue(f32::NAN),
            gesture: GesturePhase::End,
        });
        assert!(matches!(refused, Err(UiError::InvalidAction(_))));
        assert_eq!(editor.view(0).revision, before);
        assert!(!editor.view(0).can_undo);
    }

    #[test]
    fn one_gesture_is_one_undo_step() {
        let mut editor = editor();
        let start = level(&editor);
        write(&mut editor, start, GesturePhase::Begin);
        for value in [0.2, 0.3, 0.4] {
            write(&mut editor, value, GesturePhase::Change);
        }
        write(&mut editor, 0.4, GesturePhase::End);
        editor.apply_alone(UiAction::Undo).expect("undo");
        assert_eq!(level(&editor), start);
        assert!(!editor.view(0).can_undo);
        editor.apply_alone(UiAction::Redo).expect("redo");
        assert!((level(&editor) - 0.4).abs() < 0.001);
    }

    /// Sets the multicore flag, an action that takes one undo step and does not join a gesture.
    fn other_edit(editor: &mut SessionEditor, enabled: bool) {
        editor
            .apply_alone(UiAction::SetMultiCore(enabled))
            .expect("other edit");
    }

    /// The output level of each undo step.
    fn step_levels(editor: &SessionEditor) -> Vec<f32> {
        editor
            .undo
            .iter()
            .map(|frame| frame.parameters[&ParameterId("outputLevel")].0)
            .collect()
    }

    #[test]
    fn the_history_keeps_the_newest_undo_steps_up_to_the_limit() {
        let mut editor = editor();
        for edit in 1..=250 {
            write(&mut editor, edit as f32 / 1000.0, GesturePhase::End);
        }
        assert_eq!(editor.undo.len(), MAX_UNDO_STEPS);

        for _ in 0..MAX_UNDO_STEPS {
            editor.apply_alone(UiAction::Undo).expect("undo");
        }
        assert!((level(&editor) - 0.05).abs() < 1.0e-6, "{}", level(&editor));
        assert!(editor.apply_alone(UiAction::Undo).is_err());
        for _ in 0..MAX_UNDO_STEPS {
            editor.apply_alone(UiAction::Redo).expect("redo");
        }
        assert!((level(&editor) - 0.25).abs() < 1.0e-6, "{}", level(&editor));
        assert_eq!(editor.undo.len(), MAX_UNDO_STEPS);
    }

    #[test]
    fn a_gesture_whose_steps_move_at_the_limit_still_cancels() {
        let mut editor = editor();
        for edit in 1..MAX_UNDO_STEPS {
            other_edit(&mut editor, edit % 2 == 0);
        }
        let start = level(&editor);
        write(&mut editor, start, GesturePhase::Begin);
        write(&mut editor, 0.1, GesturePhase::Change);
        other_edit(&mut editor, true);
        write(&mut editor, 0.2, GesturePhase::Change);
        other_edit(&mut editor, false);
        write(&mut editor, 0.3, GesturePhase::Change);
        assert_eq!(editor.undo.len(), MAX_UNDO_STEPS);

        write(&mut editor, 0.3, GesturePhase::Cancel);

        assert_eq!(level(&editor), start);
        // The three steps of the gesture go, and the two other edits stay.
        assert_eq!(editor.undo.len(), MAX_UNDO_STEPS - 3);
        assert!(
            step_levels(&editor).iter().all(|value| *value == start),
            "{:?}",
            step_levels(&editor)
        );
        editor.apply_alone(UiAction::Undo).expect("undo");
        assert!(
            editor.view(0).multi_core,
            "the undo takes back the last flag"
        );
        assert_eq!(level(&editor), start);
    }

    #[test]
    fn a_gesture_whose_first_step_drops_at_the_limit_still_cancels() {
        let mut editor = editor();
        let start = level(&editor);
        write(&mut editor, start, GesturePhase::Begin);
        write(&mut editor, 0.1, GesturePhase::Change);
        for edit in 0..MAX_UNDO_STEPS + 5 {
            other_edit(&mut editor, edit % 2 == 0);
        }
        write(&mut editor, 0.2, GesturePhase::Change);
        assert_eq!(editor.undo.len(), MAX_UNDO_STEPS);

        write(&mut editor, 0.2, GesturePhase::Cancel);

        assert_eq!(level(&editor), start);
        // Only the newest step of the gesture is left to remove.
        assert_eq!(editor.undo.len(), MAX_UNDO_STEPS - 1);
        assert!(
            step_levels(&editor).iter().all(|value| *value == start),
            "an undo cannot show a cancelled value: {:?}",
            step_levels(&editor)
        );
    }

    #[test]
    fn a_cancelled_gesture_restores_the_value_and_leaves_no_step() {
        let mut editor = editor();
        let start = level(&editor);
        write(&mut editor, start, GesturePhase::Begin);
        write(&mut editor, 0.1, GesturePhase::Change);
        write(&mut editor, 0.1, GesturePhase::Cancel);
        assert_eq!(level(&editor), start);
        assert!(!editor.view(0).can_undo);
        assert!(matches!(
            editor.apply_alone(UiAction::SetParameter {
                id: ParameterId("outputLevel"),
                value: ParameterValue(0.1),
                gesture: GesturePhase::Cancel,
            }),
            Err(UiError::InvalidAction(_))
        ));
    }

    #[test]
    fn a_swap_needs_stereo_and_swaps_the_polarity_switches() {
        let mut editor = editor();
        assert!(matches!(
            editor.apply_alone(UiAction::SwapChains),
            Err(UiError::InvalidAction(_))
        ));
        editor
            .apply_alone(UiAction::SetStereoMode(true))
            .expect("stereo");
        editor
            .apply_alone(UiAction::SetParameter {
                id: ParameterId("chainInvertLeft"),
                value: ParameterValue(1.0),
                gesture: GesturePhase::End,
            })
            .expect("invert");
        editor.apply_alone(UiAction::SwapChains).expect("swap");
        assert_eq!(
            editor.parameter(ParameterId("chainInvertRight")),
            Some(ParameterValue(1.0))
        );
        assert_eq!(
            editor.parameter(ParameterId("chainInvertLeft")),
            Some(ParameterValue(0.0))
        );
    }

    #[test]
    fn a_device_configuration_reaches_the_preparation() {
        let mut editor = editor();
        let receipt = editor
            .apply_alone(UiAction::Device(DeviceAction::Configure(
                crate::backend::AudioDeviceConfig {
                    sample_rate: Some(96_000.0),
                    buffer_frames: Some(256),
                    ..Default::default()
                },
            )))
            .expect("configure");
        assert_eq!(receipt.effect, UiEffect::DeviceChanged);
        assert_eq!(editor.preparation().host_sample_rate, 96_000.0);
        assert_eq!(editor.preparation().max_host_frames, 256);
        editor
            .apply_alone(UiAction::Device(DeviceAction::CycleInputMode))
            .expect("cycle");
        assert_eq!(editor.preparation().input_mode, InputMode::Right);
        assert_eq!(editor.view(0).session.input_mode, InputMode::Right);
    }

    #[test]
    fn a_midi_mapping_follows_the_limits_and_ends_the_learn() {
        let mut editor = editor();
        assert!(
            editor
                .apply_alone(UiAction::Midi(MidiAction::SetChannel(17)))
                .is_err()
        );
        editor
            .apply_alone(UiAction::Midi(MidiAction::StartLearn(
                "outputLevel".to_owned(),
            )))
            .expect("learn");
        assert!(
            editor
                .apply_alone(UiAction::Midi(MidiAction::SetMapping {
                    target_id: "outputLevel".to_owned(),
                    source: MidiSource::Cc,
                    number: 128,
                }))
                .is_err()
        );
        editor
            .apply_alone(UiAction::Midi(MidiAction::SetMapping {
                target_id: "outputLevel".to_owned(),
                source: MidiSource::Cc,
                number: 7,
            }))
            .expect("map");
        let map = editor.midi_map();
        assert_eq!(map.learn_target_id, "");
        assert_eq!(map.mappings.len(), 1);
        assert_eq!(editor.current().session.midi_mappings, map.mappings);
    }

    #[test]
    fn a_device_configuration_takes_no_undo_step() {
        let mut editor = editor();
        write(&mut editor, 0.2, GesturePhase::End);
        editor.apply_alone(UiAction::Undo).expect("undo");
        write(&mut editor, 0.3, GesturePhase::End);
        editor.apply_alone(UiAction::Undo).expect("undo");
        let before = editor.view(0);
        assert!(!before.can_undo && before.can_redo);
        editor
            .apply_alone(UiAction::Device(DeviceAction::Configure(
                crate::backend::AudioDeviceConfig {
                    sample_rate: Some(44_100.0),
                    ..Default::default()
                },
            )))
            .expect("configure");
        let after = editor.view(0);
        assert_eq!(after.preparation.host_sample_rate, 44_100.0);
        assert_eq!(
            (after.can_undo, after.can_redo),
            (before.can_undo, before.can_redo)
        );
    }

    #[test]
    fn a_device_configuration_with_no_usable_rate_or_size_changes_nothing() {
        let mut editor = editor();
        let before = editor.view(0);
        for (sample_rate, buffer_frames) in [
            (Some(0.0), None),
            (Some(-48_000.0), None),
            (Some(f64::NAN), Some(256)),
            (Some(f64::INFINITY), None),
            (Some(96_000.0), Some(0)),
            (None, Some(0)),
        ] {
            let refused = editor.apply_alone(UiAction::Device(DeviceAction::Configure(
                crate::backend::AudioDeviceConfig {
                    sample_rate,
                    buffer_frames,
                    ..Default::default()
                },
            )));
            assert!(
                matches!(refused, Err(UiError::InvalidAction(_))),
                "{sample_rate:?} {buffer_frames:?}"
            );
        }
        let after = editor.view(0);
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.preparation, before.preparation);
    }

    #[test]
    fn an_undo_keeps_the_stereo_layout_of_the_stream() {
        let mut editor = editor();
        write(&mut editor, 0.2, GesturePhase::End);
        editor.set_stream_format(48_000.0, 128, true, false);
        for action in [UiAction::Undo, UiAction::Redo] {
            editor.apply_alone(action).expect("history step");
            let view = editor.view(0);
            assert!(view.preparation.stereo_input);
            assert_eq!(view.chain.stereo_input, view.preparation.stereo_input);
            assert_eq!(view.chain.stereo_output, view.preparation.stereo_output);
        }
    }

    #[test]
    fn a_saved_session_starts_with_an_empty_history() {
        let mut editor = editor();
        write(&mut editor, 0.2, GesturePhase::End);
        write(&mut editor, 0.3, GesturePhase::End);
        editor.apply_alone(UiAction::Undo).expect("undo");
        write(&mut editor, 0.4, GesturePhase::Begin);
        let frame = editor.current().clone();
        editor.load_saved_session(frame, 0);
        let view = editor.view(0);
        assert!(!view.can_undo && !view.can_redo);
        assert!(matches!(
            editor.apply_alone(UiAction::SetParameter {
                id: ParameterId("outputLevel"),
                value: ParameterValue(0.9),
                gesture: GesturePhase::Cancel,
            }),
            Err(UiError::InvalidAction(_))
        ));
    }

    #[test]
    fn a_reset_ends_each_open_gesture() {
        let mut editor = editor();
        let slot = load(&mut editor);
        write(&mut editor, 0.2, GesturePhase::Begin);
        write(&mut editor, 0.3, GesturePhase::Change);
        let mut eq = editor
            .view(0)
            .chain
            .block(&slot)
            .expect("block")
            .params
            .eq
            .clone();
        eq.enabled = !eq.enabled;
        editor
            .apply_alone(UiAction::SetEq {
                block_id: slot.clone(),
                params: eq.clone(),
                gesture: GesturePhase::Begin,
            })
            .expect("EQ begin");
        editor.apply_alone(UiAction::ResetToDefault).expect("reset");
        let reset = editor.view(0);
        assert!(
            reset.chain.block(&slot).is_some(),
            "the slot stays after a reset"
        );

        // The tone of the slot is gone, so the cancel finds nothing to restore.
        let cancel = editor
            .apply_alone(UiAction::SetEq {
                block_id: slot,
                params: eq,
                gesture: GesturePhase::Cancel,
            })
            .expect("a cancel for a lost block");
        assert_eq!(cancel.effect, UiEffect::None);
        assert!(matches!(
            editor.apply_alone(UiAction::SetParameter {
                id: ParameterId("outputLevel"),
                value: ParameterValue(0.3),
                gesture: GesturePhase::Cancel,
            }),
            Err(UiError::InvalidAction(_))
        ));
        let after = editor.view(0);
        assert_eq!(after.chain, reset.chain);
        assert_eq!(after.revision, reset.revision);
        assert_eq!(
            editor.parameter(ParameterId("outputLevel")),
            Some(ParameterValue(0.5))
        );
    }

    #[test]
    fn a_preset_move_that_keeps_the_place_has_no_effect() {
        let mut editor = editor();
        let mut storage = FakeStorage {
            edge: true,
            ..Default::default()
        };
        let before = editor.view(0);
        let receipt = editor
            .apply(
                UiAction::MovePreset {
                    id: "user:a".to_owned(),
                    delta: -1,
                },
                &mut storage,
            )
            .expect("move at the edge");
        assert_eq!(receipt.effect, UiEffect::None);
        assert_eq!(receipt.revision, before.revision);
        assert_eq!(editor.view(0).revision, before.revision);
    }

    #[test]
    fn a_move_in_order_stops_at_each_end() {
        let mut order = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(move_in_order(&mut order, "a", -1), Some(false));
        assert_eq!(move_in_order(&mut order, "c", 5), Some(false));
        assert_eq!(move_in_order(&mut order, "a", i32::MAX), Some(true));
        assert_eq!(order, ["b", "c", "a"]);
        assert_eq!(move_in_order(&mut order, "a", i32::MIN), Some(true));
        assert_eq!(order, ["a", "b", "c"]);
        assert_eq!(move_in_order(&mut order, "z", 1), None);
    }

    #[test]
    fn a_copy_never_takes_the_identity_of_a_restored_block() {
        let mut editor = editor();
        let block = load(&mut editor);
        let start = editor.view(0).revision;
        editor
            .apply_alone(UiAction::DuplicateBlock(block.clone()))
            .expect("duplicate");
        // A new launch restores the copy, and its revision starts again. The next copy then
        // gets the same suffix as the restored copy.
        let mut session = editor.current().session.clone();
        session.chain.revision = start;
        let mut restored = SessionEditor::seeded(SessionSeed {
            session,
            preparation: editor.preparation(),
            parameters: BTreeMap::new(),
            midi_channel: 0,
            midi_learn_target: String::new(),
            can_undo: false,
            can_redo: false,
        });
        restored
            .apply_alone(UiAction::DuplicateBlock(block))
            .expect("duplicate");
        let chain = restored.view(0).chain;
        let ids: Vec<_> = chain.left.iter().map(|block| &block.id).collect();
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "{ids:?}");
    }

    #[test]
    fn a_reset_restores_the_defaults_and_is_one_undo_step() {
        let mut editor = editor();
        editor
            .apply_alone(UiAction::SetStereoMode(true))
            .expect("stereo");
        write(&mut editor, 0.1, GesturePhase::End);
        let receipt = editor.apply_alone(UiAction::ResetToDefault).expect("reset");
        assert_eq!(receipt.revision, editor.view(0).revision);
        assert!(!editor.view(0).chain.stereo_enabled);
        assert_eq!(editor.view(0).session.input_mode, InputMode::Stereo);
        assert_eq!(editor.preparation().input_mode, InputMode::Stereo);
        editor.apply_alone(UiAction::Undo).expect("undo");
        assert!((level(&editor) - 0.1).abs() < 0.001);
    }

    #[test]
    fn a_seed_keeps_its_session_and_drops_what_an_edit_would_refuse() {
        let start = editor();
        let mut session = start.current().session.clone();
        session.chain.revision = 1;
        session.midi_mappings = vec![
            MidiMapping {
                target_id: "outputLevel".to_owned(),
                source: MidiSource::Cc,
                number: 7,
            },
            MidiMapping {
                target_id: "inputLevel".to_owned(),
                source: MidiSource::Cc,
                number: 200,
            },
        ];
        let mut editor = SessionEditor::seeded(SessionSeed {
            session,
            preparation: start.preparation(),
            parameters: BTreeMap::from([
                (ParameterId("outputLevel"), ParameterValue(0.3)),
                (ParameterId("noSuchParameter"), ParameterValue(0.3)),
            ]),
            midi_channel: 17,
            midi_learn_target: "outputLevel".to_owned(),
            can_undo: true,
            can_redo: false,
        });
        assert_eq!(level(&editor), 0.3);
        assert_eq!(editor.parameter(ParameterId("noSuchParameter")), None);
        let map = editor.midi_map();
        assert_eq!(map.channel, 0);
        assert_eq!(map.learn_target_id, "outputLevel");
        assert_eq!(map.mappings.len(), 1);
        let view = editor.view(0);
        assert_eq!(view.revision, 1);
        assert!(view.can_undo && !view.can_redo);

        editor.apply_alone(UiAction::Undo).expect("undo");
        assert_eq!(editor.view(0).chain.left, view.chain.left);
        assert!(editor.view(0).can_redo);
    }

    #[test]
    fn only_a_machine_default_passes_the_check() {
        assert!(check_machine_default(ParameterId("inputCalibrationLevel")).is_ok());
        assert!(matches!(
            check_machine_default(ParameterId("outputLevel")),
            Err(UiError::InvalidAction(_))
        ));
        assert!(matches!(
            check_machine_default(ParameterId("noSuchParameter")),
            Err(UiError::UnknownParameter(_))
        ));
    }

    #[test]
    fn a_refused_action_does_not_reach_the_storage() {
        let mut editor = editor();
        let mut storage = FakeStorage::default();
        let slot = editor.view(0).chain.left[0].id.clone();
        for action in [
            UiAction::LoadToneWithBytes {
                target_insert_id: slot.clone(),
                tone: tone(1),
                model_id: ModelId(99),
                bytes: bytes(),
            },
            UiAction::SavePreset {
                name: "  ".to_owned(),
            },
            UiAction::RenamePreset {
                id: "user:a".to_owned(),
                name: String::new(),
            },
            UiAction::MovePreset {
                id: "user:a".to_owned(),
                delta: 0,
            },
            UiAction::DropFile {
                target_insert_id: BlockId("no-such-block".to_owned()),
                path: "tone.wav".into(),
            },
        ] {
            assert!(editor.apply(action, &mut storage).is_err());
        }
        assert_eq!(storage.calls, Vec::<String>::new());
        assert_eq!(editor.view(0).revision, 0);
    }

    #[test]
    fn a_storage_failure_changes_nothing() {
        let mut editor = editor();
        let mut storage = FakeStorage {
            refuse: true,
            ..Default::default()
        };
        let slot = editor.view(0).chain.left[0].id.clone();
        let before = editor.view(0);
        for action in [
            UiAction::LoadToneWithBytes {
                target_insert_id: slot.clone(),
                tone: tone(1),
                model_id: ModelId(10),
                bytes: bytes(),
            },
            UiAction::DropFile {
                target_insert_id: slot,
                path: "tone.wav".into(),
            },
            UiAction::SavePreset {
                name: "Lead".to_owned(),
            },
            UiAction::LoadPreset {
                id: "user:lead".to_owned(),
            },
        ] {
            assert!(matches!(
                editor.apply(action, &mut storage),
                Err(UiError::Control(_))
            ));
        }
        let after = editor.view(0);
        assert_eq!(after.chain, before.chain);
        assert_eq!(after.session, before.session);
        assert!(!after.can_undo);
    }

    #[test]
    fn a_copy_gets_a_new_identity_and_the_bytes_of_its_source() {
        let mut editor = editor();
        let mut storage = FakeStorage::default();
        let block = load(&mut editor);
        editor
            .apply(UiAction::DuplicateBlock(block.clone()), &mut storage)
            .expect("duplicate");
        let copy = editor.view(0).chain.left[1].id.clone();
        assert_ne!(copy, block);
        assert_eq!(storage.calls, vec![format!("share {}", copy.0)]);

        editor
            .apply(UiAction::CopyBlock(block), &mut storage)
            .expect("copy");
        assert!(editor.can_paste());
        let refused = editor.apply(
            UiAction::PasteBlock {
                side: ChainSide::Right,
                index: 0,
            },
            &mut storage,
        );
        assert!(matches!(refused, Err(UiError::InvalidAction(_))));
        editor
            .apply(
                UiAction::PasteBlock {
                    side: ChainSide::Left,
                    index: 0,
                },
                &mut storage,
            )
            .expect("paste");
        assert!(editor.view(0).chain.left[0].id.0.contains("-paste-"));
    }

    #[test]
    fn a_saved_preset_is_active_and_only_its_own_rename_or_delete_changes_it() {
        let mut editor = editor();
        let receipt = editor
            .apply_alone(UiAction::SavePreset {
                name: "  Stadium Lead ".to_owned(),
            })
            .expect("save");
        assert_eq!(receipt.effect, UiEffect::PresetSaved);
        let active = editor
            .view(0)
            .session
            .active_preset
            .clone()
            .expect("active");
        assert_eq!(active.id, "user:stadium-lead");
        assert_eq!(active.name, "Stadium Lead");

        for action in [
            UiAction::RenamePreset {
                id: "user:other".to_owned(),
                name: "Other".to_owned(),
            },
            UiAction::DeletePreset {
                id: "user:other".to_owned(),
            },
            UiAction::MovePreset {
                id: "user:other".to_owned(),
                delta: 1,
            },
        ] {
            let receipt = editor.apply_alone(action).expect("preset edit");
            assert_eq!(receipt.effect, UiEffect::PresetSaved);
            assert_eq!(editor.view(0).session.active_preset.as_ref(), Some(&active));
        }

        editor
            .apply_alone(UiAction::RenamePreset {
                id: active.id.clone(),
                name: " Arena ".to_owned(),
            })
            .expect("rename");
        let renamed = editor
            .view(0)
            .session
            .active_preset
            .clone()
            .expect("active");
        assert_eq!(renamed.name, "Arena");
        editor
            .apply_alone(UiAction::DeletePreset { id: active.id })
            .expect("delete");
        assert_eq!(editor.view(0).session.active_preset, None);
    }

    #[test]
    fn a_loaded_preset_restores_its_session_as_one_undo_step() {
        let mut editor = editor();
        let receipt = editor
            .apply_alone(UiAction::LoadPreset {
                id: "factory:clean".to_owned(),
            })
            .expect("load");
        assert_eq!(receipt.effect, UiEffect::ChainChanged);
        let view = editor.view(0);
        assert!(!view.multi_core);
        assert_eq!(
            view.session.active_preset.as_ref().expect("active").name,
            "Loaded"
        );
        editor.apply_alone(UiAction::Undo).expect("undo");
        assert!(editor.view(0).multi_core);
    }

    /// Asserts that `view` shows each field of the current state of `editor`.
    fn assert_current(editor: &SessionEditor, view: &ChainViewState, step: &str) {
        let current = editor.current();
        assert_eq!(*view.chain, current.session.chain, "{step}: chain");
        assert_eq!(*view.session, current.session, "{step}: session");
        assert_eq!(view.revision, current.session.chain.revision, "{step}");
        assert_eq!(
            view.preparation,
            editor.preparation(),
            "{step}: preparation"
        );
        assert_eq!(view.can_undo, !editor.undo.is_empty(), "{step}: undo");
        assert_eq!(view.can_redo, !editor.redo.is_empty(), "{step}: redo");
        assert_eq!(view.multi_core, current.multi_core, "{step}: multicore");
        assert_eq!(
            view.nam_full_size, current.nam_full_size,
            "{step}: model size"
        );
    }

    #[test]
    fn each_view_of_an_unchanged_session_shares_one_snapshot() {
        let mut editor = editor();
        load(&mut editor);
        let first = editor.view(0);
        // A read of the editor is not a change.
        let _ = (editor.current(), editor.midi_map(), editor.can_paste());
        let _ = editor.parameter(ParameterId("outputLevel"));
        for known in [0, first.revision, u64::MAX] {
            let next = editor.view(known);
            assert!(Arc::ptr_eq(&first.chain, &next.chain));
            assert!(Arc::ptr_eq(&first.session, &next.session));
        }
    }

    #[test]
    fn a_kept_view_stays_as_it_was_after_a_change() {
        let mut editor = editor();
        let kept = editor.view(0);
        let slot = load(&mut editor);
        assert!(kept.chain.block(&slot).is_some_and(|block| block.is_insert));
        assert!(
            kept.session
                .chain
                .block(&slot)
                .is_some_and(|block| block.is_insert)
        );
        assert_eq!(kept.revision, 0);
        assert!(!kept.can_undo);
        let next = editor.view(0);
        assert!(!Arc::ptr_eq(&kept.chain, &next.chain));
        assert!(
            next.chain
                .block(&slot)
                .is_some_and(|block| !block.is_insert)
        );
    }

    #[test]
    fn changed_is_true_only_for_an_old_revision() {
        let mut editor = editor();
        let old = editor.view(0).revision;
        assert!(!editor.view(old).changed);
        write(&mut editor, 0.3, GesturePhase::End);
        let current = editor.view(old);
        assert!(current.changed);
        assert!(current.revision > old);
        // The shared snapshot does not hold the answer of an earlier caller.
        assert!(!editor.view(current.revision).changed);
        assert!(editor.view(old).changed);
        assert!(editor.view(u64::MAX).changed);
    }

    #[test]
    fn each_change_reaches_the_next_view() {
        type Step = (
            &'static str,
            fn(&mut SessionEditor),
            fn(&ChainViewState) -> bool,
        );
        fn apply(editor: &mut SessionEditor, action: UiAction) {
            editor.apply_alone(action).expect("accepted");
        }
        let steps: [Step; 16] = [
            (
                "a tone fills a slot",
                |editor| {
                    load(editor);
                },
                |view| !view.chain.left[0].is_insert,
            ),
            (
                "the first edit gives an undo step",
                |_| {},
                |view| view.can_undo && !view.can_redo,
            ),
            (
                "an undo gives a redo step",
                |editor| apply(editor, UiAction::Undo),
                |view| view.chain.left[0].is_insert && view.can_redo,
            ),
            (
                "a redo takes the redo step",
                |editor| apply(editor, UiAction::Redo),
                |view| !view.chain.left[0].is_insert && !view.can_redo,
            ),
            (
                "a parameter edit changes no chain field",
                |editor| {
                    write(editor, 0.2, GesturePhase::End);
                },
                |view| view.can_undo,
            ),
            (
                "multicore turns off",
                |editor| apply(editor, UiAction::SetMultiCore(false)),
                |view| !view.multi_core,
            ),
            (
                "full-size models turn on",
                |editor| apply(editor, UiAction::SetNamFullSize(true)),
                |view| view.nam_full_size,
            ),
            (
                "the input mode is session state",
                |editor| apply(editor, UiAction::SetInputMode(InputMode::Right)),
                |view| {
                    view.session.input_mode == InputMode::Right
                        && view.preparation.input_mode == InputMode::Right
                },
            ),
            (
                "a saved preset is active",
                |editor| {
                    apply(
                        editor,
                        UiAction::SavePreset {
                            name: "Lead".to_owned(),
                        },
                    );
                },
                |view| {
                    view.session
                        .active_preset
                        .as_ref()
                        .is_some_and(|p| p.name == "Lead")
                },
            ),
            (
                "a rename changes only the session",
                |editor| {
                    let id = editor
                        .view(0)
                        .session
                        .active_preset
                        .clone()
                        .expect("active")
                        .id;
                    apply(
                        editor,
                        UiAction::RenamePreset {
                            id,
                            name: "Solo".to_owned(),
                        },
                    );
                },
                |view| {
                    view.session
                        .active_preset
                        .as_ref()
                        .is_some_and(|p| p.name == "Solo")
                },
            ),
            (
                "a MIDI mapping is session state",
                |editor| {
                    apply(
                        editor,
                        UiAction::Midi(MidiAction::SetMapping {
                            target_id: "outputLevel".to_owned(),
                            source: MidiSource::Cc,
                            number: 7,
                        }),
                    );
                },
                |view| view.session.midi_mappings.len() == 1,
            ),
            (
                "a device change reaches the preparation",
                |editor| {
                    apply(
                        editor,
                        UiAction::Device(DeviceAction::Configure(
                            crate::backend::AudioDeviceConfig {
                                sample_rate: Some(96_000.0),
                                buffer_frames: Some(256),
                                ..Default::default()
                            },
                        )),
                    );
                },
                |view| view.preparation.host_sample_rate == 96_000.0,
            ),
            (
                "a stream format raises no revision and still shows",
                |editor| editor.set_stream_format(44_100.0, 512, true, false),
                |view| {
                    view.preparation.max_host_frames == 512
                        && view.chain.stereo_input
                        && !view.session.chain.stereo_output
                },
            ),
            (
                "a saved session replaces the session",
                |editor| {
                    let mut frame = editor.current().clone();
                    frame.session.editor_scale = 1.5;
                    frame.multi_core = true;
                    editor.load_saved_session(frame, 3);
                },
                |view| view.session.editor_scale == 1.5 && view.multi_core,
            ),
            (
                "a reset gives the default chain",
                |editor| apply(editor, UiAction::ResetToDefault),
                |view| view.chain.left.iter().all(|block| block.is_insert),
            ),
            (
                "a cancelled gesture restores the value",
                |editor| {
                    write(editor, 0.5, GesturePhase::Begin);
                    write(editor, 0.9, GesturePhase::Change);
                    write(editor, 0.9, GesturePhase::Cancel);
                },
                |view| view.can_undo,
            ),
        ];
        let mut editor = editor();
        for (step, change, shows) in steps {
            // Each step starts from a shared snapshot, so a stale one would show here.
            let before = editor.view(0);
            change(&mut editor);
            let after = editor.view(before.revision);
            assert_current(&editor, &after, step);
            assert!(shows(&after), "{step}");
            if after.revision != before.revision {
                assert!(after.changed, "{step}");
                assert!(!Arc::ptr_eq(&before.chain, &after.chain), "{step}");
            }
        }
    }

    /// Loads a tone into the open slot at `index` of the left lane.
    fn load_at(editor: &mut SessionEditor, index: usize, tone_id: i64) -> BlockId {
        let slot = editor.view(0).chain.left[index].id.clone();
        editor
            .apply_alone(UiAction::LoadTone {
                target_insert_id: slot.clone(),
                tone: tone(tone_id),
                model_id: ModelId(tone_id * 10),
            })
            .expect("load");
        slot
    }

    fn mix(editor: &SessionEditor, block: &BlockId) -> f64 {
        editor.view(0).chain.block(block).expect("block").params.mix
    }

    fn knob(
        editor: &mut SessionEditor,
        block: &BlockId,
        mix: f64,
        gesture: GesturePhase,
    ) -> Result<UiReceipt, UiError> {
        editor.apply_alone(UiAction::SetBlockParamsGesture {
            block_id: block.clone(),
            params: BlockParams {
                mix,
                ..Default::default()
            },
            gesture,
        })
    }

    #[test]
    fn a_removed_block_closes_its_gesture_and_its_end_changes_nothing() {
        let mut editor = editor();
        let first = load_at(&mut editor, 0, 1);
        let second = load_at(&mut editor, 1, 2);
        let start = mix(&editor, &second);
        knob(&mut editor, &first, start, GesturePhase::Begin).expect("begin");
        knob(&mut editor, &first, 0.1, GesturePhase::Change).expect("change");
        editor
            .apply_alone(UiAction::RemoveBlock(first.clone()))
            .expect("remove");
        assert!(editor.open_gesture.is_none());

        let before = editor.view(0);
        let steps = editor.undo.len();
        for phase in [GesturePhase::End, GesturePhase::Cancel] {
            let receipt = knob(&mut editor, &first, 0.2, phase).expect("an end for a lost block");
            assert_eq!(receipt.effect, UiEffect::None);
            assert_eq!(receipt.revision, before.revision);
            let eq = editor
                .apply_alone(UiAction::SetEq {
                    block_id: first.clone(),
                    params: EqParams::default(),
                    gesture: phase,
                })
                .expect("an EQ end for a lost block");
            assert_eq!(eq.effect, UiEffect::None);
        }
        assert_eq!(editor.view(0).chain, before.chain);
        assert_eq!(editor.undo.len(), steps);

        // Each write of the other block is a step of its own, not a part of the closed gesture.
        knob(&mut editor, &second, 0.2, GesturePhase::Change).expect("change");
        knob(&mut editor, &second, 0.3, GesturePhase::Change).expect("change");
        editor.apply_alone(UiAction::Undo).expect("undo");
        assert_eq!(mix(&editor, &second), 0.2);
        assert!(
            editor
                .view(0)
                .chain
                .block(&first)
                .is_none_or(|block| block.is_insert)
        );
    }

    #[test]
    fn an_eq_cancel_keeps_the_step_of_another_action() {
        let mut editor = editor();
        let block = load_at(&mut editor, 0, 1);
        let before = editor
            .view(0)
            .chain
            .block(&block)
            .expect("block")
            .params
            .eq
            .clone();
        let edit = EqParams {
            enabled: !before.enabled,
            ..before.clone()
        };
        let eq = |editor: &mut SessionEditor, gesture| {
            editor
                .apply_alone(UiAction::SetEq {
                    block_id: block.clone(),
                    params: edit.clone(),
                    gesture,
                })
                .expect("EQ edit");
        };
        eq(&mut editor, GesturePhase::Begin);
        editor
            .apply_alone(UiAction::SetMultiCore(false))
            .expect("multicore");
        eq(&mut editor, GesturePhase::Change);
        eq(&mut editor, GesturePhase::Cancel);
        let shown = |editor: &SessionEditor| {
            let view = editor.view(0);
            let eq = view.chain.block(&block).expect("block").params.eq.clone();
            (eq, view.multi_core)
        };
        assert_eq!(shown(&editor), (before.clone(), false));

        // Only the load and the multicore change are left, and neither shows the cancelled EQ.
        assert_eq!(editor.undo.len(), 2);
        editor.apply_alone(UiAction::Undo).expect("undo");
        assert_eq!(shown(&editor), (before, true));
    }

    #[test]
    fn an_eq_cancel_for_another_block_is_refused() {
        let mut editor = editor();
        let first = load_at(&mut editor, 0, 1);
        let second = load_at(&mut editor, 1, 2);
        let edit = EqParams {
            enabled: true,
            pre: true,
            bands: Vec::new(),
        };
        editor
            .apply_alone(UiAction::SetEq {
                block_id: first.clone(),
                params: edit.clone(),
                gesture: GesturePhase::Begin,
            })
            .expect("begin");
        let before = editor.view(0);
        let refused = editor.apply_alone(UiAction::SetEq {
            block_id: second,
            params: EqParams::default(),
            gesture: GesturePhase::Cancel,
        });
        assert!(matches!(refused, Err(UiError::InvalidAction(_))));
        assert_eq!(editor.view(0).revision, before.revision);
        assert_eq!(
            editor.view(0).chain.block(&first).expect("first").params.eq,
            edit
        );
        assert!(editor.open_gesture.is_some());
    }

    fn active_name(editor: &SessionEditor) -> Option<String> {
        editor
            .view(0)
            .session
            .active_preset
            .as_ref()
            .map(|active| active.name.clone())
    }

    #[test]
    fn an_undo_after_a_rename_shows_the_new_name() {
        let mut editor = editor();
        editor
            .apply_alone(UiAction::SavePreset {
                name: "Lead".to_owned(),
            })
            .expect("save");
        let id = editor
            .view(0)
            .session
            .active_preset
            .clone()
            .expect("active")
            .id;
        editor
            .apply_alone(UiAction::SetMultiCore(false))
            .expect("multicore");
        editor
            .apply_alone(UiAction::SetNamFullSize(true))
            .expect("model size");
        editor.apply_alone(UiAction::Undo).expect("undo");
        editor
            .apply_alone(UiAction::RenamePreset {
                id,
                name: "Solo".to_owned(),
            })
            .expect("rename");
        // The undo step is from before the rename, and the redo step is from before the undo.
        for action in [UiAction::Undo, UiAction::Redo, UiAction::Redo] {
            let shown = format!("{action:?}");
            editor.apply_alone(action).expect("history step");
            assert_eq!(active_name(&editor).as_deref(), Some("Solo"), "{shown}");
        }
    }

    #[test]
    fn no_undo_or_redo_makes_a_deleted_preset_active() {
        let mut editor = editor();
        editor
            .apply_alone(UiAction::SavePreset {
                name: "Lead".to_owned(),
            })
            .expect("save");
        let id = editor
            .view(0)
            .session
            .active_preset
            .clone()
            .expect("active")
            .id;
        editor
            .apply_alone(UiAction::SetMultiCore(false))
            .expect("multicore");
        editor
            .apply_alone(UiAction::SetNamFullSize(true))
            .expect("model size");
        editor.apply_alone(UiAction::Undo).expect("undo");
        editor
            .apply_alone(UiAction::DeletePreset { id })
            .expect("delete");
        assert_eq!(active_name(&editor), None);
        while editor.view(0).can_undo {
            editor.apply_alone(UiAction::Undo).expect("undo");
            assert_eq!(active_name(&editor), None);
        }
        while editor.view(0).can_redo {
            editor.apply_alone(UiAction::Redo).expect("redo");
            assert_eq!(active_name(&editor), None);
        }
    }
}

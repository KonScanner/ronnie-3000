//! Control-side processor state, presets, model bytes, and restore preparation.
//!
//! The session edit rules are in [`ronie_ui::session_edit`]. This module keeps the storage that
//! those rules need: the preset store, the model byte cache, the local stash, and the prepared
//! restore for the audio side.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use ronie_core::{
    BlockId, ByteCacheKey, ChainBlock, JuceTree, LocalStash, ModelByteCache, ModelBytes, ModelId,
    ModelNamespace, PresetInfo, PresetStore, Tone,
    params::{ParameterId, ParameterValue},
};
use ronie_dsp::PrepareSpec;
use ronie_ui::{
    backend::{ChainViewState, GesturePhase, MeterSnapshot, UiAction, UiEffect, UiError},
    session_edit::{SessionEditor, SessionFrame, SessionStorage, move_in_order},
};

use crate::audio_session::SessionRestore;

#[derive(Debug, Clone)]
struct PresetEntry {
    info: PresetInfo,
    /// The session document shares this tree after a load, so that a load copies no model.
    document: Arc<JuceTree>,
}

/// Everything that one chain preparation reads, copied from the session in one lock of the
/// processor, with the session state of that same lock.
///
/// The compile reads only this copy, so it holds no lock of the processor, and a frame does not
/// wait for it. Its chain always reproduces [`Self::state`], also when an edit and its undo come
/// during the compile.
pub struct SessionChainPlan {
    session: ronie_core::SessionSnapshot,
    preparation: PrepareSpec,
    generation: u64,
    stash: Option<Arc<LocalStash>>,
    document: Option<Arc<JuceTree>>,
    cache: ModelByteCache,
    state: crate::audio_session::AudioSessionState,
}

impl std::fmt::Debug for SessionChainPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionChainPlan")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl SessionChainPlan {
    /// The session state that the chain of this plan reproduces.
    #[must_use]
    pub const fn state(&self) -> &crate::audio_session::AudioSessionState {
        &self.state
    }

    /// Takes the session state that the chain of this plan reproduces.
    #[must_use]
    pub fn into_state(self) -> crate::audio_session::AudioSessionState {
        self.state
    }

    /// The generation that the chain of this plan gets.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Compiles the chain of this plan. This can be slow, and it takes no lock.
    ///
    /// A block with no usable model bytes runs as bypass and appears in
    /// [`crate::SessionChain::unavailable`].
    ///
    /// # Errors
    /// Returns a chain preparation error.
    pub fn prepare(&self) -> Result<crate::audio_session::SessionChain, UiError> {
        crate::persistence::prepare_session_chain(
            &self.session,
            self.preparation,
            self.generation,
            &crate::persistence::ModelSources {
                stash: self.stash.as_deref(),
                document: self.document.as_deref(),
                cache: &self.cache,
            },
        )
    }
}

/// The storage that the session edits of the processor use.
#[derive(Debug)]
struct ProcessorStorage {
    presets: BTreeMap<String, PresetEntry>,
    /// The order of the presets when there is no preset store. The store keeps its own order.
    preset_order: Vec<String>,
    preset_store: Option<Arc<PresetStore>>,
    session_document: Option<Arc<JuceTree>>,
    model_bytes: ModelByteCache,
    pending_restore: Option<SessionRestore>,
    prepare_generation: u64,
    stash: Option<Arc<LocalStash>>,
}

/// Owns control-side processor state and off-thread restore candidates, not the active graph.
#[derive(Debug)]
pub struct RonieProcessor {
    editor: SessionEditor,
    storage: ProcessorStorage,
}

impl RonieProcessor {
    /// Creates an empty processor with all catalog defaults and an immutable preparation contract.
    #[must_use]
    pub fn new(preparation: PrepareSpec) -> Self {
        Self {
            editor: SessionEditor::new(preparation),
            storage: ProcessorStorage {
                presets: BTreeMap::new(),
                preset_order: Vec::new(),
                preset_store: None,
                session_document: None,
                model_bytes: ModelByteCache::new(256 * 1024 * 1024),
                pending_restore: None,
                prepare_generation: 1,
                stash: LocalStash::new(std::env::temp_dir().join("ronie-stash"))
                    .ok()
                    .map(Arc::new),
            },
        }
    }

    /// Creates a processor with an explicit local stash root.
    #[must_use]
    pub fn with_stash(preparation: PrepareSpec, stash: LocalStash) -> Self {
        let mut processor = Self::new(preparation);
        processor.storage.stash = Some(Arc::new(stash));
        processor
    }

    /// Uses the supplied file-backed store for subsequent preset actions.
    #[must_use]
    pub fn with_preset_store(mut self, store: Arc<PresetStore>) -> Self {
        self.storage.preset_store = Some(store);
        self
    }

    /// Removes each undo and redo step. The session does not change.
    pub(crate) fn clear_history(&mut self) {
        self.editor.clear_history();
    }

    pub(crate) fn take_restore(&mut self) -> Option<SessionRestore> {
        self.storage.pending_restore.take()
    }

    pub(crate) fn set_stream_format(&mut self, format: crate::audio_session::StreamFormat) {
        self.editor.set_stream_format(
            format.sample_rate,
            format.max_frames,
            format.stereo_input,
            format.stereo_output,
        );
        self.editor.set_oversampling(format.oversampling);
    }

    pub(crate) fn audio_state(&self) -> crate::audio_session::AudioSessionState {
        let current = self.editor.current();
        crate::audio_session::AudioSessionState::capture(
            &current.session,
            &current.parameters,
            self.editor.preparation().stereo_output,
        )
    }

    /// Reserves a generation and copies what one chain preparation reads, with the session state
    /// that the chain reproduces.
    pub(crate) fn session_chain_plan(&mut self) -> SessionChainPlan {
        let state = self.audio_state();
        let storage = &mut self.storage;
        storage.prepare_generation += 1;
        SessionChainPlan {
            session: self.editor.current().session.clone(),
            preparation: self.editor.preparation(),
            generation: storage.prepare_generation,
            stash: storage.stash.clone(),
            document: storage.session_document.clone(),
            cache: storage.model_bytes.clone(),
            state,
        }
    }

    /// Returns an immutable host-neutral chain view.
    #[must_use]
    pub fn chain_state(&self, known_revision: u64) -> ChainViewState {
        self.editor.view(known_revision)
    }

    /// Returns a coherent meter snapshot without touching real-time state.
    #[must_use]
    pub fn meter_levels(&self) -> MeterSnapshot {
        MeterSnapshot::default()
    }

    /// Returns shared MIDI mapping and learn state.
    #[must_use]
    pub fn midi_map(&self) -> ronie_ui::default::MidiMapSnapshot {
        self.editor.midi_map()
    }

    /// Returns the UI-only calibration draft.
    #[must_use]
    pub fn calibration_draft(&self) -> Option<String> {
        self.editor.calibration_draft()
    }

    /// Reads one Boolean UI preference.
    #[must_use]
    pub fn preference(&self, key: &str) -> Option<bool> {
        self.editor.preference(key)
    }

    /// Reads one text UI preference.
    #[must_use]
    pub fn text_preference(&self, key: &str) -> Option<String> {
        self.editor.text_preference(key).map(str::to_owned)
    }

    /// Keeps a stored text UI preference as the start state. The revision does not change.
    pub fn seed_text_preference(&mut self, key: &str, value: &str) {
        self.editor.seed_text_preference(key, value);
    }

    /// Reads one parameter or returns `None` for an unknown ID.
    #[must_use]
    pub fn parameter(&self, id: ParameterId) -> Option<ParameterValue> {
        self.editor.parameter(id)
    }

    /// Whether the block clipboard contains a tone.
    #[must_use]
    pub fn can_paste_block(&self) -> bool {
        self.editor.can_paste()
    }

    /// Validates and writes one parameter. With no open gesture, this is one undo step.
    ///
    /// # Errors
    /// Returns an unknown-parameter or non-finite-value error.
    pub fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), UiError> {
        self.edit_parameter(id, value, GesturePhase::Change)
    }

    /// Validates and writes one parameter, inside a gesture or as one undo step of its own.
    ///
    /// See [`UiAction::SetParameter`] for the gesture rules.
    ///
    /// # Errors
    /// Returns an unknown-parameter or non-finite-value error, or an invalid-action error for a
    /// cancel with no open gesture.
    pub fn edit_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<(), UiError> {
        self.dispatch(UiAction::SetParameter { id, value, gesture })
            .map(|_| ())
    }

    /// Executes one typed action.
    ///
    /// # Errors
    /// Returns an invalid action or missing target error.
    pub fn dispatch(&mut self, action: UiAction) -> Result<(u64, UiEffect), UiError> {
        let receipt = self.editor.apply(action, &mut self.storage)?;
        Ok((receipt.revision, receipt.effect))
    }

    pub(crate) fn session_bytes(&self) -> Result<Vec<u8>, UiError> {
        crate::persistence::encode_session(
            self.editor.current(),
            self.editor.midi_map().channel,
            self.storage.stash.as_deref(),
            self.storage.session_document.as_deref(),
            &self.storage.model_bytes,
        )
    }

    pub(crate) fn restore_session_bytes(&mut self, bytes: &[u8]) -> Result<(), UiError> {
        let decoded = crate::persistence::decode_session(bytes, self.editor.current())?;
        let storage = &mut self.storage;
        // A tone without usable bytes runs as bypass in a live session, so it must not stop the
        // next launch. The audio engine prepares that session from its own sources, and the
        // document keeps the bytes for the next save.
        let change = if crate::persistence::active_model_lacks_bytes(&decoded.document) {
            for entry in decoded.document.model_cache.iter() {
                let key = ByteCacheKey {
                    block_id: entry.block_id.clone(),
                    model_id: entry.model_id,
                    namespace: entry.namespace,
                };
                // A refused entry is larger than the cache. The document still holds its bytes.
                let _ = storage.model_bytes.insert(key, entry.bytes.clone());
            }
            None
        } else {
            crate::persistence::prepare_session_restore(
                &decoded.document,
                self.editor.current(),
                self.editor.preparation(),
                storage.prepare_generation + 1,
                &storage.model_bytes,
            )?
        };
        storage.prepare_generation += 1;
        storage.session_document = Some(Arc::new(decoded.document.raw));
        storage.pending_restore = change.map(|change| SessionRestore::Prepared(Box::new(change)));
        self.editor
            .load_saved_session(decoded.frame, decoded.midi_channel);
        Ok(())
    }
}

impl SessionStorage for ProcessorStorage {
    fn keep_model_bytes(
        &mut self,
        block_id: &BlockId,
        model_id: ModelId,
        bytes: ModelBytes,
    ) -> Result<(), UiError> {
        self.model_bytes
            .insert(
                ByteCacheKey {
                    block_id: block_id.clone(),
                    model_id,
                    namespace: ModelNamespace::Catalog,
                },
                bytes,
            )
            .map_err(|error| UiError::Control(error.to_string()))
    }

    /// Bytes that came from a preset or a session can exist only in this cache, which is keyed
    /// by block.
    fn share_model_bytes(&mut self, source: &ChainBlock, copy: &BlockId) {
        let Some(tone) = &source.tone else {
            return;
        };
        for model in &tone.models {
            let namespace = ronie_core::model_namespace(source, model.id);
            let key = |block_id: &BlockId| ByteCacheKey {
                block_id: block_id.clone(),
                model_id: model.id,
                namespace,
            };
            if let Some(bytes) = self.model_bytes.get(&key(&source.id)) {
                // Only an entry larger than the whole cache is refused, and this one is in it.
                let _ = self.model_bytes.insert(key(copy), bytes);
            }
        }
    }

    fn load_local(&mut self, path: &Path) -> Result<Tone, UiError> {
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
        current: &SessionFrame,
    ) -> Result<PresetInfo, UiError> {
        let name = proposed.name.as_str();
        let mut previous = self
            .presets
            .values()
            .find(|entry| entry.info.name.eq_ignore_ascii_case(name))
            .map(|entry| Arc::clone(&entry.document));
        if previous.is_none()
            && let Some(store) = &self.preset_store
        {
            let existing = store
                .list()
                .map_err(control)?
                .into_iter()
                .find(|info| !info.factory && info.name.eq_ignore_ascii_case(name));
            if let Some(existing) = existing {
                previous = Some(Arc::new(store.load(&existing.id).map_err(control)?));
            }
        }
        let mut document = crate::persistence::encode_preset(
            current,
            self.stash.as_deref(),
            previous.as_deref(),
            &self.model_bytes,
        )?;
        let info = match &self.preset_store {
            Some(store) => store.save(name, document.clone()).map_err(control)?,
            None => {
                if !self.preset_order.contains(&proposed.id) {
                    self.preset_order.push(proposed.id.clone());
                }
                proposed
            }
        };
        crate::persistence::finalize_preset(&mut document, &info.id, &info.name);
        self.presets.insert(
            info.id.clone(),
            PresetEntry {
                info: info.clone(),
                document: Arc::new(document),
            },
        );
        Ok(info)
    }

    /// Decodes and checks the preset, and keeps its embedded models as model sources. This
    /// compiles no model: a compile is slow, and the caller can hold the locks of a frame. The
    /// audio side compiles the new chain from those sources, and a model that cannot compile
    /// runs as bypass with its fault.
    fn load_preset(
        &mut self,
        id: &str,
        current: &SessionFrame,
        _preparation: PrepareSpec,
    ) -> Result<(PresetInfo, SessionFrame), UiError> {
        let (info, document) = if let Some(entry) = self.presets.get(id) {
            (entry.info.clone(), Arc::clone(&entry.document))
        } else {
            let store = self
                .preset_store
                .as_ref()
                .ok_or_else(|| UiError::UnknownTarget(id.to_owned()))?;
            let document = store.load(id).map_err(control)?;
            let info = store
                .list()
                .map_err(control)?
                .into_iter()
                .find(|info| info.id == id)
                .ok_or_else(|| UiError::UnknownTarget(id.to_owned()))?;
            (info, Arc::new(document))
        };
        let decoded = crate::persistence::decode_preset(&document, current)?;
        crate::persistence::check_restore(&decoded.document, current, &self.model_bytes)?;
        // The live chain reads the embedded models from this document and from the byte cache.
        let document = Arc::new(decoded.document.raw);
        self.session_document = Some(Arc::clone(&document));
        self.presets.insert(
            id.to_owned(),
            PresetEntry {
                info: info.clone(),
                document,
            },
        );
        self.pending_restore = Some(SessionRestore::Compile);
        Ok((info, decoded.frame))
    }

    fn rename_preset(&mut self, id: &str, name: &str) -> Result<(), UiError> {
        if let Some(store) = &self.preset_store {
            if !store.rename(id, name).map_err(control)? {
                return Err(UiError::UnknownTarget(id.to_owned()));
            }
            if let Some(entry) = self.presets.get_mut(id) {
                name.clone_into(&mut entry.info.name);
                crate::persistence::finalize_preset(Arc::make_mut(&mut entry.document), id, name);
            }
        } else {
            let entry = self
                .presets
                .get_mut(id)
                .ok_or_else(|| UiError::UnknownTarget(id.to_owned()))?;
            name.clone_into(&mut entry.info.name);
        }
        Ok(())
    }

    fn delete_preset(&mut self, id: &str) -> Result<(), UiError> {
        if let Some(store) = &self.preset_store {
            if !store.remove(id).map_err(control)? {
                return Err(UiError::UnknownTarget(id.to_owned()));
            }
            self.presets.remove(id);
        } else if self.presets.remove(id).is_none() {
            return Err(UiError::UnknownTarget(id.to_owned()));
        } else {
            self.preset_order.retain(|known| known != id);
        }
        Ok(())
    }

    fn move_preset(&mut self, id: &str, delta: i32) -> Result<bool, UiError> {
        let Some(store) = &self.preset_store else {
            return move_in_order(&mut self.preset_order, id, delta)
                .ok_or_else(|| UiError::UnknownTarget(id.to_owned()));
        };
        if !store
            .list()
            .map_err(control)?
            .iter()
            .any(|info| info.id == id)
        {
            return Err(UiError::UnknownTarget(id.to_owned()));
        }
        store.move_by(id, delta).map_err(control)
    }
}

fn control(error: impl std::fmt::Display) -> UiError {
    UiError::Control(error.to_string())
}

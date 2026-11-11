//! Catalog/local load targeting and generation-safe completion.

use std::sync::{Arc, Mutex, MutexGuard};

use ronie_core::{BlockId, ChainSide, ModelBytes, ModelId, Tone};

use crate::backend::{UiAction, UiBackend, UiError};

/// Load lifecycle visible to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPhase {
    /// No active load.
    Idle,
    /// Background acquisition or preparation is active.
    Loading,
    /// A tone landed in the chain.
    Ready,
    /// Acquisition failed and can be retried.
    Failed,
    /// The load was cancelled.
    Cancelled,
}

/// Insert or swap target remembered while browsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadTarget {
    /// Destination lane.
    pub side: ChainSide,
    /// Insert or tone block identity.
    pub block_id: String,
}

/// The one current generation of a flow.
///
/// Each cancellation and each publication takes this lock, so a cancellation that returned cannot
/// be followed by a publication of an older generation. The lock is never held across a compile.
#[derive(Clone, Default)]
struct LoadGate(Arc<Mutex<u64>>);

impl LoadGate {
    fn lock(&self) -> MutexGuard<'_, u64> {
        // The value is one number, so a panic cannot leave it partly written.
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn current(&self) -> u64 {
        *self.lock()
    }

    fn advance(&self) -> u64 {
        let mut current = self.lock();
        *current += 1;
        *current
    }

    /// Ends `generation`. This does nothing when a newer generation is current.
    fn revoke(&self, generation: u64) {
        let mut current = self.lock();
        if *current == generation {
            *current += 1;
        }
    }
}

/// The right of one background load to publish, while its generation stays current.
#[derive(Clone)]
pub struct LoadTicket {
    generation: u64,
    gate: LoadGate,
    backend: Arc<dyn UiBackend>,
    target: LoadTarget,
}

impl std::fmt::Debug for LoadTicket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadTicket")
            .field("generation", &self.generation)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl LoadTicket {
    /// Generation that this ticket can publish.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Validates, publishes, and prepares one downloaded model on the calling thread.
    ///
    /// Returns whether the load reached the backend. False means that a cancellation or a newer
    /// load came first, and no byte reached the backend. A compile takes time, so this must not
    /// run on the UI thread or the audio thread.
    ///
    /// # Errors
    /// Returns an error when the bytes cannot become a model or when the backend refuses them.
    /// The chain stays as it was.
    pub fn publish(
        &self,
        tone: Tone,
        model_id: ModelId,
        bytes: ModelBytes,
    ) -> Result<bool, UiError> {
        publish_model(
            &self.gate,
            self.generation,
            self.backend.as_ref(),
            Downloaded {
                tone,
                model_id,
                bytes,
            },
            |downloaded| {
                let target_insert_id = resolve_target(self.backend.as_ref(), &self.target)
                    .ok_or_else(|| UiError::UnknownTarget(self.target.block_id.clone()))?;
                Ok(Some(UiAction::LoadToneWithBytes {
                    target_insert_id,
                    tone: downloaded.tone,
                    model_id: downloaded.model_id,
                    bytes: downloaded.bytes,
                }))
            },
        )
    }

    /// Stops this ticket. This does nothing when the flow has already moved on.
    pub fn revoke(&self) {
        self.gate.revoke(self.generation);
    }
}

/// One downloaded model on its way to the backend.
struct Downloaded {
    tone: Tone,
    model_id: ModelId,
    bytes: ModelBytes,
}

/// Publishes one downloaded model in three steps.
///
/// The compile that validates the bytes holds no lock. The check of the generation and the
/// session change are then one step for every cancellation, so a late completion cannot pass the
/// check. `action` runs in that step, and `None` from it stops the publication. The dispatch does
/// not compile, so a frame that cancels waits only for a short lock. The audio chain follows with
/// no lock.
fn publish_model(
    gate: &LoadGate,
    generation: u64,
    backend: &dyn UiBackend,
    downloaded: Downloaded,
    action: impl FnOnce(Downloaded) -> Result<Option<UiAction>, UiError>,
) -> Result<bool, UiError> {
    if gate.current() != generation {
        return Ok(false);
    }
    backend.validate_model(&downloaded.tone, downloaded.model_id, &downloaded.bytes)?;
    {
        let current = gate.lock();
        if *current != generation {
            return Ok(false);
        }
        let Some(action) = action(downloaded)? else {
            return Ok(false);
        };
        backend.dispatch(action)?;
    }
    backend.settle_audio()?;
    Ok(true)
}

/// The remembered block, or the first open insert of its lane when the block is gone.
fn resolve_target(backend: &dyn UiBackend, target: &LoadTarget) -> Option<BlockId> {
    let chain = backend.chain_state(0).chain;
    let remembered = BlockId(target.block_id.clone());
    if chain.block(&remembered).is_some() {
        return Some(remembered);
    }
    let lane = match target.side {
        ChainSide::Left => &chain.left,
        ChainSide::Right => &chain.right,
    };
    lane.iter()
        .find(|block| block.is_insert)
        .map(|insert| insert.id.clone())
}

/// The one current generation of the model switches of one editor.
///
/// Clones share the generation. All block details of a controller use one flow, so that the
/// generation of a closed detail cannot match a switch of the next detail.
#[derive(Clone)]
pub struct ModelSwitchFlow {
    gate: LoadGate,
    backend: Arc<dyn UiBackend>,
}

impl ModelSwitchFlow {
    /// Creates a flow with no switch.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>) -> Self {
        Self {
            gate: LoadGate::default(),
            backend,
        }
    }

    /// Starts a switch of `block_id` away from `replaces`, which ends each older switch.
    #[must_use]
    pub fn begin(&self, block_id: BlockId, replaces: Option<ModelId>) -> ModelSwitchTicket {
        ModelSwitchTicket {
            generation: self.gate.advance(),
            gate: self.gate.clone(),
            backend: self.backend.clone(),
            block_id,
            replaces,
        }
    }
}

/// The right of one background switch to publish, while its generation stays current and the
/// block stays as the switch found it.
#[derive(Clone)]
pub struct ModelSwitchTicket {
    generation: u64,
    gate: LoadGate,
    backend: Arc<dyn UiBackend>,
    block_id: BlockId,
    replaces: Option<ModelId>,
}

impl std::fmt::Debug for ModelSwitchTicket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelSwitchTicket")
            .field("generation", &self.generation)
            .field("block_id", &self.block_id)
            .field("replaces", &self.replaces)
            .finish_non_exhaustive()
    }
}

impl ModelSwitchTicket {
    /// Generation that this ticket can publish.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Active model when the switch started.
    #[must_use]
    pub const fn replaces(&self) -> Option<ModelId> {
        self.replaces
    }

    /// Validates, publishes, and prepares one downloaded model on the calling thread.
    ///
    /// Returns whether the switch reached the backend. False means that a cancellation, a newer
    /// switch, or a change of the block came first, and no byte reached the backend. A compile
    /// takes time, so this must not run on the UI thread or the audio thread.
    ///
    /// # Errors
    /// Returns an error when the bytes cannot become a model or when the backend refuses them.
    /// The chain stays as it was.
    pub fn publish(
        &self,
        tone: Tone,
        model_id: ModelId,
        bytes: ModelBytes,
    ) -> Result<bool, UiError> {
        publish_model(
            &self.gate,
            self.generation,
            self.backend.as_ref(),
            Downloaded {
                tone,
                model_id,
                bytes,
            },
            |downloaded| {
                let Downloaded {
                    tone,
                    model_id,
                    bytes,
                } = downloaded;
                Ok(self
                    .holds_block(&tone)
                    .then(|| UiAction::SelectModelWithBytes {
                        block_id: self.block_id.clone(),
                        tone,
                        model_id,
                        replaces: self.replaces,
                        bytes,
                    }))
            },
        )
    }

    /// Stops this ticket. This does nothing when the flow has already moved on.
    pub fn revoke(&self) {
        self.gate.revoke(self.generation);
    }

    fn holds_block(&self, tone: &Tone) -> bool {
        self.backend
            .chain_state(0)
            .chain
            .block(&self.block_id)
            .is_some_and(|block| {
                block.active_model_id == self.replaces
                    && block
                        .tone
                        .as_ref()
                        .is_some_and(|held| held.id == tone.id && !held.local)
            })
    }
}

/// Generation-safe load-flow controller.
pub struct LoadFlow {
    backend: Arc<dyn UiBackend>,
    pending: Option<LoadTarget>,
    phase: LoadPhase,
    gate: LoadGate,
    closed: bool,
    browser_open: bool,
    offline: bool,
    last_tone: Option<Tone>,
}

impl LoadFlow {
    /// Creates an idle load flow.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>) -> Self {
        Self {
            backend,
            pending: None,
            phase: LoadPhase::Idle,
            gate: LoadGate::default(),
            closed: false,
            browser_open: false,
            offline: false,
            last_tone: None,
        }
    }

    /// Remembers an insert or swap target and opens tone selection.
    pub fn select(&mut self, side: ChainSide, target_block_id: &str) {
        // A load in progress belongs to the target that it started for.
        if self.phase == LoadPhase::Loading {
            self.gate.advance();
        }
        self.pending = Some(LoadTarget {
            side,
            block_id: target_block_id.to_owned(),
        });
        self.browser_open = true;
        self.closed = false;
        self.phase = LoadPhase::Idle;
    }

    /// Pending target.
    #[must_use]
    pub const fn pending_target(&self) -> Option<&LoadTarget> {
        self.pending.as_ref()
    }

    /// Current phase.
    #[must_use]
    pub const fn phase(&self) -> LoadPhase {
        self.phase
    }

    /// Tone currently being acquired, if a load is active.
    #[must_use]
    pub fn loading_tone_id(&self) -> Option<ronie_core::ToneId> {
        (self.phase == LoadPhase::Loading)
            .then(|| self.last_tone.as_ref().map(|tone| tone.id))
            .flatten()
    }

    /// Current generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.gate.current()
    }

    /// Whether tone selection is open.
    #[must_use]
    pub const fn browser_open(&self) -> bool {
        self.browser_open
    }

    /// Simulates or records connectivity for this flow.
    pub fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    /// Whether the flow is offline.
    #[must_use]
    pub const fn offline(&self) -> bool {
        self.offline
    }

    /// Starts a load and returns its completion generation.
    ///
    /// # Errors
    /// Returns an invalid-action error for a missing target or empty model list.
    pub fn begin_load(&mut self, tone: Tone) -> Result<u64, UiError> {
        if self.pending.is_none() {
            return Err(UiError::InvalidAction(
                "no load target is selected".to_owned(),
            ));
        }
        if tone.models.is_empty() {
            return Err(UiError::InvalidAction("tone has no models".to_owned()));
        }
        self.closed = false;
        let generation = self.gate.advance();
        self.last_tone = Some(tone);
        self.phase = LoadPhase::Loading;
        Ok(generation)
    }

    /// Starts a load that a background worker completes, and returns its ticket.
    ///
    /// A catalog row can have no models, because the worker asks for the model list.
    ///
    /// # Errors
    /// Returns an invalid-action error for a missing target.
    pub fn begin_remote_load(&mut self, tone: Tone) -> Result<LoadTicket, UiError> {
        let target = self
            .pending
            .clone()
            .ok_or_else(|| UiError::InvalidAction("no load target is selected".to_owned()))?;
        self.closed = false;
        let generation = self.gate.advance();
        self.last_tone = Some(tone);
        self.phase = LoadPhase::Loading;
        Ok(LoadTicket {
            generation,
            gate: self.gate.clone(),
            backend: self.backend.clone(),
            target,
        })
    }

    /// Starts the last failed background load again with a fresh generation.
    ///
    /// # Errors
    /// Returns an invalid-action error when there is no failed load to retry.
    pub fn retry_remote(&mut self) -> Result<(LoadTicket, Tone), UiError> {
        let tone = self
            .last_tone
            .clone()
            .filter(|_| self.phase == LoadPhase::Failed)
            .ok_or_else(|| UiError::InvalidAction("no failed load to retry".to_owned()))?;
        Ok((self.begin_remote_load(tone.clone())?, tone))
    }

    /// Ends a background load that published. Returns false for a generation that is not current.
    pub fn finish_remote(&mut self, generation: u64) -> bool {
        if self.closed || generation != self.gate.current() {
            return false;
        }
        self.phase = LoadPhase::Ready;
        self.browser_open = false;
        self.pending = None;
        true
    }

    /// Records a failed load, which can retry. Returns false for a generation that is not current.
    pub fn fail_load(&mut self, generation: u64) -> bool {
        if self.closed || generation != self.gate.current() {
            return false;
        }
        self.phase = LoadPhase::Failed;
        true
    }

    /// Stops the active load and keeps the target and the open browser, for a session change.
    pub fn abandon(&mut self) {
        self.gate.advance();
        if matches!(self.phase, LoadPhase::Loading | LoadPhase::Failed) {
            self.phase = LoadPhase::Idle;
        }
    }

    /// Completes an acquisition only when it is still current.
    pub fn complete_load(&mut self, generation: u64, result: Result<Tone, UiError>) -> bool {
        if self.closed || generation != self.gate.current() {
            return false;
        }
        match result {
            Ok(tone) => self.land(tone),
            Err(_) => {
                self.phase = LoadPhase::Failed;
                true
            }
        }
    }

    /// Completes a cache-first load while offline.
    pub fn complete_cached(&mut self, generation: u64, tone: Tone) -> bool {
        if self.closed || generation != self.gate.current() {
            return false;
        }
        self.land(tone)
    }

    /// Cancels the active generation without mutating the chain.
    pub fn cancel(&mut self) {
        self.gate.advance();
        self.phase = LoadPhase::Cancelled;
        self.browser_open = false;
    }

    /// Marks token refresh before a retry.
    pub fn refresh(&mut self) -> bool {
        if self.phase != LoadPhase::Failed {
            return false;
        }
        self.phase = LoadPhase::Loading;
        true
    }

    /// Retries the last failed tone with a fresh generation.
    ///
    /// # Errors
    /// Returns an invalid-action error when there is no failed load to retry.
    pub fn retry(&mut self) -> Result<u64, UiError> {
        if self.phase != LoadPhase::Failed && self.phase != LoadPhase::Loading {
            return Err(UiError::InvalidAction("no failed load to retry".to_owned()));
        }
        let tone = self
            .last_tone
            .clone()
            .ok_or_else(|| UiError::InvalidAction("no failed load to retry".to_owned()))?;
        self.begin_load(tone)
    }

    /// Closes the flow, forgets its target, and rejects every late callback.
    pub fn close(&mut self) {
        self.closed = true;
        self.browser_open = false;
        self.gate.advance();
        self.pending = None;
        self.phase = LoadPhase::Cancelled;
    }

    fn land(&mut self, tone: Tone) -> bool {
        let Some(target) = self.pending.clone() else {
            self.phase = LoadPhase::Failed;
            return false;
        };
        let model_id = tone.models[0].id;
        let Some(target_insert_id) = resolve_target(self.backend.as_ref(), &target) else {
            self.phase = LoadPhase::Failed;
            return false;
        };
        let result = self.backend.dispatch(UiAction::LoadTone {
            target_insert_id,
            tone,
            model_id,
        });
        if result.is_err() {
            self.phase = LoadPhase::Failed;
            return false;
        }
        self.phase = LoadPhase::Ready;
        self.browser_open = false;
        self.pending = None;
        true
    }
}

/// A closed editor must win over a compile that an abort cannot stop.
impl Drop for LoadFlow {
    fn drop(&mut self) {
        self.gate.advance();
    }
}

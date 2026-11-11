//! Block detail takeover, model selection, metadata, and EQ navigation.

use std::sync::Arc;

use eframe::egui::{self, Color32, Rect, Sense, TextureHandle};
use ronie_core::{BlockId, BlockParams, ChainBlock, ModelId, ModelKind, ToneId, ToneModel};

use crate::{
    backend::{GesturePhase, UiAction, UiBackend, UiError},
    services::{
        community_worker::{ModelList, ModelSwitchRequest, ModelsRequest},
        links::{is_web_url, open_web_url},
        load_flow::{ModelSwitchFlow, ModelSwitchTicket},
        session::SessionStatus,
    },
    widgets::{
        KnobEdit, KnobSpec,
        knob::{OpenDrag, Tracked},
    },
};

use super::{
    block_info::{BlockInfoPanel, InfoAction},
    chain::TileVisualState,
    eq::{EqEditor, EqViewMode},
    model_select::{ModelOption, ModelSelect},
    tile::{draw_gear, gear_glyph},
};

/// Detail body selected by the card header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockView {
    /// Tone artwork and model controls.
    Tone,
    /// Six-band EQ editor.
    Eq,
    /// Expanded metadata.
    Info,
}

/// Complete block detail controller.
pub struct BlockDetail {
    backend: Arc<dyn UiBackend>,
    block_id: BlockId,
    block: ChainBlock,
    calibrate_input: bool,
    view: BlockView,
    models: ModelSelect,
    info: BlockInfoPanel,
    eq: EqEditor,
    closed: bool,
    swap_requested: Option<BlockId>,
    sign_in_requested: bool,
    artwork: Option<TextureHandle>,
    avatar: Option<TextureHandle>,
    /// Model work over a community service. A detail with no service has none.
    remote: Option<RemoteModels>,
    /// The trim knob whose drag has an open gesture on the backend.
    trim_drag: OpenDrag<&'static str>,
    /// The chain revision of the last copy of the block, or `None` after a local change of the
    /// copy. A frame copies the block again only when this is not the current revision.
    synced_revision: Option<u64>,
}

/// The model list and the switch of one detail.
struct RemoteModels {
    flow: ModelSwitchFlow,
    /// Rows of the newest list. A switch needs the address of its model.
    rows: Vec<ToneModel>,
    list_due: bool,
    unsent: Option<ModelSwitchRequest>,
    pending: Option<PendingSwitch>,
}

struct PendingSwitch {
    ticket: ModelSwitchTicket,
    model: ModelId,
}

impl Drop for BlockDetail {
    /// A closed detail must win over a compile that an abort cannot stop, and a trim or EQ drag
    /// must not keep its gesture open.
    fn drop(&mut self) {
        self.revoke_switch();
        if self.trim_drag.take().is_some() {
            self.write_params_in_gesture(GesturePhase::End);
        }
        self.eq.end_open_drag();
    }
}

impl BlockDetail {
    /// Returns true one time after the user asks to sign in from this page.
    pub fn take_sign_in_request(&mut self) -> bool {
        std::mem::take(&mut self.sign_in_requested)
    }

    /// Creates a detail controller for one block.
    ///
    /// # Errors
    /// Returns an unknown-target error when the block does not exist.
    pub fn new(
        backend: Arc<dyn UiBackend>,
        block_id: &str,
        calibrate_input: bool,
    ) -> Result<Self, UiError> {
        let id = BlockId(block_id.to_owned());
        let block = backend
            .chain_state(0)
            .chain
            .block(&id)
            .cloned()
            .ok_or_else(|| UiError::UnknownTarget(block_id.to_owned()))?;
        let models = model_select_for(&block);
        let eq = EqEditor::new(backend.clone(), block_id)?;
        let mut info = BlockInfoPanel::default();
        info.set_tone(block.tone.clone());
        info.set_page_url(tone_url(&block));
        Ok(Self {
            backend,
            block_id: id,
            block,
            calibrate_input,
            view: BlockView::Tone,
            models,
            info,
            eq,
            closed: false,
            swap_requested: None,
            sign_in_requested: false,
            artwork: None,
            avatar: None,
            remote: None,
            trim_drag: OpenDrag::default(),
            synced_revision: None,
        })
    }

    /// Current block snapshot.
    #[must_use]
    pub const fn block(&self) -> &ChainBlock {
        &self.block
    }

    /// Synchronizes native state and closes when the block vanishes.
    pub fn sync(&mut self) {
        let view = self.backend.chain_state(0);
        self.sync_from(&view);
    }

    /// Synchronizes native state only when the chain changed since the last copy.
    ///
    /// Each change of the chain gets a new revision, so a still frame makes no copy of the block
    /// or of its EQ. A local change of the copy, and an EQ that does not show the EQ of the
    /// chain, also make a copy, so that the next frame shows the value that the backend holds.
    fn sync_on_change(&mut self) {
        let view = self.backend.chain_state(0);
        let current = self.synced_revision == Some(view.revision)
            && view
                .chain
                .block(&self.block_id)
                .is_some_and(|block| block.params.eq == *self.eq.params());
        if !current {
            self.sync_from(&view);
        }
    }

    fn sync_from(&mut self, view: &crate::backend::ChainViewState) {
        let Some(block) = view.chain.block(&self.block_id).cloned() else {
            self.revoke_switch();
            self.closed = true;
            return;
        };
        self.synced_revision = Some(view.revision);
        self.eq.sync_params(block.params.eq.clone());
        let replaced = tone_identity(&block) != tone_identity(&self.block);
        self.block = block;
        self.reconcile_model_work(replaced);
    }

    /// Whether this detail should return to the gallery.
    #[must_use]
    pub const fn closed(&self) -> bool {
        self.closed
    }

    /// Requests a return to the gallery without changing the chain.
    pub fn close(&mut self) {
        self.revoke_switch();
        self.closed = true;
        self.eq.set_spectrum_visible(false);
    }

    /// Current body view.
    #[must_use]
    pub const fn view(&self) -> BlockView {
        self.view
    }

    /// Changes body view and enables analysis only for EQ.
    pub fn set_view(&mut self, view: BlockView) {
        self.view = view;
        self.eq.set_spectrum_visible(view == BlockView::Eq);
        if view == BlockView::Info {
            self.info.set_tone(self.block.tone.clone());
            self.info.set_page_url(self.share_url());
        }
    }

    /// Model picker state.
    #[must_use]
    pub const fn model_select(&self) -> &ModelSelect {
        &self.models
    }

    /// Mutable model picker state.
    pub fn model_select_mut(&mut self) -> &mut ModelSelect {
        &mut self.models
    }

    /// Metadata panel state.
    #[must_use]
    pub const fn info(&self) -> &BlockInfoPanel {
        &self.info
    }

    /// Mutable metadata panel state.
    pub fn info_mut(&mut self) -> &mut BlockInfoPanel {
        &mut self.info
    }

    /// EQ controller.
    #[must_use]
    pub const fn eq(&self) -> &EqEditor {
        &self.eq
    }

    /// Mutable EQ controller.
    pub fn eq_mut(&mut self) -> &mut EqEditor {
        &mut self.eq
    }

    /// Selects a model and writes the active model to the backend.
    ///
    /// # Errors
    /// Returns a signed-out, unknown-model, or backend error.
    pub fn select_model(&mut self, id: ModelId, session: SessionStatus) -> Result<(), UiError> {
        if self.models.select(id, session).is_none() {
            return Err(UiError::InvalidAction(
                "model is not selectable in the current session".to_owned(),
            ));
        }
        if self.remote.is_some() && self.holds_catalog_tone() {
            return self.request_switch(id);
        }
        self.backend.dispatch(UiAction::SelectModel {
            block_id: self.block_id.clone(),
            model_id: id,
        })?;
        self.block.active_model_id = Some(id);
        self.block.model_loading = true;
        self.synced_revision = None;
        Ok(())
    }

    /// Gives this detail a community service. A catalog tone then lists its models through the
    /// service, and a choice waits for its bytes. A local tone stays on the stash path.
    pub fn attach_model_switching(&mut self, flow: ModelSwitchFlow) {
        self.models.set_deferred(self.holds_catalog_tone());
        self.remote = Some(RemoteModels {
            flow,
            rows: Vec::new(),
            list_due: true,
            unsent: None,
            pending: None,
        });
    }

    /// Takes the model list that this detail needs. A signed-out picker is disabled and needs
    /// none, and a failed list is not asked for again until the detail opens again.
    pub fn take_models_request(&mut self, session: SessionStatus) -> Option<ModelsRequest> {
        let remote = self.remote.as_mut()?;
        let tone = self.block.tone.as_ref().filter(|tone| !tone.local)?;
        if self.closed || !session.signed_in() || !remote.list_due {
            return None;
        }
        remote.list_due = false;
        Some(ModelsRequest {
            generation: self.models.begin_fetch(tone.id),
            tone: tone.id,
            kind: tone.kind,
        })
    }

    /// Takes the switch that the user chose and that no worker has yet.
    pub fn take_switch_request(&mut self) -> Option<ModelSwitchRequest> {
        self.remote.as_mut()?.unsent.take()
    }

    /// Applies a model list when it is the newest list of the current tone. The active model
    /// stays in the picker when the service omits it.
    pub fn complete_models(&mut self, generation: u64, tone: ToneId, list: ModelList) -> bool {
        let ModelList { models, total } = list;
        let mut options = models
            .iter()
            .map(|model| ModelOption {
                id: model.id,
                name: model.name.clone(),
            })
            .collect::<Vec<_>>();
        let active = self.block.active_model_id;
        if !options.iter().any(|option| Some(option.id) == active)
            && let Some(model) = self
                .block
                .tone
                .as_ref()
                .and_then(|tone| tone.models.iter().find(|model| Some(model.id) == active))
        {
            options.insert(
                0,
                ModelOption {
                    id: model.id,
                    name: model.name.clone(),
                },
            );
        }
        let total = usize::try_from(total)
            .unwrap_or(usize::MAX)
            .max(options.len());
        if !self.models.complete_fetch(generation, tone, options, total) {
            return false;
        }
        if let Some(remote) = &mut self.remote {
            remote.rows = models;
        }
        true
    }

    /// Ends a failed model list when it is the newest list. The picker stays as it is.
    pub fn fail_models(&mut self, generation: u64) -> bool {
        self.models.fail_fetch(generation)
    }

    /// Ends the current switch after its audio is prepared. Returns false for any other switch.
    pub fn finish_switch(&mut self, generation: u64) -> bool {
        let Some(model) = self.take_pending(generation) else {
            return false;
        };
        self.models.set_active(model);
        self.models.clear_target();
        self.sync();
        true
    }

    /// Ends the current switch, which failed. The active model stays, and the same choice can
    /// start again. Returns false for any other switch.
    pub fn fail_switch(&mut self, generation: u64) -> bool {
        if self.take_pending(generation).is_none() {
            return false;
        }
        self.models.clear_target();
        true
    }

    /// Stops the model work of a session that ended. The next signed-in session asks again.
    pub fn abandon_model_work(&mut self) {
        self.revoke_switch();
        self.models.clear_target();
        self.models.cancel_fetch();
        if let Some(remote) = &mut self.remote {
            remote.list_due = true;
        }
    }

    /// Whether a model list or a switch of this detail is in progress.
    #[must_use]
    pub fn has_model_work(&self) -> bool {
        self.models.loading()
            || self
                .remote
                .as_ref()
                .is_some_and(|remote| remote.pending.is_some())
    }

    /// Replaces the decoded artwork and avatar of a controller that fetches them.
    pub(crate) fn set_remote_images(
        &mut self,
        artwork: Option<TextureHandle>,
        avatar: Option<TextureHandle>,
    ) {
        self.artwork = artwork;
        self.avatar = avatar;
    }

    /// Decoded artwork of the detail card.
    #[must_use]
    pub const fn artwork(&self) -> Option<&TextureHandle> {
        self.artwork.as_ref()
    }

    /// Decoded avatar of the tone creator.
    #[must_use]
    pub const fn avatar(&self) -> Option<&TextureHandle> {
        self.avatar.as_ref()
    }

    fn holds_catalog_tone(&self) -> bool {
        self.block.tone.as_ref().is_some_and(|tone| !tone.local)
    }

    /// Starts a remote switch. A choice of the model that already downloads changes nothing, and
    /// any other choice ends the switch in progress.
    fn request_switch(&mut self, id: ModelId) -> Result<(), UiError> {
        let downloading = self
            .remote
            .as_ref()
            .and_then(|remote| remote.pending.as_ref())
            .is_some_and(|pending| pending.model == id);
        if downloading {
            return Ok(());
        }
        self.revoke_switch();
        if self.block.active_model_id == Some(id) {
            return Ok(());
        }
        let tone = self.block.tone.clone();
        let replaces = self.block.active_model_id;
        let Some(remote) = &mut self.remote else {
            return Ok(());
        };
        let model = remote.rows.iter().find(|model| model.id == id).cloned();
        let (Some(tone), Some(model)) = (tone, model) else {
            self.models.clear_target();
            return Err(UiError::InvalidAction(
                "model is not in the current list".to_owned(),
            ));
        };
        let ticket = remote.flow.begin(self.block_id.clone(), replaces);
        remote.pending = Some(PendingSwitch {
            ticket: ticket.clone(),
            model: id,
        });
        remote.unsent = Some(ModelSwitchRequest {
            tone,
            model,
            ticket,
        });
        Ok(())
    }

    /// Ends the switch in progress. Its late completion cannot publish.
    fn revoke_switch(&mut self) {
        if let Some(remote) = &mut self.remote {
            remote.unsent = None;
            if let Some(pending) = remote.pending.take() {
                pending.ticket.revoke();
            }
        }
    }

    fn take_pending(&mut self, generation: u64) -> Option<ModelId> {
        let remote = self.remote.as_mut()?;
        let pending = remote
            .pending
            .take_if(|pending| pending.ticket.generation() == generation)?;
        Some(pending.model)
    }

    /// Follows a change of the block that came from outside this detail.
    fn reconcile_model_work(&mut self, replaced: bool) {
        if self.remote.is_none() {
            return;
        }
        if replaced {
            self.revoke_switch();
            self.models = model_select_for(&self.block);
            self.models.set_deferred(self.holds_catalog_tone());
            self.info.set_tone(self.block.tone.clone());
            self.info.set_page_url(tone_url(&self.block));
            if let Some(remote) = &mut self.remote {
                remote.rows.clear();
                remote.list_due = true;
            }
            return;
        }
        let active = self.block.active_model_id;
        let pending = self
            .remote
            .as_ref()
            .and_then(|remote| remote.pending.as_ref());
        if let Some(pending) = pending {
            // The chain holds the old model, or the new model before its audio is prepared.
            if active == Some(pending.model) || active == pending.ticket.replaces() {
                return;
            }
            self.revoke_switch();
            self.models.clear_target();
        }
        if let Some(active) = active
            && self.models.active() != Some(active)
        {
            self.models.set_active(active);
        }
    }

    /// Toggles block power.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), UiError> {
        self.block.params.enabled = enabled;
        self.write_params()
    }

    /// Sets normalized input gain.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_input_gain(&mut self, value: f64) -> Result<(), UiError> {
        self.block.params.input_gain = value.clamp(0.0, 1.0);
        self.write_params()
    }

    /// Sets normalized output gain.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_output_gain(&mut self, value: f64) -> Result<(), UiError> {
        self.block.params.output_gain = value.clamp(0.0, 1.0);
        self.write_params()
    }

    /// Sets normalized wet mix.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_mix(&mut self, value: f64) -> Result<(), UiError> {
        self.block.params.mix = value.clamp(0.0, 1.0);
        self.write_params()
    }

    /// Toggles loudness normalization.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_normalize(&mut self, enabled: bool) -> Result<(), UiError> {
        self.block.params.normalize = enabled;
        self.write_params()
    }

    /// Selects lite or full model size.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_slim_full(&mut self, full: bool) -> Result<(), UiError> {
        self.block.params.slim_size = if full { 1.0 } else { 0.0 };
        self.write_params()
    }

    /// Toggles EQ processing.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_eq_enabled(&mut self, enabled: bool) -> Result<(), UiError> {
        self.eq.set_enabled(enabled);
        self.block.params.eq = self.eq.params().clone();
        self.synced_revision = None;
        Ok(())
    }

    /// Moves EQ before or after the model.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_eq_pre(&mut self, pre: bool) -> Result<(), UiError> {
        self.eq.set_pre(pre);
        self.block.params.eq = self.eq.params().clone();
        self.synced_revision = None;
        Ok(())
    }

    /// Retries the active model load.
    ///
    /// # Errors
    /// Returns an invalid-action or backend error.
    pub fn retry(&mut self) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::RetryModelLoad(self.block_id.clone()))?;
        self.block.load_failed = false;
        self.block.model_loading = true;
        self.synced_revision = None;
        Ok(())
    }

    /// Removes the block and requests a return to the gallery.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn remove(&mut self) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::RemoveBlock(self.block_id.clone()))?;
        self.revoke_switch();
        self.closed = true;
        Ok(())
    }

    /// Public tone URL used by Share and metadata.
    #[must_use]
    pub fn share_url(&self) -> String {
        tone_url(&self.block)
    }

    /// Calibration metadata in dBu when present.
    #[must_use]
    pub fn calibration_dbu(&self) -> Option<f32> {
        self.block.tone.as_ref().and_then(|tone| {
            tone.raw
                .get("output_level_dbu")
                .or_else(|| tone.raw.get("outputLevelDbu"))
                .and_then(|value| value.as_f64())
                .map(|value| value as f32)
        })
    }

    /// Whether a loaded, enabled NAM follows this block in its lane.
    #[must_use]
    pub fn nam_downstream(&self) -> bool {
        let chain = self.backend.chain_state(0).chain;
        for lane in [&chain.left, &chain.right] {
            let Some(index) = lane.iter().position(|block| block.id == self.block_id) else {
                continue;
            };
            return lane[index + 1..].iter().any(|block| {
                !block.is_insert
                    && block.loaded
                    && block.params.enabled
                    && block
                        .tone
                        .as_ref()
                        .is_some_and(|tone| tone.kind == ModelKind::Nam)
            });
        }
        false
    }

    /// Whether calibrated downstream hand-off overrides normalization.
    #[must_use]
    pub fn normalize_overridden(&self) -> bool {
        let is_nam = self
            .block
            .tone
            .as_ref()
            .is_some_and(|tone| tone.kind == ModelKind::Nam);
        let sane = self
            .calibration_dbu()
            .is_some_and(|dbu| (-60.0..=60.0).contains(&dbu));
        is_nam && self.calibrate_input && self.nam_downstream() && sane
    }

    /// Whether the lite/full control is visible for this block.
    #[must_use]
    pub fn show_slim_size_control(&self) -> bool {
        if !self
            .block
            .tone
            .as_ref()
            .is_some_and(|tone| tone.kind == ModelKind::Nam)
        {
            return false;
        }
        let preference = self
            .backend
            .preference("t3k.showBlockSizeControl")
            .unwrap_or(false);
        let default_full = self.backend.chain_state(0).nam_full_size;
        preference || (self.block.params.slim_size >= 0.5) != default_full
    }

    /// Whether the normalization control is visible for this block.
    #[must_use]
    pub fn show_normalize_control(&self) -> bool {
        self.block
            .tone
            .as_ref()
            .is_some_and(|tone| tone.kind == ModelKind::Nam)
            && self
                .backend
                .preference("t3k.showBlockNormalizeControl")
                .unwrap_or(false)
    }

    /// Selects the EQ curve or slider body.
    pub fn set_eq_curve(&mut self, curve: bool) {
        self.eq.set_view(if curve {
            super::eq::EqViewMode::Curve
        } else {
            super::eq::EqViewMode::Sliders
        });
    }

    /// Opens the model picker used by the detail card.
    pub fn open_model_picker(&mut self, session: SessionStatus) {
        self.models.open(session);
    }

    /// Loading, failure, or ready state for the artwork and retry badge.
    #[must_use]
    pub fn load_state(&self) -> TileVisualState {
        let switching = self
            .remote
            .as_ref()
            .is_some_and(|remote| remote.pending.is_some());
        if self.block.load_failed {
            TileVisualState::Failed
        } else if switching || self.block.model_loading || !self.block.loaded {
            TileVisualState::Loading
        } else {
            TileVisualState::Ready
        }
    }

    /// Supplies decoded artwork for the detail card.
    pub fn set_artwork(&mut self, texture: TextureHandle) {
        self.artwork = Some(texture);
    }

    /// Supplies the tone creator's avatar.
    pub fn set_avatar(&mut self, texture: TextureHandle) {
        self.avatar = Some(texture);
    }

    /// Takes a pending tone-swap navigation request.
    pub fn take_swap_request(&mut self) -> Option<BlockId> {
        self.swap_requested.take()
    }

    /// Renders the centered detail takeover.
    pub fn show(&mut self, ui: &mut egui::Ui, session: SessionStatus) {
        self.show_detail(ui, session);
        self.close_hidden_drags(ui.ctx().cumulative_pass_nr());
    }

    /// Closes the gesture of a trim knob drag or an EQ drag or field edit when `pass` did not
    /// show its control.
    ///
    /// The detail calls this at the end of its own frame. The page switch also calls it in a pass
    /// that does not show the detail, so that a page that covers the detail closes the drag at
    /// once.
    pub(crate) fn close_hidden_drags(&mut self, pass: u64) {
        if self.trim_drag.take_hidden(pass).is_some() {
            self.write_params_in_gesture(GesturePhase::End);
        }
        self.eq.end_hidden_drag(pass);
    }

    fn show_detail(&mut self, ui: &mut egui::Ui, session: SessionStatus) {
        if self.closed {
            return;
        }
        self.sync_on_change();
        let bounds = ui.available_rect_before_wrap();
        let card_x = bounds.left() + (bounds.width() - 800.0) / 2.0;
        let back = Rect::from_min_size(
            egui::pos2(card_x, bounds.top() + 24.0),
            egui::vec2(104.0, 22.0),
        );
        if back_link(ui, back).clicked() {
            self.revoke_switch();
            self.closed = true;
        }
        let card_y = bounds.top() + 62.0;
        let card_height = if self.view == BlockView::Info {
            (bounds.bottom() - card_y).max(320.0)
        } else {
            320.0
        };
        let card = Rect::from_min_size(egui::pos2(card_x, card_y), egui::vec2(800.0, card_height));
        let painter = ui.painter_at(card);
        painter.rect_filled(card, 16.0, Color32::BLACK);
        painter.rect_stroke(
            card,
            16.0,
            egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
            egui::StrokeKind::Inside,
        );
        let inner = card.shrink(1.0);
        let header = Rect::from_min_size(inner.min, egui::vec2(inner.width(), 45.0));
        self.show_header(ui, header);
        let body = Rect::from_min_size(
            header.left_bottom(),
            egui::vec2(inner.width(), card_height - 45.0),
        );
        match self.view {
            BlockView::Tone => self.show_tone(ui, body, session),
            BlockView::Eq => {
                let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
                self.eq.show(&mut body_ui);
            }
            BlockView::Info => self.show_info(ui, body, session),
        }
    }

    fn show_header(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
        );
        let cy = rect.center().y;
        let power =
            Rect::from_center_size(egui::pos2(rect.left() + 26.0, cy), egui::Vec2::splat(20.0));
        if header_button(
            ui,
            power,
            "Block power",
            HeaderIcon::Power,
            self.block.params.enabled,
        )
        .clicked()
        {
            let _ = self.set_enabled(!self.block.params.enabled);
        }
        let mut left = power.right() + 24.0;
        if self.show_slim_size_control() {
            let full = self.block.params.slim_size >= 0.5;
            let size = Rect::from_center_size(egui::pos2(left + 32.0, cy), egui::vec2(64.0, 20.0));
            if text_header_button(ui, size, if full { "FULL" } else { "LITE" }, full).clicked() {
                let _ = self.set_slim_full(!full);
            }
            left = size.right() + 24.0;
        }
        if self.calibrate_input && self.calibration_dbu().is_some() {
            painter.text(
                egui::pos2(left + 10.0, cy),
                egui::Align2::CENTER_CENTER,
                "CAL",
                egui::FontId::proportional(10.0),
                Color32::WHITE,
            );
        }

        let remove =
            Rect::from_center_size(egui::pos2(rect.right() - 26.0, cy), egui::Vec2::splat(20.0));
        let swap = Rect::from_center_size(
            egui::pos2(remove.left() - 24.0 - 10.0, cy),
            egui::Vec2::splat(20.0),
        );
        let share = Rect::from_center_size(
            egui::pos2(swap.left() - 24.0 - 10.0, cy),
            egui::Vec2::splat(20.0),
        );
        let info = Rect::from_center_size(
            egui::pos2(share.left() - 24.0 - 10.0, cy),
            egui::Vec2::splat(20.0),
        );
        let mut eq = Rect::from_center_size(
            egui::pos2(info.left() - 24.0 - 10.0, cy),
            egui::vec2(24.0, 20.0),
        );
        if header_button(ui, remove, "Remove block", HeaderIcon::Remove, false).clicked() {
            let _ = self.remove();
        }
        if header_button(ui, swap, "Swap tone", HeaderIcon::Swap, false).clicked() {
            self.swap_requested = Some(self.block_id.clone());
        }
        if header_button(ui, share, "Share tone", HeaderIcon::Share, false).clicked() {
            ui.ctx().copy_text(self.share_url());
        }
        let local = self.block.tone.as_ref().is_some_and(|tone| tone.local);
        if !local
            && header_button(
                ui,
                info,
                "Tone info",
                HeaderIcon::Info,
                self.view == BlockView::Info,
            )
            .clicked()
        {
            self.set_view(if self.view == BlockView::Info {
                BlockView::Tone
            } else {
                BlockView::Info
            });
        }
        if self.view == BlockView::Eq {
            let pill = Rect::from_center_size(
                egui::pos2(info.left() - 110.0, cy),
                egui::vec2(196.0, 28.0),
            );
            painter.rect_filled(pill, 14.0, crate::theme::palette::CONTROL_FILL);
            let eq_power = Rect::from_min_size(
                pill.left_top() + egui::vec2(12.0, 4.0),
                egui::Vec2::splat(20.0),
            );
            let pre = Rect::from_min_size(
                eq_power.right_top() + egui::vec2(16.0, 0.0),
                egui::vec2(32.0, 20.0),
            );
            let views = Rect::from_min_size(
                pre.right_top() + egui::vec2(16.0, 0.0),
                egui::vec2(48.0, 20.0),
            );
            painter.rect_filled(
                views,
                2.0,
                Color32::from_rgba_unmultiplied(118, 118, 128, 61),
            );
            let sliders = Rect::from_min_size(views.min, egui::vec2(24.0, 20.0));
            let curve = Rect::from_min_size(sliders.right_top(), egui::vec2(24.0, 20.0));
            eq = Rect::from_min_size(
                views.right_top() + egui::vec2(16.0, 0.0),
                egui::vec2(24.0, 20.0),
            );
            if header_button(
                ui,
                eq_power,
                "EQ power",
                HeaderIcon::Power,
                self.eq.params().enabled,
            )
            .clicked()
            {
                let _ = self.set_eq_enabled(!self.eq.params().enabled);
            }
            if text_header_button(ui, pre, "PRE", self.eq.params().pre).clicked() {
                let _ = self.set_eq_pre(!self.eq.params().pre);
            }
            if eq_view_button(
                ui,
                sliders,
                "Sliders",
                false,
                self.eq.view() == EqViewMode::Sliders,
            )
            .clicked()
            {
                self.eq.set_view(EqViewMode::Sliders);
            }
            if eq_view_button(
                ui,
                curve,
                "Curve",
                true,
                self.eq.view() == EqViewMode::Curve,
            )
            .clicked()
            {
                self.eq.set_view(EqViewMode::Curve);
            }
        }
        if header_button(ui, eq, "EQ", HeaderIcon::Eq, self.view == BlockView::Eq).clicked() {
            self.set_view(if self.view == BlockView::Eq {
                BlockView::Tone
            } else {
                BlockView::Eq
            });
        }
    }

    fn show_tone(&mut self, ui: &mut egui::Ui, rect: Rect, session: SessionStatus) {
        let content = rect.shrink(16.0);
        let bottom = content.bottom();
        let normalize = self.show_normalize_control();
        let out_rail_width = 36.0 + if normalize { 30.0 } else { 0.0 };
        let out_x = content.right() - out_rail_width;
        let mix_x = out_x - 60.0;
        let centre_x = content.left() + 60.0;
        let centre_width = mix_x - 24.0 - centre_x;
        let input_rect = Rect::from_min_size(
            egui::pos2(content.left(), bottom - 63.0),
            egui::vec2(36.0, 63.0),
        );
        let output_rect = Rect::from_min_size(
            egui::pos2(content.right() - 36.0, bottom - 63.0),
            egui::vec2(36.0, 63.0),
        );
        let mix_rect =
            Rect::from_min_size(egui::pos2(mix_x, bottom - 63.0), egui::vec2(36.0, 63.0));
        let gain = KnobSpec::block_gain();
        let mix = KnobSpec::percent(BlockParams::default().mix);
        let id = ui.scope_id().with(("trim", &self.block_id.0));
        let pass = ui.ctx().cumulative_pass_nr();
        let edit = trim_knob(
            ui,
            id,
            input_rect,
            "In",
            &gain,
            self.block.params.input_gain,
        );
        if let Some(edit) = self.track_trim("In", pass, edit) {
            self.block.params.input_gain = edit.value;
            self.write_params_in_gesture(edit.phase);
        }
        let edit = trim_knob(ui, id, mix_rect, "Mix", &mix, self.block.params.mix);
        if let Some(edit) = self.track_trim("Mix", pass, edit) {
            self.block.params.mix = edit.value;
            self.write_params_in_gesture(edit.phase);
        }
        let edit = trim_knob(
            ui,
            id,
            output_rect,
            "Out",
            &gain,
            self.block.params.output_gain,
        );
        if let Some(edit) = self.track_trim("Out", pass, edit) {
            self.block.params.output_gain = edit.value;
            self.write_params_in_gesture(edit.phase);
        }
        self.show_meter(
            ui,
            Rect::from_min_max(
                egui::pos2(input_rect.left(), content.top()),
                egui::pos2(input_rect.right(), input_rect.top() - 12.0),
            ),
            true,
        );
        self.show_meter(
            ui,
            Rect::from_min_max(
                egui::pos2(output_rect.left(), content.top()),
                egui::pos2(output_rect.right(), output_rect.top() - 12.0),
            ),
            false,
        );
        if normalize {
            let overridden = self.normalize_overridden();
            let button = Rect::from_center_size(
                egui::pos2(out_x + 10.0, bottom - 18.0),
                egui::Vec2::splat(20.0),
            );
            if header_button(
                ui,
                button,
                "Normalize",
                HeaderIcon::Equal,
                self.block.params.normalize && !overridden,
            )
            .clicked()
                && !overridden
            {
                let _ = self.set_normalize(!self.block.params.normalize);
            }
        }

        let artwork = Rect::from_min_size(
            egui::pos2(centre_x, content.top()),
            egui::Vec2::splat(192.0),
        );
        self.show_artwork(ui, artwork);
        let meta = Rect::from_min_size(
            egui::pos2(artwork.right() + 24.0, content.top() + 30.0),
            egui::vec2(centre_width - 216.0, 135.0),
        );
        self.show_metadata(ui, meta);
        let picker = Rect::from_min_size(
            egui::pos2(centre_x, bottom - 36.0),
            egui::vec2(centre_width, 36.0),
        );
        let mut picker_ui = ui.new_child(egui::UiBuilder::new().max_rect(picker));
        if let Some(id) = self.models.show(&mut picker_ui, session) {
            let _ = self.select_model(id, session);
        }
    }

    fn show_artwork(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 8.0, crate::theme::palette::PANEL);
        if let Some(texture) = &self.artwork {
            let tint = if self.load_state() == TileVisualState::Ready {
                Color32::WHITE
            } else {
                Color32::from_white_alpha(89)
            };
            egui::Image::from_texture(texture)
                .uv(super::tile::cover_uv(
                    texture.size(),
                    [rect.width() as usize, rect.height() as usize],
                ))
                .tint(tint)
                .corner_radius(8.0)
                .paint_at(ui, rect);
        } else {
            let gear = self
                .block
                .tone
                .as_ref()
                .and_then(|tone| tone.raw.get("gear"))
                .and_then(|gear| gear.as_str())
                .unwrap_or_default();
            draw_gear(&painter, rect, gear_glyph(gear));
            // A tile does the same, so that the icon does not show through a message.
            if self.load_state() != TileVisualState::Ready {
                painter.rect_filled(rect, 8.0, crate::theme::palette::ART_SCRIM);
            }
        }
        match self.load_state() {
            TileVisualState::Loading => {
                for x in [-10.0, 0.0, 10.0] {
                    painter.circle_filled(rect.center() + egui::vec2(x, 0.0), 3.0, Color32::WHITE);
                }
            }
            TileVisualState::Failed => {
                let group_top = rect.center().y - 24.0;
                painter.text(
                    egui::pos2(rect.center().x, group_top),
                    egui::Align2::CENTER_TOP,
                    "Download failed",
                    egui::FontId::proportional(11.0),
                    Color32::from_rgba_unmultiplied(255, 255, 255, 230),
                );
                let badge = Rect::from_min_size(
                    egui::pos2(rect.center().x - 35.0, group_top + 21.0),
                    egui::vec2(70.0, 26.0),
                );
                painter.rect_filled(badge, 13.0, Color32::WHITE);
                let icon = badge.left_center() + egui::vec2(17.0, 0.0);
                let arc = (0..=20)
                    .map(|step| {
                        let angle = (45.0 + step as f32 * 270.0 / 20.0).to_radians();
                        icon + egui::vec2(angle.cos(), angle.sin()) * 5.0
                    })
                    .collect();
                painter.add(egui::Shape::line(
                    arc,
                    egui::Stroke::new(1.2, Color32::BLACK),
                ));
                painter.line_segment(
                    [icon + egui::vec2(1.0, -7.0), icon + egui::vec2(5.0, -5.0)],
                    egui::Stroke::new(1.2, Color32::BLACK),
                );
                painter.line_segment(
                    [icon + egui::vec2(1.0, -7.0), icon + egui::vec2(1.0, -2.5)],
                    egui::Stroke::new(1.2, Color32::BLACK),
                );
                painter.text(
                    badge.center() + egui::vec2(7.0, 0.0),
                    egui::Align2::CENTER_CENTER,
                    "Retry",
                    egui::FontId::proportional(12.0),
                    Color32::BLACK,
                );
                let retry = ui.interact(badge, ui.scope_id().with("detail-retry"), Sense::click());
                retry.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, "Retry model load")
                });
                if retry.clicked() {
                    let _ = self.retry();
                }
            }
            TileVisualState::Ready | TileVisualState::DropArmed => {}
        }
    }

    fn show_meter(&self, ui: &mut egui::Ui, rect: Rect, input: bool) {
        let meters = self.backend.meter_levels();
        let Some(meter) = meters
            .blocks
            .iter()
            .find(|meter| meter.block_id == self.block_id)
        else {
            return;
        };
        let db = if input {
            meter.input_db
        } else {
            meter.output_db
        };
        dot_meter(ui, rect, db);
    }

    fn show_metadata(&self, ui: &mut egui::Ui, rect: Rect) {
        let Some(tone) = &self.block.tone else {
            return;
        };
        let painter = ui.painter_at(rect);
        let title_painter = painter.with_clip_rect(Rect::from_min_size(
            rect.min,
            egui::vec2(rect.width(), 50.0),
        ));
        baseline_text(
            ui,
            &title_painter,
            egui::pos2(rect.left(), rect.top() + 18.0),
            tone.title.as_str(),
            egui::FontId::new(18.0, egui::FontFamily::Name("arimo-bold".into())),
            Color32::WHITE,
        );
        let gear = gear_label(tone);
        baseline_text(
            ui,
            &painter,
            egui::pos2(rect.left(), rect.top() + 48.0),
            gear,
            egui::FontId::proportional(14.0),
            muted(),
        );
        let badge_x = rect.left() + text_width(ui, gear, 14.0) + 16.0;
        let nam = tone.kind == ModelKind::Nam;
        let badge = Rect::from_min_size(
            egui::pos2(badge_x, rect.top() + 34.0),
            egui::vec2(if nam { 34.0 } else { 27.0 }, 18.0),
        );
        painter.rect_filled(badge, 2.0, Color32::from_rgb(161, 161, 170));
        painter.text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            if nam { "NAM" } else { "IR" },
            egui::FontId::monospace(12.0),
            Color32::BLACK,
        );
        if nam {
            super::brand::draw_architecture_chip(
                &painter,
                Rect::from_center_size(
                    egui::pos2(badge.right() + 19.0, rect.top() + 43.0),
                    egui::Vec2::splat(18.0),
                ),
            );
        }
        let downloads = tone
            .raw
            .get("downloads_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let favorites = tone
            .raw
            .get("favorites_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let mut x = rect.left();
        for (index, count) in [downloads, favorites, model_total(&self.block) as u64]
            .into_iter()
            .enumerate()
        {
            stat_icon(&painter, egui::pos2(x + 8.0, rect.top() + 76.0), index);
            let count = format_count(count);
            painter.text(
                egui::pos2(x + 24.0, rect.top() + 76.0),
                egui::Align2::LEFT_CENTER,
                &count,
                egui::FontId::proportional(14.0),
                muted(),
            );
            x += 48.0 + text_width(ui, &count, 14.0).round();
        }
        if let Some(user) = tone.raw.get("user") {
            let avatar = Rect::from_min_size(
                egui::pos2(rect.left(), rect.top() + 100.0),
                egui::Vec2::splat(32.0),
            );
            if let Some(texture) = &self.avatar {
                egui::Image::from_texture(texture)
                    .corner_radius(16.0)
                    .paint_at(ui, avatar);
            } else {
                painter.circle_filled(avatar.center(), 16.0, crate::theme::palette::TEXT_MUTED);
            }
            if let Some(name) = user.get("username").and_then(serde_json::Value::as_str) {
                painter.text(
                    egui::pos2(avatar.right() + 12.0, avatar.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(14.0),
                    crate::theme::palette::TEXT_MUTED,
                );
            }
        }
    }

    fn show_info(&mut self, ui: &mut egui::Ui, rect: Rect, session: SessionStatus) {
        let content = Rect::from_min_max(
            rect.min + egui::vec2(16.0, 16.0),
            rect.max - egui::vec2(16.0, 0.0),
        );
        self.show_artwork(
            ui,
            Rect::from_min_size(content.min, egui::Vec2::splat(160.0)),
        );
        let meta = Rect::from_min_size(
            content.min + egui::vec2(184.0, 0.0),
            egui::vec2(content.width() - 184.0, 132.0),
        );
        self.show_metadata(ui, meta);
        let info_rect = Rect::from_min_max(
            meta.left_bottom() + egui::vec2(0.0, 24.0),
            content.right_bottom(),
        );
        let mut info_ui = ui.new_child(egui::UiBuilder::new().max_rect(info_rect));
        info_ui.set_clip_rect(info_rect);
        if let Some(action) = self.info.show(&mut info_ui, session) {
            match action {
                // Only the chrome owns the flow, and only a finished flow signs the user in.
                InfoAction::SignIn => self.sign_in_requested = true,
                InfoAction::Retry => self.info.set_error(None),
                InfoAction::OpenUrl(url) => open_web_url(ui.ctx(), url),
            }
        }
    }

    /// Records one frame of a trim knob, and first closes a drag gesture that its knob left open.
    fn track_trim(
        &mut self,
        label: &'static str,
        pass: u64,
        edit: Option<KnobEdit>,
    ) -> Option<KnobEdit> {
        let Tracked { stale, .. } = self.trim_drag.track(&label, pass, edit.as_ref());
        if stale.is_some() {
            self.write_params_in_gesture(GesturePhase::End);
        }
        edit
    }

    /// Writes the settings of one frame of a trim knob. A view has no way to show this error,
    /// and the next frame shows the value that the backend holds.
    fn write_params_in_gesture(&mut self, gesture: GesturePhase) {
        self.synced_revision = None;
        let _ = self.backend.dispatch(UiAction::SetBlockParamsGesture {
            block_id: self.block_id.clone(),
            params: self.block.params.clone(),
            gesture,
        });
    }

    fn write_params(&mut self) -> Result<(), UiError> {
        self.synced_revision = None;
        self.backend
            .dispatch(UiAction::SetBlockParams {
                block_id: self.block_id.clone(),
                params: self.block.params.clone(),
            })
            .map(|_| ())
    }
}

fn back_link(ui: &mut egui::Ui, rect: Rect) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with("back-to-chain"), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Back to chain"));
    let painter = ui.painter_at(rect);
    let center = egui::pos2(rect.left() + 8.0, rect.center().y);
    let stroke = egui::Stroke::new(1.5, Color32::WHITE);
    painter.line_segment(
        [
            center + egui::vec2(5.0, -6.0),
            center + egui::vec2(-3.0, 0.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center + egui::vec2(-3.0, 0.0),
            center + egui::vec2(5.0, 6.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center + egui::vec2(-3.0, 0.0),
            center + egui::vec2(10.0, 0.0),
        ],
        stroke,
    );
    painter.text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "BLOCK",
        egui::FontId::monospace(16.0),
        Color32::WHITE,
    );
    response
}

/// One icon of the block header, drawn with the painter and so with no font glyph.
#[derive(Clone, Copy)]
pub(super) enum HeaderIcon {
    Power,
    Eq,
    Info,
    Share,
    Swap,
    Remove,
    Equal,
}

fn header_button(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &'static str,
    icon: HeaderIcon,
    active: bool,
) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(label), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let painter = ui.painter_at(rect);
    let muted = crate::theme::palette::TEXT_MUTED;
    let color = match icon {
        HeaderIcon::Power | HeaderIcon::Equal => {
            if !active {
                painter.rect_filled(rect, 2.0, crate::theme::palette::TINT_FILL);
            }
            if active { Color32::WHITE } else { muted }
        }
        HeaderIcon::Eq | HeaderIcon::Info => {
            if active {
                painter.rect_filled(rect, 2.0, Color32::WHITE);
                Color32::BLACK
            } else {
                muted
            }
        }
        HeaderIcon::Share | HeaderIcon::Swap | HeaderIcon::Remove => Color32::WHITE,
    };
    draw_header_icon(&painter, rect, icon, color, active);
    response
}

/// Paints one header icon in the center of `rect`.
pub(super) fn draw_header_icon(
    painter: &egui::Painter,
    rect: Rect,
    icon: HeaderIcon,
    color: Color32,
    active: bool,
) {
    let stroke = egui::Stroke::new(1.4, color);
    let center = rect.center();
    match icon {
        HeaderIcon::Power => {
            let power_stroke = egui::Stroke::new(1.2, color);
            let mut points = Vec::new();
            for index in 0..=32 {
                let angle = (-40.0 + index as f32 / 32.0 * 260.0).to_radians();
                points.push(center + egui::vec2(angle.cos() * 5.25, angle.sin() * 5.25));
            }
            painter.add(egui::epaint::PathShape::line(points, power_stroke));
            painter.line_segment([center - egui::vec2(0.0, 5.83), center], power_stroke);
        }
        HeaderIcon::Eq => {
            if !active {
                painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
            }
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                "EQ",
                egui::FontId::monospace(12.0),
                color,
            );
        }
        HeaderIcon::Info => {
            painter.rect_stroke(
                Rect::from_center_size(center, egui::Vec2::splat(18.0)),
                3.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                "i",
                egui::FontId::proportional(12.0),
                color,
            );
        }
        HeaderIcon::Share => {
            painter.line_segment(
                [center - egui::vec2(0.0, 7.0), center + egui::vec2(0.0, 2.0)],
                stroke,
            );
            painter.line_segment(
                [
                    center - egui::vec2(4.0, -2.0),
                    center - egui::vec2(0.0, 7.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(4.0, -2.0),
                    center - egui::vec2(0.0, 7.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-6.0, 3.0),
                    center + egui::vec2(-6.0, 7.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-6.0, 7.0),
                    center + egui::vec2(6.0, 7.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(6.0, 7.0), center + egui::vec2(6.0, 3.0)],
                stroke,
            );
        }
        HeaderIcon::Swap => {
            for (y, direction) in [(-3.0, 1.0), (3.0, -1.0)] {
                let row = center + egui::vec2(0.0, y);
                painter.line_segment(
                    [row - egui::vec2(7.0, 0.0), row + egui::vec2(7.0, 0.0)],
                    stroke,
                );
                let tip = row + egui::vec2(7.0 * direction, 0.0);
                painter.line_segment([tip, tip + egui::vec2(-4.0 * direction, -3.0)], stroke);
                painter.line_segment([tip, tip + egui::vec2(-4.0 * direction, 3.0)], stroke);
            }
        }
        HeaderIcon::Remove => {
            painter.rect_stroke(
                Rect::from_center_size(center + egui::vec2(0.0, 2.0), egui::vec2(9.0, 11.0)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-6.0, -6.0),
                    center + egui::vec2(6.0, -6.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-3.0, -8.0),
                    center + egui::vec2(3.0, -8.0),
                ],
                stroke,
            );
        }
        HeaderIcon::Equal => {
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                "=",
                egui::FontId::proportional(17.0),
                color,
            );
        }
    }
}

fn eq_view_button(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &'static str,
    curve: bool,
    active: bool,
) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(label), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let painter = ui.painter_at(rect);
    let color = if active {
        Color32::WHITE
    } else {
        crate::theme::palette::TEXT_MUTED
    };
    let origin = rect.center() - egui::Vec2::splat(8.0);
    let point = |x: f32, y: f32| origin + egui::vec2(x, y);
    if curve {
        let mut points = Vec::with_capacity(34);
        for index in 0..=16 {
            let t = index as f32 / 16.0;
            let u = 1.0 - t;
            points.push(point(
                u * u * u + 15.0 * u * u * t + 16.5 * u * t * t + 8.0 * t * t * t,
                13.5 * u * u * u + 40.5 * u * u * t + 7.5 * u * t * t + 2.5 * t * t * t,
            ));
        }
        for index in 1..=16 {
            let t = index as f32 / 16.0;
            let u = 1.0 - t;
            points.push(point(
                8.0 * u * u * u + 31.5 * u * u * t + 33.0 * u * t * t + 15.0 * t * t * t,
                2.5 * u * u * u + 7.5 * u * u * t + 40.5 * u * t * t + 13.5 * t * t * t,
            ));
        }
        painter.add(egui::epaint::PathShape::line(
            points,
            egui::Stroke::new(1.5, color),
        ));
    } else {
        let stroke = egui::Stroke::new(1.333, color);
        painter.line_segment([point(11.333, 6.667), point(11.333, 12.667)], stroke);
        painter.line_segment([point(4.667, 3.333), point(4.667, 9.333)], stroke);
        painter.circle_stroke(point(11.333, 4.667), 2.0, stroke);
        painter.circle_stroke(point(4.667, 11.333), 2.0, stroke);
    }
    response
}

fn text_header_button(
    ui: &mut egui::Ui,
    rect: Rect,
    text: &'static str,
    active: bool,
) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(text), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text));
    let painter = ui.painter();
    let pre = text == "PRE";
    if pre {
        if active {
            painter.rect_filled(rect, 2.0, crate::theme::palette::BLAZE);
        } else {
            painter.rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                egui::StrokeKind::Inside,
            );
        }
    } else if active {
        painter.rect_filled(rect, 2.0, crate::theme::palette::TINT_FILL);
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        if pre {
            egui::FontId::monospace(12.0)
        } else {
            egui::FontId::proportional(11.0)
        },
        if pre && active {
            Color32::BLACK
        } else if active {
            Color32::WHITE
        } else {
            crate::theme::palette::TEXT_MUTED
        },
    );
    response
}

/// Draws one trim knob, and returns its edit in this frame, if any.
///
/// The ID of the knob is `base` with its label, so it does not change with the widgets before it.
fn trim_knob(
    ui: &mut egui::Ui,
    base: egui::Id,
    rect: Rect,
    label: &'static str,
    spec: &KnobSpec,
    value: f64,
) -> Option<KnobEdit> {
    // The cursor still moves, so that the layout of the other widgets stays the same.
    let _ = ui.advance_cursor_after_rect(rect);
    let id = base.with(label);
    let output = crate::widgets::knob::show(ui, id, rect, spec, label, value);
    let face = Rect::from_min_size(rect.min + egui::vec2(0.0, 27.0), egui::Vec2::splat(36.0));
    crate::widgets::knob::paint_knob(
        ui.painter(),
        face,
        spec.position(output.shown) as f32,
        spec.bipolar(),
    );
    ui.painter().text(
        egui::pos2(rect.center().x, rect.top() + 10.5),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(14.0),
        crate::theme::palette::TEXT_MUTED,
    );
    output.edit
}

fn dot_meter(ui: &mut egui::Ui, rect: Rect, db: f32) {
    let count = 12;
    let height = 158.0;
    let top = rect.center().y - height / 2.0;
    let painter = ui.painter_at(rect);
    for index in 0..count {
        let position = index as f32 / (count - 1) as f32;
        let dot_db = -60.0 + position * 60.0;
        let color = crate::theme::palette::fire_ramp(position);
        painter.circle_filled(
            egui::pos2(
                rect.center().x,
                top + 2.0 + (count - 1 - index) as f32 * 14.0,
            ),
            2.0,
            if db >= dot_db {
                color
            } else {
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 56)
            },
        );
    }
}

fn gear_label(tone: &ronie_core::Tone) -> &'static str {
    match tone
        .raw
        .get("gear")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
    {
        "amp" => "Amp Head",
        "amp+cab" => "Amp + Cab",
        "cab" => "Cabinet",
        "pedal" => "Pedal",
        "outboard" => "Outboard",
        "space" => "Spaces",
        _ => "Experimental",
    }
}

fn muted() -> Color32 {
    crate::theme::palette::TEXT_SECONDARY
}

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                Color32::WHITE,
            )
            .size()
            .x
    })
}

fn baseline_text(
    ui: &egui::Ui,
    painter: &egui::Painter,
    baseline: egui::Pos2,
    text: &str,
    font: egui::FontId,
    color: Color32,
) {
    let galley = ui
        .ctx()
        .fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font, color));
    let glyph_baseline = galley
        .rows
        .first()
        .and_then(|row| row.row.glyphs.first())
        .map_or(0.0, |glyph| glyph.pos.y);
    painter.galley(
        egui::pos2(baseline.x, baseline.y - glyph_baseline),
        galley,
        color,
    );
}

fn stat_icon(painter: &egui::Painter, center: egui::Pos2, index: usize) {
    let color = crate::theme::palette::TEXT_MUTED;
    let stroke = egui::Stroke::new(4.0 / 3.0, color);
    let point =
        |x: f32, y: f32| center + egui::vec2((x - 12.0) * 2.0 / 3.0, (y - 12.0) * 2.0 / 3.0);
    match index {
        0 => {
            painter.line_segment([point(12.0, 3.0), point(12.0, 15.0)], stroke);
            painter.add(egui::epaint::PathShape::line(
                vec![
                    point(21.0, 15.0),
                    point(21.0, 19.0),
                    point(19.0, 21.0),
                    point(5.0, 21.0),
                    point(3.0, 19.0),
                    point(3.0, 15.0),
                ],
                stroke,
            ));
            painter.add(egui::epaint::PathShape::line(
                vec![point(7.0, 10.0), point(12.0, 15.0), point(17.0, 10.0)],
                stroke,
            ));
        }
        1 => {
            painter.add(egui::epaint::PathShape::line(
                vec![
                    point(17.0, 3.0),
                    point(19.0, 5.0),
                    point(19.0, 20.0),
                    point(18.504, 20.868),
                    point(13.992, 18.29),
                    point(12.008, 18.29),
                    point(6.496, 20.868),
                    point(5.0, 20.0),
                    point(5.0, 5.0),
                    point(7.0, 3.0),
                    point(17.0, 3.0),
                ],
                stroke,
            ));
        }
        _ => {
            painter.add(egui::epaint::PathShape::line(
                vec![
                    point(20.0, 20.0),
                    point(22.0, 18.0),
                    point(22.0, 8.0),
                    point(20.0, 6.0),
                    point(12.1, 6.0),
                    point(10.41, 5.1),
                    point(9.6, 3.9),
                    point(7.93, 3.0),
                    point(4.0, 3.0),
                    point(2.0, 5.0),
                    point(2.0, 18.0),
                    point(4.0, 20.0),
                    point(20.0, 20.0),
                ],
                stroke,
            ));
            painter.line_segment([point(2.0, 10.0), point(22.0, 10.0)], stroke);
        }
    }
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

/// The picker of one block, before any list from a service.
fn model_select_for(block: &ChainBlock) -> ModelSelect {
    let local = block.tone.as_ref().is_some_and(|tone| tone.local);
    let mut models = ModelSelect::new(local);
    if block.tone.is_some() {
        models.set_options(model_options(block), model_total(block));
        if let Some(active) = block.active_model_id {
            models.set_active(active);
        }
    }
    models
}

/// What a replacement of the block changes.
fn tone_identity(block: &ChainBlock) -> Option<(ToneId, bool)> {
    block.tone.as_ref().map(|tone| (tone.id, tone.local))
}

fn model_options(block: &ChainBlock) -> Vec<ModelOption> {
    block
        .tone
        .as_ref()
        .map(|tone| {
            tone.models
                .iter()
                .map(|model| ModelOption {
                    id: model.id,
                    name: model.name.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn model_total(block: &ChainBlock) -> usize {
    block.tone.as_ref().map_or(0, |tone| {
        tone.raw
            .get("models_count")
            .or_else(|| tone.raw.get("a2_models_count"))
            .and_then(|value| value.as_u64())
            .map_or(tone.models.len(), |count| count as usize)
    })
}

/// The public page of a tone. An empty result hides the page button.
fn tone_url(block: &ChainBlock) -> String {
    block
        .tone
        .as_ref()
        .and_then(|tone| tone.raw.get("url"))
        .and_then(|value| value.as_str())
        .filter(|url| is_web_url(url))
        .unwrap_or_default()
        .to_owned()
}

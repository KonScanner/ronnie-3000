//! Block detail takeover, model selection, metadata, and EQ navigation.

use std::sync::Arc;

use eframe::egui::{self, Color32, Rect, Sense, TextureHandle};
use ronie_core::{BlockId, BlockParams, ChainBlock, IrB, ModelId, ModelKind, ToneId, ToneModel};

use crate::{
    backend::{GesturePhase, UiAction, UiBackend, UiError},
    services::{
        community_worker::{ModelList, ModelSwitchRequest, ModelsRequest},
        links::{is_web_url, open_web_url},
        load_flow::{ModelSlot, ModelSwitchFlow, ModelSwitchTicket},
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
    tile::{block_glyph, draw_gear},
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

/// How the detail page looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockPage {
    /// The dark card of the reference layout.
    #[default]
    Reference,
    /// The light Stage page: a title row, GEAR / EQ / INFO tabs and a stage.
    Stage,
}

/// Smallest hit square of a knob on a render, in points.
const MIN_KNOB_TARGET: f32 = 28.0;
/// The least height that a click on a pedal's LCD can land in.
const MIN_LCD_TARGET_HEIGHT: f32 = 20.0;
/// Space between pedals that share the stage, in points.
const PEDAL_GAP: f32 = 6.0;

/// Width of the model picker under the stage.
const PICKER_WIDTH: f32 = 420.0;

/// Tint of the render of a bypassed block.
const BYPASSED_TINT: Color32 = Color32::from_rgb(0xb4, 0xb4, 0xb2);

/// Tallest a rendered skin draws, in points (the design's stage height).
const RENDER_HEIGHT: f32 = 300.0;
/// What a stage with a render keeps free under it, for the SKIN chip at the left and an amp's
/// capture lights in the middle.
const CAPTURE_LIGHTS_ROOM: f32 = 28.0;
/// Height of the SKIN chip at the bottom left of the stage.
const SKIN_CHIP_HEIGHT: f32 = 26.0;
/// Width of the skin list that opens above the chip, the height of one of its rows, and its
/// inner padding.
const SKIN_LIST_WIDTH: f32 = 230.0;
const SKIN_ROW: f32 = 28.0;
const SKIN_LIST_PAD: f32 = 6.0;
/// The space between the chip and the list above it.
const SKIN_LIST_GAP: f32 = 8.0;
/// Tracking of the chip's SKIN caption.
const SKIN_TRACKING: f32 = 1.33;
/// The most capture lights a pill shows at once.
const CAPTURE_LIGHT_COUNT: usize = 8;
const LIGHT_SIZE: f32 = 12.0;
const LIGHT_GAP: f32 = 8.0;
const LIGHT_PITCH: f32 = LIGHT_SIZE + LIGHT_GAP;
const CAPTURE_PILL_HEIGHT: f32 = 26.0;
const CAPTURE_TRACKING: f32 = 1.3;
/// How many characters of the active capture's name the pill shows.
const CAPTURE_NAME_CHARS: usize = 18;
/// The pill of capture lights: the padding at its ends and the gap between its parts.
const CAPTURE_PILL_PAD: f32 = 14.0;
const CAPTURE_PILL_GAP: f32 = 12.0;

/// Width of a cab's MICS panel, and the space between it and the render.
const MICS_PANEL_WIDTH: f32 = 300.0;
const MICS_PANEL_GAP: f32 = 20.0;
/// Inner padding of the MICS panel, and the space between its parts.
const MICS_PAD: f32 = 14.0;
const MICS_GAP: f32 = 10.0;
/// Heights of the parts of the MICS panel: the title, a mic row, the add button and the part
/// with the blend.
const MICS_HEADER: f32 = 12.0;
const MICS_ROW: f32 = 28.0;
const MICS_ADD: f32 = 40.0;
const MICS_FOOTER: f32 = 5.0 + 14.0 + 6.0 + BLEND_KNOB + 6.0 + 13.0;
/// Width of the level readout of a mic row.
const MICS_LEVEL_WIDTH: f32 = 44.0;
/// Side of the BLEND knob.
const BLEND_KNOB: f32 = 46.0;
/// The help line under the add button.
const MICS_HELP: &str = "Blend a second mic: pick another mic position from this cab. Both play at once; BLEND mixes them.";

/// Where the stage of a block offers a slot to add gear: the block each slot is placed
/// against, or `None` where the slot does not show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageSlots {
    /// A pedal after this block (the last pedal of a run), on the pedals stage.
    pub pedal_after: Option<BlockId>,
}

/// A click on a slot of the stage.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotRequest {
    /// The kind of gear the slot adds.
    pub category: ronie_core::gear::GearCategory,
    /// Where the added block goes.
    pub placement: super::Placement,
    /// The slot on screen, for the menu to open beside.
    pub anchor: Rect,
}

/// One slot as the stage lays it out.
struct SlotSpec {
    category: ronie_core::gear::GearCategory,
    placement: super::Placement,
    label: &'static str,
    name: &'static str,
    /// Width and height at the full render height.
    size: egui::Vec2,
}

/// Space between a slot and the render next to it.
const SLOT_GAP: f32 = 18.0;

/// Complete block detail controller.
pub struct BlockDetail {
    backend: Arc<dyn UiBackend>,
    /// Whether the page shows its "Back to chain" link. A layout whose stage always shows a
    /// block has no chain to go back to.
    back_link: bool,
    /// Which page the detail draws.
    page: BlockPage,
    /// Pedals right before and after this one in its lane, which share the light stage.
    neighbours: (Vec<ChainBlock>, Vec<ChainBlock>),
    /// A neighbour pedal that was clicked, for the layout to open.
    select_request: Option<BlockId>,
    /// Where the stage offers a slot to add gear.
    slots: StageSlots,
    /// The slot whose menu is open, drawn solid.
    open_slot: Option<(ronie_core::gear::GearCategory, super::Placement)>,
    /// A slot that was clicked, for the layout to open its menu.
    slot_request: Option<SlotRequest>,
    /// Where each slot was drawn on the latest frame, for an open menu to stay beside it.
    slot_rects: Vec<(ronie_core::gear::GearCategory, super::Placement, Rect)>,
    block_id: BlockId,
    block: ChainBlock,
    calibrate_input: bool,
    view: BlockView,
    models: ModelSelect,
    /// The picker of a cab's second IR: the models of the tone other than the active one.
    models_b: ModelSelect,
    info: BlockInfoPanel,
    eq: EqEditor,
    closed: bool,
    swap_requested: Option<BlockId>,
    /// A click on REPLACE of the light page, by where the button was drawn, for the layout to
    /// open its menu under it.
    replace_request: Option<Rect>,
    /// Whether the replace menu of this block is open, which draws REPLACE solid.
    replace_open: bool,
    /// Where REPLACE was drawn on the latest frame, for an open menu to stay under it.
    replace_rect: Option<Rect>,
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
    /// The first capture light of the amp's window when it has more captures than lights, and
    /// the capture the window last followed.
    light_start: usize,
    light_followed: Option<usize>,
    /// Whether the skin list over the stage's SKIN chip is open.
    skin_open: bool,
    /// The pass that last drew the SKIN chip; the list closes when a pass went without it, as
    /// when another tab or the tuner had the stage.
    skin_drawn: Option<u64>,
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
    /// The slot that the switch changes. All switches of a detail share one generation, so a
    /// detail has at most one pending switch.
    slot: ModelSlot,
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
        let models_b = ModelSelect::new(block.tone.as_ref().is_some_and(|tone| tone.local));
        let eq = EqEditor::new(backend.clone(), block_id)?;
        let mut info = BlockInfoPanel::default();
        info.set_tone(block.tone.clone());
        info.set_page_url(tone_url(&block));
        Ok(Self {
            backend,
            back_link: true,
            page: BlockPage::Reference,
            neighbours: (Vec::new(), Vec::new()),
            select_request: None,
            slots: StageSlots::default(),
            open_slot: None,
            slot_request: None,
            slot_rects: Vec::new(),
            block_id: id,
            block,
            calibrate_input,
            view: BlockView::Tone,
            models,
            models_b,
            info,
            eq,
            closed: false,
            swap_requested: None,
            replace_request: None,
            replace_open: false,
            replace_rect: None,
            sign_in_requested: false,
            artwork: None,
            avatar: None,
            remote: None,
            trim_drag: OpenDrag::default(),
            synced_revision: None,
            light_start: 0,
            light_followed: None,
            skin_open: false,
            skin_drawn: None,
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
            return self.request_switch(ModelSlot::A, id);
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

    /// Sets the second IR of a cab to another model of its tone, or removes it with `None`.
    ///
    /// A catalog tone waits for the bytes of the model, as a switch of the active model does.
    /// A removal also ends a download of B.
    ///
    /// # Errors
    /// Returns a signed-out, unknown-model, or backend error.
    pub fn select_second_model(
        &mut self,
        id: Option<ModelId>,
        session: SessionStatus,
    ) -> Result<(), UiError> {
        let Some(id) = id else {
            if self.pending_slot() == Some(ModelSlot::B) {
                self.revoke_switch();
            }
            self.models_b.clear_target();
            if self.block.ir_b.is_none() {
                return Ok(());
            }
            self.backend.dispatch(UiAction::SelectSecondModel {
                block_id: self.block_id.clone(),
                model_id: None,
            })?;
            self.block.ir_b = None;
            self.block.ir_b_missing = false;
            self.synced_revision = None;
            return Ok(());
        };
        self.refresh_b_picker();
        if self.models_b.select(id, session).is_none() {
            return Err(UiError::InvalidAction(
                "model is not selectable as the second IR".to_owned(),
            ));
        }
        if self.remote.is_some() && self.holds_catalog_tone() {
            return self.request_switch(ModelSlot::B, id);
        }
        self.backend.dispatch(UiAction::SelectSecondModel {
            block_id: self.block_id.clone(),
            model_id: Some(id),
        })?;
        self.block.ir_b = Some(IrB { model_id: id });
        self.block.ir_b_missing = false;
        self.synced_revision = None;
        Ok(())
    }

    /// Inverts the polarity of a cab's second IR, or puts it back.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_ir_b_invert(&mut self, inverted: bool) -> Result<(), UiError> {
        self.block.params.ir_b_invert = inverted;
        self.write_params()
    }

    /// The picker of a cab's second IR.
    #[must_use]
    pub const fn second_model_select(&self) -> &ModelSelect {
        &self.models_b
    }

    /// Gives this detail a community service. A catalog tone then lists its models through the
    /// service, and a choice waits for its bytes. A local tone stays on the stash path.
    pub fn attach_model_switching(&mut self, flow: ModelSwitchFlow) {
        self.models.set_deferred(self.holds_catalog_tone());
        self.models_b.set_deferred(self.holds_catalog_tone());
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
        let Some((slot, model)) = self.take_pending(generation) else {
            return false;
        };
        let picker = self.picker_mut(slot);
        picker.set_active(model);
        picker.clear_target();
        self.sync();
        true
    }

    /// Ends the current switch, which failed. The active model stays, and the same choice can
    /// start again. Returns false for any other switch.
    pub fn fail_switch(&mut self, generation: u64) -> bool {
        let Some((slot, _)) = self.take_pending(generation) else {
            return false;
        };
        self.picker_mut(slot).clear_target();
        true
    }

    /// Stops the model work of a session that ended. The next signed-in session asks again.
    pub fn abandon_model_work(&mut self) {
        self.revoke_switch();
        self.models.clear_target();
        self.models_b.clear_target();
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

    /// Starts a remote switch of one slot. A choice of the model that already downloads changes
    /// nothing, and any other choice ends the switch in progress, of either slot. A choice of
    /// the model that the slot holds needs no download, unless it is a B with no bytes.
    fn request_switch(&mut self, slot: ModelSlot, id: ModelId) -> Result<(), UiError> {
        let pending = self
            .remote
            .as_ref()
            .and_then(|remote| remote.pending.as_ref())
            .map(|pending| (pending.slot, pending.model));
        if pending == Some((slot, id)) {
            return Ok(());
        }
        let held = self.slot_model(slot);
        let missing = slot == ModelSlot::B && self.block.ir_b_missing;
        if held == Some(id) && !missing {
            // The slot already plays this model. A switch of this slot to another model ends,
            // and a switch of the other slot goes on.
            if pending.is_some_and(|(other, _)| other == slot) {
                self.revoke_switch();
            }
            return Ok(());
        }
        self.revoke_switch();
        if let Some((other, _)) = pending.filter(|(other, _)| *other != slot) {
            self.picker_mut(other).clear_target();
        }
        let tone = self.block.tone.clone();
        let Some(remote) = &mut self.remote else {
            return Ok(());
        };
        let model = remote.rows.iter().find(|model| model.id == id).cloned();
        let (Some(tone), Some(model)) = (tone, model) else {
            self.picker_mut(slot).clear_target();
            return Err(UiError::InvalidAction(
                "model is not in the current list".to_owned(),
            ));
        };
        let ticket = remote.flow.begin_slot(self.block_id.clone(), slot, held);
        remote.pending = Some(PendingSwitch {
            ticket: ticket.clone(),
            model: id,
            slot,
        });
        remote.unsent = Some(ModelSwitchRequest {
            tone,
            model,
            ticket,
        });
        Ok(())
    }

    /// The model that a slot of the block holds.
    fn slot_model(&self, slot: ModelSlot) -> Option<ModelId> {
        match slot {
            ModelSlot::A => self.block.active_model_id,
            ModelSlot::B => self.block.ir_b.map(|ir_b| ir_b.model_id),
        }
    }

    fn picker_mut(&mut self, slot: ModelSlot) -> &mut ModelSelect {
        match slot {
            ModelSlot::A => &mut self.models,
            ModelSlot::B => &mut self.models_b,
        }
    }

    /// The slot of the switch in progress.
    fn pending_slot(&self) -> Option<ModelSlot> {
        self.remote
            .as_ref()
            .and_then(|remote| remote.pending.as_ref())
            .map(|pending| pending.slot)
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

    fn take_pending(&mut self, generation: u64) -> Option<(ModelSlot, ModelId)> {
        let remote = self.remote.as_mut()?;
        let pending = remote
            .pending
            .take_if(|pending| pending.ticket.generation() == generation)?;
        Some((pending.slot, pending.model))
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
            self.models_b =
                ModelSelect::new(self.block.tone.as_ref().is_some_and(|tone| tone.local));
            self.models_b.set_deferred(self.holds_catalog_tone());
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
            .and_then(|remote| remote.pending.as_ref())
            .map(|pending| (pending.slot, pending.model, pending.ticket.replaces()));
        if let Some((slot, model, replaces)) = pending {
            // The slot holds the old model, or the new model before its audio is prepared.
            let held = self.slot_model(slot);
            if held == Some(model) || held == replaces {
                if slot == ModelSlot::A {
                    return;
                }
            } else {
                self.revoke_switch();
                self.picker_mut(slot).clear_target();
            }
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
        let switching = self.pending_slot() == Some(ModelSlot::A);
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

    /// A click on REPLACE of the light page, once, by where the button was drawn.
    pub fn take_replace_request(&mut self) -> Option<Rect> {
        self.replace_request.take()
    }

    /// Draws REPLACE solid while the replace menu of this block is open.
    pub fn set_replace_open(&mut self, open: bool) {
        self.replace_open = open;
    }

    /// Where REPLACE of the light page was drawn on the latest frame.
    #[must_use]
    pub const fn replace_rect(&self) -> Option<Rect> {
        self.replace_rect
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
        if self.page == BlockPage::Stage {
            self.show_stage_page(ui, session);
            return;
        }
        let bounds = ui.available_rect_before_wrap();
        let card_x = bounds.left() + (bounds.width() - 800.0) / 2.0;
        let back = Rect::from_min_size(
            egui::pos2(card_x, bounds.top() + 24.0),
            egui::vec2(104.0, 22.0),
        );
        if self.back_link && back_link(ui, back).clicked() {
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
        if self.page == BlockPage::Stage {
            self.show_gear_stage(ui, rect, session);
            return;
        }
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
            draw_gear(&painter, rect, block_glyph(&self.block));
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

    /// Shows or hides the "Back to chain" link.
    pub fn set_back_link(&mut self, shown: bool) {
        self.back_link = shown;
    }

    /// The block this detail shows.
    #[must_use]
    pub const fn block_id(&self) -> &BlockId {
        &self.block_id
    }

    /// The page the detail draws.
    #[must_use]
    pub const fn page(&self) -> BlockPage {
        self.page
    }

    /// Sets the pedals that share the stage with this one: those right before it in its lane,
    /// in chain order, and those right after it.
    pub fn set_stage_neighbours(&mut self, before: Vec<ChainBlock>, after: Vec<ChainBlock>) {
        self.neighbours = (before, after);
    }

    /// The neighbour pedal the user clicked on the stage, once.
    pub fn take_select_request(&mut self) -> Option<BlockId> {
        self.select_request.take()
    }

    /// Sets where the stage offers a slot to add gear.
    pub fn set_stage_slots(&mut self, slots: StageSlots) {
        self.slots = slots;
        self.slot_rects.clear();
    }

    /// Where the slot of `category` that places a block at `placement` was drawn since the
    /// latest `set_stage_slots`, if it was.
    #[must_use]
    pub fn slot_rect(
        &self,
        category: ronie_core::gear::GearCategory,
        placement: &super::Placement,
    ) -> Option<Rect> {
        self.slot_rects
            .iter()
            .find(|(each, place, _)| *each == category && place == placement)
            .map(|(_, _, rect)| *rect)
    }

    /// The slot the user clicked on the stage, once.
    pub fn take_slot_request(&mut self) -> Option<SlotRequest> {
        self.slot_request.take()
    }

    /// Draws the slot of a kind of gear and a placement solid while its menu is open.
    pub fn set_open_slot(
        &mut self,
        slot: Option<(ronie_core::gear::GearCategory, super::Placement)>,
    ) {
        self.open_slot = slot;
    }

    /// Chooses the page the detail draws.
    pub fn set_page(&mut self, page: BlockPage) {
        self.page = page;
    }

    /// The light Stage page: title row, tabs, then the body of the open tab.
    fn show_stage_page(&mut self, ui: &mut egui::Ui, session: SessionStatus) {
        let look = crate::theme::Look::get(ui.ctx());
        let bounds = ui.available_rect_before_wrap();
        let page = Rect::from_min_max(
            bounds.min + egui::vec2(24.0, 12.0),
            bounds.max - egui::vec2(24.0, 14.0),
        );
        let title_row = Rect::from_min_size(page.min, egui::vec2(page.width(), 36.0));
        self.show_title_row(ui, title_row, look);
        let tabs = Rect::from_min_size(title_row.left_bottom(), egui::vec2(page.width(), 30.0));
        self.show_tabs(ui, tabs, look);
        let body = Rect::from_min_max(tabs.left_bottom(), page.max);
        match self.view {
            BlockView::Tone => self.show_tone(ui, body, session),
            BlockView::Eq => self.show_light_eq(ui, body, look),
            BlockView::Info => self.show_light_info(ui, body, look),
        }
    }

    /// The EQ tab of the light page: the curve on a light card, and a row with the EQ power,
    /// the band chips and the PRE / POST switch.
    fn show_light_eq(&mut self, ui: &mut egui::Ui, rect: Rect, look: crate::theme::Look) {
        if self.eq.view() != EqViewMode::Curve {
            self.eq.set_view(EqViewMode::Curve);
        }
        let card = Rect::from_min_max(
            rect.min + egui::vec2(0.0, 12.0),
            rect.max - egui::vec2(0.0, 36.0),
        );
        ui.painter().rect_filled(card, 10.0, look.panel);
        ui.painter().rect_stroke(
            card,
            10.0,
            egui::Stroke::new(1.0, look.card_edge),
            egui::StrokeKind::Inside,
        );
        let inner = card.shrink(1.0);
        let mut card_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        card_ui.set_clip_rect(inner);
        self.eq.show(&mut card_ui);

        let cy = card.bottom() + 20.0;
        let mut x = rect.left();
        let enabled = self.eq.params().enabled;
        let label = if enabled { "EQ ON" } else { "EQ OFF" };
        let width = text_width(ui, label, 10.0) + 4.0;
        let power = Rect::from_min_max(egui::pos2(x, cy - 11.0), egui::pos2(x + width, cy + 11.0));
        if page_toggle(ui, power, label, "EQ power", enabled, look).clicked() {
            let _ = self.set_eq_enabled(!enabled);
        }
        x = power.right() + 16.0;
        for index in 0..self.eq.params().bands.len() {
            let chip = Rect::from_min_size(egui::pos2(x, cy - 11.0), egui::Vec2::splat(22.0));
            x = chip.right() + 6.0;
            let selected = self.eq.selected() == index;
            let name = format!("Select band {}", index + 1);
            let response = ui.interact(
                chip,
                ui.scope_id().with(("band-chip", index)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, true, selected, &name)
            });
            let fill = if selected {
                look.accent
            } else if response.hovered() {
                look.chip_hover
            } else {
                look.fill_soft
            };
            ui.painter().rect_filled(chip, 4.0, fill);
            ui.painter().text(
                chip.center(),
                egui::Align2::CENTER_CENTER,
                (index + 1).to_string(),
                bold_font(11.0),
                if selected { Color32::WHITE } else { look.text },
            );
            if response.clicked() {
                self.eq.select_band(index);
            }
        }

        // PRE [switch] POST on the right; the knob sits by the active word.
        let pre = self.eq.params().pre;
        let pre_width = text_width(ui, "PRE", 11.0);
        let post_width = text_width(ui, "POST", 11.0);
        let total = pre_width + 8.0 + 30.0 + 8.0 + post_width;
        let area = Rect::from_min_max(
            egui::pos2(rect.right() - total, cy - 11.0),
            egui::pos2(rect.right(), cy + 11.0),
        );
        let response = ui.interact(area, ui.scope_id().with("eq-pre-post"), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, pre, "PRE / POST")
        });
        let painter = ui.painter();
        let word = |on: bool| if on { look.text } else { look.text_idle };
        painter.text(
            egui::pos2(area.left(), cy),
            egui::Align2::LEFT_CENTER,
            "PRE",
            bold_font(11.0),
            word(pre),
        );
        let track = Rect::from_min_size(
            egui::pos2(area.left() + pre_width + 8.0, cy - 8.0),
            egui::vec2(30.0, 16.0),
        );
        painter.rect_filled(track, 8.0, look.fill_soft);
        painter.rect_stroke(
            track,
            8.0,
            egui::Stroke::new(1.0, look.control_edge),
            egui::StrokeKind::Inside,
        );
        let knob_x = if pre {
            track.left() + 8.0
        } else {
            track.right() - 8.0
        };
        painter.circle_filled(egui::pos2(knob_x, cy), 6.0, look.text);
        painter.text(
            egui::pos2(area.right(), cy),
            egui::Align2::RIGHT_CENTER,
            "POST",
            bold_font(11.0),
            word(!pre),
        );
        if response.clicked() {
            let _ = self.set_eq_pre(!pre);
        }
    }

    /// The INFO tab of the light page: artwork, title, maker and counts, makes and tags,
    /// description and the tone's page.
    fn show_light_info(&mut self, ui: &mut egui::Ui, rect: Rect, look: crate::theme::Look) {
        let Some(tone) = self.block.tone.clone() else {
            return;
        };
        let side = (rect.height() - 24.0).clamp(80.0, 200.0);
        let artwork =
            Rect::from_min_size(rect.min + egui::vec2(0.0, 16.0), egui::Vec2::splat(side));
        self.show_artwork(ui, artwork);
        let column = Rect::from_min_max(
            egui::pos2(artwork.right() + 24.0, artwork.top()),
            rect.right_bottom(),
        );
        let mut info_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(column)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        info_ui.set_clip_rect(column);
        info_ui.spacing_mut().item_spacing = egui::vec2(6.0, 10.0);
        info_ui.add(
            egui::Label::new(
                egui::RichText::new(&tone.title)
                    .font(bold_font(18.0))
                    .color(look.text),
            )
            .selectable(false)
            .truncate(),
        );
        let maker = tone.raw.get("user").and_then(|user| {
            user.get("display_name")
                .or_else(|| user.get("username"))
                .and_then(serde_json::Value::as_str)
        });
        let count = |key: &str| tone.raw.get(key).and_then(serde_json::Value::as_u64);
        let mut byline = Vec::new();
        if let Some(maker) = maker {
            byline.push(format!("by {maker}"));
        }
        if let Some(downloads) = count("downloads_count") {
            byline.push(format!("{} downloads", format_count(downloads)));
        }
        if let Some(favorites) = count("favorites_count") {
            byline.push(format!("{} favourites", format_count(favorites)));
        }
        if !byline.is_empty() {
            info_ui.add(
                egui::Label::new(
                    egui::RichText::new(byline.join(" · "))
                        .size(12.0)
                        .color(look.text_muted),
                )
                .selectable(false)
                .truncate(),
            );
        }
        let chips: Vec<String> = super::block_info::clean_strings(tone.raw.get("makes"))
            .into_iter()
            .chain(super::block_info::clean_strings(tone.raw.get("tags")))
            .take(8)
            .collect();
        if !chips.is_empty() {
            info_ui.horizontal_wrapped(|ui| {
                for chip in &chips {
                    let galley = ui.painter().layout_no_wrap(
                        chip.clone(),
                        egui::FontId::proportional(11.0),
                        look.text,
                    );
                    let (chip_rect, _) = ui.allocate_exact_size(
                        galley.size() + egui::vec2(20.0, 12.0),
                        Sense::hover(),
                    );
                    let radius = chip_rect.height() / 2.0;
                    ui.painter().rect_filled(chip_rect, radius, look.panel);
                    ui.painter().rect_stroke(
                        chip_rect,
                        radius,
                        egui::Stroke::new(1.0, look.control_edge),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter()
                        .galley(chip_rect.min + egui::vec2(10.0, 6.0), galley, look.text);
                }
            });
        }
        if let Some(text) = super::block_info::description(&tone) {
            info_ui.add(
                egui::Label::new(egui::RichText::new(text).size(12.0).color(look.text_body))
                    .selectable(false)
                    .wrap(),
            );
        }
        let url = self.share_url();
        if is_web_url(&url) {
            let galley = info_ui.painter().layout_no_wrap(
                "Open on TONE3000".to_owned(),
                bold_font(12.0),
                look.text,
            );
            let (button, response) =
                info_ui.allocate_exact_size(galley.size() + egui::vec2(28.0, 16.0), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Link, true, "Open on TONE3000")
            });
            let painter = info_ui.painter();
            let fill = if response.hovered() {
                look.panel_hover
            } else {
                look.panel
            };
            painter.rect_filled(button, 6.0, fill);
            painter.rect_stroke(
                button,
                6.0,
                egui::Stroke::new(1.0, look.control_edge),
                egui::StrokeKind::Inside,
            );
            painter.galley(button.min + egui::vec2(14.0, 8.0), galley, look.text);
            if response.clicked() {
                open_web_url(ui.ctx(), url);
            }
        }
    }

    /// "AMP · TITLE" over the source line, with BYPASS / REPLACE / REMOVE on the right.
    fn show_title_row(&mut self, ui: &mut egui::Ui, rect: Rect, look: crate::theme::Look) {
        let bold = egui::FontId::new(10.0, egui::FontFamily::Name("arimo-bold".into()));
        let title = if self.neighbours.0.is_empty() && self.neighbours.1.is_empty() {
            page_title(&self.block)
        } else {
            let titles: Vec<String> = self
                .neighbours
                .0
                .iter()
                .chain(std::iter::once(&self.block))
                .chain(&self.neighbours.1)
                .map(|block| {
                    block
                        .tone
                        .as_ref()
                        .map_or(String::new(), |tone| tone.title.to_uppercase())
                })
                .collect();
            format!("PEDALS · {}", titles.join(" → "))
        };
        let right_reserve = [PageAction::Bypass, PageAction::Replace, PageAction::Remove]
            .into_iter()
            .map(|action| page_action_width(ui, action) + PAGE_ACTION_GAP)
            .sum::<f32>()
            + 8.0;
        let label_width = (rect.width() - right_reserve).max(40.0);
        let mut text_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    rect.min,
                    egui::vec2(label_width, rect.height()),
                ))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        text_ui.spacing_mut().item_spacing.y = 2.0;
        text_ui.add_space(3.0);
        text_ui.add(
            egui::Label::new(egui::RichText::new(title).font(bold).color(look.text))
                .selectable(false)
                .truncate(),
        );
        text_ui.add(
            egui::Label::new(
                egui::RichText::new(page_meta(&self.block))
                    .size(11.0)
                    .color(look.text_muted),
            )
            .selectable(false)
            .truncate(),
        );
        let mut right = rect.right();
        let cy = rect.center().y;
        let actions = [PageAction::Remove, PageAction::Replace, PageAction::Bypass];
        for (index, action) in actions.into_iter().enumerate() {
            if index > 0 {
                right -= PAGE_ACTION_GAP;
            }
            let width = page_action_width(ui, action);
            let button = Rect::from_min_max(
                egui::pos2(right - width, cy - PAGE_ACTION_HEIGHT / 2.0),
                egui::pos2(right, cy + PAGE_ACTION_HEIGHT / 2.0),
            );
            right = button.left();
            let selected = match action {
                PageAction::Bypass => !self.block.params.enabled,
                PageAction::Replace => self.replace_open,
                PageAction::Remove => false,
            };
            if action == PageAction::Replace {
                self.replace_rect = Some(button);
            }
            if page_action_button(ui, button, action, selected, look).clicked() {
                match action {
                    PageAction::Bypass => {
                        let _ = self.set_enabled(!self.block.params.enabled);
                    }
                    PageAction::Replace => self.replace_request = Some(button),
                    PageAction::Remove => {
                        let _ = self.remove();
                    }
                }
            }
        }
    }

    /// GEAR / EQ / INFO, with a line under the open tab.
    fn show_tabs(&mut self, ui: &mut egui::Ui, rect: Rect, look: crate::theme::Look) {
        let painter = ui.painter_at(rect.expand2(egui::vec2(0.0, 1.0)));
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, look.edge),
        );
        let local = self.block.tone.as_ref().is_some_and(|tone| tone.local);
        let mut tabs = vec![("GEAR", BlockView::Tone), ("EQ", BlockView::Eq)];
        if !local {
            tabs.push(("INFO", BlockView::Info));
        }
        let font = egui::FontId::new(10.0, egui::FontFamily::Name("arimo-bold".into()));
        let mut x = rect.left();
        for (label, view) in tabs {
            let width = text_width(ui, label, 10.0) + 4.0;
            let tab = Rect::from_min_max(
                egui::pos2(x, rect.bottom() - 28.0),
                egui::pos2(x + width, rect.bottom()),
            );
            x = tab.right() + 20.0;
            let response =
                ui.interact(tab, ui.scope_id().with(("page-tab", label)), Sense::click());
            let open = self.view == view;
            response
                .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, label));
            let color = if open || response.hovered() {
                look.text
            } else {
                look.text_idle
            };
            painter.text(
                tab.center(),
                egui::Align2::CENTER_CENTER,
                label,
                font.clone(),
                color,
            );
            if open {
                painter.rect_filled(
                    Rect::from_min_max(
                        egui::pos2(tab.left(), tab.bottom() - 2.0),
                        tab.right_bottom(),
                    ),
                    0.0,
                    look.text,
                );
            }
            if response.clicked() && !open {
                self.set_view(view);
            }
        }
    }

    /// The GEAR tab of the light page: meters at the sides, the render (or the artwork) in the
    /// middle with its knobs or trims, and a row under it with the size and normalize toggles,
    /// the model picker and a note.
    fn show_gear_stage(&mut self, ui: &mut egui::Ui, rect: Rect, session: SessionStatus) {
        let look = crate::theme::Look::get(ui.ctx());
        let row = Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - 36.0),
            rect.right_bottom(),
        );
        let meter_top = rect.top() + 14.0;
        let meter_bottom = row.top() - 8.0;
        self.show_meter(
            ui,
            Rect::from_min_max(
                egui::pos2(rect.left(), meter_top),
                egui::pos2(rect.left() + 36.0, meter_bottom),
            ),
            true,
        );
        self.show_meter(
            ui,
            Rect::from_min_max(
                egui::pos2(rect.right() - 36.0, meter_top),
                egui::pos2(rect.right(), meter_bottom),
            ),
            false,
        );
        let stage = Rect::from_min_max(
            egui::pos2(rect.left() + 56.0, rect.top() + 6.0),
            egui::pos2(rect.right() - 56.0, row.top() - 4.0),
        );
        // A cab with more than one mic position shows its MICS panel at the right of the stage,
        // and the render moves left of it.
        let mics = self.mics_offered();
        let stage_full = stage;
        let stage = if mics {
            Rect::from_min_max(
                stage.min,
                egui::pos2(
                    (stage.right() - MICS_PANEL_WIDTH - MICS_PANEL_GAP).max(stage.left()),
                    stage.bottom(),
                ),
            )
        } else {
            stage
        };
        let skin = super::skins::skin_for(&self.block)
            .and_then(|skin| Some((skin, super::skins::textures(ui.ctx(), skin.id)?)));
        let lights = skin.as_ref().is_some_and(|(skin, _)| {
            skin.category == ronie_core::gear::GearCategory::Amp && self.models.options().len() >= 2
        });
        // The SKIN chip sits at the bottom left of the stage, level with the lights. When the
        // pill of lights does not fit between it and the right of the stage, the pill drops
        // its short name first (the picker shows it), and then the chip reads only "SKIN".
        let mut chip = skin
            .as_ref()
            .map(|(shown, _)| self.skin_chip_rect(ui, stage, *shown, false));
        let clear_of = |chip: &Option<(Rect, String)>| {
            chip.as_ref()
                .map_or(stage.left(), |(rect, _)| rect.right() + 12.0)
        };
        let mut light_name = true;
        if lights {
            let fits = |clear: f32, with_name: bool| {
                clear + self.capture_pill_width(ui, with_name) <= stage.right()
            };
            if !fits(clear_of(&chip), true) {
                light_name = false;
                if !fits(clear_of(&chip), false) {
                    chip = skin
                        .as_ref()
                        .map(|(shown, _)| self.skin_chip_rect(ui, stage, *shown, true));
                }
            }
        }
        let gear = match &skin {
            Some((skin, textures)) => {
                // The render leaves room under it for the chip and an amp's lights.
                let render_stage = Rect::from_min_max(
                    stage.min,
                    egui::pos2(stage.right(), stage.bottom() - CAPTURE_LIGHTS_ROOM),
                );
                let body = if matches!(
                    skin.category,
                    ronie_core::gear::GearCategory::Pedal | ronie_core::gear::GearCategory::Amp
                ) {
                    self.show_pedalboard(ui, render_stage, textures, skin.category)
                } else {
                    textures.manifest.fit(render_stage, RENDER_HEIGHT)
                };
                // The lights register first, so a knob keeps the clicks where they overlap.
                if lights {
                    let room = (clear_of(&chip), stage.right());
                    self.show_capture_lights(
                        ui,
                        body,
                        stage.bottom() - 2.0,
                        room,
                        light_name,
                        session,
                    );
                }
                self.show_render(ui, body, *skin, textures, session);
                if textures.manifest.knobs.is_empty() {
                    self.show_trims_beside(ui, body, stage);
                }
                body
            }
            None => {
                let side = stage.height().min(200.0);
                let artwork = Rect::from_center_size(stage.center(), egui::Vec2::splat(side));
                self.show_artwork(ui, artwork);
                self.show_trims_beside(ui, artwork, stage);
                artwork
            }
        };
        let _ = gear;
        if mics {
            self.show_mics_panel(ui, stage_full, session, look);
        }
        match (&skin, chip) {
            (Some((shown, _)), Some((rect, value))) => {
                self.show_skin_chip(ui, rect, &value, *shown, look);
            }
            _ => self.skin_open = false,
        }

        // The row under the stage.
        let cy = row.center().y;
        let mut x = stage.left();
        if self.show_slim_size_control() {
            let full = self.block.params.slim_size >= 0.5;
            let label = if full { "FULL" } else { "LITE" };
            let width = text_width(ui, label, 10.0) + 4.0;
            let button =
                Rect::from_min_max(egui::pos2(x, cy - 11.0), egui::pos2(x + width, cy + 11.0));
            if page_toggle(ui, button, label, "Full size", full, look).clicked() {
                let _ = self.set_slim_full(!full);
            }
            x = button.right() + 16.0;
        }
        if self.show_normalize_control() {
            let overridden = self.normalize_overridden();
            let on = self.block.params.normalize && !overridden;
            let width = text_width(ui, "NORMALIZE", 10.0) + 4.0;
            let button =
                Rect::from_min_max(egui::pos2(x, cy - 11.0), egui::pos2(x + width, cy + 11.0));
            if page_text_button(ui, button, "NORMALIZE", on, look).clicked() && !overridden {
                let _ = self.set_normalize(!self.block.params.normalize);
            }
        }
        if mics {
            // Row A of the panel holds the picker, and the panel replaces the note.
            return;
        }
        let picker = Rect::from_center_size(
            row.center(),
            egui::vec2(PICKER_WIDTH.min(row.width()), 36.0),
        );
        let mut picker_ui = ui.new_child(egui::UiBuilder::new().max_rect(picker));
        if let Some(id) = self.models.show(&mut picker_ui, session) {
            let _ = self.select_model(id, session);
        }
        let note_right = stage.right();
        let note_width = (note_right - picker.right() - 24.0).clamp(0.0, 230.0);
        if note_width > 80.0 {
            let note = if skin
                .as_ref()
                .is_some_and(|(_, textures)| !textures.manifest.knobs.is_empty())
            {
                "Drag a knob on the render to turn it; its value shows while you drag."
            } else {
                "Drag a trim to set the level and the mix."
            };
            let galley = ui.painter().layout(
                note.to_owned(),
                egui::FontId::proportional(10.0),
                look.text_muted,
                note_width,
            );
            let pos = egui::pos2(note_right - galley.size().x, cy - galley.size().y / 2.0);
            ui.painter().galley(pos, galley, look.text_muted);
        }
    }

    /// Where the SKIN chip goes at the bottom left of `stage`, and the choice it reads:
    /// "Auto · <family>" or the chosen family, or nothing when `compact`.
    fn skin_chip_rect(
        &self,
        ui: &egui::Ui,
        stage: Rect,
        shown: super::skins::BlockSkin,
        compact: bool,
    ) -> (Rect, String) {
        let value = if compact {
            String::new()
        } else if super::skins::auto_skin_for(&self.block)
            .and_then(|auto| super::skins::chosen_skin(&self.block, auto))
            .is_some()
        {
            super::skins::friendly_name(shown.id).to_owned()
        } else {
            format!("Auto \u{b7} {}", super::skins::friendly_name(shown.id))
        };
        let caption = tracked_width(ui.painter(), "SKIN", &bold_font(9.5), SKIN_TRACKING);
        let value_width = if value.is_empty() {
            0.0
        } else {
            8.0 + text_width(ui, &value, 11.0)
        };
        let width = 10.0 + caption + value_width + 8.0 + 10.0 + 10.0;
        let rect = Rect::from_min_size(
            egui::pos2(stage.left(), stage.bottom() - 2.0 - SKIN_CHIP_HEIGHT),
            egui::vec2(width.min(stage.width()), SKIN_CHIP_HEIGHT),
        );
        (rect, value)
    }

    /// The SKIN chip, and the list of the skins of the block's kind of gear that it opens
    /// upward: Auto first, with the automatic family on its right, then each skin by name.
    fn show_skin_chip(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        value: &str,
        shown: super::skins::BlockSkin,
        look: crate::theme::Look,
    ) {
        let pass = ui.ctx().cumulative_pass_nr();
        if self.skin_drawn.is_none_or(|drawn| drawn + 1 < pass) {
            self.skin_open = false;
        }
        self.skin_drawn = Some(pass);
        let id = ui.scope_id().with(("skin-chip", &self.block_id.0));
        let response = ui.interact(rect, id, Sense::click());
        let open = self.skin_open;
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, "Skin"));
        let border = if response.hovered() || open {
            look.edge_strong
        } else {
            look.control_edge
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            SKIN_CHIP_HEIGHT / 2.0,
            look.panel,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        let clip = painter.with_clip_rect(rect.shrink(1.0).intersect(painter.clip_rect()));
        let caption_font = bold_font(9.5);
        let caption = tracked_width(&clip, "SKIN", &caption_font, SKIN_TRACKING);
        let mut x = rect.left() + 10.0;
        paint_tracked(
            &clip,
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "SKIN",
            &caption_font,
            look.text_muted,
            SKIN_TRACKING,
        );
        x += caption + 8.0;
        if !value.is_empty() {
            let galley = clip.layout_no_wrap(
                value.to_owned(),
                egui::FontId::proportional(11.0),
                look.text_body,
            );
            let value_rect = Rect::from_min_size(
                egui::pos2(x, rect.center().y - galley.size().y / 2.0),
                galley.size(),
            );
            clip.galley(value_rect.min, galley, look.text);
            static_text(ui, value_rect, id.with("value"), value);
        }
        // The chevron points up, where the list opens.
        let chevron = Rect::from_center_size(
            egui::pos2(rect.right() - 15.0, rect.center().y),
            egui::Vec2::splat(10.0),
        );
        let at = |x: f32, y: f32| chevron.min + egui::vec2(x, y) * (10.0 / 24.0);
        clip.line(
            vec![at(6.0, 15.0), at(12.0, 9.0), at(18.0, 15.0)],
            egui::Stroke::new(1.2, look.text),
        );
        if response.clicked() {
            self.skin_open = !self.skin_open;
        }
        if !self.skin_open {
            return;
        }

        let Some(auto) = super::skins::auto_skin_for(&self.block) else {
            self.skin_open = false;
            return;
        };
        let chosen = super::skins::chosen_skin(&self.block, auto).map(|skin| skin.id);
        let rows: Vec<(Option<&'static str>, &'static str, &'static str)> =
            std::iter::once((None, "Auto", super::skins::friendly_name(auto.id)))
                .chain(
                    super::skins::skins_in(shown.category)
                        .into_iter()
                        .map(|id| (Some(id), super::skins::friendly_name(id), "")),
                )
                .collect();
        // The list ends above the chip, and its rows scroll when the window is too short for
        // all of them, so the list never covers the chip that closes it.
        let full = 2.0 * SKIN_LIST_PAD + rows.len() as f32 * SKIN_ROW;
        let screen = ui.ctx().content_rect();
        let bottom = rect.top() - SKIN_LIST_GAP;
        let height = full.min(bottom - (screen.top() + 4.0)).max(0.0);
        let list = Rect::from_min_size(
            egui::pos2(rect.left(), bottom - height),
            egui::vec2(SKIN_LIST_WIDTH, height),
        );
        let mut picked = None;
        egui::Area::new(id.with("list"))
            .order(egui::Order::Foreground)
            .fixed_pos(list.min)
            .show(ui.ctx(), |ui| {
                ui.set_min_size(list.size());
                ui.set_max_size(list.size());
                let painter = ui.painter();
                painter.add(
                    egui::epaint::Shadow {
                        offset: [0, 10],
                        blur: 24,
                        spread: 0,
                        color: look.popover_shadow,
                    }
                    .as_shape(list, 8.0),
                );
                painter.rect(
                    list,
                    8.0,
                    look.panel,
                    egui::Stroke::new(1.0, look.control_edge),
                    egui::StrokeKind::Inside,
                );
                let group = ui.interact(list, id.with("list-group"), Sense::hover());
                group.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ListBox, true, "Skins"));
                let inner = list.shrink(SKIN_LIST_PAD);
                let mut rows_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
                egui::ScrollArea::vertical()
                    .id_salt(id.with("list-scroll"))
                    .auto_shrink(false)
                    .max_height(inner.height())
                    .show(&mut rows_ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (index, (skin, name, note)) in rows.iter().enumerate() {
                            let (row, _) = ui.allocate_exact_size(
                                egui::vec2(inner.width(), SKIN_ROW),
                                Sense::hover(),
                            );
                            let selected = *skin == chosen;
                            let response =
                                ui.interact(row, id.with(("row", index)), Sense::click());
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::ListBoxOption,
                                    true,
                                    selected,
                                    *name,
                                )
                            });
                            let painter = ui.painter();
                            if selected {
                                painter.rect_filled(row, 5.0, look.fill_soft);
                            } else if response.hovered() {
                                painter.rect_filled(row, 5.0, look.row_hover);
                            }
                            let font = if selected {
                                bold_font(11.5)
                            } else {
                                egui::FontId::proportional(11.5)
                            };
                            painter.text(
                                egui::pos2(row.left() + 10.0, row.center().y),
                                egui::Align2::LEFT_CENTER,
                                *name,
                                font,
                                look.text,
                            );
                            if !note.is_empty() {
                                let note_rect = painter.text(
                                    egui::pos2(row.right() - 10.0, row.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    *note,
                                    egui::FontId::proportional(10.0),
                                    look.text_muted,
                                );
                                static_text(ui, note_rect, id.with(("note", index)), note);
                            }
                            if response.clicked() {
                                picked = Some(*skin);
                            }
                        }
                    });
            });
        if let Some(skin) = picked {
            self.skin_open = false;
            // Against the stored skin, so Auto also clears one the block ignores.
            if skin != self.block.skin.as_deref() {
                let _ = self.set_skin(skin.map(str::to_owned));
            }
            return;
        }
        let (escape, clicked_at) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::Escape),
                input
                    .pointer
                    .primary_clicked()
                    .then(|| input.pointer.latest_pos())
                    .flatten(),
            )
        });
        if escape || clicked_at.is_some_and(|pos| !list.contains(pos) && !rect.contains(pos)) {
            self.skin_open = false;
        }
    }

    /// Chooses the skin of the block's render, or Auto with `None`. Only the drawing changes.
    ///
    /// # Errors
    /// Returns a backend error.
    pub fn set_skin(&mut self, skin: Option<String>) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::SetBlockSkin {
            block_id: self.block_id.clone(),
            skin: skin.clone(),
        })?;
        self.block.skin = skin;
        self.synced_revision = None;
        Ok(())
    }

    /// Whether the stage shows the MICS panel: an IR cab whose tone has more than one mic
    /// position, or that already blends two.
    fn mics_offered(&self) -> bool {
        let Some(tone) = self.block.tone.as_ref() else {
            return false;
        };
        if tone.kind != ModelKind::Ir || page_category(&self.block) != "CAB" {
            return false;
        }
        self.block.ir_b.is_some() || self.models.options().len().max(model_total(&self.block)) >= 2
    }

    /// Gives the picker of B the models of the tone other than the active one, and B as its
    /// model.
    fn refresh_b_picker(&mut self) {
        let active = self.block.active_model_id;
        let options: Vec<ModelOption> = self
            .models
            .options()
            .iter()
            .filter(|option| Some(option.id) != active)
            .cloned()
            .collect();
        if self.models_b.options() != options.as_slice() {
            let total = options.len();
            self.models_b.set_options(options, total);
        }
        if let Some(ir_b) = self.block.ir_b
            && self.models_b.active() != Some(ir_b.model_id)
        {
            self.models_b.set_active(ir_b.model_id);
        }
    }

    /// The MICS panel at the right of `stage`: IR A, then either a button that adds a second IR
    /// or IR B with its phase, its removal and the blend.
    fn show_mics_panel(
        &mut self,
        ui: &mut egui::Ui,
        stage: Rect,
        session: SessionStatus,
        look: crate::theme::Look,
    ) {
        self.refresh_b_picker();
        let downloading_b = self.pending_slot() == Some(ModelSlot::B);
        let b_shown = self.block.ir_b.is_some() || downloading_b;
        let inner_width = MICS_PANEL_WIDTH - 2.0 * MICS_PAD;
        let help = ui.painter().layout(
            MICS_HELP.to_owned(),
            egui::FontId::proportional(10.0),
            look.text_muted,
            inner_width,
        );
        let body = if b_shown {
            MICS_ROW + MICS_GAP + MICS_FOOTER
        } else {
            MICS_ADD + MICS_GAP + help.size().y
        };
        let height = 2.0 * MICS_PAD + MICS_HEADER + MICS_GAP + MICS_ROW + MICS_GAP + body;
        let panel = Rect::from_min_size(
            egui::pos2(
                stage.right() - MICS_PANEL_WIDTH,
                stage.center().y - height / 2.0,
            ),
            egui::vec2(MICS_PANEL_WIDTH, height),
        );
        let id = ui.scope_id().with(("mics", &self.block_id.0));
        ui.interact(panel, id, Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, "Cab mics"));
        let painter = ui.painter().clone();
        painter.rect_filled(panel, 10.0, look.panel);
        painter.rect_stroke(
            panel,
            10.0,
            egui::Stroke::new(1.0, look.card_edge),
            egui::StrokeKind::Inside,
        );
        let inner = panel.shrink(MICS_PAD);

        let header = Rect::from_min_size(inner.min, egui::vec2(inner.width(), MICS_HEADER));
        paint_tracked(
            &painter,
            header.left_center(),
            egui::Align2::LEFT_CENTER,
            "MICS",
            &bold_font(10.0),
            look.text,
            1.4,
        );
        static_text(ui, header, id.with("title"), "MICS");
        let summary = if self.block.ir_b.is_some() {
            "2 IRs blended"
        } else {
            "1 IR"
        };
        let summary_rect = painter.text(
            header.right_center(),
            egui::Align2::RIGHT_CENTER,
            summary,
            egui::FontId::proportional(10.0),
            look.text_muted,
        );
        static_text(ui, summary_rect, id.with("summary"), summary);

        let row_a = Rect::from_min_size(
            egui::pos2(inner.left(), header.bottom() + MICS_GAP),
            egui::vec2(inner.width(), MICS_ROW),
        );
        self.show_mic_row(ui, row_a, ModelSlot::A, session, look);
        let below = row_a.bottom() + MICS_GAP;
        if !b_shown {
            let add = Rect::from_min_size(
                egui::pos2(inner.left(), below),
                egui::vec2(inner.width(), MICS_ADD),
            );
            let first = self.models_b.options().first().map(|option| option.id);
            let enabled = first.is_some() && !self.models_b.disabled(session);
            if self.show_add_ir(ui, add, enabled, look) && enabled {
                let _ = self.select_second_model(first, session);
            }
            let help_rect = Rect::from_min_size(
                egui::pos2(inner.left(), add.bottom() + MICS_GAP),
                help.size(),
            );
            painter.galley(help_rect.min, help, look.text_muted);
            static_text(ui, help_rect, id.with("help"), MICS_HELP);
            return;
        }
        let row_b = Rect::from_min_size(
            egui::pos2(inner.left(), below),
            egui::vec2(inner.width(), MICS_ROW),
        );
        self.show_mic_row(ui, row_b, ModelSlot::B, session, look);
        let footer = Rect::from_min_max(
            egui::pos2(inner.left(), row_b.bottom() + MICS_GAP),
            egui::pos2(inner.right(), inner.bottom()),
        );
        self.show_mics_footer(ui, footer, session, look);
    }

    /// One row of the MICS panel: the slot badge, its capture picker and its level.
    fn show_mic_row(
        &mut self,
        ui: &mut egui::Ui,
        row: Rect,
        slot: ModelSlot,
        session: SessionStatus,
        look: crate::theme::Look,
    ) {
        let (letter, badge_fill, label, level_label) = match slot {
            ModelSlot::A => ("A", look.solid, "IR A capture", "IR A level"),
            ModelSlot::B => ("B", look.accent, "IR B capture", "IR B level"),
        };
        // A B with no bytes, or still on its way, is drawn faded, as the render of a model that
        // is not loaded is.
        let loading = slot == ModelSlot::B
            && (self.block.ir_b_missing || self.pending_slot() == Some(ModelSlot::B));
        let fade = |color: Color32| {
            if loading {
                color.gamma_multiply(0.4)
            } else {
                color
            }
        };
        let painter = ui.painter().clone();
        let badge = Rect::from_min_size(
            egui::pos2(row.left(), row.center().y - 11.0),
            egui::Vec2::splat(22.0),
        );
        painter.rect_filled(badge, 4.0, fade(badge_fill));
        painter.text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            bold_font(11.0),
            fade(match slot {
                ModelSlot::A => look.solid_content,
                ModelSlot::B => Color32::WHITE,
            }),
        );
        let level = Rect::from_min_max(
            egui::pos2(row.right() - MICS_LEVEL_WIDTH, row.top()),
            row.right_bottom(),
        );
        let picker = Rect::from_min_max(
            egui::pos2(badge.right() + 10.0, row.top()),
            egui::pos2(level.left() - 10.0, row.bottom()),
        );
        let fallback = self
            .slot_model(slot)
            .and_then(|id| {
                self.block
                    .tone
                    .as_ref()?
                    .models
                    .iter()
                    .find(|model| model.id == id)
            })
            .map_or_else(String::new, |model| model.name.clone());
        let name = if self.block.ir_b_missing && slot == ModelSlot::B {
            "IR B capture (not loaded)"
        } else {
            label
        };
        let mut picker_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("mic-picker", letter))
                .max_rect(picker),
        );
        let chosen = match slot {
            ModelSlot::A => {
                self.models
                    .show_compact(&mut picker_ui, picker, name, &fallback, false, session)
            }
            ModelSlot::B => self.models_b.show_compact(
                &mut picker_ui,
                picker,
                name,
                &fallback,
                loading,
                session,
            ),
        };
        if let Some(id) = chosen {
            let _ = match slot {
                ModelSlot::A => self.select_model(id, session),
                ModelSlot::B => self.select_second_model(Some(id), session),
            };
        }

        let gain = KnobSpec::block_gain();
        let (value, editable) = match slot {
            ModelSlot::A => (self.block.params.ir_a_level, self.block.ir_b.is_some()),
            ModelSlot::B => (self.block.params.ir_b_level, self.block.ir_b.is_some()),
        };
        let mut shown = value;
        if editable {
            // The readout drags like a knob, so it has its gesture, its reset and its type-in.
            let id = ui.scope_id().with(("mic-level", &self.block_id.0));
            let pass = ui.ctx().cumulative_pass_nr();
            let output = crate::widgets::knob::show(
                ui,
                id.with(level_label),
                level,
                &gain,
                level_label,
                value,
            );
            let _ = output
                .response
                .on_hover_cursor(egui::CursorIcon::ResizeVertical);
            shown = output.shown;
            if let Some(edit) = self.track_trim(level_label, pass, output.edit) {
                match slot {
                    ModelSlot::A => self.block.params.ir_a_level = edit.value,
                    ModelSlot::B => self.block.params.ir_b_level = edit.value,
                }
                self.write_params_in_gesture(edit.phase);
            }
        }
        painter.text(
            level.right_center(),
            egui::Align2::RIGHT_CENTER,
            gain.format(shown),
            egui::FontId::monospace(10.0),
            fade(look.text_muted),
        );
    }

    /// The dashed button that adds a second IR. Returns whether it was clicked.
    fn show_add_ir(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        enabled: bool,
        look: crate::theme::Look,
    ) -> bool {
        let response = ui.interact(
            rect,
            ui.scope_id().with(("add-ir", &self.block_id.0)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, enabled, "Add a second IR")
        });
        let response = if enabled {
            response.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            response
        };
        let content = if !enabled {
            look.text_muted.gamma_multiply(0.5)
        } else if response.hovered() {
            look.text
        } else {
            look.text_muted
        };
        let painter = ui.painter();
        painter.rect_filled(rect, 8.0, look.slot_fill);
        let outline = rounded_outline(rect.shrink(0.75), 8.0);
        painter.extend(egui::Shape::dashed_line(
            &outline,
            egui::Stroke::new(1.5, look.edge_strong),
            5.0,
            4.0,
        ));
        let font = bold_font(10.0);
        let text_width = tracked_width(painter, "ADD A SECOND IR", &font, 1.2);
        let left = rect.center().x - (14.0 + 8.0 + text_width) / 2.0;
        let icon = egui::pos2(left + 7.0, rect.center().y);
        let stroke = egui::Stroke::new(2.4, content);
        let arm = 5.25;
        painter.line_segment(
            [icon - egui::vec2(arm, 0.0), icon + egui::vec2(arm, 0.0)],
            stroke,
        );
        painter.line_segment(
            [icon - egui::vec2(0.0, arm), icon + egui::vec2(0.0, arm)],
            stroke,
        );
        paint_tracked(
            painter,
            egui::pos2(left + 22.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "ADD A SECOND IR",
            &font,
            content,
            1.2,
        );
        response.clicked()
    }

    /// The last part of the panel with B: the phase of B and its removal at the left, and the
    /// blend from A to B at the right.
    fn show_mics_footer(
        &mut self,
        ui: &mut egui::Ui,
        footer: Rect,
        session: SessionStatus,
        look: crate::theme::Look,
    ) {
        let painter = ui.painter().clone();
        painter.hline(
            footer.x_range(),
            footer.top() + 0.5,
            egui::Stroke::new(1.0, look.fill_soft),
        );
        let content = Rect::from_min_max(egui::pos2(footer.left(), footer.top() + 5.0), footer.max);
        let cy = content.center().y;

        // Ø B over REMOVE B.
        let column_top = cy - (24.0 + 6.0 + 12.0) / 2.0;
        let phase_font = bold_font(10.0);
        let phase_width = tracked_width(&painter, "Ø B", &phase_font, 0.8) + 20.0;
        let phase = Rect::from_min_size(
            egui::pos2(content.left(), column_top),
            egui::vec2(phase_width, 24.0),
        );
        // The pair settings wait for B to land: a download that fails or that a switch of A
        // ends must not leave them on a block with no B.
        let landed = self.block.ir_b.is_some();
        let fade = |color: Color32| {
            if landed {
                color
            } else {
                color.gamma_multiply(0.4)
            }
        };
        let inverted = self.block.params.ir_b_invert;
        let response = ui
            .add_enabled_ui(landed, |ui| {
                ui.interact(
                    phase,
                    ui.scope_id().with(("ir-b-invert", &self.block_id.0)),
                    Sense::click(),
                )
            })
            .inner;
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, landed, inverted, "Invert IR B phase")
        });
        let (fill, edge, ink) = if inverted {
            (fade(look.solid), fade(look.solid), fade(look.solid_content))
        } else {
            (look.panel, fade(look.control_edge), fade(look.text))
        };
        painter.rect_filled(phase, 12.0, fill);
        painter.rect_stroke(
            phase,
            12.0,
            egui::Stroke::new(1.0, edge),
            egui::StrokeKind::Inside,
        );
        paint_tracked(
            &painter,
            phase.center(),
            egui::Align2::CENTER_CENTER,
            "Ø B",
            &phase_font,
            ink,
            0.8,
        );
        if landed && response.clicked() {
            let _ = self.set_ir_b_invert(!inverted);
        }
        let remove_width = tracked_width(&painter, "REMOVE B", &phase_font, 0.8);
        let remove = Rect::from_min_size(
            egui::pos2(content.left(), phase.bottom() + 6.0),
            egui::vec2(remove_width, 12.0),
        );
        let response = ui.interact(
            remove.expand2(egui::vec2(2.0, 3.0)),
            ui.scope_id().with(("ir-b-remove", &self.block_id.0)),
            Sense::click(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Remove IR B"));
        paint_tracked(
            &painter,
            remove.left_center(),
            egui::Align2::LEFT_CENTER,
            "REMOVE B",
            &phase_font,
            if response.hovered() {
                look.text
            } else {
                look.text_muted
            },
            0.8,
        );
        if response.clicked() {
            let _ = self.select_second_model(None, session);
        }

        // A, the BLEND knob, B.
        let mut painter = painter.clone();
        painter.multiply_opacity(if landed { 1.0 } else { 0.4 });
        let letter_font = bold_font(11.0);
        let b_width = text_width(ui, "B", 11.0);
        let knob_column = Rect::from_center_size(
            egui::pos2(content.right() - b_width - 8.0 - BLEND_KNOB / 2.0, cy),
            egui::vec2(BLEND_KNOB, 14.0 + 6.0 + BLEND_KNOB + 6.0 + 13.0),
        );
        painter.text(
            egui::pos2(content.right(), cy),
            egui::Align2::RIGHT_CENTER,
            "B",
            letter_font.clone(),
            look.accent,
        );
        painter.text(
            egui::pos2(knob_column.left() - 8.0, cy),
            egui::Align2::RIGHT_CENTER,
            "A",
            letter_font,
            look.text,
        );
        paint_tracked(
            &painter,
            egui::pos2(knob_column.center().x, knob_column.top() + 7.0),
            egui::Align2::CENTER_CENTER,
            "BLEND",
            &bold_font(11.0),
            look.knob_label,
            0.44,
        );
        let face = Rect::from_min_size(
            egui::pos2(knob_column.left(), knob_column.top() + 20.0),
            egui::Vec2::splat(BLEND_KNOB),
        );
        let spec = KnobSpec::blend();
        let id = ui.scope_id().with(("ir-blend", &self.block_id.0));
        let pass = ui.ctx().cumulative_pass_nr();
        let blend = self.block.params.ir_blend;
        let output = ui
            .add_enabled_ui(landed, |ui| {
                crate::widgets::knob::show(ui, id, face, &spec, "Blend", blend)
            })
            .inner;
        let edit = output.edit.filter(|_| landed);
        if let Some(edit) = self.track_trim("Blend", pass, edit) {
            self.block.params.ir_blend = edit.value;
            self.write_params_in_gesture(edit.phase);
        }
        paint_panel_knob(&painter, face, spec.position(output.shown) as f32, look);
        painter.text(
            egui::pos2(face.center().x, face.bottom() + 6.0 + 6.5),
            egui::Align2::CENTER_CENTER,
            spec.format(output.shown),
            egui::FontId::proportional(11.0),
            look.knob_value,
        );
    }

    /// The slots this stage offers, leading and trailing the renders.
    fn stage_slots(
        &self,
        category: ronie_core::gear::GearCategory,
    ) -> (Option<SlotSpec>, Option<SlotSpec>) {
        use ronie_core::gear::GearCategory::Pedal;

        use super::Placement::After;
        match category {
            Pedal => (
                None,
                self.slots.pedal_after.clone().map(|id| SlotSpec {
                    category: Pedal,
                    placement: After(id),
                    label: "PEDAL",
                    name: "Add a pedal after these",
                    size: egui::vec2(110.0, 176.0),
                }),
            ),
            _ => (None, None),
        }
    }

    /// Lays the gear, its pedal neighbours and the slots out side by side at one height,
    /// draws the neighbours and the slots, and returns where this block's body goes. The
    /// renders and the slots shrink together when they do not fit.
    fn show_pedalboard(
        &mut self,
        ui: &mut egui::Ui,
        stage: Rect,
        textures: &super::skins::SkinTextures,
        category: ronie_core::gear::GearCategory,
    ) -> Rect {
        let neighbour = |block: &ChainBlock| {
            let skin = super::skins::skin_for(block)?;
            Some((block.clone(), super::skins::textures(ui.ctx(), skin.id)?))
        };
        let before: Vec<_> = self.neighbours.0.iter().filter_map(neighbour).collect();
        let after: Vec<_> = self.neighbours.1.iter().filter_map(neighbour).collect();
        let (lead, trail) = self.stage_slots(category);
        if before.is_empty() && after.is_empty() && lead.is_none() && trail.is_none() {
            return textures.manifest.fit(stage, RENDER_HEIGHT);
        }
        let aspect = |manifest: &super::skins::SkinManifest| {
            manifest.width as f32 / manifest.height.max(1) as f32
        };
        let count = before.len() + after.len() + 1;
        let aspects: f32 = before
            .iter()
            .chain(&after)
            .map(|(_, each)| aspect(&each.manifest))
            .sum::<f32>()
            + aspect(&textures.manifest);
        let slot_count = usize::from(lead.is_some()) + usize::from(trail.is_some());
        let slot_widths: f32 = [&lead, &trail]
            .into_iter()
            .flatten()
            .map(|slot| slot.size.x / RENDER_HEIGHT)
            .sum();
        let gaps = PEDAL_GAP * (count - 1) as f32 + SLOT_GAP * slot_count as f32;
        let height = stage
            .height()
            .min(RENDER_HEIGHT)
            .min((stage.width() - gaps) / (aspects + slot_widths));
        let scale = height / RENDER_HEIGHT;
        let total = (aspects + slot_widths) * height + gaps;
        let mut x = stage.center().x - total / 2.0;
        let top = stage.center().y - height / 2.0;
        let place_slot = |slot: &SlotSpec, x: &mut f32| {
            let size = slot.size * scale;
            let rect = Rect::from_min_size(egui::pos2(*x, stage.center().y - size.y / 2.0), size);
            *x = rect.right() + SLOT_GAP;
            rect
        };
        let lead_rect = lead.as_ref().map(|slot| place_slot(slot, &mut x));
        let place = |manifest: &super::skins::SkinManifest, x: &mut f32| {
            let rect = Rect::from_min_size(
                egui::pos2(*x, top),
                egui::vec2(aspect(manifest) * height, height),
            );
            *x = rect.right() + PEDAL_GAP;
            rect
        };
        let before_rects: Vec<_> = before
            .iter()
            .map(|(_, each)| place(&each.manifest, &mut x))
            .collect();
        let own = place(&textures.manifest, &mut x);
        let after_rects: Vec<_> = after
            .iter()
            .map(|(_, each)| place(&each.manifest, &mut x))
            .collect();
        // The run's last render is followed by the slot's gap, not by a pedal gap.
        x += SLOT_GAP - PEDAL_GAP;
        let trail_rect = trail.as_ref().map(|slot| place_slot(slot, &mut x));
        for ((block, each), rect) in before
            .iter()
            .zip(before_rects)
            .chain(after.iter().zip(after_rects))
        {
            self.show_neighbour(ui, rect, block, each);
        }
        for (slot, rect) in [(lead, lead_rect), (trail, trail_rect)] {
            if let (Some(slot), Some(rect)) = (slot, rect) {
                self.show_slot(ui, rect, &slot);
            }
        }
        own
    }

    /// A dashed slot where gear can be added: a plus and a label, solid while its menu is open.
    fn show_slot(&mut self, ui: &mut egui::Ui, rect: Rect, slot: &SlotSpec) {
        let look = crate::theme::Look::get(ui.ctx());
        let response = ui.interact(
            rect,
            ui.scope_id().with(("slot", slot.name)),
            Sense::click(),
        );
        let open = self
            .open_slot
            .as_ref()
            .is_some_and(|(category, placement)| {
                *category == slot.category && *placement == slot.placement
            });
        response
            .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, slot.name));
        self.slot_rects
            .push((slot.category, slot.placement.clone(), rect));
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            self.slot_request = Some(SlotRequest {
                category: slot.category,
                placement: slot.placement.clone(),
                anchor: rect,
            });
        }
        let (fill, edge, content) = if open {
            (look.solid, look.solid, look.solid_content)
        } else {
            (look.slot_fill, look.edge_strong, look.text_muted)
        };
        let painter = ui.painter();
        painter.rect_filled(rect, 10.0, fill);
        if open {
            painter.rect_stroke(
                rect,
                10.0,
                egui::Stroke::new(1.5, edge),
                egui::StrokeKind::Inside,
            );
        } else {
            let outline = rounded_outline(rect.shrink(0.75), 10.0);
            painter.extend(egui::Shape::dashed_line(
                &outline,
                egui::Stroke::new(1.5, edge),
                5.0,
                4.0,
            ));
        }
        // The plus and the tracked label, centred as a column with an 8 pt gap.
        let font = bold_font(10.0);
        let letters: Vec<_> = slot
            .label
            .chars()
            .map(|letter| painter.layout_no_wrap(letter.to_string(), font.clone(), content))
            .collect();
        let tracking = 1.2;
        let width: f32 = letters.iter().map(|galley| galley.size().x).sum::<f32>()
            + tracking * (letters.len() - 1) as f32;
        let text_height = letters.first().map_or(0.0, |galley| galley.size().y);
        let column = 18.0 + 8.0 + text_height;
        let icon = egui::Pos2::new(rect.center().x, rect.center().y - column / 2.0 + 9.0);
        let stroke = egui::Stroke::new(2.2, content);
        let arm = 7.0;
        painter.line_segment(
            [icon - egui::vec2(arm, 0.0), icon + egui::vec2(arm, 0.0)],
            stroke,
        );
        painter.line_segment(
            [icon - egui::vec2(0.0, arm), icon + egui::vec2(0.0, arm)],
            stroke,
        );
        let mut at = egui::pos2(rect.center().x - width / 2.0, icon.y + 9.0 + 8.0);
        for galley in letters {
            let advance = galley.size().x + tracking;
            painter.galley(at, galley, content);
            at.x += advance;
        }
    }

    /// A pedal next to the open one: drawn with its knobs where its settings put them, and
    /// opened by a click.
    fn show_neighbour(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        block: &ChainBlock,
        textures: &super::skins::SkinTextures,
    ) {
        use super::skins::KnobControl;

        let title = block.tone.as_ref().map_or("", |tone| tone.title.as_str());
        let label = format!("Pedal render: {title}");
        let response = ui.interact(
            rect,
            ui.scope_id().with(("neighbour", &block.id.0)),
            Sense::click(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            self.select_request = Some(block.id.clone());
        }
        let gain = KnobSpec::block_gain();
        let mix = KnobSpec::percent(BlockParams::default().mix);
        let params = &block.params;
        let tint = if params.enabled {
            Color32::WHITE
        } else {
            BYPASSED_TINT
        };
        super::skins::paint(
            ui.painter(),
            textures,
            rect,
            |control| match control {
                KnobControl::Gain => gain.position(params.input_gain) as f32,
                KnobControl::Output => gain.position(params.output_gain) as f32,
                KnobControl::Mix => mix.position(params.mix) as f32,
            },
            tint,
        );
        // Its LCD shows its capture, and a click on it still opens the neighbour.
        let text = super::skins::lcd_text(&capture_name(block).unwrap_or_default());
        if let Some(glass) = super::skins::draw_lcd(ui.painter(), &textures.manifest, rect, &text) {
            static_text(
                ui,
                glass,
                ui.scope_id().with(("neighbour-lcd", &block.id.0)),
                &text,
            );
        }
    }

    /// GAIN IN / OUTPUT / MIX trims in a column to the right of `gear`, for gear whose render
    /// has no knobs (cabs) or that has no render.
    fn show_trims_beside(&mut self, ui: &mut egui::Ui, gear: Rect, stage: Rect) {
        let gain = KnobSpec::block_gain();
        let mix = KnobSpec::percent(BlockParams::default().mix);
        let id = ui.scope_id().with(("trim", &self.block_id.0));
        let pass = ui.ctx().cumulative_pass_nr();
        let x = (gear.right() + 40.0).min(stage.right() - 36.0);
        let height = 3.0 * 63.0 + 2.0 * 12.0;
        let top = stage.center().y - height / 2.0;
        let slot = |index: usize| {
            Rect::from_min_size(
                egui::pos2(x, top + index as f32 * 75.0),
                egui::vec2(36.0, 63.0),
            )
        };
        let edit = trim_knob(ui, id, slot(0), "In", &gain, self.block.params.input_gain);
        if let Some(edit) = self.track_trim("In", pass, edit) {
            self.block.params.input_gain = edit.value;
            self.write_params_in_gesture(edit.phase);
        }
        let edit = trim_knob(ui, id, slot(1), "Mix", &mix, self.block.params.mix);
        if let Some(edit) = self.track_trim("Mix", pass, edit) {
            self.block.params.mix = edit.value;
            self.write_params_in_gesture(edit.phase);
        }
        let edit = trim_knob(ui, id, slot(2), "Out", &gain, self.block.params.output_gain);
        if let Some(edit) = self.track_trim("Out", pass, edit) {
            self.block.params.output_gain = edit.value;
            self.write_params_in_gesture(edit.phase);
        }
    }

    /// Draws the render in `body` with its live knobs and its capture display, and a hint over
    /// the knob or the display under the pointer, being dragged, or holding keyboard focus.
    fn show_render(
        &mut self,
        ui: &mut egui::Ui,
        body: Rect,
        skin: super::skins::BlockSkin,
        textures: &super::skins::SkinTextures,
        session: SessionStatus,
    ) {
        use super::skins::KnobControl;

        let render = ui.interact(
            body,
            ui.scope_id().with(("render", &self.block_id.0)),
            egui::Sense::hover(),
        );
        render.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, skin.label));

        let gain = KnobSpec::block_gain();
        let mix = KnobSpec::percent(BlockParams::default().mix);
        let id = ui.scope_id().with(("render-knob", &self.block_id.0));
        let pass = ui.ctx().cumulative_pass_nr();
        let mut positions = Vec::new();
        let mut hint = None;
        // The display goes under the knobs, so a knob keeps the clicks where their hit areas
        // overlap on a small render.
        let display = self.capture_display_input(ui, body, textures, session);
        for knob in &textures.manifest.knobs {
            let (label, spec, value) = match knob.control {
                KnobControl::Gain => ("GAIN IN", &gain, self.block.params.input_gain),
                KnobControl::Output => ("OUTPUT", &gain, self.block.params.output_gain),
                KnobControl::Mix => ("MIX", &mix, self.block.params.mix),
            };
            let knob_rect = knob.rect_in(body, &textures.manifest);
            let hit = Rect::from_center_size(
                knob_rect.center(),
                knob_rect.size().max(egui::Vec2::splat(MIN_KNOB_TARGET)),
            );
            let output = crate::widgets::knob::show(ui, id.with(label), hit, spec, label, value);
            positions.push((knob.control, spec.position(output.shown) as f32));
            let response = &output.response;
            if response.hovered() || response.dragged() || response.has_focus() {
                hint = Some((knob_rect, format!("{label}  {}", spec.format(output.shown))));
            }
            if let Some(edit) = self.track_trim(label, pass, output.edit) {
                match knob.control {
                    KnobControl::Gain => self.block.params.input_gain = edit.value,
                    KnobControl::Output => self.block.params.output_gain = edit.value,
                    KnobControl::Mix => self.block.params.mix = edit.value,
                }
                self.write_params_in_gesture(edit.phase);
            }
        }

        let tint = if self.load_state() != TileVisualState::Ready {
            Color32::from_white_alpha(89)
        } else if self.block.params.enabled {
            Color32::WHITE
        } else {
            BYPASSED_TINT
        };
        let painter = ui.painter().clone();
        super::skins::paint(
            &painter,
            textures,
            body,
            |control| {
                positions
                    .iter()
                    .find(|(each, _)| *each == control)
                    .map_or(0.5, |(_, position)| *position)
            },
            tint,
        );
        if let Some(lcd_hint) =
            display.and_then(|display| self.paint_capture_display(ui, &painter, &display))
        {
            hint = hint.or(Some(lcd_hint));
        }
        if let Some((anchor, text)) = hint {
            hint_bubble(ui, &painter, anchor, &text, id.with("hint"));
        }
    }

    /// The LCD of the open pedal as a control: a click steps to the next capture, wrapping,
    /// when the tone has more than one. `None` when the skin has no LCD.
    fn capture_display_input(
        &mut self,
        ui: &mut egui::Ui,
        body: Rect,
        textures: &super::skins::SkinTextures,
        session: SessionStatus,
    ) -> Option<CaptureDisplay> {
        let manifest = &textures.manifest;
        let glass = manifest.lcd.as_ref()?.rect_in(body, manifest);
        let id = ui.scope_id().with(("capture-display", &self.block_id.0));
        let hit = Rect::from_center_size(
            glass.center(),
            glass
                .size()
                .max(egui::vec2(MIN_KNOB_TARGET, MIN_LCD_TARGET_HEIGHT)),
        );
        let (label, ..) = self.capture_shown();
        let response = ui.interact(hit, id, Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        let count = self.models.options().len();
        let steps = count > 1 && !self.models.disabled(session);
        let response = if steps {
            response.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            response
        };
        if steps && response.clicked() {
            let next = self.capture_index().map_or(0, |index| (index + 1) % count);
            let id = self.models.options()[next].id;
            let _ = self.select_model(id, session);
        }
        Some(CaptureDisplay {
            glass,
            id,
            response,
            steps,
        })
    }

    /// Draws the LCD's text: its capture, or dots while one downloads, and the ring under the
    /// pointer. Returns the hint to show while the pointer or the keyboard focus is on it.
    fn paint_capture_display(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        display: &CaptureDisplay,
    ) -> Option<(Rect, String)> {
        let CaptureDisplay {
            glass,
            id,
            response,
            steps,
        } = display;
        let (_, name, text) = self.capture_shown();
        super::skins::paint_lcd(painter, *glass, &text);
        static_text(ui, *glass, id.with("text"), &text);
        if !(response.hovered() || response.has_focus()) {
            return None;
        }
        // A capture missing from the list is 0, as in the picker.
        let index = self.capture_index().map_or(0, |index| index + 1);
        let count = self.models.options().len();
        let place = format!("CAPTURE {index} / {count} \u{b7} {name}");
        if !steps {
            return Some((glass.expand(4.0), place));
        }
        painter.rect_stroke(
            glass.expand(2.0),
            4.0,
            egui::Stroke::new(2.0, crate::theme::Look::get(painter.ctx()).accent),
            egui::StrokeKind::Outside,
        );
        // The bubble sits above the ring.
        Some((glass.expand(4.0), format!("{place} \u{b7} click for next")))
    }

    /// The width of the pill of capture lights, with or without the shown capture's short
    /// name.
    fn capture_pill_width(&self, ui: &egui::Ui, with_name: bool) -> f32 {
        let count = self.models.options().len();
        let visible = count.min(CAPTURE_LIGHT_COUNT);
        let label_width = tracked_width(ui.painter(), "CAPTURE", &bold_font(9.5), CAPTURE_TRACKING);
        let arrow = if count > visible { LIGHT_PITCH } else { 0.0 };
        let lights_width = visible as f32 * LIGHT_PITCH - LIGHT_GAP / 2.0 + 2.0 * arrow;
        let name = if with_name {
            CAPTURE_PILL_GAP + self.capture_name_width(ui)
        } else {
            0.0
        };
        CAPTURE_PILL_PAD * 2.0 + label_width + CAPTURE_PILL_GAP + lights_width + name
    }

    /// The shown capture's short name in the pill.
    fn capture_short_name(&self) -> String {
        self.capture_shown()
            .1
            .chars()
            .take(CAPTURE_NAME_CHARS)
            .collect()
    }

    /// The width of the short name in the pill.
    fn capture_name_width(&self, ui: &egui::Ui) -> f32 {
        ui.painter()
            .layout_no_wrap(
                self.capture_short_name(),
                egui::FontId::monospace(10.0),
                Color32::BLACK,
            )
            .size()
            .x
    }

    /// The pill of capture lights centred under the amp render `body`, its bottom at `bottom`,
    /// kept inside `room` (left and right): a window of at most eight lights on the picker's
    /// captures, and arrows that move the window when there are more, then the shown
    /// capture's short name when `with_name`. A click on a light selects its capture.
    fn show_capture_lights(
        &mut self,
        ui: &mut egui::Ui,
        body: Rect,
        bottom: f32,
        room: (f32, f32),
        with_name: bool,
        session: SessionStatus,
    ) {
        let look = crate::theme::Look::get(ui.ctx());
        let options: Vec<(ModelId, String)> = self
            .models
            .options()
            .iter()
            .map(|option| (option.id, option.name.clone()))
            .collect();
        let count = options.len();
        // The shown capture, as the display and the picker have it; none lit when it is not in
        // the list.
        let active = self.capture_index();
        // The window follows the active light whenever the active capture changes, and the
        // arrows move it by one from there.
        let visible = count.min(CAPTURE_LIGHT_COUNT);
        let last_start = count - visible;
        if self.light_followed != active {
            self.light_followed = active;
            if let Some(active) = active {
                let lowest = active.saturating_sub(visible - 1);
                self.light_start = self.light_start.clamp(lowest, active);
            }
        }
        self.light_start = self.light_start.min(last_start);
        let start = self.light_start;
        let paged = count > visible;
        let enabled = !self.models.disabled(session);

        let label_font = bold_font(9.5);
        let label_width = tracked_width(ui.painter(), "CAPTURE", &label_font, CAPTURE_TRACKING);
        let name = self.capture_short_name();
        let name_font = egui::FontId::monospace(10.0);
        let name_width = self.capture_name_width(ui);
        let pad = CAPTURE_PILL_PAD;
        let gap = CAPTURE_PILL_GAP;
        let width = self.capture_pill_width(ui, with_name);
        // Centred under the render, unless that would put it over the SKIN chip at the left;
        // and never past the right of the stage.
        let center_x = body
            .center()
            .x
            .max(room.0 + width / 2.0)
            .min(room.1 - width / 2.0);
        let pill = Rect::from_center_size(
            egui::pos2(center_x, bottom - CAPTURE_PILL_HEIGHT / 2.0),
            egui::vec2(width, CAPTURE_PILL_HEIGHT),
        );
        let painter = ui.painter().clone();
        painter.rect(
            pill,
            CAPTURE_PILL_HEIGHT / 2.0,
            look.panel,
            egui::Stroke::new(1.0, look.card_edge),
            egui::StrokeKind::Inside,
        );
        let group = ui.interact(
            pill,
            ui.scope_id().with(("capture-lights", &self.block_id.0)),
            Sense::hover(),
        );
        group.widget_info(|| egui::WidgetInfo::labeled(egui::Role::RadioGroup, true, "Captures"));
        let muted = look.text_muted;
        let mut x = pill.left() + pad;
        paint_tracked(
            &painter,
            egui::pos2(x, pill.center().y),
            egui::Align2::LEFT_CENTER,
            "CAPTURE",
            &label_font,
            muted,
            CAPTURE_TRACKING,
        );
        static_text(
            ui,
            Rect::from_min_size(
                egui::pos2(x, pill.top()),
                egui::vec2(label_width, pill.height()),
            ),
            ui.scope_id()
                .with(("capture-lights-label", &self.block_id.0)),
            "CAPTURE",
        );
        x += label_width + gap;
        let mut step_to = None;
        let mut picked = None;
        let page_id = ui
            .scope_id()
            .with(("capture-lights-page", &self.block_id.0));
        let arrow_button = |ui: &mut egui::Ui, x: f32, glyph: &str, label: &str, can: bool| {
            let rect = Rect::from_center_size(
                egui::pos2(x + LIGHT_PITCH / 2.0 - LIGHT_GAP / 2.0, pill.center().y),
                egui::vec2(LIGHT_PITCH, CAPTURE_PILL_HEIGHT - 4.0),
            );
            let response = ui.interact(rect, page_id.with(label), Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, can, label));
            let color = if can { look.text } else { look.text_subtle };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(13.0),
                color,
            );
            can && response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        };
        if paged {
            if arrow_button(ui, x, "\u{2039}", "Previous captures", start > 0) {
                step_to = Some(start - 1);
            }
            x += LIGHT_PITCH;
        }
        for (offset, (id, name)) in options.iter().enumerate().skip(start).take(visible) {
            let center = egui::pos2(x + LIGHT_SIZE / 2.0, pill.center().y);
            let on = Some(offset) == active;
            let hit = Rect::from_center_size(center, egui::Vec2::splat(LIGHT_PITCH));
            let response = ui.interact(
                hit,
                ui.scope_id()
                    .with(("capture-light", &self.block_id.0, offset)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, on, name)
            });
            let response = response.on_hover_text(name);
            if on {
                for (grow, alpha) in [(5.0, 18), (3.5, 36), (2.0, 64)] {
                    painter.circle_filled(
                        center,
                        LIGHT_SIZE / 2.0 + grow,
                        Color32::from_rgba_unmultiplied(0xff, 0xb3, 0x47, alpha),
                    );
                }
            }
            let (fill, edge) = if on {
                (
                    Color32::from_rgb(0xff, 0xb3, 0x47),
                    Color32::from_rgb(0xc8, 0x7a, 0x1e),
                )
            } else {
                (
                    Color32::from_rgb(0x3a, 0x3a, 0x38),
                    Color32::from_rgb(0x1d, 0x1d, 0x1c),
                )
            };
            painter.circle(
                center,
                LIGHT_SIZE / 2.0 - 0.5,
                fill,
                egui::Stroke::new(1.0, edge),
            );
            if enabled
                && !on
                && response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
            {
                picked = Some(*id);
            }
            x += LIGHT_PITCH;
        }
        if paged && arrow_button(ui, x, "\u{203a}", "Next captures", start < last_start) {
            step_to = Some(start + 1);
        }
        if let Some(to) = step_to {
            self.light_start = to;
        }
        if let Some(id) = picked {
            let _ = self.select_model(id, session);
        }
        if !with_name {
            return;
        }
        let name_pos = egui::pos2(pill.right() - pad - name_width, pill.center().y);
        painter.text(
            name_pos,
            egui::Align2::LEFT_CENTER,
            &name,
            name_font,
            look.text,
        );
        static_text(
            ui,
            Rect::from_min_size(
                egui::pos2(name_pos.x, pill.top()),
                egui::vec2(name_width, pill.height()),
            ),
            ui.scope_id()
                .with(("capture-lights-name", &self.block_id.0)),
            &name,
        );
    }

    /// The capture the display shows: the one on its way, or else the one the block plays.
    fn capture_shown_id(&self) -> Option<ModelId> {
        self.models.target().or(self.block.active_model_id)
    }

    /// Where the shown capture is in the picker's list.
    fn capture_index(&self) -> Option<usize> {
        let shown = self.capture_shown_id()?;
        self.models
            .options()
            .iter()
            .position(|option| option.id == shown)
    }

    /// The display's accessible name, the shown capture's full name, and the LCD text.
    fn capture_shown(&self) -> (String, String, String) {
        let shown = self.capture_shown_id();
        let name = self
            .models
            .options()
            .iter()
            .find(|option| Some(option.id) == shown)
            .map(|option| option.name.clone())
            .or_else(|| capture_name(&self.block))
            .unwrap_or_default();
        let (label, text) = if self.models.target().is_some() {
            (
                format!("Capture display: {name} (loading)"),
                super::skins::LCD_LOADING.to_owned(),
            )
        } else {
            (
                format!("Capture display: {name}"),
                super::skins::lcd_text(&name),
            )
        };
        (label, name, text)
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

/// The outline of a rounded rectangle as a closed list of points.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<egui::Pos2> {
    use std::f32::consts::FRAC_PI_2;
    let corners = [
        (rect.right_top(), egui::vec2(-radius, radius), 3.0),
        (rect.right_bottom(), egui::vec2(-radius, -radius), 0.0),
        (rect.left_bottom(), egui::vec2(radius, -radius), 1.0),
        (rect.left_top(), egui::vec2(radius, radius), 2.0),
    ];
    let mut points = Vec::new();
    for (corner, inset, quarter) in corners {
        let centre = corner + inset;
        for step in 0..=6 {
            let angle = (quarter + step as f32 / 6.0) * FRAC_PI_2;
            points.push(centre + egui::vec2(angle.cos(), angle.sin()) * radius);
        }
    }
    points.push(points[0]);
    points
}

fn bold_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("arimo-bold".into()))
}

/// The open pedal's LCD control, between its input and its drawing.
struct CaptureDisplay {
    /// The glass.
    glass: Rect,
    /// The control's id.
    id: egui::Id,
    /// The control's response.
    response: egui::Response,
    /// Whether a click steps the capture.
    steps: bool,
}

/// The name of the capture a block plays.
fn capture_name(block: &ChainBlock) -> Option<String> {
    let active = block.active_model_id?;
    block
        .tone
        .as_ref()?
        .models
        .iter()
        .find(|model| model.id == active)
        .map(|model| model.name.clone())
}

/// The dark hint bubble 14 pt above `anchor`, with its text as a static label.
fn hint_bubble(ui: &egui::Ui, painter: &egui::Painter, anchor: Rect, text: &str, id: egui::Id) {
    let look = crate::theme::Look::get(ui.ctx());
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::monospace(10.0),
        look.solid_content,
    );
    let bubble = Rect::from_center_size(
        egui::pos2(anchor.center().x, anchor.top() - 14.0),
        galley.size() + egui::vec2(16.0, 8.0),
    );
    painter.rect_filled(bubble, 4.0, look.solid);
    painter.galley(
        bubble.min + egui::vec2(8.0, 4.0),
        galley,
        look.solid_content,
    );
    static_text(ui, bubble, id, text);
}

/// Gives a text that a painter drew its accessible label, as a static label has.
fn static_text(ui: &egui::Ui, rect: Rect, id: egui::Id, text: &str) {
    ui.interact(rect, id, Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
}

/// One line of `text` with `tracking` points between its letters.
fn tracked_galley(
    painter: &egui::Painter,
    text: &str,
    font: &egui::FontId,
    color: Color32,
    tracking: f32,
) -> Arc<egui::Galley> {
    let mut format = egui::TextFormat::simple(font.clone(), color);
    format.extra_letter_spacing = tracking;
    painter.layout_job(egui::text::LayoutJob::single_section(
        text.to_owned(),
        format,
    ))
}

/// The width of `text` with `tracking` points between its letters.
fn tracked_width(painter: &egui::Painter, text: &str, font: &egui::FontId, tracking: f32) -> f32 {
    tracked_galley(painter, text, font, Color32::WHITE, tracking)
        .size()
        .x
}

/// Paints `text` with `tracking` points between its letters, placed by `align` at `anchor`.
fn paint_tracked(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    align: egui::Align2,
    text: &str,
    font: &egui::FontId,
    color: Color32,
    tracking: f32,
) {
    let galley = tracked_galley(painter, text, font, color, tracking);
    let rect = align.anchor_size(anchor, galley.size());
    painter.galley(rect.min, galley, color);
}

/// The knob of a panel: a track with an arc from the start of the travel, and a cap with a
/// pointer, in the colours of `look` (light: grey track, black arc, light cap).
fn paint_panel_knob(painter: &egui::Painter, rect: Rect, position: f32, look: crate::theme::Look) {
    let center = rect.center();
    let radius = rect.width() / 2.0 - 3.0;
    let at = |degrees: f32, radius: f32| {
        let radians = degrees.to_radians();
        center + egui::vec2(radians.sin(), -radians.cos()) * radius
    };
    let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
        let steps = (((to - from).abs() / 4.0).ceil() as usize).max(1);
        (0..=steps)
            .map(|step| at(from + (to - from) * step as f32 / steps as f32, radius))
            .collect()
    };
    let end = -135.0 + 270.0 * position.clamp(0.0, 1.0);
    painter.add(egui::epaint::PathShape::line(
        arc(-135.0, 135.0),
        egui::Stroke::new(3.0, look.knob_track),
    ));
    if end > -134.0 {
        painter.add(egui::epaint::PathShape::line(
            arc(-135.0, end),
            egui::Stroke::new(3.0, look.knob_arc),
        ));
    }
    let cap = radius - 6.0;
    painter.circle_filled(center + egui::vec2(0.0, 1.5), cap, look.knob_shadow);
    painter.circle(
        center,
        cap,
        look.knob_cap,
        egui::Stroke::new(1.0, look.knob_cap_edge),
    );
    painter.circle_filled(
        center - egui::vec2(cap * 0.25, cap * 0.35),
        cap * 0.45,
        look.knob_cap_highlight,
    );
    painter.line_segment(
        [at(end, cap - 2.5), at(end, cap * 0.35)],
        egui::Stroke::new(2.2, look.knob_pointer),
    );
}

/// A small uppercase text action of the light page, such as BYPASS.
fn page_text_button(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &'static str,
    active: bool,
    look: crate::theme::Look,
) -> egui::Response {
    page_toggle(ui, rect, label, label, active, look)
}

/// Height of an action pill in the title row of the light page.
const PAGE_ACTION_HEIGHT: f32 = 28.0;
/// Space between two action pills.
const PAGE_ACTION_GAP: f32 = 8.0;
/// Space between the icon and the label inside a pill, and its side padding.
const PAGE_ACTION_ICON_GAP: f32 = 6.0;
const PAGE_ACTION_PADDING: f32 = 12.0;
/// Side of the icon of a pill.
const PAGE_ACTION_ICON: f32 = 12.0;
/// Letter spacing of a pill label: 0.08 em of 10 pt.
const PAGE_ACTION_TRACKING: f32 = 0.8;

/// One action pill in the title row of the light page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PageAction {
    Bypass,
    Replace,
    Remove,
}

impl PageAction {
    const fn label(self) -> &'static str {
        match self {
            Self::Bypass => "BYPASS",
            Self::Replace => "REPLACE",
            Self::Remove => "REMOVE",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Bypass => "Bypass",
            Self::Replace => "Replace",
            Self::Remove => "Remove",
        }
    }
}

fn page_action_font() -> egui::FontId {
    bold_font(10.0)
}

/// The width of an action pill: padding, icon, gap, tracked label, padding.
fn page_action_width(ui: &egui::Ui, action: PageAction) -> f32 {
    let label = tracked_width(
        ui.painter(),
        action.label(),
        &page_action_font(),
        PAGE_ACTION_TRACKING,
    );
    (PAGE_ACTION_PADDING * 2.0 + PAGE_ACTION_ICON + PAGE_ACTION_ICON_GAP + label).round()
}

/// An action pill of the light page: an outlined pill with a 12 pt icon and a tracked label,
/// drawn solid (#111 with white content when light) while `selected` (a bypassed block, or an
/// open replace menu). REMOVE is red.
fn page_action_button(
    ui: &mut egui::Ui,
    rect: Rect,
    action: PageAction,
    selected: bool,
    look: crate::theme::Look,
) -> egui::Response {
    // The label first, so that the button above it takes the clicks.
    static_text(
        ui,
        rect,
        ui.scope_id().with(("page-action-text", action.name())),
        action.label(),
    );
    let response = ui
        .interact(
            rect,
            ui.scope_id().with(("page-action", action.name())),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| match action {
        PageAction::Bypass => {
            egui::WidgetInfo::selected(egui::Role::Button, true, selected, action.name())
        }
        _ => egui::WidgetInfo::labeled(egui::Role::Button, true, action.name()),
    });
    let (fill, edge, content) = if selected {
        (look.solid, look.solid, look.solid_content)
    } else {
        let fill = if response.hovered() {
            look.panel_hover
        } else {
            look.panel
        };
        match action {
            PageAction::Remove => (fill, look.danger_edge, look.danger),
            _ => (
                fill,
                if response.hovered() {
                    look.text
                } else {
                    look.control_edge
                },
                look.text,
            ),
        }
    };
    let painter = ui.painter();
    let radius = PAGE_ACTION_HEIGHT / 2.0;
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
    let icon = Rect::from_min_size(
        egui::pos2(
            rect.left() + PAGE_ACTION_PADDING,
            rect.center().y - PAGE_ACTION_ICON / 2.0,
        ),
        egui::Vec2::splat(PAGE_ACTION_ICON),
    );
    draw_page_action_icon(painter, icon, action, content);
    paint_tracked(
        painter,
        egui::pos2(icon.right() + PAGE_ACTION_ICON_GAP, rect.center().y),
        egui::Align2::LEFT_CENTER,
        action.label(),
        &page_action_font(),
        content,
        PAGE_ACTION_TRACKING,
    );
    response
}

/// The 12 pt icon of an action pill, from the 24-unit drawings of the design at half scale:
/// a power sign, two opposite arrows, and a bin.
fn draw_page_action_icon(painter: &egui::Painter, rect: Rect, action: PageAction, color: Color32) {
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y) * (rect.width() / 12.0);
    let stroke = egui::Stroke::new(1.2, color);
    let path = |points: Vec<egui::Pos2>| {
        painter.add(egui::epaint::PathShape::line(points, stroke));
    };
    match action {
        PageAction::Bypass => {
            path(vec![at(6.0, 1.5), at(6.0, 6.0)]);
            // The arc of radius 4 around (6, 6.5), from the upper right round the bottom to the
            // upper left, open at the top where the line passes.
            let center = at(6.0, 6.5);
            let radius = 4.0 * rect.width() / 12.0;
            let points = (0..=24)
                .map(|step| {
                    let radians = (-45.0 + 270.0 * step as f32 / 24.0_f32).to_radians();
                    center + egui::vec2(radians.cos(), radians.sin()) * radius
                })
                .collect();
            path(points);
        }
        PageAction::Replace => {
            path(vec![at(2.0, 4.0), at(8.5, 4.0), at(7.0, 2.5)]);
            path(vec![at(10.0, 8.0), at(3.5, 8.0), at(5.0, 9.5)]);
        }
        PageAction::Remove => {
            path(vec![at(2.0, 3.5), at(10.0, 3.5)]);
            path(vec![at(4.5, 3.5), at(4.5, 2.0), at(7.5, 2.0), at(7.5, 3.5)]);
            path(vec![
                at(3.0, 3.5),
                at(3.5, 10.0),
                at(8.5, 10.0),
                at(9.0, 3.5),
            ]);
        }
    }
}

/// A small uppercase text action whose accessible name may differ from its text.
fn page_toggle(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &'static str,
    name: &'static str,
    active: bool,
    look: crate::theme::Look,
) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("page-action", name)),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, active, name));
    let color = if active {
        look.accent
    } else if response.hovered() {
        look.text
    } else {
        look.text_body
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::new(10.0, egui::FontFamily::Name("arimo-bold".into())),
        color,
    );
    response
}

/// The category word of the light page title, from the gear the block models.
fn page_category(block: &ChainBlock) -> &'static str {
    use ronie_core::gear::GearCategory;
    match super::skins::gear_category(block) {
        Some(GearCategory::Pedal) => "PEDAL",
        Some(GearCategory::Amp) => "AMP",
        Some(GearCategory::Cab) => "CAB",
        Some(GearCategory::FullRig) => "RIG",
        Some(GearCategory::Outboard) => "OUTBOARD",
        Some(GearCategory::Space) => "SPACE",
        _ => "BLOCK",
    }
}

/// "AMP · '02 VOX AC30/6 TOP BOOST".
fn page_title(block: &ChainBlock) -> String {
    let title = block.tone.as_ref().map_or("", |tone| tone.title.as_str());
    format!("{} · {}", page_category(block), title.to_uppercase())
}

/// Where the block's sound comes from and how many captures its tone holds.
fn page_meta(block: &ChainBlock) -> String {
    let Some(tone) = block.tone.as_ref() else {
        return String::new();
    };
    let ir = block.tone.as_ref().is_some_and(|tone| {
        tone.raw.get("format").and_then(serde_json::Value::as_str) == Some("ir")
    });
    if tone.local {
        return if ir {
            "Local impulse response"
        } else {
            "Local NAM capture"
        }
        .to_owned();
    }
    let count = model_total(block);
    match (ir, count) {
        (true, 0 | 1) => "IR from TONE3000".to_owned(),
        (true, n) => format!("IR pack from TONE3000 · {n} mic positions"),
        (false, 0 | 1) => "NAM capture from TONE3000".to_owned(),
        (false, n) => format!("NAM capture from TONE3000 · {n} captures in this tone"),
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

//! Snapshot-driven chain gallery, branch geometry, tile actions, and drops.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use eframe::egui::{self, Color32, Pos2, Rect, Sense};
use ronie_core::{
    BlockId, BlockParams, ChainBlock, ChainBranch, ChainSide, ChainSnapshot,
    params::{ParameterId, ParameterValue},
};

use crate::{
    backend::{GesturePhase, UiAction, UiBackend, UiError},
    widgets::{KnobEdit, knob::OpenDrag},
};

/// Design distance from the signal-chain title to the top of the tile row.
const TITLE_ABOVE_TILES: f32 = 57.5;
/// Side of one scroll arrow button at a chain edge.
const ARROW_SIZE: f32 = 32.0;
/// Space between a chain edge and its scroll arrow.
const ARROW_INSET: f32 = 4.0;
/// Horizontal gap between two tiles of a lane, in design pixels.
const TILE_GAP: i32 = 24;

use super::{
    drag::{DragController, DragOutcome},
    lane::{self, lane_width, routing_for, slot_at},
    popover::{Placement, Popover, Surface, Trigger},
    tile::{self, TileAction},
    tile_menu::{self, TileMenuItem},
};

/// Tile presentation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileVisualState {
    /// Playable block.
    Ready,
    /// Load or preparation in flight.
    Loading,
    /// Failed load with retry affordance.
    Failed,
    /// Armed local file/folder drop target.
    DropArmed,
}

/// Current gallery geometry in design pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GalleryLayout {
    /// Tile side length.
    pub tile_size: i32,
    /// Horizontal connector gap.
    pub tile_gap: i32,
    /// Stereo lane gap.
    pub lane_gap: i32,
    /// Scroll-edge fade width.
    pub edge_fade: i32,
    /// Branch-lane indentation past the trunk tap.
    pub branch_indent: i32,
}

/// Navigation selected by a tile click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GallerySelection {
    /// Gallery remains visible.
    None,
    /// Open one block's detail view.
    BlockDetail(BlockId),
    /// Open tone selection to add or replace a block.
    SelectTone {
        /// Destination lane.
        side: ChainSide,
        /// Insert or tone target.
        target_block_id: BlockId,
    },
}

/// Both lanes as a drag preview shows them.
#[derive(Clone)]
struct PreviewLanes {
    left: Vec<ChainBlock>,
    right: Vec<ChainBlock>,
    /// The lane, the index, and the duplicate mode of the drop that the preview shows. The drag
    /// stops when the chain changes, so the same target gives the same lanes.
    target: Option<(ChainSide, usize, bool)>,
}

struct PressState {
    id: BlockId,
    started: Instant,
    position: Pos2,
    touch: bool,
    panning: bool,
}

/// The branch taps of one lane in one frame.
struct BranchGaps<'a> {
    side: ChainSide,
    blocks: &'a [ChainBlock],
    origin: Pos2,
    tile_size: i32,
    branch: Option<&'a ChainBranch>,
    revision: u64,
}

struct TileContext<'a> {
    block: &'a ChainBlock,
    side: ChainSide,
    tile_size: i32,
}

/// Complete chain gallery controller.
pub struct ChainGallery {
    backend: Arc<dyn UiBackend>,
    drag: DragController,
    drop_armed: Option<BlockId>,
    /// The drag preview. Each frame of the gallery shares it, and only a new target copies it.
    preview: Option<Arc<PreviewLanes>>,
    selection: GallerySelection,
    artwork: BTreeMap<BlockId, egui::TextureHandle>,
    tile_rects: Vec<(BlockId, Rect)>,
    lane_regions: Vec<(ChainSide, Rect)>,
    menu_for: Option<BlockId>,
    menu_anchor: Pos2,
    menu_focus: Option<egui::Id>,
    suppress_click_until: Option<(BlockId, Instant)>,
    press: Option<PressState>,
    scroll_offset: f32,
    reveal: Option<BlockId>,
    /// The chain revision and the tones of the last frame that showed the chain, to find a tone
    /// that a load added.
    known_tones: Option<(u64, Vec<BlockId>)>,
    /// The lane pan knob whose drag has an open gesture on the backend.
    pan_drag: OpenDrag<ChainSide>,
    /// The tone whose tile Gain drag has an open gesture on the backend.
    gain_drag: OpenDrag<BlockId>,
}

impl ChainGallery {
    /// Creates a gallery from the current backend snapshot.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>) -> Self {
        Self {
            backend,
            drag: DragController::default(),
            drop_armed: None,
            preview: None,
            selection: GallerySelection::None,
            artwork: BTreeMap::new(),
            tile_rects: Vec::new(),
            lane_regions: Vec::new(),
            menu_for: None,
            menu_anchor: Pos2::ZERO,
            menu_focus: None,
            suppress_click_until: None,
            press: None,
            scroll_offset: 0.0,
            reveal: None,
            pan_drag: OpenDrag::default(),
            gain_drag: OpenDrag::default(),
            known_tones: None,
        }
    }

    /// Current mono/stereo/branch geometry.
    #[must_use]
    pub fn layout(&self) -> GalleryLayout {
        Self::layout_of(&self.backend.chain_state(0).chain)
    }

    /// The geometry of one chain snapshot.
    fn layout_of(chain: &ChainSnapshot) -> GalleryLayout {
        let tile_size = if chain.stereo_enabled { 160 } else { 224 };
        let branch_indent = chain.branch.as_ref().map_or(0, |branch| {
            let lane = match branch.side {
                ChainSide::Left => &chain.left,
                ChainSide::Right => &chain.right,
            };
            lane.iter()
                .position(|block| block.id == branch.after_block_id)
                .map_or(0, |index| (index as i32 + 1) * (tile_size + TILE_GAP))
        });
        GalleryLayout {
            tile_size,
            tile_gap: TILE_GAP,
            lane_gap: 24,
            edge_fade: 32,
            branch_indent,
        }
    }

    /// Current lane IDs in display order, including an optimistic drag preview.
    #[must_use]
    pub fn lane_ids(&self, side: ChainSide) -> Vec<String> {
        self.read_lane(side, |blocks| {
            blocks.iter().map(|block| block.id.0.clone()).collect()
        })
    }

    /// Current tile-selected navigation target.
    #[must_use]
    pub fn selection(&self) -> &GallerySelection {
        &self.selection
    }

    /// Returns to the gallery without changing the chain.
    pub fn clear_selection(&mut self) {
        self.selection = GallerySelection::None;
    }

    /// Activates a tile's detail or tone-selection destination.
    pub fn activate(&mut self, id: &str) {
        for side in [ChainSide::Left, ChainSide::Right] {
            if let Some(block) = self
                .native_lane(side)
                .into_iter()
                .find(|block| block.id.0 == id)
            {
                self.reveal = Some(block.id.clone());
                self.selection = if block.is_insert {
                    GallerySelection::SelectTone {
                        side,
                        target_block_id: block.id,
                    }
                } else {
                    GallerySelection::BlockDetail(block.id)
                };
                return;
            }
        }
    }

    /// Opens tone selection for an add or swap target.
    pub fn select_tone(&mut self, side: ChainSide, target_block_id: BlockId) {
        self.reveal = Some(target_block_id.clone());
        self.selection = GallerySelection::SelectTone {
            side,
            target_block_id,
        };
    }

    /// Brings one tile into view without changing navigation selection.
    pub fn reveal_tile(&mut self, id: &str) {
        self.reveal = Some(BlockId(id.to_owned()));
    }

    /// Supplies decoded artwork for one tile.
    pub fn set_artwork(&mut self, id: BlockId, texture: egui::TextureHandle) {
        self.artwork.insert(id, texture);
    }

    /// Replaces the artwork of all tiles, for a controller that fetches it.
    pub(crate) fn replace_artwork(&mut self, artwork: BTreeMap<BlockId, egui::TextureHandle>) {
        self.artwork = artwork;
    }

    /// Whether one tile has decoded artwork.
    #[must_use]
    pub fn has_artwork(&self, id: &BlockId) -> bool {
        self.artwork.contains_key(id)
    }

    /// A copy of one lane of the backend chain, for an edit of it.
    fn native_lane(&self, side: ChainSide) -> Vec<ChainBlock> {
        let view = self.backend.chain_state(0);
        match side {
            ChainSide::Left => view.chain.left.clone(),
            ChainSide::Right => view.chain.right.clone(),
        }
    }

    fn native_lanes(&self) -> PreviewLanes {
        PreviewLanes {
            left: self.native_lane(ChainSide::Left),
            right: self.native_lane(ChainSide::Right),
            target: None,
        }
    }

    /// Reads both lanes in display order, including a drag preview, with no copy of them.
    fn read_lanes<R>(&self, read: impl FnOnce(&[ChainBlock], &[ChainBlock]) -> R) -> R {
        if let Some(preview) = &self.preview {
            return read(&preview.left, &preview.right);
        }
        let view = self.backend.chain_state(0);
        read(&view.chain.left, &view.chain.right)
    }

    /// Reads one lane in display order, including a drag preview, with no copy of it.
    fn read_lane<R>(&self, side: ChainSide, read: impl FnOnce(&[ChainBlock]) -> R) -> R {
        self.read_lanes(|left, right| match side {
            ChainSide::Left => read(left),
            ChainSide::Right => read(right),
        })
    }

    fn lane_mut(lanes: &mut PreviewLanes, side: ChainSide) -> &mut Vec<ChainBlock> {
        match side {
            ChainSide::Left => &mut lanes.left,
            ChainSide::Right => &mut lanes.right,
        }
    }

    /// Current tile visual state.
    #[must_use]
    pub fn tile_state(&self, id: &str) -> TileVisualState {
        if self.drop_armed.as_ref().is_some_and(|armed| armed.0 == id) {
            return TileVisualState::DropArmed;
        }
        self.read_lanes(|left, right| {
            left.iter()
                .chain(right)
                .find(|block| block.id.0 == id)
                .map_or(TileVisualState::Ready, |block| self.visual_state(block))
        })
    }

    /// The visual state of one tile of the lanes that a frame shows.
    fn visual_state(&self, block: &ChainBlock) -> TileVisualState {
        if self.drop_armed.as_ref() == Some(&block.id) {
            TileVisualState::DropArmed
        } else if block.load_failed {
            TileVisualState::Failed
        } else if block.model_loading || !block.loaded {
            TileVisualState::Loading
        } else {
            TileVisualState::Ready
        }
    }

    /// Marks one tile as an armed local drop target.
    pub fn with_drop_armed(&mut self, id: &str) -> &mut Self {
        self.drop_armed = Some(BlockId(id.to_owned()));
        self
    }

    /// Whether the backend clipboard contains a tone.
    #[must_use]
    pub fn can_paste(&self) -> bool {
        self.backend.can_paste_block()
    }

    /// Toggles a block's enabled state without replacing other parameters.
    ///
    /// # Errors
    /// Returns missing-target or backend errors.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), UiError> {
        let mut block = self
            .native_lane(ChainSide::Left)
            .into_iter()
            .chain(self.native_lane(ChainSide::Right))
            .find(|block| block.id.0 == id)
            .ok_or_else(|| UiError::UnknownTarget(id.to_owned()))?;
        block.params = BlockParams {
            enabled,
            ..block.params
        };
        self.backend.dispatch(UiAction::SetBlockParams {
            block_id: BlockId(id.to_owned()),
            params: block.params,
        })?;
        Ok(())
    }

    /// Sets the normalized input gain of one tone as one step of a knob gesture. The backend
    /// keeps a complete gesture as one undo step.
    ///
    /// # Errors
    /// Returns missing-target or backend errors.
    pub fn set_input_gain(
        &mut self,
        id: &str,
        value: f64,
        gesture: GesturePhase,
    ) -> Result<(), UiError> {
        let chain = self.backend.chain_state(0).chain;
        let block_id = BlockId(id.to_owned());
        let block = chain
            .block(&block_id)
            .filter(|block| !block.is_insert)
            .ok_or_else(|| UiError::UnknownTarget(id.to_owned()))?;
        self.backend.dispatch(UiAction::SetBlockParamsGesture {
            block_id,
            params: BlockParams {
                input_gain: value.clamp(0.0, 1.0),
                ..block.params.clone()
            },
            gesture,
        })?;
        Ok(())
    }

    /// Copies a block.
    ///
    /// # Errors
    /// Returns missing-target or backend errors.
    pub fn copy(&mut self, id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::CopyBlock(BlockId(id.to_owned())))?;
        Ok(())
    }

    /// Pastes the clipboard once at the selected lane and index.
    ///
    /// # Errors
    /// Returns empty-clipboard or backend errors.
    pub fn paste(&mut self, side: ChainSide, index: usize) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::PasteBlock { side, index })?;
        Ok(())
    }

    /// Duplicates a block after itself.
    ///
    /// # Errors
    /// Returns missing-target or backend errors.
    pub fn duplicate(&mut self, id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::DuplicateBlock(BlockId(id.to_owned())))?;
        Ok(())
    }

    /// Replaces a block with an open insert.
    ///
    /// # Errors
    /// Returns missing-target or backend errors.
    pub fn remove(&mut self, id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::RemoveBlock(BlockId(id.to_owned())))?;
        Ok(())
    }

    /// Sets the branch after a tone block on the selected lane.
    ///
    /// # Errors
    /// Returns a backend branch error.
    pub fn set_branch(&mut self, side: ChainSide, id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::SetBranch(Some(ChainBranch {
                side,
                after_block_id: BlockId(id.to_owned()),
            })))?;
        Ok(())
    }

    /// Clears the active branch.
    ///
    /// # Errors
    /// Returns a backend branch error.
    pub fn clear_branch(&mut self) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::SetBranch(None))?;
        Ok(())
    }

    /// Swaps complete lanes, including branch and polarity state.
    ///
    /// # Errors
    /// Returns an invalid-action error outside stereo mode.
    pub fn swap_chains(&mut self) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::SwapChains)?;
        Ok(())
    }

    /// Retries a failed model load.
    ///
    /// # Errors
    /// Returns an invalid-action error when the block is not failed.
    pub fn retry(&mut self, id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::RetryModelLoad(BlockId(id.to_owned())))?;
        Ok(())
    }

    /// Loads and stashes a local file or folder into one insert.
    ///
    /// # Errors
    /// Returns validation, missing-target, or local I/O errors.
    pub fn drop_file(&mut self, insert_id: &str, path: &Path) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::DropFile {
            target_insert_id: BlockId(insert_id.to_owned()),
            path: path.to_path_buf(),
        })?;
        self.drop_armed = None;
        Ok(())
    }

    pub(crate) fn parameter(&self, id: &'static str) -> f32 {
        self.backend
            .parameter(ParameterId(id))
            .map_or(0.0, |value| value.0)
    }

    fn set_parameter(&self, id: &'static str, value: f32) -> Result<(), UiError> {
        if !value.is_finite() {
            return Err(UiError::InvalidAction(
                "parameter value is not finite".to_owned(),
            ));
        }
        self.backend
            .set_parameter(ParameterId(id), ParameterValue(value), GesturePhase::Change)
    }

    /// Sets one lane pan and mirrors it while pan-link is active.
    ///
    /// # Errors
    /// Returns a backend parameter error.
    pub fn set_pan(&mut self, side: ChainSide, value: f32) -> Result<(), UiError> {
        let (id, other_id, min, max) = match side {
            ChainSide::Left => ("chainPanLeft", "chainPanRight", 0.0, 0.5),
            ChainSide::Right => ("chainPanRight", "chainPanLeft", 0.5, 1.0),
        };
        let value = value.clamp(min, max);
        self.set_parameter(id, value)?;
        if self.parameter("chainPanLinked") > 0.5 {
            self.set_parameter(other_id, 1.0 - value)?;
        }
        Ok(())
    }

    /// Takes one frame of a lane pan knob that `pass` showed, with its edit, if any.
    ///
    /// A view has no way to show a backend error, and the next frame shows the value that the
    /// backend holds.
    pub(crate) fn pan_knob_frame(&mut self, side: ChainSide, pass: u64, edit: Option<KnobEdit>) {
        let tracked = self.pan_drag.track(&side, pass, edit.as_ref());
        if let Some(stale) = tracked.stale {
            let _ = self.pan_gesture(stale, GesturePhase::End);
        }
        if let Some(edit) = edit {
            let _ = self.edit_pan(side, edit, tracked.was_open);
        }
    }

    /// Sends one edit of a lane pan knob, and mirrors it while pan-link is active.
    ///
    /// A linked pan writes two parameters. A drag keeps them in its one gesture. A reset, a key
    /// press, or a typed value is a whole edit on its own, so it gets a gesture of its own, and
    /// one undo takes both pans back. `drag_open` tells whether a drag of this pan has an open
    /// gesture.
    fn edit_pan(
        &mut self,
        side: ChainSide,
        edit: KnobEdit,
        drag_open: bool,
    ) -> Result<(), UiError> {
        let value = edit.value as f32;
        match edit.phase {
            GesturePhase::Begin => {
                self.pan_gesture(side, GesturePhase::Begin)?;
                self.set_pan(side, value)
            }
            GesturePhase::Change => self.set_pan(side, value),
            GesturePhase::End => {
                if !drag_open {
                    self.pan_gesture(side, GesturePhase::Begin)?;
                }
                self.set_pan(side, value)?;
                self.pan_gesture(side, GesturePhase::End)
            }
            GesturePhase::Cancel => self.pan_gesture(side, GesturePhase::Cancel),
        }
    }

    /// Closes the gesture of a tile Gain drag with the settings that the backend holds.
    fn end_gain_drag(&self, id: &BlockId) {
        let chain = self.backend.chain_state(0).chain;
        if let Some(block) = chain.block(id) {
            let _ = self.backend.dispatch(UiAction::SetBlockParamsGesture {
                block_id: id.clone(),
                params: block.params.clone(),
                gesture: GesturePhase::End,
            });
        }
    }

    /// Opens, closes, or cancels the gesture of a lane pan with the value that it has now.
    fn pan_gesture(&self, side: ChainSide, gesture: GesturePhase) -> Result<(), UiError> {
        let id = match side {
            ChainSide::Left => "chainPanLeft",
            ChainSide::Right => "chainPanRight",
        };
        self.backend
            .set_parameter(ParameterId(id), ParameterValue(self.parameter(id)), gesture)
    }

    /// Changes pan-link and snaps the right pan to the left anchor when linked.
    ///
    /// # Errors
    /// Returns a backend parameter error.
    pub fn set_pan_linked(&mut self, linked: bool) -> Result<(), UiError> {
        self.set_parameter("chainPanLinked", f32::from(linked))?;
        if linked {
            let left = self.parameter("chainPanLeft");
            self.set_parameter("chainPanRight", 1.0 - left)?;
        }
        Ok(())
    }

    /// Toggles one lane's solo and clears the other lane when solo engages.
    ///
    /// # Errors
    /// Returns a backend parameter error.
    pub fn toggle_solo(&mut self, side: ChainSide) -> Result<(), UiError> {
        let (id, other_id) = match side {
            ChainSide::Left => ("chainSoloLeft", "chainSoloRight"),
            ChainSide::Right => ("chainSoloRight", "chainSoloLeft"),
        };
        let next = self.parameter(id) <= 0.5;
        self.set_parameter(id, f32::from(next))?;
        if next && self.parameter(other_id) > 0.5 {
            self.set_parameter(other_id, 0.0)?;
        }
        Ok(())
    }

    /// Toggles one lane's polarity.
    ///
    /// # Errors
    /// Returns a backend parameter error.
    pub fn toggle_invert(&mut self, side: ChainSide) -> Result<(), UiError> {
        let id = match side {
            ChainSide::Left => "chainInvertLeft",
            ChainSide::Right => "chainInvertRight",
        };
        let next = self.parameter(id) <= 0.5;
        self.set_parameter(id, f32::from(next))
    }

    /// Begins a pointer drag at the current revision.
    pub fn begin_drag(&mut self, id: &str, duplicate: bool) {
        let side = if self
            .native_lane(ChainSide::Right)
            .iter()
            .any(|block| block.id.0 == id)
        {
            ChainSide::Right
        } else {
            ChainSide::Left
        };
        self.preview = None;
        self.drag.begin(
            BlockId(id.to_owned()),
            side,
            self.backend.chain_state(0).revision,
            duplicate,
        );
    }

    /// Begins an already-engaged keyboard sort.
    pub fn begin_keyboard_sort(&mut self, id: &str) {
        let side = if self
            .native_lane(ChainSide::Right)
            .iter()
            .any(|block| block.id.0 == id)
        {
            ChainSide::Right
        } else {
            ChainSide::Left
        };
        self.preview = None;
        self.drag.begin_keyboard(
            BlockId(id.to_owned()),
            side,
            self.backend.chain_state(0).revision,
        );
    }

    /// Advances pointer travel.
    pub fn drag_by(&mut self, travel: f32) -> DragOutcome {
        self.drag.drag_by(travel)
    }

    /// Changes duplicate mode without resetting the active drag.
    pub fn set_drag_duplicate(&mut self, duplicate: bool) {
        self.drag.set_duplicate(duplicate);
        self.preview = None;
    }

    /// Updates the optimistic target without mutating the backend.
    pub fn preview_drop(&mut self, side: ChainSide, index: usize) -> DragOutcome {
        let Some((id, origin_side, duplicate)) =
            self.drag.commit(self.backend.chain_state(0).revision)
        else {
            return DragOutcome::Stale;
        };
        if side == ChainSide::Right && !self.backend.chain_state(0).chain.stereo_enabled {
            return DragOutcome::Stale;
        }
        let target = Some((side, index, duplicate));
        if self
            .preview
            .as_ref()
            .is_some_and(|preview| preview.target == target)
        {
            // A held pointer gives the same target in each frame, and the preview shows it.
            return DragOutcome::Engaged;
        }
        let mut lanes = if duplicate {
            self.native_lanes()
        } else {
            self.preview
                .as_deref()
                .cloned()
                .unwrap_or_else(|| self.native_lanes())
        };
        if duplicate {
            let origin = Self::lane_mut(&mut lanes, origin_side);
            let Some(origin_index) = origin.iter().position(|block| block.id == id) else {
                return DragOutcome::Stale;
            };
            let Some(source) = origin.get(origin_index).cloned() else {
                return DragOutcome::Stale;
            };
            if source.is_insert {
                return DragOutcome::Stale;
            }
            let mut stand_in = source.clone();
            stand_in.id = BlockId("__duplicate-stand-in__".to_owned());
            origin[origin_index] = stand_in;
            let target = Self::lane_mut(&mut lanes, side);
            target.insert(index.min(target.len()), source);
        } else {
            let mut source_side = None;
            let mut source_index = 0;
            for candidate in [ChainSide::Left, ChainSide::Right] {
                if let Some(index) = Self::lane_mut(&mut lanes, candidate)
                    .iter()
                    .position(|block| block.id == id)
                {
                    source_side = Some(candidate);
                    source_index = index;
                    break;
                }
            }
            let Some(source_side) = source_side else {
                return DragOutcome::Stale;
            };
            let source = Self::lane_mut(&mut lanes, source_side).remove(source_index);
            if source_side != side && source.is_insert {
                return DragOutcome::Stale;
            }
            let target = Self::lane_mut(&mut lanes, side);
            target.insert(index.min(target.len()), source);
        }
        lanes.target = target;
        self.preview = Some(Arc::new(lanes));
        DragOutcome::Engaged
    }

    /// Moves a keyboard-sorted block by one lane or slot.
    pub fn move_keyboard_by(&mut self, delta_x: i32, delta_y: i32) -> DragOutcome {
        let Some((id, _origin, _duplicate)) =
            self.drag.commit(self.backend.chain_state(0).revision)
        else {
            return DragOutcome::Stale;
        };
        let mut current_side = None;
        let mut current_index = 0;
        for side in [ChainSide::Left, ChainSide::Right] {
            if let Some(index) = self.read_lane(side, |blocks| {
                blocks.iter().position(|block| block.id == id)
            }) {
                current_side = Some(side);
                current_index = index;
                break;
            }
        }
        let Some(current_side) = current_side else {
            return DragOutcome::Stale;
        };
        let target_side = if delta_y < 0 {
            ChainSide::Left
        } else if delta_y > 0 {
            ChainSide::Right
        } else {
            current_side
        };
        let target_len = self.read_lane(target_side, <[ChainBlock]>::len);
        let target_index = if delta_x < 0 {
            current_index.saturating_sub(1)
        } else if delta_x > 0 {
            current_index
                .saturating_add(1)
                .min(target_len.saturating_sub(1))
        } else {
            current_index.min(target_len.saturating_sub(1))
        };
        self.preview_drop(target_side, target_index)
    }

    /// Commits one move or duplicate when ID and revision remain current.
    pub fn drop(&mut self, side: ChainSide, index: usize) -> DragOutcome {
        let Some((id, _origin, duplicate)) = self.drag.commit(self.backend.chain_state(0).revision)
        else {
            self.preview = None;
            return DragOutcome::Stale;
        };
        if self.preview_drop(side, index) == DragOutcome::Stale {
            self.drag.cancel();
            self.preview = None;
            return DragOutcome::Stale;
        }
        let result = if duplicate {
            self.backend.dispatch(UiAction::DuplicateBlockToLane {
                block_id: id.clone(),
                target_side: side,
                target_index: index,
            })
        } else {
            self.backend.dispatch(UiAction::MoveBlockToLane {
                block_id: id.clone(),
                target_side: side,
                target_index: index,
            })
        };
        self.drag.cancel();
        self.preview = None;
        if result.is_err() {
            return DragOutcome::Stale;
        }
        if duplicate {
            // The copy is a new tone, so the next frame reveals it as a landed tone.
            DragOutcome::Duplicated
        } else {
            // The lane closes its gaps, so the tone can land away from the slot of the drop.
            self.reveal = Some(id);
            DragOutcome::Moved
        }
    }

    /// Cancels without a mutation.
    pub fn cancel_drag(&mut self) {
        self.drag.cancel();
        self.preview = None;
    }

    /// Routes quick sideways touch flicks to the parent scroller.
    #[must_use]
    pub fn touch_flick(&self, travel_x: f32, travel_y: f32, elapsed_ms: u64) -> bool {
        DragController::touch_flick(travel_x, travel_y, elapsed_ms)
    }

    /// Whether a stationary held touch should open the tile menu.
    #[must_use]
    pub fn touch_hold(&self, travel_x: f32, travel_y: f32, elapsed_ms: u64) -> bool {
        DragController::touch_hold(travel_x, travel_y, elapsed_ms)
    }

    /// Renders the gallery and executes at most one tile action per frame.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        // One snapshot for the frame. The tiles borrow its lanes.
        let view = self.backend.chain_state(0);
        let chain = Arc::clone(&view.chain);
        let layout = Self::layout_of(&chain);
        let meters = self.backend.meter_levels();
        self.tile_rects.clear();
        self.lane_regions.clear();
        let bounds = ui.available_rect_before_wrap();
        let chain_area = Rect::from_min_max(
            bounds.min + egui::vec2(58.0, 0.0),
            bounds.max - egui::vec2(58.0, 0.0),
        );
        let mut scroller_rect = Rect::from_min_max(
            chain_area.min + egui::vec2(24.0, 24.0),
            chain_area.max - egui::vec2(24.0, 24.0),
        );
        // egui panics on a negative child size, and a very short window can give one.
        scroller_rect.max = scroller_rect.max.max(scroller_rect.min);
        if chain.stereo_enabled {
            let rail_rect = Rect::from_min_size(
                egui::pos2(scroller_rect.left(), scroller_rect.center().y - 172.0),
                egui::vec2(57.0, 344.0),
            );
            let mut rail_ui = ui.new_child(egui::UiBuilder::new().max_rect(rail_rect));
            lane::show_pan_rail(
                &mut rail_ui,
                self,
                chain.stereo_enabled && !chain.stereo_output,
            );
            scroller_rect.min.x += 57.0;
        }
        let lane_rows = if chain.stereo_enabled { 2 } else { 1 };
        let lanes_height = layout.tile_size * lane_rows + layout.lane_gap * (lane_rows - 1);
        self.reveal_landed_tone(&chain);
        let mut scroll_ui = ui.new_child(egui::UiBuilder::new().max_rect(scroller_rect));
        // The chain scrolls only from side to side, so a normal mouse wheel scrolls it too.
        scroll_ui.style_mut().always_scroll_the_only_direction = true;
        let output = egui::ScrollArea::horizontal()
            .id_salt("default-chain-scroll")
            .horizontal_scroll_offset(self.scroll_offset)
            .show(&mut scroll_ui, |ui| {
                // A tile action needs `self`, so the frame holds its own share of the drag
                // preview. The chain is shared too, and the tiles borrow the lanes with no copy.
                let preview = self.preview.clone();
                let (left, right): (&[ChainBlock], &[ChainBlock]) = match preview.as_deref() {
                    Some(preview) => (&preview.left, &preview.right),
                    None => (&chain.left, &chain.right),
                };
                let left_width = lane_width(left.len(), layout.tile_size)
                    + if chain
                        .branch
                        .as_ref()
                        .is_some_and(|branch| branch.side == ChainSide::Right)
                    {
                        layout.branch_indent
                    } else {
                        0
                    };
                let right_width = if chain.stereo_enabled {
                    lane_width(right.len(), layout.tile_size)
                        + if chain
                            .branch
                            .as_ref()
                            .is_some_and(|branch| branch.side == ChainSide::Left)
                        {
                            layout.branch_indent
                        } else {
                            0
                        }
                } else {
                    0
                };
                let content_width = (left_width.max(right_width) + 64) as f32;
                let content_size = egui::vec2(
                    content_width.max(scroller_rect.width()),
                    scroller_rect.height(),
                );
                let (content_rect, _) = ui.allocate_exact_size(content_size, Sense::hover());
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
                if !chain.stereo_enabled {
                    // The title stays above the tiles when a tall window centers them.
                    let tiles_top = (scroller_rect.height() - lanes_height as f32) / 2.0;
                    let title_y = (tiles_top - TITLE_ABOVE_TILES).max(9.5);
                    content.painter().text(
                        content_rect.left_top() + egui::vec2(32.0, title_y),
                        egui::Align2::LEFT_CENTER,
                        "SIGNAL CHAIN",
                        egui::FontId::monospace(16.0),
                        egui::Color32::WHITE,
                    );
                }
                let mut lanes = Vec::with_capacity(2);
                for side in [ChainSide::Left, ChainSide::Right] {
                    if side == ChainSide::Right && !chain.stereo_enabled {
                        continue;
                    }
                    let blocks = if side == ChainSide::Left { left } else { right };
                    let indent = if chain
                        .branch
                        .as_ref()
                        .is_some_and(|branch| branch.side != side)
                    {
                        layout.branch_indent
                    } else {
                        0
                    };
                    let row = if side == ChainSide::Left { 0 } else { 1 };
                    let origin = content_rect.left_top()
                        + egui::vec2(
                            32.0 + indent as f32,
                            (scroller_rect.height() - lanes_height as f32) / 2.0
                                + row as f32 * (layout.tile_size + layout.lane_gap) as f32,
                        );
                    let region = Rect::from_min_size(
                        origin,
                        egui::vec2(
                            lane_width(blocks.len(), layout.tile_size) as f32,
                            layout.tile_size as f32,
                        ),
                    );
                    lanes.push((side, blocks, origin));
                    self.lane_regions.push((side, region));
                }
                // A drag on a tile of the left lane can drop on the right lane, so each lane has
                // its region before any tile takes input.
                for (side, blocks, origin) in lanes {
                    Self::draw_ghost_rail(&mut content, blocks.len(), origin, layout.tile_size);
                    for (index, block) in blocks.iter().enumerate() {
                        let rect = Rect::from_min_size(
                            origin
                                + egui::vec2(
                                    (index as i32 * (layout.tile_size + TILE_GAP)) as f32,
                                    0.0,
                                ),
                            egui::vec2(layout.tile_size as f32, layout.tile_size as f32),
                        );
                        // A drag preview moves a tile to another place in this tree. The tile keeps
                        // the ID of its block, so the held drag stays on the tile that it began on.
                        let mut tile_ui = content.new_child(
                            egui::UiBuilder::new()
                                .scope_id(content.unique_id().with(("chain-tile", &block.id.0)))
                                .max_rect(rect),
                        );
                        let frame = tile::show(
                            &mut tile_ui,
                            block,
                            layout.tile_size,
                            self.visual_state(block),
                            meters
                                .blocks
                                .iter()
                                .find(|meter| meter.block_id == block.id),
                            self.artwork.get(&block.id),
                            routing_for(index, blocks.len()),
                        );
                        self.tile_rects.push((block.id.clone(), rect));
                        self.handle_tile_frame(
                            &mut content,
                            frame,
                            TileContext {
                                block,
                                side,
                                tile_size: layout.tile_size,
                            },
                        );
                    }
                    self.draw_branch_gaps(
                        &mut content,
                        BranchGaps {
                            side,
                            blocks,
                            origin,
                            tile_size: layout.tile_size,
                            branch: chain.branch.as_ref(),
                            revision: view.revision,
                        },
                    );
                }
                self.draw_branch_elbow(
                    &mut content,
                    layout.tile_size,
                    chain.branch.as_ref(),
                    [left, right],
                );
            });
        self.scroll_offset = output.state.clamped_offset().x;
        let max_offset = (output.content_size.x - output.inner_rect.width()).max(0.0);
        // A drop in this frame changed the chain after the tiles took their places, so the
        // places of this frame are old, and the reveal waits for the next frame.
        let places_are_current = self.backend.chain_state(0).chain.revision == chain.revision;
        if let Some(target) = self.reveal.clone() {
            if !places_are_current {
                ui.ctx().request_repaint();
            } else if let Some((_, rect)) = self.tile_rects.iter().find(|(id, _)| *id == target) {
                let revealed = reveal_offset(
                    self.scroll_offset,
                    max_offset,
                    output.inner_rect,
                    *rect,
                    reveal_margin(layout.edge_fade),
                );
                if revealed != self.scroll_offset {
                    self.scroll_offset = revealed;
                    ui.ctx().request_repaint();
                }
                self.reveal = None;
            } else if chain.block(&target).is_none() {
                // A tile that no lane holds can never come into view.
                self.reveal = None;
            }
        }
        Self::paint_edge_fades(ui, output.inner_rect, layout.edge_fade);
        self.show_scroll_arrows(ui, output.inner_rect, max_offset, layout.tile_size);

        self.show_meters_of(ui, &view);
        self.show_context_menu(ui);
        self.handle_file_drops(ui);
        self.close_hidden_drags(ui.ctx().cumulative_pass_nr());
    }

    /// Closes the gesture of a knob drag when `pass` did not show that knob.
    ///
    /// The gallery calls this at the end of its own frame. The page switch also calls it in a
    /// pass that does not show the gallery, so that a page that covers the gallery closes the
    /// drag at once.
    pub(crate) fn close_hidden_drags(&mut self, pass: u64) {
        if let Some(id) = self.gain_drag.take_hidden(pass) {
            self.end_gain_drag(&id);
        }
        if let Some(side) = self.pan_drag.take_hidden(pass) {
            let _ = self.pan_gesture(side, GesturePhase::End);
        }
    }

    /// Asks to reveal the one tone that is new since the last frame that showed the chain.
    ///
    /// A load puts its tone where an open slot was, and the lane then closes its gaps, so the
    /// tone can land away from the slot that the user picked. The browser hides the chain while
    /// the load runs, so the reveal must come when the chain shows the tone again.
    fn reveal_landed_tone(&mut self, chain: &ronie_core::ChainSnapshot) {
        // Each change of the chain gets a new revision, so the same revision has the same tones.
        if self
            .known_tones
            .as_ref()
            .is_some_and(|(revision, _)| *revision == chain.revision)
        {
            return;
        }
        let tones: Vec<BlockId> = chain
            .left
            .iter()
            .chain(&chain.right)
            .filter(|block| !block.is_insert)
            .map(|block| block.id.clone())
            .collect();
        if let Some((_, known)) = &self.known_tones
            && let Some(landed) = landed_tone(known, &tones)
        {
            self.reveal = Some(landed);
        }
        self.known_tones = Some((chain.revision, tones));
    }

    /// Paints an arrow button at each edge that has more tiles past it. A click scrolls one tile.
    fn show_scroll_arrows(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        max_offset: f32,
        tile: i32,
    ) {
        let (left, right) = hidden_edges(viewport, self.tile_rects.iter().map(|(_, rect)| *rect));
        let step = (tile + TILE_GAP) as f32;
        for (shown, toward_end, label) in [
            (left, false, "Scroll chain left"),
            (right, true, "Scroll chain right"),
        ] {
            if !shown {
                continue;
            }
            let x = if toward_end {
                viewport.right() - ARROW_INSET - ARROW_SIZE
            } else {
                viewport.left() + ARROW_INSET
            };
            let rect = Rect::from_min_size(
                egui::pos2(x, viewport.center().y - ARROW_SIZE / 2.0),
                egui::Vec2::splat(ARROW_SIZE),
            );
            let response = ui.interact(rect, ui.scope_id().with(label), Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
            let fill = if response.hovered() {
                crate::theme::palette::PLATE_HOVER
            } else {
                crate::theme::palette::ART_SCRIM
            };
            let painter = ui.painter_at(viewport);
            painter.rect_filled(rect, ARROW_SIZE / 2.0, fill);
            super::header::draw_chevron(&painter, rect, toward_end, false);
            if response.clicked() {
                let delta = if toward_end { step } else { -step };
                self.scroll_offset = (self.scroll_offset + delta).clamp(0.0, max_offset);
                ui.ctx().request_repaint();
            }
        }
    }

    /// Renders the main input and output rails independently of the chain view.
    pub fn show_meters(&self, ui: &mut egui::Ui) {
        self.show_meters_of(ui, &self.backend.chain_state(0));
    }

    fn show_meters_of(&self, ui: &mut egui::Ui, view: &crate::backend::ChainViewState) {
        let bounds = ui.available_rect_before_wrap();
        let input_stereo = view.chain.stereo_input
            && matches!(
                view.session.input_mode,
                ronie_core::InputMode::Stereo | ronie_core::InputMode::DualMono
            );
        let chain = &view.chain;
        let meters = self.backend.meter_levels();
        let meter_height = 342.0;
        let meter_y = bounds.top() + (bounds.height() - meter_height) / 2.0;
        let spread = self.parameter("spreadEnabled") > 0.5;
        let output_stereo =
            ((chain.stereo_enabled || spread) && chain.stereo_output) || chain.dual_mono_active;
        Self::draw_db_meter(
            ui,
            Rect::from_min_size(
                egui::pos2(bounds.left() + 18.0, meter_y),
                egui::vec2(46.0, meter_height),
            ),
            true,
            meters.input_db,
            input_stereo,
        );
        Self::draw_db_meter(
            ui,
            Rect::from_min_size(
                egui::pos2(bounds.right() - 64.0, meter_y),
                egui::vec2(46.0, meter_height),
            ),
            false,
            meters.output_db,
            output_stereo,
        );
    }

    fn draw_db_meter(
        ui: &mut egui::Ui,
        rect: Rect,
        labels_left: bool,
        levels: [f32; 2],
        stereo: bool,
    ) {
        // The end labels are taller than the 3 px margin of the meter rectangle.
        let painter = ui.painter_at(rect.expand2(egui::vec2(0.0, 6.0)));
        let gap = if labels_left { 10.0 } else { 6.0 };
        let count = if stereo { 2 } else { 1 };
        let dots_width = count as f32 * 6.0 + (count - 1) as f32 * 5.0;
        let slot_width = 18.0 + if labels_left { 10.0 } else { 6.0 } + 6.0;
        let row_width = 18.0 + gap + dots_width;
        let row_x = rect.left() + 6.0 + (slot_width - row_width) / 2.0;
        let dots_x = if labels_left { row_x + 28.0 } else { row_x };
        for db in [-60, -48, -36, -24, -18, -12, -9, -6, -3, 0] {
            let unit = ((db as f32 + 60.0) / 60.0).clamp(0.0, 1.0);
            let y = rect.bottom() - (3.0 + unit * (rect.height() - 6.0));
            let label_x = if labels_left {
                dots_x - gap - 18.0
            } else {
                dots_x + dots_width + gap
            };
            painter.text(
                egui::pos2(label_x + 18.0, y),
                egui::Align2::RIGHT_CENTER,
                db.to_string(),
                egui::FontId::monospace(8.0),
                crate::theme::palette::TEXT_MUTED,
            );
        }
        for (column, level) in levels.iter().copied().enumerate().take(count) {
            for index in 0..22 {
                let position = index as f32 / 21.0;
                let dot_db = -60.0 + position * 60.0;
                let active = level >= dot_db;
                let center = egui::pos2(
                    dots_x + column as f32 * 11.0 + 3.0,
                    rect.bottom() - (3.0 + index as f32 * 16.0),
                );
                let color = crate::theme::palette::fire_ramp(position);
                painter.circle_filled(
                    center,
                    3.0,
                    if active {
                        color
                    } else {
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 56)
                    },
                );
            }
        }
    }

    fn draw_ghost_rail(ui: &mut egui::Ui, count: usize, origin: Pos2, tile_size: i32) {
        let painter = ui.painter();
        let radius = if tile_size <= 160 { 17.0 } else { 21.0 };
        let mut previous = None;
        for index in 0..count {
            let center = origin
                + egui::vec2(
                    index as f32 * (tile_size + TILE_GAP) as f32 + tile_size as f32 / 2.0,
                    tile_size as f32 / 2.0,
                );
            if let Some(previous) = previous {
                painter.line_segment(
                    [
                        previous + egui::vec2(radius - 4.0, 0.0),
                        center - egui::vec2(radius - 4.0, 0.0),
                    ],
                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                );
            }
            painter.circle_stroke(center, radius, egui::Stroke::new(1.5, egui::Color32::WHITE));
            previous = Some(center);
        }
    }

    fn draw_branch_elbow(
        &self,
        ui: &mut egui::Ui,
        tile_size: i32,
        branch: Option<&ChainBranch>,
        [left, right]: [&[ChainBlock]; 2],
    ) {
        let Some(branch) = branch else {
            return;
        };
        let Some((_, trunk)) = self
            .lane_regions
            .iter()
            .find(|(side, _)| *side == branch.side)
        else {
            return;
        };
        let Some((_, other)) = self
            .lane_regions
            .iter()
            .find(|(side, _)| *side != branch.side)
        else {
            return;
        };
        let trunk_blocks = match branch.side {
            ChainSide::Left => left,
            ChainSide::Right => right,
        };
        let Some(index) = trunk_blocks
            .iter()
            .position(|block| block.id == branch.after_block_id)
        else {
            return;
        };
        let x = trunk.left() + lane::gap_center_x(index + 1, tile_size);
        let trunk_y = trunk.center().y;
        let other_y = other.center().y;
        let stroke = egui::Stroke::new(2.0, egui::Color32::WHITE);
        ui.painter()
            .line_segment([egui::pos2(x, trunk_y), egui::pos2(x, other_y)], stroke);
        ui.painter().line_segment(
            [egui::pos2(x, other_y), egui::pos2(x + 12.0, other_y)],
            stroke,
        );
    }

    fn draw_branch_gaps(&mut self, ui: &mut egui::Ui, gaps: BranchGaps<'_>) {
        let BranchGaps {
            side,
            blocks,
            origin,
            tile_size,
            branch,
            revision,
        } = gaps;
        let dragging = self.drag.commit(revision).is_some();
        for (index, block) in blocks
            .iter()
            .enumerate()
            .take(blocks.len().saturating_sub(1))
        {
            if block.is_insert {
                continue;
            }
            let active_tap = branch
                .is_some_and(|branch| branch.side == side && branch.after_block_id == block.id);
            if dragging && !active_tap {
                continue;
            }
            let center = origin
                + egui::vec2(
                    lane::gap_center_x(index + 1, tile_size),
                    tile_size as f32 / 2.0,
                );
            let rect = Rect::from_center_size(center, egui::vec2(24.0, tile_size as f32));
            let response = ui.interact(
                rect,
                ui.scope_id().with(("branch", side.as_str(), &block.id.0)),
                Sense::click(),
            );
            let action = if active_tap {
                "Clear branch after"
            } else {
                "Branch after"
            };
            let title = block.tone.as_ref().map_or(&block.id.0, |tone| &tone.title);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{action} {title}"))
            });
            if response.hovered() {
                ui.painter()
                    .circle_filled(center, 5.0, egui::Color32::WHITE);
            }
            if response.clicked() {
                if active_tap {
                    let _ = self.clear_branch();
                } else {
                    let _ = self.set_branch(side, &block.id.0);
                }
            }
        }
    }

    fn handle_tile_frame(
        &mut self,
        ui: &mut egui::Ui,
        frame: tile::TileFrame,
        context: TileContext<'_>,
    ) {
        let id = &context.block.id;
        if frame.response.secondary_clicked() {
            let at = ui.ctx().pointer_latest_pos().unwrap_or(frame.rect.center());
            self.open_menu(id.clone(), at);
        }
        self.track_press(ui, &frame, context.block);
        if self.menu_for.as_ref() == Some(id) {
            self.menu_focus = Some(frame.response.id);
        }
        let panning = self
            .press
            .as_ref()
            .is_some_and(|press| press.id == *id && press.panning);
        if frame.response.drag_started() && !panning {
            self.begin_drag(id.0.as_str(), ui.input(|input| input.modifiers.alt));
            frame.response.request_focus();
        }
        if frame.response.dragged()
            && !panning
            && self.drag_by(frame.response.drag_delta().length()) == DragOutcome::Engaged
        {
            let alt = ui.input(|input| input.modifiers.alt);
            if self.drag.duplicate() != Some(alt) {
                self.set_drag_duplicate(alt);
            }
            if let Some(pointer) = ui.ctx().pointer_latest_pos() {
                let (side, index) = self.drag_target(pointer, context.tile_size);
                let _ = self.preview_drop(side, index);
            }
        }
        if frame.response.drag_stopped() && !panning && self.active_drag_id().as_ref() == Some(id) {
            if let Some(pointer) = ui.ctx().pointer_latest_pos() {
                let (side, index) = self.drag_target(pointer, context.tile_size);
                let _ = self.drop(side, index);
            } else {
                self.cancel_drag();
            }
        }
        if frame.response.has_focus() {
            self.handle_tile_keyboard(ui, id);
        }
        if frame.gain_shown {
            let pass = ui.ctx().cumulative_pass_nr();
            if let Some(stale) = self.gain_drag.track(id, pass, frame.gain.as_ref()).stale {
                self.end_gain_drag(&stale);
            }
        }
        if let Some(edit) = frame.gain {
            // The next frame shows the value that the backend holds.
            let _ = self.set_input_gain(&id.0, edit.value, edit.phase);
        }
        let Some(action) = frame.action else {
            return;
        };
        if action == TileAction::Open
            && self
                .suppress_click_until
                .as_ref()
                .is_some_and(|(blocked, until)| *blocked == *id && Instant::now() < *until)
        {
            self.suppress_click_until = None;
            return;
        }
        match action {
            TileAction::Open => self.activate(&id.0),
            TileAction::Power => {
                let _ = self.set_enabled(&id.0, !context.block.params.enabled);
            }
            TileAction::Swap => {
                self.selection = GallerySelection::SelectTone {
                    side: context.side,
                    target_block_id: id.clone(),
                };
            }
            TileAction::Remove => {
                let _ = self.remove(&id.0);
            }
            TileAction::Retry => {
                let _ = self.retry(&id.0);
            }
        }
    }

    fn track_press(&mut self, ui: &mut egui::Ui, frame: &tile::TileFrame, block: &ChainBlock) {
        if frame.response.is_pointer_button_down_on() {
            let pointer = ui.ctx().pointer_latest_pos().unwrap_or(frame.rect.center());
            if self.press.as_ref().is_none_or(|press| press.id != block.id) {
                let touch = ui.input(|input| {
                    input
                        .events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Touch { .. }))
                });
                self.press = Some(PressState {
                    id: block.id.clone(),
                    started: Instant::now(),
                    position: pointer,
                    touch,
                    panning: false,
                });
            }
            let mut open_at = None;
            let mut pan = false;
            if let Some(press) = &mut self.press
                && press.id == block.id
                && press.touch
            {
                let travel = pointer - press.position;
                let elapsed = press.started.elapsed().as_millis() as u64;
                if !press.panning && DragController::touch_flick(travel.x, travel.y, elapsed) {
                    press.panning = true;
                    pan = true;
                }
                if !press.panning
                    && DragController::touch_hold(travel.x, travel.y, elapsed)
                    && self.menu_for.as_ref() != Some(&block.id)
                {
                    open_at = Some(pointer + egui::vec2(0.0, 24.0));
                }
            }
            if pan {
                self.scroll_offset = (self.scroll_offset - frame.response.drag_motion().x).max(0.0);
                self.cancel_drag();
                self.menu_for = None;
            }
            if let Some(at) = open_at {
                self.open_menu(block.id.clone(), at);
            }
        } else if self
            .press
            .as_ref()
            .is_some_and(|press| press.id == block.id)
        {
            self.press = None;
        }
    }

    fn handle_tile_keyboard(&mut self, ui: &mut egui::Ui, id: &BlockId) {
        let active = self.active_drag_id();
        let (escape, pick, left, right, up, down) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::Escape),
                input.key_pressed(egui::Key::Space) || input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
            )
        });
        if escape && active.is_some() {
            self.cancel_drag();
            return;
        }
        if pick {
            if active.as_ref() == Some(id) {
                if let Some((side, index)) = self.current_drag_slot() {
                    let _ = self.drop(side, index);
                }
            } else {
                self.begin_keyboard_sort(&id.0);
            }
            return;
        }
        if active.as_ref() != Some(id) {
            return;
        }
        let (x, y) = match (left, right, up, down) {
            (true, false, false, false) => (-1, 0),
            (false, true, false, false) => (1, 0),
            (false, false, true, false) => (0, -1),
            (false, false, false, true) => (0, 1),
            _ => (0, 0),
        };
        if x != 0 || y != 0 {
            let _ = self.move_keyboard_by(x, y);
        }
    }

    fn active_drag_id(&self) -> Option<BlockId> {
        self.drag
            .commit(self.backend.chain_state(0).revision)
            .map(|(id, _, _)| id)
    }

    fn current_drag_slot(&self) -> Option<(ChainSide, usize)> {
        let id = self.active_drag_id()?;
        for side in [ChainSide::Left, ChainSide::Right] {
            if let Some(index) = self.read_lane(side, |blocks| {
                blocks.iter().position(|block| block.id == id)
            }) {
                return Some((side, index));
            }
        }
        None
    }

    fn drag_target(&self, pointer: Pos2, tile_size: i32) -> (ChainSide, usize) {
        let side = if self.lane_regions.len() == 2 {
            let seam = self.lane_regions[0].1.bottom() + 12.0;
            if pointer.y < seam {
                ChainSide::Left
            } else {
                ChainSide::Right
            }
        } else {
            ChainSide::Left
        };
        let x = self
            .lane_regions
            .iter()
            .find(|(candidate, _)| *candidate == side)
            .map_or(0.0, |(_, rect)| pointer.x - rect.left());
        (
            side,
            slot_at(x, self.read_lane(side, <[ChainBlock]>::len), tile_size),
        )
    }

    /// Opens a tile context menu at its tile anchor.
    pub fn open_tile_menu(&mut self, id: &str) {
        let center = self
            .tile_rects
            .iter()
            .find(|(block_id, _)| block_id.0 == id)
            .map(|(_, rect)| rect.center())
            .unwrap_or_else(|| egui::pos2(400.0, 200.0));
        self.open_menu(BlockId(id.to_owned()), center);
    }

    fn open_menu(&mut self, id: BlockId, at: Pos2) {
        self.menu_for = Some(id.clone());
        self.menu_anchor = at;
        self.suppress_click_until = Some((id, Instant::now() + Duration::from_millis(700)));
    }

    fn show_context_menu(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.menu_for.clone() else {
            return;
        };
        let Some((side, index, block)) =
            [ChainSide::Left, ChainSide::Right]
                .into_iter()
                .find_map(|side| {
                    self.read_lane(side, |blocks| {
                        blocks
                            .iter()
                            .position(|block| block.id == id)
                            .and_then(|index| {
                                blocks.get(index).cloned().map(|block| (side, index, block))
                            })
                    })
                })
        else {
            self.menu_for = None;
            return;
        };
        let can_paste = self.can_paste();
        let items = tile_menu::items(block.is_insert, can_paste);
        let trigger = self
            .tile_rects
            .iter()
            .find(|(tile, _)| *tile == id)
            .map(|(_, rect)| Trigger::new(*rect, self.menu_focus));
        let shown = Popover::new(
            egui::Id::unique(("chain-menu", &id.0)),
            Placement::At(self.menu_anchor + egui::vec2(6.0, 6.0)),
            egui::vec2(148.0, 14.0 + items.len() as f32 * 34.0),
            Surface::edged(12.0),
        )
        .trigger(trigger)
        .show(ui, |ui, panel| {
            let mut pick = None;
            for (row, item) in items.into_iter().enumerate() {
                let rect = egui::Rect::from_min_size(
                    panel.min + egui::vec2(6.0, 6.0 + row as f32 * 34.0),
                    egui::vec2(136.0, 34.0),
                );
                let enabled = item != TileMenuItem::Paste || can_paste;
                if Self::tile_menu_row(ui, rect, item, enabled).clicked() {
                    pick = Some(item);
                }
            }
            pick
        });
        match shown.pick {
            Some(TileMenuItem::Copy) => {
                let _ = self.copy(&id.0);
            }
            Some(TileMenuItem::Paste) => {
                let _ = self.paste(side, index);
            }
            Some(TileMenuItem::LoadFile) => self.pick_local(&id.0, false),
            Some(TileMenuItem::LoadFolder) => self.pick_local(&id.0, true),
            Some(TileMenuItem::Remove) => {
                let _ = self.remove(&id.0);
            }
            None => {}
        }
        if shown.dismissed.is_some() {
            self.menu_for = None;
        }
    }

    fn tile_menu_row(
        ui: &mut egui::Ui,
        rect: egui::Rect,
        item: TileMenuItem,
        enabled: bool,
    ) -> egui::Response {
        let label = match item {
            TileMenuItem::Copy => "Copy",
            TileMenuItem::Paste => "Paste",
            TileMenuItem::LoadFile => "Load File",
            TileMenuItem::LoadFolder => "Load Folder",
            TileMenuItem::Remove => "Remove",
        };
        let response = ui.interact(
            rect,
            ui.scope_id().with(("tile-menu-row", label)),
            egui::Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, enabled, label.to_owned())
        });
        let color = if enabled {
            egui::Color32::WHITE
        } else {
            egui::Color32::GRAY
        };
        let painter = ui.painter_at(rect);
        if enabled && response.hovered() {
            painter.rect_filled(rect, 8.0, crate::theme::palette::HOVER_FILL);
        }
        let center = egui::pos2(rect.left() + 20.0, rect.center().y);
        let stroke = egui::Stroke::new(1.2, color);
        match item {
            TileMenuItem::Copy | TileMenuItem::Paste => {
                let back = egui::Rect::from_center_size(
                    center + egui::vec2(-3.0, -2.0),
                    egui::vec2(9.0, 11.0),
                );
                let front = egui::Rect::from_center_size(
                    center + egui::vec2(3.0, 2.0),
                    egui::vec2(9.0, 11.0),
                );
                painter.rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
                painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
            }
            TileMenuItem::LoadFile => {
                let page = egui::Rect::from_center_size(center, egui::vec2(10.0, 14.0));
                painter.rect_stroke(page, 1.0, stroke, egui::StrokeKind::Inside);
            }
            TileMenuItem::LoadFolder => {
                let folder = egui::Rect::from_center_size(
                    center + egui::vec2(0.0, 2.0),
                    egui::vec2(14.0, 10.0),
                );
                painter.rect_stroke(folder, 1.0, stroke, egui::StrokeKind::Inside);
                painter.line_segment(
                    [
                        folder.left_top() + egui::vec2(0.0, 2.0),
                        folder.left_top() + egui::vec2(5.0, 2.0),
                    ],
                    stroke,
                );
            }
            TileMenuItem::Remove => {
                painter.line_segment(
                    [
                        center + egui::vec2(-5.0, -5.0),
                        center + egui::vec2(5.0, 5.0),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        center + egui::vec2(-5.0, 5.0),
                        center + egui::vec2(5.0, -5.0),
                    ],
                    stroke,
                );
            }
        }
        painter.text(
            egui::pos2(rect.left() + 42.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            color,
        );
        response
    }

    fn pick_local(&mut self, id: &str, folder: bool) {
        let dialog = rfd::FileDialog::new()
            .set_title("Open a Ronnie model")
            .add_filter("Audio models", &["nam", "wav"]);
        let path = if folder {
            dialog.pick_folder()
        } else {
            dialog.pick_file()
        };
        if let Some(path) = path {
            let _ = self.drop_file(id, &path);
        }
    }

    fn handle_file_drops(&mut self, ui: &mut egui::Ui) {
        let (hovering, dropped, pointer) = ui.input(|input| {
            (
                !input.raw.hovered_files.is_empty(),
                input.raw.dropped_files.clone(),
                input.pointer.latest_pos(),
            )
        });
        let target = pointer.and_then(|position| {
            self.tile_rects
                .iter()
                .find(|(_, rect)| rect.contains(position))
                .map(|(id, _)| id.clone())
        });
        self.drop_armed = if hovering { target.clone() } else { None };
        if let Some(path) = dropped.first().map(|file| file.path())
            && let Some(target) = target
        {
            let _ = self.drop_file(&target.0, path);
        }
    }

    fn paint_edge_fades(ui: &mut egui::Ui, rect: Rect, width: i32) {
        let painter = ui.painter_at(rect);
        let width = width as f32;
        let opaque = egui::Color32::BLACK;
        let clear = egui::Color32::TRANSPARENT;
        let left = Rect::from_min_size(rect.min, egui::vec2(width, rect.height()));
        let right = Rect::from_min_max(egui::pos2(rect.right() - width, rect.top()), rect.max);
        painter.add(horizontal_gradient(left, opaque, clear));
        painter.add(horizontal_gradient(right, clear, opaque));
    }
}

/// The distance from a chain edge inside which a revealed tile is not fully visible, because the
/// edge fade and the scroll arrow cover it.
fn reveal_margin(edge_fade: i32) -> f32 {
    (edge_fade as f32).max(ARROW_INSET + ARROW_SIZE) + ARROW_INSET
}

/// The scroll offset that shows `tile` fully inside `viewport`, clear of `margin` at each edge.
///
/// The offset changes as little as it can, and stays in `0..=max_offset`.
fn reveal_offset(offset: f32, max_offset: f32, viewport: Rect, tile: Rect, margin: f32) -> f32 {
    let margin = margin.min(((viewport.width() - tile.width()) / 2.0).max(0.0));
    let target = if tile.right() > viewport.right() - margin {
        offset + tile.right() - (viewport.right() - margin)
    } else if tile.left() < viewport.left() + margin {
        offset - ((viewport.left() + margin) - tile.left())
    } else {
        offset
    };
    target.clamp(0.0, max_offset.max(0.0))
}

/// Whether tiles continue past the left edge and past the right edge of `viewport`.
fn hidden_edges(viewport: Rect, tiles: impl Iterator<Item = Rect>) -> (bool, bool) {
    tiles.fold((false, false), |(left, right), tile| {
        (
            left || tile.left() < viewport.left() - 0.5,
            right || tile.right() > viewport.right() + 0.5,
        )
    })
}

/// The one tone in `current` that `known` does not have. More than one new tone is a preset or
/// a session change, which has no single tone to show.
fn landed_tone(known: &[BlockId], current: &[BlockId]) -> Option<BlockId> {
    let mut new = current.iter().filter(|id| !known.contains(id));
    let landed = new.next()?;
    new.next().is_none().then(|| landed.clone())
}

/// One smooth quad, because stacked translucent strips show seams at fractional zoom.
fn horizontal_gradient(rect: Rect, left: egui::Color32, right: egui::Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(2, 1, 3);
    egui::Shape::mesh(mesh)
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Rect, pos2, vec2};
    use ronie_core::BlockId;

    use super::{hidden_edges, landed_tone, reveal_margin, reveal_offset};

    fn viewport() -> Rect {
        Rect::from_min_size(pos2(100.0, 0.0), vec2(800.0, 300.0))
    }

    fn tile(left: f32) -> Rect {
        Rect::from_min_size(pos2(left, 40.0), vec2(224.0, 224.0))
    }

    #[test]
    fn a_tile_past_the_right_edge_scrolls_fully_clear_of_the_fade() {
        let margin = reveal_margin(32);
        let offset = reveal_offset(100.0, 2_000.0, viewport(), tile(850.0), margin);
        let shown = tile(850.0).translate(vec2(-(offset - 100.0), 0.0));
        assert!(shown.right() <= viewport().right() - margin + 0.001);
        assert!(shown.left() >= viewport().left() + margin);
    }

    #[test]
    fn a_tile_past_the_left_edge_scrolls_back_and_the_offset_stays_in_range() {
        let margin = reveal_margin(32);
        let offset = reveal_offset(300.0, 2_000.0, viewport(), tile(20.0), margin);
        assert_eq!(offset, 300.0 - (100.0 + margin - 20.0));
        assert_eq!(
            reveal_offset(10.0, 2_000.0, viewport(), tile(-400.0), margin),
            0.0
        );
        assert_eq!(
            reveal_offset(0.0, 50.0, viewport(), tile(2_000.0), margin),
            50.0
        );
    }

    #[test]
    fn a_visible_tile_does_not_scroll() {
        let margin = reveal_margin(32);
        assert_eq!(
            reveal_offset(120.0, 2_000.0, viewport(), tile(300.0), margin),
            120.0
        );
    }

    #[test]
    fn an_arrow_shows_only_at_an_edge_with_tiles_past_it() {
        let tiles = [tile(132.0), tile(380.0), tile(628.0), tile(876.0)];
        assert_eq!(hidden_edges(viewport(), tiles.into_iter()), (false, true));
        let tiles = [tile(-200.0), tile(300.0)];
        assert_eq!(hidden_edges(viewport(), tiles.into_iter()), (true, false));
        assert_eq!(
            hidden_edges(viewport(), [tile(300.0)].into_iter()),
            (false, false)
        );
    }

    #[test]
    fn only_one_new_tone_is_a_tone_to_reveal() {
        let id = |name: &str| BlockId(name.to_owned());
        let known = [id("a"), id("b")];
        assert_eq!(
            landed_tone(&known, &[id("a"), id("b"), id("c")]),
            Some(id("c"))
        );
        assert_eq!(landed_tone(&known, &[id("b")]), None);
        assert_eq!(landed_tone(&known, &[id("c"), id("d")]), None);
    }
}

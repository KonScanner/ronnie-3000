use crate::{
    ChainError, PlanarBuffer, PrepareSpec,
    block::{BlockModel, prepare_block},
    lane::{Lane, LaneSnapshot, unique_block_id},
    routing::{RoutingControls, RoutingState, Topology},
};
use ronie_core::{BlockId, ChainBlock, ChainSide, InputMode};

/// One branch tap selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Trunk side containing the tap block.
    pub side: ChainSide,
    /// Tone block after which the other lane starts.
    pub after_block: BlockId,
}

/// Both real-time lanes and their immutable topology settings.
pub struct LaneSet {
    /// Left lane.
    pub left: Lane,
    /// Right lane.
    pub right: Lane,
    /// Whether stereo chains are enabled.
    pub stereo_enabled: bool,
    /// Active or dormant branch settings.
    pub branch: Option<Branch>,
    /// Session input mode shared by structural operations.
    pub input_mode: InputMode,
}

impl LaneSet {
    /// Builds and normalizes both lanes from immutable snapshots.
    ///
    /// # Errors
    ///
    /// Returns block preparation errors.
    pub fn new(
        left: LaneSnapshot,
        right: LaneSnapshot,
        spec: PrepareSpec,
        stereo_enabled: bool,
        input_mode: InputMode,
    ) -> Result<Self, ChainError> {
        let mut ids = std::collections::BTreeSet::new();
        if left
            .blocks
            .iter()
            .chain(&right.blocks)
            .any(|block| !ids.insert(&block.id))
        {
            return Err(ChainError::Preparation(
                "block identities must be unique across both lanes".into(),
            ));
        }
        let mut lanes = Self {
            left: Lane::from_snapshot(left, spec)?,
            right: Lane::from_snapshot(right, spec)?,
            stereo_enabled,
            branch: None,
            input_mode,
        };
        lanes.align();
        Ok(lanes)
    }

    /// Captures settings without sharing processor history.
    #[must_use]
    pub fn snapshot(&self) -> LaneSetSnapshot {
        LaneSetSnapshot {
            left: self.left.snapshot(),
            right: self.right.snapshot(),
            stereo_enabled: self.stereo_enabled,
            branch: self.branch.clone(),
            input_mode: self.input_mode,
        }
    }

    /// Restores both lanes from immutable snapshots.
    ///
    /// # Errors
    ///
    /// Returns model preparation errors.
    pub fn restore(
        &mut self,
        snapshot: LaneSetSnapshot,
        spec: PrepareSpec,
    ) -> Result<(), ChainError> {
        let restored = Self::new(
            snapshot.left,
            snapshot.right,
            spec,
            snapshot.stereo_enabled,
            snapshot.input_mode,
        )?;
        self.left = restored.left;
        self.right = restored.right;
        self.stereo_enabled = restored.stereo_enabled;
        self.branch = snapshot.branch;
        self.input_mode = restored.input_mode;
        self.align();
        Ok(())
    }

    /// Current topology after branch validation.
    #[must_use]
    pub fn topology(&self) -> Topology {
        if !self.stereo_enabled {
            return Topology::Mono;
        }
        if let Some(branch) = &self.branch {
            return Topology::Branched {
                trunk: branch.side,
                after_block: branch.after_block.clone(),
            };
        }
        Topology::Stereo
    }

    fn align(&mut self) {
        self.left.normalize_inserts();
        self.right.normalize_inserts();
        let tap = self.branch.as_ref().and_then(|branch| {
            let trunk = self.lane(branch.side);
            trunk.position(&branch.after_block).filter(|index| {
                trunk
                    .blocks
                    .get(*index)
                    .is_some_and(|block| !block.is_insert)
            })
        });
        if self.branch.is_some() && tap.is_none() {
            self.branch = None;
        }
        let Some(tap) = tap else {
            return;
        };
        let Some(branch) = self.branch.as_ref() else {
            return;
        };
        let target = self.lane(branch.side).blocks.len().saturating_sub(tap + 1);
        self.lane_mut(branch.side.other())
            .trim_trailing_inserts_to(target);
    }

    fn lane(&self, side: ChainSide) -> &Lane {
        match side {
            ChainSide::Left => &self.left,
            ChainSide::Right => &self.right,
        }
    }

    fn lane_mut(&mut self, side: ChainSide) -> &mut Lane {
        match side {
            ChainSide::Left => &mut self.left,
            ChainSide::Right => &mut self.right,
        }
    }
}

/// Immutable snapshot of both lanes.
#[derive(Clone)]
pub struct LaneSetSnapshot {
    /// Left lane snapshot.
    pub left: LaneSnapshot,
    /// Right lane snapshot.
    pub right: LaneSnapshot,
    /// Stereo enable state.
    pub stereo_enabled: bool,
    /// Branch selection, including a dormant branch.
    pub branch: Option<Branch>,
    /// Session input mode.
    pub input_mode: InputMode,
}

/// Callback-owner topology, structural history, and chain-edit fade.
pub struct RtTopology {
    spec: PrepareSpec,
    edit_fade: crate::block::gain::LinearGain,
    history: Vec<LaneSetSnapshot>,
    future: Vec<LaneSetSnapshot>,
    clipboard: Option<(ChainBlock, BlockModel)>,
    input_scratch: Vec<f32>,
}

impl RtTopology {
    /// Creates topology state without model preparation.
    #[must_use]
    pub fn new(spec: PrepareSpec) -> Self {
        let mut edit_fade = crate::block::gain::LinearGain::new(spec.host_sample_rate, 0.025, 1.0);
        edit_fade.set_current_and_target(1.0);
        Self {
            spec,
            edit_fade,
            history: Vec::new(),
            future: Vec::new(),
            clipboard: None,
            input_scratch: vec![0.0; spec.max_host_frames],
        }
    }

    /// Processes input routing, lane topology, output fold, and structural fade.
    ///
    /// # Errors
    ///
    /// Returns invalid-layout or block processing errors.
    pub fn process(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        lanes: &mut LaneSet,
        controls: RoutingControls,
    ) -> Result<(), ChainError> {
        self.process_with_pool(buffer, lanes, controls, None)
    }

    /// Processes routing and lanes with optional phase/voice scheduling.
    ///
    /// # Errors
    ///
    /// Returns invalid-layout or block processing errors.
    pub fn process_with_pool(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        lanes: &mut LaneSet,
        mut controls: RoutingControls,
        pool: Option<&crate::rt::RtWorkerPool>,
    ) -> Result<(), ChainError> {
        let channels = buffer.channels();
        let frames = buffer.frames();
        if !matches!(channels, 1 | 2) || frames > self.spec.max_host_frames {
            return Err(ChainError::InvalidLayout("invalid topology callback"));
        }
        controls.input_mode = lanes.input_mode;
        let state = RoutingState::resolve(channels, &controls, lanes.stereo_enabled);
        controls.block.dual_mono = state.dual_mono_active;
        self.input_scratch[..frames].copy_from_slice(buffer.channel(0)?);

        if channels > 1 {
            if !controls.stereo_input {
                let (left, right) = buffer.as_mut_slice().split_at_mut(frames);
                right.copy_from_slice(left);
            } else {
                match controls.input_mode {
                    InputMode::Left => {
                        let (left, right) = buffer.as_mut_slice().split_at_mut(frames);
                        right.copy_from_slice(left);
                    }
                    InputMode::Right => {
                        let (left, right) = buffer.as_mut_slice().split_at_mut(frames);
                        left.copy_from_slice(right);
                    }
                    InputMode::Stereo | InputMode::DualMono => {
                        if !state.dual_mono_active && !lanes.stereo_enabled {
                            let (left, right) = buffer.as_mut_slice().split_at_mut(frames);
                            for frame in 0..frames {
                                let folded = 0.5 * (left[frame] + right[frame]);
                                left[frame] = folded;
                                right[frame] = folded;
                            }
                        }
                    }
                }
            }
        }

        match (lanes.stereo_enabled, lanes.branch.as_ref()) {
            (false, _) => {
                lanes.left.process_range_with_pool(
                    buffer,
                    0,
                    lanes.left.blocks.len(),
                    controls.block,
                    pool,
                )?;
            }
            (true, None) => {
                if channels == 1 {
                    let mut right =
                        PlanarBuffer::new(&mut self.input_scratch[..frames], 1, frames)?;
                    lanes.left.process_range_with_pool(
                        buffer,
                        0,
                        lanes.left.blocks.len(),
                        controls.block,
                        pool,
                    )?;
                    lanes.right.process_range_with_pool(
                        &mut right,
                        0,
                        lanes.right.blocks.len(),
                        controls.block,
                        pool,
                    )?;
                } else {
                    let (left_data, right_data) = buffer.as_mut_slice().split_at_mut(frames);
                    let mut left = PlanarBuffer::new(left_data, 1, frames)?;
                    let mut right = PlanarBuffer::new(right_data, 1, frames)?;
                    lanes.left.process_range_with_pool(
                        &mut left,
                        0,
                        lanes.left.blocks.len(),
                        controls.block,
                        pool,
                    )?;
                    lanes.right.process_range_with_pool(
                        &mut right,
                        0,
                        lanes.right.blocks.len(),
                        controls.block,
                        pool,
                    )?;
                }
            }
            (true, Some(branch)) => {
                let trunk = branch.side;
                let tap = self
                    .lane(lanes, trunk)
                    .position(&branch.after_block)
                    .ok_or(ChainError::Processing("branch tap block is stale"))?;
                let branch_side = trunk.other();
                let trunk_len = self.lane(lanes, trunk).blocks.len();
                let branch_len = self.lane(lanes, branch_side).blocks.len();
                if channels == 1 {
                    match trunk {
                        ChainSide::Left => lanes.left.process_range_with_pool(
                            buffer,
                            0,
                            tap + 1,
                            controls.block,
                            pool,
                        )?,
                        ChainSide::Right => lanes.right.process_range_with_pool(
                            buffer,
                            0,
                            tap + 1,
                            controls.block,
                            pool,
                        )?,
                    }
                    self.input_scratch[..frames].copy_from_slice(buffer.channel(0)?);
                    match trunk {
                        ChainSide::Left => lanes.left.process_range_with_pool(
                            buffer,
                            tap + 1,
                            trunk_len,
                            controls.block,
                            pool,
                        )?,
                        ChainSide::Right => lanes.right.process_range_with_pool(
                            buffer,
                            tap + 1,
                            trunk_len,
                            controls.block,
                            pool,
                        )?,
                    }
                    let mut branch =
                        PlanarBuffer::new(&mut self.input_scratch[..frames], 1, frames)?;
                    match branch_side {
                        ChainSide::Left => lanes.left.process_range_with_pool(
                            &mut branch,
                            0,
                            branch_len,
                            controls.block,
                            pool,
                        )?,
                        ChainSide::Right => lanes.right.process_range_with_pool(
                            &mut branch,
                            0,
                            branch_len,
                            controls.block,
                            pool,
                        )?,
                    }
                } else {
                    let (trunk_data, branch_data) = buffer.as_mut_slice().split_at_mut(frames);
                    let mut trunk_buffer = PlanarBuffer::new(trunk_data, 1, frames)?;
                    let mut branch_buffer = PlanarBuffer::new(branch_data, 1, frames)?;
                    self.lane_mut(lanes, trunk).process_range_with_pool(
                        &mut trunk_buffer,
                        0,
                        tap + 1,
                        controls.block,
                        pool,
                    )?;
                    branch_buffer
                        .channel_mut(0)?
                        .copy_from_slice(trunk_buffer.channel(0)?);
                    self.lane_mut(lanes, trunk).process_range_with_pool(
                        &mut trunk_buffer,
                        tap + 1,
                        trunk_len,
                        controls.block,
                        pool,
                    )?;
                    self.lane_mut(lanes, branch_side).process_range_with_pool(
                        &mut branch_buffer,
                        0,
                        branch_len,
                        controls.block,
                        pool,
                    )?;
                }
            }
        }

        if lanes.stereo_enabled && !controls.stereo_output {
            for frame in 0..frames {
                let left = buffer.channel(0)?[frame];
                let right = if channels > 1 {
                    buffer.channel(1)?[frame]
                } else {
                    self.input_scratch[frame]
                };
                let left = if controls.solo_right { 0.0 } else { left };
                let right = if controls.solo_left { 0.0 } else { right };
                let left = if controls.invert_left { -left } else { left };
                let right = if controls.invert_right { -right } else { right };
                let folded = 0.5 * (controls.balance_left * left + controls.balance_right * right);
                buffer.channel_mut(0)?[frame] = folded;
                if channels > 1 {
                    buffer.channel_mut(1)?[frame] = folded;
                }
            }
        }
        if self.edit_fade.is_smoothing() {
            for frame in 0..frames {
                let fade = self.edit_fade.next();
                for channel in 0..channels {
                    buffer.channel_mut(channel)?[frame] *= fade;
                }
            }
        }
        Ok(())
    }

    /// Starts a structural edit and records its previous snapshot.
    pub fn begin_edit(&mut self, lanes: &LaneSet) {
        self.history.push(lanes.snapshot());
        self.future.clear();
        self.edit_fade.set_current_and_target(0.0);
        self.edit_fade.set_target(1.0);
    }

    /// Sets or repoints a branch and returns whether it is valid.
    pub fn set_branch(
        &mut self,
        lanes: &mut LaneSet,
        side: ChainSide,
        after_block: BlockId,
    ) -> bool {
        if !lanes.stereo_enabled {
            return false;
        }
        let Some(index) = self.lane(lanes, side).position(&after_block) else {
            return false;
        };
        if self.lane(lanes, side).blocks[index].is_insert {
            return false;
        }
        if lanes
            .branch
            .as_ref()
            .is_some_and(|branch| branch.side == side && branch.after_block == after_block)
        {
            return true;
        }
        self.begin_edit(lanes);
        lanes.branch = Some(Branch { side, after_block });
        if matches!(lanes.input_mode, InputMode::Stereo | InputMode::DualMono) {
            lanes.input_mode = InputMode::Left;
        }
        lanes.align();
        true
    }

    /// Clears a branch and restores independent chains.
    pub fn clear_branch(&mut self, lanes: &mut LaneSet) -> bool {
        if lanes.branch.is_none() {
            return false;
        }
        self.begin_edit(lanes);
        lanes.branch = None;
        lanes.align();
        true
    }

    /// Changes stereo mode without discarding dormant branch settings.
    pub fn set_stereo_enabled(&mut self, lanes: &mut LaneSet, enabled: bool) {
        if lanes.stereo_enabled == enabled {
            return;
        }
        self.begin_edit(lanes);
        lanes.stereo_enabled = enabled;
        if enabled
            && lanes.branch.is_some()
            && matches!(lanes.input_mode, InputMode::Stereo | InputMode::DualMono)
        {
            lanes.input_mode = InputMode::Left;
        }
        lanes.align();
    }

    /// Duplicates a tone with independent model history.
    pub fn duplicate_block(
        &mut self,
        lanes: &mut LaneSet,
        source: &BlockId,
        side: ChainSide,
        index: usize,
    ) -> Option<BlockId> {
        if side == ChainSide::Right && !lanes.stereo_enabled {
            return None;
        }
        let (source_side, source_index) = self.find(lanes, source)?;
        let source_lane = self.lane(lanes, source_side);
        if source_lane.blocks[source_index].is_insert {
            return None;
        }
        let block = source_lane.blocks[source_index].clone();
        let model = source_lane.snapshot().models[source_index].clone()?;
        self.begin_edit(lanes);
        let id = unique_block_id();
        let mut clone = block;
        clone.id = id.clone();
        let processor = prepare_block(model, self.spec, clone.params.clone()).ok()?;
        self.lane_mut(lanes, side)
            .land_block(clone, processor, index);
        lanes.align();
        Some(id)
    }

    /// Copies a self-contained block snapshot for later paste.
    pub fn copy_block(&mut self, lanes: &LaneSet, source: &BlockId) -> bool {
        let Some((side, index)) = self.find(lanes, source) else {
            return false;
        };
        let lane = self.lane(lanes, side);
        if lane.blocks[index].is_insert {
            return false;
        }
        let Some(model) = lane.snapshot().models[index].clone() else {
            return false;
        };
        self.clipboard = Some((lane.blocks[index].clone(), model));
        true
    }

    /// Pastes the clipboard with fresh identity and independent history.
    pub fn paste_block(
        &mut self,
        lanes: &mut LaneSet,
        side: ChainSide,
        index: usize,
    ) -> Option<BlockId> {
        if side == ChainSide::Right && !lanes.stereo_enabled {
            return None;
        }
        let (block, model) = self.clipboard.clone()?;
        self.begin_edit(lanes);
        let id = unique_block_id();
        let mut block = block;
        block.id = id.clone();
        let processor = prepare_block(model, self.spec, block.params.clone()).ok()?;
        self.lane_mut(lanes, side)
            .land_block(block, processor, index);
        lanes.align();
        Some(id)
    }

    /// Removes a tone block and invalidates any branch that tapped it.
    pub fn remove_block(&mut self, lanes: &mut LaneSet, id: &BlockId) -> bool {
        let Some((side, _)) = self.find(lanes, id) else {
            return false;
        };
        self.begin_edit(lanes);
        let removed = self.lane_mut(lanes, side).remove(id).is_some();
        if removed {
            if lanes
                .branch
                .as_ref()
                .is_some_and(|branch| branch.after_block == *id)
            {
                lanes.branch = None;
            }
            lanes.align();
        }
        removed
    }

    /// Moves a tone to the other lane.
    pub fn move_block_to_chain(
        &mut self,
        lanes: &mut LaneSet,
        id: &BlockId,
        side: ChainSide,
        index: usize,
    ) -> bool {
        if !lanes.stereo_enabled {
            return false;
        }
        let source_side = side.other();
        let Some(source_index) = self.lane(lanes, source_side).position(id) else {
            return false;
        };
        if self.lane(lanes, source_side).blocks[source_index].is_insert {
            return false;
        }
        self.begin_edit(lanes);
        let Some((block, processor)) = self.lane_mut(lanes, source_side).remove(id) else {
            return false;
        };
        self.lane_mut(lanes, side)
            .land_block(block, processor, index);
        if lanes
            .branch
            .as_ref()
            .is_some_and(|branch| branch.after_block == *id)
        {
            lanes.branch = None;
        }
        lanes.align();
        true
    }

    /// Swaps both lanes and the trunk side.
    pub fn swap_chains(&mut self, lanes: &mut LaneSet) -> bool {
        if !lanes.stereo_enabled {
            return false;
        }
        self.begin_edit(lanes);
        std::mem::swap(&mut lanes.left, &mut lanes.right);
        if let Some(branch) = &mut lanes.branch {
            branch.side = branch.side.other();
        }
        lanes.align();
        true
    }

    /// Reorders one lane by a complete ID permutation.
    pub fn reorder(&mut self, lanes: &mut LaneSet, order: &[BlockId]) -> bool {
        let target = if self.same_ids(&lanes.left, order) {
            ChainSide::Left
        } else if self.same_ids(&lanes.right, order) {
            ChainSide::Right
        } else {
            return false;
        };
        if self
            .lane(lanes, target)
            .blocks
            .iter()
            .map(|block| &block.id)
            .eq(order)
        {
            return true;
        }
        self.begin_edit(lanes);
        let lane = self.lane_mut(lanes, target);
        let mut blocks = std::mem::take(&mut lane.blocks);
        lane.blocks = order
            .iter()
            .filter_map(|id| {
                let index = blocks.iter().position(|block| block.id == *id)?;
                Some(blocks.remove(index))
            })
            .collect();
        lanes.align();
        true
    }

    /// Undoes one structural step and rebuilds independent processors.
    pub fn undo(&mut self, lanes: &mut LaneSet) -> bool {
        let Some(snapshot) = self.history.pop() else {
            return false;
        };
        self.future.push(lanes.snapshot());
        if lanes.restore(snapshot, self.spec).is_err() {
            return false;
        }
        self.edit_fade.set_current_and_target(0.0);
        self.edit_fade.set_target(1.0);
        true
    }

    /// Redoes one structural step and rebuilds independent processors.
    pub fn redo(&mut self, lanes: &mut LaneSet) -> bool {
        let Some(snapshot) = self.future.pop() else {
            return false;
        };
        self.history.push(lanes.snapshot());
        if lanes.restore(snapshot, self.spec).is_err() {
            return false;
        }
        self.edit_fade.set_current_and_target(0.0);
        self.edit_fade.set_target(1.0);
        true
    }

    /// Whether a copied block is available for paste.
    #[must_use]
    pub fn can_paste_block(&self) -> bool {
        self.clipboard.is_some()
    }

    fn find(&self, lanes: &LaneSet, id: &BlockId) -> Option<(ChainSide, usize)> {
        for side in [ChainSide::Left, ChainSide::Right] {
            if let Some(index) = self.lane(lanes, side).position(id) {
                return Some((side, index));
            }
        }
        None
    }

    fn lane<'a>(&'a self, lanes: &'a LaneSet, side: ChainSide) -> &'a Lane {
        match side {
            ChainSide::Left => &lanes.left,
            ChainSide::Right => &lanes.right,
        }
    }

    fn lane_mut<'a>(&'a self, lanes: &'a mut LaneSet, side: ChainSide) -> &'a mut Lane {
        match side {
            ChainSide::Left => &mut lanes.left,
            ChainSide::Right => &mut lanes.right,
        }
    }

    fn same_ids(&self, lane: &Lane, order: &[BlockId]) -> bool {
        if lane.blocks.len() != order.len() {
            return false;
        }
        let mut current: Vec<_> = lane.blocks.iter().map(|block| &block.id).collect();
        let mut requested: Vec<_> = order.iter().collect();
        current.sort();
        requested.sort();
        current == requested
    }
}

trait OtherSide {
    fn other(self) -> Self;
}

impl OtherSide for ChainSide {
    fn other(self) -> Self {
        match self {
            ChainSide::Left => ChainSide::Right,
            ChainSide::Right => ChainSide::Left,
        }
    }
}

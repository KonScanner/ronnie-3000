//! Revision-safe pointer, keyboard, and touch drag state.

use ronie_core::{BlockId, ChainSide};

/// Result of one drag phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    /// No committed action.
    None,
    /// Six pixels engaged a sort.
    Engaged,
    /// Move accepted.
    Moved,
    /// Alt duplicate accepted.
    Duplicated,
    /// Missing block or stale revision; no mutation.
    Stale,
}

#[derive(Debug)]
struct DragState {
    id: BlockId,
    origin_side: ChainSide,
    origin_revision: u64,
    duplicate: bool,
    engaged: bool,
}

/// Pointer and keyboard drag state with revision and slop tracking.
#[derive(Debug, Default)]
pub struct DragController {
    state: Option<DragState>,
}

impl DragController {
    /// Begins a pointer drag without mutating the chain.
    pub fn begin(&mut self, id: BlockId, side: ChainSide, revision: u64, duplicate: bool) {
        self.state = Some(DragState {
            id,
            origin_side: side,
            origin_revision: revision,
            duplicate,
            engaged: false,
        });
    }

    /// Begins an already-engaged keyboard sort.
    pub fn begin_keyboard(&mut self, id: BlockId, side: ChainSide, revision: u64) {
        self.begin(id, side, revision, false);
        if let Some(state) = &mut self.state {
            state.engaged = true;
        }
    }

    /// Engages at six pixels, never before.
    pub fn drag_by(&mut self, travel: f32) -> DragOutcome {
        let Some(state) = &mut self.state else {
            return DragOutcome::Stale;
        };
        if !state.engaged && travel.abs() < 6.0 {
            return DragOutcome::None;
        }
        state.engaged = true;
        DragOutcome::Engaged
    }

    /// Changes Alt-duplicate mode without resetting the active drag.
    pub fn set_duplicate(&mut self, duplicate: bool) {
        if let Some(state) = &mut self.state {
            state.duplicate = duplicate;
        }
    }

    /// Current duplicate mode while a drag is active.
    #[must_use]
    pub fn duplicate(&self) -> Option<bool> {
        self.state.as_ref().map(|state| state.duplicate)
    }

    /// Validates the active ID and revision, returning its move parameters.
    #[must_use]
    pub fn commit(&self, current_revision: u64) -> Option<(BlockId, ChainSide, bool)> {
        let state = self.state.as_ref()?;
        if !state.engaged || state.origin_revision != current_revision {
            return None;
        }
        Some((state.id.clone(), state.origin_side, state.duplicate))
    }

    /// Clears state without dispatching a mutation.
    pub fn cancel(&mut self) {
        self.state = None;
    }

    /// Quick sideways touch flicks belong to the parent scroller.
    #[must_use]
    pub fn touch_flick(travel_x: f32, travel_y: f32, elapsed_ms: u64) -> bool {
        elapsed_ms < 150 && travel_x.abs() > travel_y.abs() && travel_x.abs() >= 6.0
    }

    /// A stationary 500 ms touch opens the context menu instead of sorting.
    #[must_use]
    pub fn touch_hold(travel_x: f32, travel_y: f32, elapsed_ms: u64) -> bool {
        elapsed_ms >= 500 && travel_x.abs() <= 5.0 && travel_y.abs() <= 5.0
    }
}

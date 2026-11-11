use crate::block::BlockControls;
use ronie_core::{BlockId, ChainSide, InputMode};

/// Lane topology after branch resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Topology {
    /// One chain processes every input channel.
    Mono,
    /// Two independent mono chains.
    Stereo,
    /// One trunk feeds the other lane after a tone block.
    Branched {
        /// Lane containing the tap block.
        trunk: ChainSide,
        /// Tap block identity.
        after_block: BlockId,
    },
}

/// Immutable routing controls for one callback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutingControls {
    /// Session input mode.
    pub input_mode: InputMode,
    /// Whether two input channels are available.
    pub stereo_input: bool,
    /// Whether two output channels are audible.
    pub stereo_output: bool,
    /// Left chain linear balance gain.
    pub balance_left: f32,
    /// Right chain linear balance gain.
    pub balance_right: f32,
    /// Solo the left chain.
    pub solo_left: bool,
    /// Solo the right chain.
    pub solo_right: bool,
    /// Invert the left chain.
    pub invert_left: bool,
    /// Invert the right chain.
    pub invert_right: bool,
    /// Per-block processing controls.
    pub block: BlockControls,
}

impl Default for RoutingControls {
    fn default() -> Self {
        Self {
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
            balance_left: 1.0,
            balance_right: 1.0,
            solo_left: false,
            solo_right: false,
            invert_left: false,
            invert_right: false,
            block: BlockControls::default(),
        }
    }
}

/// Effective input-routing state for one callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingState {
    /// Whether independent left and right voices actually run.
    pub dual_mono_active: bool,
}

impl RoutingState {
    /// Resolves dual mono from the complete rig and current chain topology.
    #[must_use]
    pub fn resolve(
        buffer_channels: usize,
        controls: &RoutingControls,
        stereo_chains: bool,
    ) -> Self {
        let dual_mono_active = buffer_channels > 1
            && controls.stereo_input
            && controls.stereo_output
            && !stereo_chains
            && controls.input_mode == InputMode::DualMono;
        Self { dual_mono_active }
    }
}

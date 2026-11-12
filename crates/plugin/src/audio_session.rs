//! Audio-facing view of the control session: what a running stream must reproduce.

use std::collections::BTreeMap;

use ronie_core::{
    BlockId, BlockParams, ChainBlock, ChainBranch, ChainSide, InputMode, SessionSnapshot,
    params::{ParameterCatalog, ParameterId, ParameterValue},
};
use ronie_dsp::{
    block::BlockControls,
    image::{AlignParams, ImageControls, SpreadParams, chain_balance_gains},
    input::{GateParams, InputControls, PitchParams, PitchWindow},
    output::OutputControls,
    prepare::PreparedChainChange,
    routing::RoutingControls,
    rt_chain::ChainControls,
    tone_stack::ToneControls,
};
use ronie_ui::backend::ChainFault;

/// Decibel span of a normalized level control, centered on unity at 0.5.
const LEVEL_SPAN_DB: f32 = 48.0;
/// The top of the tonality control leaves the whole signal shifted.
const TONALITY_OFF_HZ: f32 = 20_000.0;

/// Final stream layout that the audio chain must be prepared for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamFormat {
    /// Actual device sample rate in hertz.
    pub sample_rate: f64,
    /// Largest callback that the audio chain must accept.
    pub max_frames: usize,
    /// Whether the stream supplies two input channels.
    pub stereo_input: bool,
    /// Whether the stream can reproduce two output channels.
    pub stereo_output: bool,
    /// The oversampling of the chain: 1, 2, 4, or 8. The engine sets 1 while OPTIMIZE is on.
    pub oversampling: u8,
}

/// Everything about a session that only a new prepared chain can reproduce.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainFingerprint {
    left: Vec<ChainBlock>,
    right: Vec<ChainBlock>,
    stereo_enabled: bool,
    branch: Option<ChainBranch>,
    input_mode: InputMode,
}

impl ChainFingerprint {
    /// The block at one position of one lane.
    #[must_use]
    pub fn block(&self, side: ChainSide, position: usize) -> Option<&ChainBlock> {
        match side {
            ChainSide::Left => self.left.get(position),
            ChainSide::Right => self.right.get(position),
        }
    }
}

/// One comparable snapshot of the session state that reaches audio.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSessionState {
    /// Callback controls that the current parameters select.
    pub controls: ChainControls,
    /// Structure that needs a new chain when it changes.
    pub fingerprint: ChainFingerprint,
    /// Per-block settings that a running chain can take without losing engine state.
    pub block_params: Vec<(BlockId, BlockParams)>,
}

impl AudioSessionState {
    pub(crate) fn capture(
        session: &SessionSnapshot,
        parameters: &BTreeMap<ParameterId, ParameterValue>,
        stereo_output: bool,
    ) -> Self {
        let chain = &session.chain;
        let structure = |blocks: &[ChainBlock]| {
            blocks
                .iter()
                .map(|block| ChainBlock {
                    params: BlockParams::default(),
                    ..block.clone()
                })
                .collect()
        };
        Self {
            controls: chain_controls(session, parameters, stereo_output),
            fingerprint: ChainFingerprint {
                left: structure(&chain.left),
                right: structure(&chain.right),
                stereo_enabled: chain.stereo_enabled,
                branch: chain.branch.clone(),
                input_mode: session.input_mode,
            },
            block_params: chain
                .left
                .iter()
                .chain(&chain.right)
                .filter(|block| !block.is_insert)
                .map(|block| (block.id.clone(), block.params.clone()))
                .collect(),
        }
    }
}

/// A chain prepared from the current session.
pub struct SessionChain {
    /// Complete chain ready for callback publication.
    pub change: PreparedChainChange,
    /// Blocks that run as bypass because their model has no usable bytes, with the fault.
    pub unavailable: Vec<(BlockId, ChainFault)>,
}

/// What a restore of a saved session or of a preset leaves for the audio side.
pub enum SessionRestore {
    /// A chain that the restore prepared, for the stream layout of its time.
    Prepared(Box<PreparedChainChange>),
    /// The restore prepared no chain, because a compile is too slow for the thread of the
    /// restore. The audio side must prepare a new chain of the session, also for the same
    /// structure, because the restore can bring other model bytes.
    Compile,
}

impl std::fmt::Debug for SessionRestore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Prepared(change) => formatter
                .debug_tuple("Prepared")
                .field(&change.generation)
                .finish(),
            Self::Compile => formatter.write_str("Compile"),
        }
    }
}

/// Maps the parameter catalog and session routing to callback controls.
#[must_use]
pub fn chain_controls(
    session: &SessionSnapshot,
    parameters: &BTreeMap<ParameterId, ParameterValue>,
    stereo_output: bool,
) -> ChainControls {
    let value = |id: &'static str| {
        parameters
            .get(&ParameterId(id))
            .map_or_else(|| catalog_default(id), |value| value.0)
    };
    chain_controls_from(session.input_mode, value, stereo_output)
}

/// The default of a catalog parameter, or 0 for an unknown ID.
#[must_use]
pub fn catalog_default(id: &str) -> f32 {
    ParameterCatalog::v1()
        .iter()
        .find(|spec| spec.id.0 == id)
        .map_or(0.0, |spec| spec.default)
}

/// Maps catalog values and an input mode to callback controls.
///
/// `value` gives the value of each catalog ID. This does not allocate, so an audio callback can
/// call it, for example to follow host automation.
#[must_use]
pub fn chain_controls_from(
    input_mode: InputMode,
    value: impl Fn(&'static str) -> f32,
    stereo_output: bool,
) -> ChainControls {
    let on = |id: &'static str| value(id) >= 0.5;
    let tonality = value("pitchTonality");
    let input = InputControls {
        input_gain: normalized_level(value("inputLevel")),
        gate_enabled: on("gateEnabled"),
        gate: GateParams {
            threshold_db: value("gateThreshold"),
            release_ms: value("gateRelease"),
            hold_ms: value("gateHold"),
            range_db: value("gateRange"),
        },
        pitch_enabled: on("pitchEnabled"),
        pitch_step: on("pitchStep"),
        pitch: PitchParams {
            semitones: value("pitchSemitones"),
            tonality_hz: if tonality >= TONALITY_OFF_HZ {
                0.0
            } else {
                tonality
            },
            window: PitchWindow::from_index(value("pitchWindow").round() as i32),
        },
    };
    let solo_left = on("chainSoloLeft");
    let solo_right = on("chainSoloRight");
    let invert_left = on("chainInvertLeft");
    let invert_right = on("chainInvertRight");
    let balance = value("outputBalance");
    let mut routing = RoutingControls {
        input_mode,
        block: BlockControls {
            calibrate_input: on("calibrateInput"),
            input_calibration_dbu: value("inputCalibrationLevel"),
            target_loudness_db: value("targetLoudness"),
            ..BlockControls::default()
        },
        ..RoutingControls::default()
    };
    let mut image = ImageControls {
        dual_mono: input_mode == InputMode::DualMono,
        spread_enabled: on("spreadEnabled"),
        spread: SpreadParams::from_normalized(
            value("spreadOffset"),
            value("spreadWobble"),
            value("spreadCrossover"),
            on("spreadWobbleEnabled"),
            on("spreadCrossoverEnabled"),
            on("spreadDiffuseEnabled"),
        ),
        align_enabled: on("alignEnabled"),
        align: AlignParams::from_normalized(
            value("alignOffset"),
            value("alignWobble"),
            value("alignCrossover"),
            on("alignWobbleEnabled"),
            on("alignCrossoverEnabled"),
            on("alignDiffuseEnabled"),
        ),
        pan_left: value("chainPanLeft"),
        pan_right: value("chainPanRight"),
        ..ImageControls::default()
    };
    // A mono rig folds both chains in the topology, so that the image stage sees no chain
    // difference. Each of balance, solo, and invert must apply at one of the two stages only.
    if stereo_output {
        image.balance = balance;
        image.solo_left = solo_left;
        image.solo_right = solo_right;
        image.invert_left = invert_left;
        image.invert_right = invert_right;
    } else {
        (routing.balance_left, routing.balance_right) = chain_balance_gains(balance);
        routing.solo_left = solo_left;
        routing.solo_right = solo_right;
        routing.invert_left = invert_left;
        routing.invert_right = invert_right;
    }
    ChainControls {
        input,
        routing,
        output: OutputControls {
            image,
            tone: ToneControls {
                enabled: on("toneEqEnabled"),
                bass: value("toneBass"),
                mid: value("toneMid"),
                treble: value("toneTreble"),
            },
            output_level: value("outputLevel"),
        },
    }
}

fn normalized_level(value: f32) -> f32 {
    10.0_f32.powf((value - 0.5) * LEVEL_SPAN_DB / 20.0)
}

use std::{collections::BTreeSet, fmt, time::Duration};

use serde_json::Value;

use crate::{ChainError, JuceTree, JuceValue, ModelId, Tone};

const MIN_MONO_LANE_SLOTS: usize = 5;
const MIN_STEREO_LANE_SLOTS: usize = 3;
const MAX_LANE_BLOCKS: usize = 64;
const MAX_EQ_BANDS: usize = 6;
const MAX_MIDI_MAPPINGS: usize = 128;

/// Stable identity of one block across session restoration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub String);

impl fmt::Display for BlockId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// One side of a stereo chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChainSide {
    /// Left or first lane.
    Left,
    /// Right or second lane.
    Right,
}

impl ChainSide {
    /// Parses the persisted side, defaulting to left like the Reference UI.
    #[must_use]
    pub fn from_persisted(value: &str) -> Self {
        if value == "right" {
            Self::Right
        } else {
            Self::Left
        }
    }

    /// Persisted side spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// Session-only input routing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputMode {
    /// Natural stereo or mono routing.
    Stereo,
    /// Fold the left input onto both channels.
    Left,
    /// Fold the right input onto both channels.
    Right,
    /// Run independent mono voices for left and right inputs.
    DualMono,
}

impl InputMode {
    /// Parses a persisted mode, with unknown values restored as stereo.
    #[must_use]
    pub fn from_persisted(value: &str) -> Self {
        match value {
            "left" => Self::Left,
            "right" => Self::Right,
            "dual" => Self::DualMono,
            _ => Self::Stereo,
        }
    }

    /// Persisted mode spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stereo => "stereo",
            Self::Left => "left",
            Self::Right => "right",
            Self::DualMono => "dual",
        }
    }
}

/// EQ filter shape for one band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EqBandType {
    /// Low cut.
    Lowcut,
    /// Low shelf.
    Lowshelf,
    /// Bell.
    Bell,
    /// High shelf.
    Highshelf,
    /// High cut.
    Highcut,
}

impl EqBandType {
    /// Parses a band type, defaulting to bell.
    #[must_use]
    pub fn from_persisted(value: &str) -> Self {
        match value {
            "lowcut" => Self::Lowcut,
            "lowshelf" => Self::Lowshelf,
            "highshelf" => Self::Highshelf,
            "highcut" => Self::Highcut,
            _ => Self::Bell,
        }
    }
}

/// One six-band EQ entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqBand {
    /// Filter shape.
    pub kind: EqBandType,
    /// Center or corner frequency in hertz.
    pub freq_hz: f64,
    /// Gain in decibels.
    pub gain_db: f64,
    /// Quality factor.
    pub q: f64,
}

impl Default for EqBand {
    fn default() -> Self {
        Self {
            kind: EqBandType::Bell,
            freq_hz: 1000.0,
            gain_db: 0.0,
            q: 1.0,
        }
    }
}

impl EqBand {
    fn from_value(value: &Value) -> Self {
        Self {
            kind: EqBandType::from_persisted(string_field(value.get("type"), "bell")),
            freq_hz: number_field(value.get("freqHz"), 1000.0),
            gain_db: number_field(value.get("gainDb"), 0.0),
            q: number_field(value.get("q"), 1.0),
        }
    }
}

/// Per-block pre/post six-band EQ settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EqParams {
    /// Whether EQ processing is enabled.
    pub enabled: bool,
    /// Whether EQ runs before the model rather than after it.
    pub pre: bool,
    /// Ordered EQ bands.
    pub bands: Vec<EqBand>,
}

impl EqParams {
    fn from_value(value: &Value) -> Result<Self, ChainError> {
        let enabled = bool_field(value.get("enabled"), false);
        let pre = bool_field(value.get("pre"), false);
        let band_values = match value.get("bands") {
            Some(Value::Array(bands)) => bands,
            Some(_) => return Err(ChainError::InvalidDocument("EQ bands must be an array")),
            None => &Vec::new(),
        };
        if band_values.len() > MAX_EQ_BANDS {
            return Err(ChainError::TooManyEqBands {
                actual: band_values.len(),
                max: MAX_EQ_BANDS,
            });
        }
        let mut bands = Vec::with_capacity(band_values.len());
        for band in band_values {
            bands.push(EqBand::from_value(band));
        }
        Ok(Self {
            enabled,
            pre,
            bands,
        })
    }
}

/// Serializable per-block processing parameters, without engine state.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockParams {
    /// Block enable state.
    pub enabled: bool,
    /// Loudness normalization enable state.
    pub normalize: bool,
    /// Slimmable model size request: zero is lite and one is full.
    pub slim_size: f64,
    /// Normalized input gain.
    pub input_gain: f64,
    /// Normalized output gain.
    pub output_gain: f64,
    /// Normalized dry/wet mix.
    pub mix: f64,
    /// Block EQ settings.
    pub eq: EqParams,
    /// Normalized crossfade from IR A (0) to IR B (1). Unused while a block has no IR B.
    pub ir_blend: f64,
    /// Normalized level of IR A, where 0.5 is unity like `input_gain`.
    pub ir_a_level: f64,
    /// Normalized level of IR B, where 0.5 is unity like `input_gain`.
    pub ir_b_level: f64,
    /// Whether IR B is polarity-inverted.
    pub ir_b_invert: bool,
}

const DEFAULT_IR_BLEND: f64 = 0.5;
const DEFAULT_IR_LEVEL: f64 = 0.5;

impl Default for BlockParams {
    fn default() -> Self {
        Self {
            enabled: true,
            normalize: true,
            slim_size: 0.0,
            input_gain: 0.5,
            output_gain: 0.5,
            mix: 1.0,
            eq: EqParams::default(),
            ir_blend: DEFAULT_IR_BLEND,
            ir_a_level: DEFAULT_IR_LEVEL,
            ir_b_level: DEFAULT_IR_LEVEL,
            ir_b_invert: false,
        }
    }
}

impl BlockParams {
    fn from_value(value: &Value) -> Result<Self, ChainError> {
        Ok(Self {
            enabled: bool_field(value.get("enabled"), true),
            normalize: bool_field(value.get("normalize"), true),
            slim_size: number_field(value.get("slimSize"), 0.0),
            input_gain: number_field(value.get("inputGain"), 0.5),
            output_gain: number_field(value.get("outputGain"), 0.5),
            mix: number_field(value.get("mix"), 1.0),
            eq: EqParams::from_value(value.get("eq").unwrap_or(&Value::Null))?,
            ir_blend: unit_or(
                value.get("irBlend").and_then(Value::as_f64),
                DEFAULT_IR_BLEND,
            ),
            ir_a_level: unit_or(
                value.get("irALevel").and_then(Value::as_f64),
                DEFAULT_IR_LEVEL,
            ),
            ir_b_level: unit_or(
                value.get("irBLevel").and_then(Value::as_f64),
                DEFAULT_IR_LEVEL,
            ),
            ir_b_invert: bool_field(value.get("irBInvert"), false),
        })
    }
}

/// The second impulse response of a cab block, blended with the active model (IR A).
///
/// In v1 it is another model of the block's own tone; a later version may add a `tone` field
/// for an IR from another tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IrB {
    /// Model of the block's tone that plays as IR B.
    pub model_id: ModelId,
}

/// Plain-data chain block. Mutable audio engines and histories never appear here.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainBlock {
    /// Stable block identity.
    pub id: BlockId,
    /// Whether this is an open insert slot rather than a tone block.
    pub is_insert: bool,
    /// Tone metadata for a non-insert block.
    pub tone: Option<Tone>,
    /// Selected model for this block.
    pub active_model_id: Option<ModelId>,
    /// Whether a playable model is available.
    pub loaded: bool,
    /// Whether a model load or preparation is in flight.
    pub model_loading: bool,
    /// Whether the last load failed and offers retry.
    pub load_failed: bool,
    /// Second IR of a cab block, blended with the active model. `None` plays the active model
    /// alone, exactly as a block without dual-IR support does.
    pub ir_b: Option<IrB>,
    /// Whether IR B has no usable bytes, so the block plays the active model alone until they
    /// arrive. A restore sets it; it is never stored.
    pub ir_b_missing: bool,
    /// The skin the user chose for the block's render, as a skin identity such as `"british"`.
    /// `None` is Auto: the skin of the gear family the tone identifies. It changes only the
    /// drawing, never the audio.
    pub skin: Option<String>,
    /// Persisted block parameters.
    pub params: BlockParams,
}

impl ChainBlock {
    /// Creates an open insert slot.
    #[must_use]
    pub fn insert(id: impl Into<String>) -> Self {
        Self {
            id: BlockId(id.into()),
            is_insert: true,
            tone: None,
            active_model_id: None,
            loaded: true,
            model_loading: false,
            load_failed: false,
            ir_b: None,
            ir_b_missing: false,
            skin: None,
            params: BlockParams::default(),
        }
    }

    /// Creates a tone block with Reference-default parameters.
    #[must_use]
    pub fn tone(id: impl Into<String>, tone: Tone, active_model_id: ModelId) -> Self {
        Self {
            id: BlockId(id.into()),
            is_insert: false,
            tone: Some(tone),
            active_model_id: Some(active_model_id),
            loaded: true,
            model_loading: false,
            load_failed: false,
            ir_b: None,
            ir_b_missing: false,
            skin: None,
            params: BlockParams::default(),
        }
    }

    fn from_value(value: &Value) -> Result<Self, ChainError> {
        if !value.is_object() {
            return Err(ChainError::InvalidDocument("chain block must be an object"));
        }
        let id = BlockId(string_field(value.get("blockId"), "").to_owned());
        let is_insert = string_field(value.get("kind"), "insert") != "tone";
        if is_insert {
            return Ok(Self {
                id,
                is_insert,
                tone: None,
                active_model_id: None,
                loaded: true,
                model_loading: false,
                load_failed: false,
                ir_b: None,
                ir_b_missing: false,
                skin: None,
                params: BlockParams::default(),
            });
        }
        let tone = Tone::from_value(value.get("tone").cloned().unwrap_or(Value::Null))?;
        let active_model_id = value
            .get("activeModelId")
            .and_then(Value::as_i64)
            .map(ModelId);
        let params = BlockParams::from_value(value.get("params").unwrap_or(&Value::Null))?;
        let ir_b = listed_ir_b(&tone, value.get(IR_B_MODEL_ID).and_then(Value::as_i64));
        Ok(Self {
            id,
            is_insert: false,
            tone: Some(tone),
            active_model_id,
            loaded: bool_field(value.get("loaded"), true),
            model_loading: bool_field(value.get("modelLoading"), false),
            load_failed: bool_field(value.get("loadFailed"), false),
            ir_b,
            ir_b_missing: false,
            skin: skin_override(value.get(SKIN_OVERRIDE).and_then(Value::as_str)),
            params,
        })
    }
}

/// Returns whether a model remains reachable from a block's persisted tone.
/// Local tones retain every listed model; catalog tones retain only the active model and IR B.
#[must_use]
pub fn references_model(block: &ChainBlock, id: ModelId) -> bool {
    if block.active_model_id == Some(id) || block.ir_b.is_some_and(|ir_b| ir_b.model_id == id) {
        return true;
    }
    block
        .tone
        .as_ref()
        .is_some_and(|tone| tone.local && tone.models.iter().any(|model| model.id == id))
}

/// Resolves the source namespace for one model, including models without a retry URL.
#[must_use]
pub fn model_namespace(block: &ChainBlock, id: ModelId) -> crate::ModelNamespace {
    block
        .tone
        .as_ref()
        .and_then(|tone| tone.models.iter().find(|model| model.id == id))
        .and_then(|model| model.source.as_ref())
        .map_or_else(
            || {
                if block.tone.as_ref().is_some_and(|tone| tone.local) {
                    crate::ModelNamespace::Local
                } else {
                    crate::ModelNamespace::Catalog
                }
            },
            crate::ModelNamespace::from_source,
        )
}

/// Dormant or active branch location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainBranch {
    /// Trunk side.
    pub side: ChainSide,
    /// Tone after which the branch starts.
    pub after_block_id: BlockId,
}

/// Immutable two-lane chain data and the 400 ms gesture-settle policy.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainSnapshot {
    /// UI revision counter.
    pub revision: u64,
    /// Left lane: tones first, then empty slots. One empty slot follows the last tone, or more
    /// empty slots fill the lane to the minimum of five slots in mono or three in stereo.
    pub left: Vec<ChainBlock>,
    /// Right lane: tones first, then empty slots. One empty slot follows the last tone, or more
    /// empty slots fill the lane to the minimum of five slots in mono or three in stereo.
    pub right: Vec<ChainBlock>,
    /// Stereo chain enable state.
    pub stereo_enabled: bool,
    /// Currently edited side.
    pub active_side: ChainSide,
    /// Stereo input enable state.
    pub stereo_input: bool,
    /// Stereo output enable state.
    pub stereo_output: bool,
    /// Whether dual-mono processing is active.
    pub dual_mono_active: bool,
    /// Optional branch location.
    pub branch: Option<ChainBranch>,
    /// Quiet period before a continuous gesture becomes one revision.
    pub gesture_settle: Duration,
}

impl ChainSnapshot {
    /// Creates two empty Reference-shaped lanes.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            revision: 0,
            left: normalized_lane(Vec::new(), ChainSide::Left),
            right: normalized_lane(Vec::new(), ChainSide::Right),
            stereo_enabled: false,
            active_side: ChainSide::Left,
            stereo_input: false,
            stereo_output: true,
            dual_mono_active: false,
            branch: None,
            gesture_settle: Duration::from_millis(400),
        }
    }

    /// Closes the gaps between tones, keeps one empty slot after the last tone and the minimum
    /// slot count, and removes an invalid branch tap after an edit.
    pub fn normalize(&mut self) {
        let min_slots = if self.stereo_enabled {
            MIN_STEREO_LANE_SLOTS
        } else {
            MIN_MONO_LANE_SLOTS
        };
        self.left =
            normalized_lane_with_min(std::mem::take(&mut self.left), ChainSide::Left, min_slots);
        self.right =
            normalized_lane_with_min(std::mem::take(&mut self.right), ChainSide::Right, min_slots);
        unique_insert_ids(&mut self.left, &mut self.right);
        let valid_branch = self
            .branch
            .as_ref()
            .is_some_and(|branch| self.stereo_enabled && self.has_tap(branch));
        if !valid_branch {
            self.branch = None;
        }
    }

    /// Whether `branch` splits after a tone of its lane. An open slot or an unknown block has no
    /// tap. This does not look at the stereo mode.
    #[must_use]
    pub fn has_tap(&self, branch: &ChainBranch) -> bool {
        match branch.side {
            ChainSide::Left => &self.left,
            ChainSide::Right => &self.right,
        }
        .iter()
        .any(|block| block.id == branch.after_block_id && !block.is_insert)
    }

    /// Removes one tone and its slot. The tones after it move one slot to the left.
    ///
    /// A branch after the removed tone moves to the tone before it, because the signal then
    /// splits at the same point. A branch with no tone before it is removed. Returns false, and
    /// changes nothing, when `id` names no tone.
    #[must_use]
    pub fn remove_tone(&mut self, id: &BlockId) -> bool {
        let Some((side, index)) = [
            (ChainSide::Left, &self.left),
            (ChainSide::Right, &self.right),
        ]
        .into_iter()
        .find_map(|(side, lane)| {
            lane.iter()
                .position(|block| &block.id == id && !block.is_insert)
                .map(|index| (side, index))
        }) else {
            return false;
        };
        let lane = match side {
            ChainSide::Left => &mut self.left,
            ChainSide::Right => &mut self.right,
        };
        lane.remove(index);
        let previous = lane[..index]
            .iter()
            .rev()
            .find(|block| !block.is_insert)
            .map(|block| block.id.clone());
        if let Some(branch) = &mut self.branch
            && branch.side == side
            && &branch.after_block_id == id
        {
            match previous {
                Some(previous) => branch.after_block_id = previous,
                None => self.branch = None,
            }
        }
        self.normalize();
        true
    }

    /// Finds one block in either lane.
    #[must_use]
    pub fn block(&self, id: &BlockId) -> Option<&ChainBlock> {
        self.left
            .iter()
            .chain(&self.right)
            .find(|block| &block.id == id)
    }

    /// Finds one mutable block in either lane.
    #[must_use]
    pub fn block_mut(&mut self, id: &BlockId) -> Option<&mut ChainBlock> {
        self.left
            .iter_mut()
            .chain(&mut self.right)
            .find(|block| &block.id == id)
    }

    /// Parses the official UI chain shape and normalizes both lanes.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed blocks, more than 64 blocks per lane,
    /// invalid EQ data, or invalid tone data.
    pub fn from_ui_value(value: Value) -> Result<Self, ChainError> {
        if !value.is_object() {
            return Err(ChainError::InvalidDocument("chain must be an object"));
        }
        let stereo_enabled = bool_field(value.get("stereoEnabled"), false);
        let min_slots = if stereo_enabled {
            MIN_STEREO_LANE_SLOTS
        } else {
            MIN_MONO_LANE_SLOTS
        };
        let mut left = parse_lane(value.get("chain"), ChainSide::Left, min_slots)?;
        let mut right = parse_lane(value.get("chainRight"), ChainSide::Right, min_slots)?;
        unique_insert_ids(&mut left, &mut right);
        let branch = value
            .get("branch")
            .filter(|branch| branch.is_object())
            .map(|branch| ChainBranch {
                side: ChainSide::from_persisted(string_field(branch.get("side"), "left")),
                after_block_id: BlockId(string_field(branch.get("afterBlockId"), "").to_owned()),
            });
        Ok(Self {
            revision: value.get("revision").and_then(Value::as_u64).unwrap_or(0),
            left,
            right,
            stereo_enabled: bool_field(value.get("stereoEnabled"), false),
            active_side: ChainSide::from_persisted(string_field(value.get("activeSide"), "left")),
            stereo_input: bool_field(value.get("stereoInput"), false),
            stereo_output: bool_field(value.get("stereoOutput"), true),
            dual_mono_active: bool_field(value.get("dualMonoActive"), false),
            branch,
            gesture_settle: Duration::from_millis(400),
        })
    }
}

/// MIDI source kind persisted in a session mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MidiSource {
    /// Control change.
    Cc,
    /// Note on.
    Note,
}

impl MidiSource {
    /// Parses a source, defaulting to CC.
    #[must_use]
    pub fn from_persisted(value: &str) -> Self {
        if value == "note" {
            Self::Note
        } else {
            Self::Cc
        }
    }
}

/// One session-only MIDI mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiMapping {
    /// Parameter or action identity.
    pub target_id: String,
    /// MIDI event family.
    pub source: MidiSource,
    /// Controller or note number.
    pub number: u8,
}

/// Active preset identity stored in a session, not in a preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePreset {
    /// Namespaced preset identity.
    pub id: String,
    /// Display name.
    pub name: String,
}

/// Complete plain-data session snapshot with session-only fields.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    /// Chain and block data.
    pub chain: ChainSnapshot,
    /// Input routing mode.
    pub input_mode: InputMode,
    /// Editor scale, defaulting to 1.0 in legacy sessions.
    pub editor_scale: f64,
    /// Additional editor height, defaulting to 36 pixels.
    pub editor_extra_height: i32,
    /// Session-only MIDI mappings.
    pub midi_mappings: Vec<MidiMapping>,
    /// Active preset identity, when one is selected.
    pub active_preset: Option<ActivePreset>,
}

impl SessionSnapshot {
    /// Parses session fields and the official UI chain shape.
    ///
    /// # Errors
    ///
    /// Returns chain, tone, EQ, or MIDI validation errors.
    pub fn from_ui_value(value: Value) -> Result<Self, ChainError> {
        let chain = ChainSnapshot::from_ui_value(value.clone())?;
        let input_mode = InputMode::from_persisted(string_field(value.get("inputMode"), "stereo"));
        let editor_scale = number_field(value.get("editorScale"), 1.0);
        let editor_extra_height = value
            .get("editorExtraHeight")
            .and_then(Value::as_i64)
            .unwrap_or(36)
            .try_into()
            .map_err(|_| ChainError::InvalidDocument("editor height is out of range"))?;
        let mapping_values = match value.get("midiMappings") {
            Some(Value::Array(mappings)) => mappings,
            Some(_) => {
                return Err(ChainError::InvalidDocument(
                    "MIDI mappings must be an array",
                ));
            }
            None => &Vec::new(),
        };
        if mapping_values.len() > MAX_MIDI_MAPPINGS {
            return Err(ChainError::TooManyMidiMappings {
                actual: mapping_values.len(),
                max: MAX_MIDI_MAPPINGS,
            });
        }
        let mut midi_mappings = Vec::with_capacity(mapping_values.len());
        for mapping in mapping_values {
            let number = mapping.get("number").and_then(Value::as_i64).unwrap_or(-1);
            let number = u8::try_from(number)
                .ok()
                .filter(|number| *number <= 127)
                .ok_or(ChainError::InvalidDocument(
                    "MIDI number must be between 0 and 127",
                ))?;
            midi_mappings.push(MidiMapping {
                target_id: string_field(mapping.get("targetId"), "").to_owned(),
                source: MidiSource::from_persisted(string_field(mapping.get("source"), "cc")),
                number,
            });
        }
        let preset = value.get("preset");
        let preset_id = string_field(
            value
                .get("activePresetId")
                .or_else(|| preset.and_then(|preset| preset.get("id"))),
            "",
        );
        let preset_name = string_field(
            value
                .get("activePresetName")
                .or_else(|| preset.and_then(|preset| preset.get("name"))),
            "",
        );
        let active_preset = if preset_id.is_empty() && preset_name.is_empty() {
            None
        } else {
            Some(ActivePreset {
                id: preset_id.to_owned(),
                name: preset_name.to_owned(),
            })
        };
        Ok(Self {
            chain,
            input_mode,
            editor_scale,
            editor_extra_height,
            midi_mappings,
            active_preset,
        })
    }
}

impl ChainSnapshot {
    /// Converts the binary `ChainSnapshot` ValueTree into plain chain data.
    ///
    /// # Errors
    ///
    /// Returns tone, block, EQ, or lane validation errors.
    pub fn from_juce_tree(tree: &JuceTree) -> Result<Self, ChainError> {
        if tree.identifier != "ChainSnapshot" {
            return Err(ChainError::InvalidDocument("expected ChainSnapshot"));
        }
        let stereo_enabled = juce_bool_prop(tree, "stereoEnabled").unwrap_or(false);
        let min_slots = if stereo_enabled {
            MIN_STEREO_LANE_SLOTS
        } else {
            MIN_MONO_LANE_SLOTS
        };
        let mut left =
            parse_juce_lane(child_tree(tree, "ChainBlocks"), ChainSide::Left, min_slots)?;
        let mut right = parse_juce_lane(
            child_tree(tree, "RightChainBlocks"),
            ChainSide::Right,
            min_slots,
        )?;
        unique_insert_ids(&mut left, &mut right);
        let after = juce_string_prop(tree, "branchAfterBlockId").unwrap_or_default();
        let branch = (!after.is_empty()).then(|| ChainBranch {
            side: ChainSide::from_persisted(juce_string_prop(tree, "branchSide").unwrap_or("left")),
            after_block_id: BlockId(after.to_owned()),
        });
        Ok(Self {
            revision: 0,
            left,
            right,
            stereo_enabled: juce_bool_prop(tree, "stereoEnabled").unwrap_or(false),
            active_side: ChainSide::from_persisted(
                juce_string_prop(tree, "branchSide").unwrap_or("left"),
            ),
            stereo_input: false,
            stereo_output: true,
            dual_mono_active: false,
            branch,
            gesture_settle: Duration::from_millis(400),
        })
    }
}

impl SessionSnapshot {
    /// Converts the session root ValueTree into session-only typed data.
    ///
    /// # Errors
    ///
    /// Returns chain, tone, EQ, or MIDI validation errors.
    pub fn from_juce_tree(tree: &JuceTree) -> Result<Self, ChainError> {
        let chain = child_tree(tree, "ChainSnapshot")
            .map(ChainSnapshot::from_juce_tree)
            .transpose()?
            .unwrap_or_else(ChainSnapshot::empty);
        let input_mode =
            InputMode::from_persisted(juce_string_prop(tree, "inputMode").unwrap_or("stereo"));
        let editor_scale = juce_number_prop(tree, "editorScale").unwrap_or(1.0);
        let editor_extra_height = juce_int_prop(tree, "editorExtraHeight")
            .unwrap_or(36)
            .try_into()
            .map_err(|_| ChainError::InvalidDocument("editor height is out of range"))?;
        let mut midi_mappings = Vec::new();
        if let Some(midi) = child_tree(tree, "MidiMappings") {
            if midi.children.len() > MAX_MIDI_MAPPINGS {
                return Err(ChainError::TooManyMidiMappings {
                    actual: midi.children.len(),
                    max: MAX_MIDI_MAPPINGS,
                });
            }
            for mapping in &midi.children {
                if mapping.identifier != "Mapping" {
                    continue;
                }
                let number = juce_int_prop(mapping, "number").unwrap_or(0);
                let number = u8::try_from(number)
                    .ok()
                    .filter(|number| *number <= 127)
                    .ok_or(ChainError::InvalidDocument(
                        "MIDI number must be between 0 and 127",
                    ))?;
                midi_mappings.push(MidiMapping {
                    target_id: juce_string_prop(mapping, "targetId")
                        .unwrap_or_default()
                        .to_owned(),
                    source: MidiSource::from_persisted(
                        juce_string_prop(mapping, "source").unwrap_or("cc"),
                    ),
                    number,
                });
            }
        }
        let id = juce_string_prop(tree, "activePresetId").unwrap_or_default();
        let name = juce_string_prop(tree, "activePresetName").unwrap_or_default();
        let active_preset = if id.is_empty() && name.is_empty() {
            None
        } else {
            Some(ActivePreset {
                id: id.to_owned(),
                name: name.to_owned(),
            })
        };
        Ok(Self {
            chain,
            input_mode,
            editor_scale,
            editor_extra_height,
            midi_mappings,
            active_preset,
        })
    }
}

impl ChainBlock {
    fn from_juce_tree(tree: &JuceTree) -> Result<Self, ChainError> {
        let id = BlockId(juce_string_prop(tree, "id").unwrap_or_default().to_owned());
        if juce_string_prop(tree, "type").unwrap_or("INSERT") == "INSERT" {
            return Ok(Self {
                id,
                is_insert: true,
                tone: None,
                active_model_id: None,
                loaded: true,
                model_loading: false,
                load_failed: false,
                ir_b: None,
                ir_b_missing: false,
                skin: None,
                params: BlockParams::default(),
            });
        }
        let tone_json = juce_string_prop(tree, "toneJson").unwrap_or_default();
        let tone = Tone::from_json(tone_json)?;
        let active_model_id = juce_int_prop(tree, "activeModelId").map(ModelId);
        let ir_b = listed_ir_b(&tone, juce_int_prop(tree, IR_B_MODEL_ID));
        Ok(Self {
            id,
            is_insert: false,
            tone: Some(tone),
            active_model_id,
            loaded: true,
            model_loading: false,
            load_failed: false,
            ir_b,
            ir_b_missing: false,
            skin: skin_override(juce_string_prop(tree, SKIN_OVERRIDE)),
            params: BlockParams::from_juce_tree(tree)?,
        })
    }

    /// The IR B that a save keeps: the block's B when its tone lists that model.
    ///
    /// A B outside the tone has nothing to store or download, so a save leaves it out rather
    /// than fail on it.
    #[must_use]
    pub fn stored_ir_b(&self) -> Option<IrB> {
        let tone = self.tone.as_ref()?;
        listed_ir_b(tone, self.ir_b.map(|ir_b| ir_b.model_id.0))
    }

    /// Writes IR B and the blend settings into a persisted `ChainBlock` tree.
    ///
    /// `irBModelId` (Int64) is present only while the block has an IR B and is removed when B is
    /// cleared, so a tree reused from an earlier save never keeps a stale B. `irBlend`,
    /// `irALevel`, `irBLevel` and `irBInvert` are always written. The other block properties are
    /// left as they are.
    pub fn write_ir_pair_juce(&self, tree: &mut JuceTree) {
        match self.stored_ir_b() {
            Some(ir_b) => set_juce_prop(tree, IR_B_MODEL_ID, JuceValue::Int64(ir_b.model_id.0)),
            None => tree.properties.retain(|(name, _)| name != IR_B_MODEL_ID),
        }
        set_juce_prop(tree, "irBlend", JuceValue::Double(self.params.ir_blend));
        set_juce_prop(tree, "irALevel", JuceValue::Double(self.params.ir_a_level));
        set_juce_prop(tree, "irBLevel", JuceValue::Double(self.params.ir_b_level));
        set_juce_prop(tree, "irBInvert", JuceValue::Bool(self.params.ir_b_invert));
    }

    /// Writes the skin override into a persisted `ChainBlock` tree.
    ///
    /// `skinOverride` (String) is present only while the block has an override and is removed
    /// for Auto, so a tree reused from an earlier save never keeps a stale skin.
    pub fn write_skin_juce(&self, tree: &mut JuceTree) {
        match skin_override(self.skin.as_deref()) {
            Some(skin) => set_juce_prop(tree, SKIN_OVERRIDE, JuceValue::String(skin)),
            None => tree.properties.retain(|(name, _)| name != SKIN_OVERRIDE),
        }
    }
}

/// A stored skin override; an empty one is Auto.
fn skin_override(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|skin| !skin.is_empty())
        .map(str::to_owned)
}

/// The persisted `ChainBlock` property that names the skin a block's render uses.
pub const SKIN_OVERRIDE: &str = "skinOverride";

/// IR B for model `id`, when `tone` lists that model. A B outside the tone is dropped, so a
/// later save never has to store a model the tone cannot give.
fn listed_ir_b(tone: &Tone, id: Option<i64>) -> Option<IrB> {
    let model_id = ModelId(id?);
    tone.models
        .iter()
        .any(|model| model.id == model_id)
        .then_some(IrB { model_id })
}

/// The persisted `ChainBlock` property that names a cab block's IR B.
pub const IR_B_MODEL_ID: &str = "irBModelId";

fn set_juce_prop(tree: &mut JuceTree, name: &str, value: JuceValue) {
    if let Some((_, existing)) = tree.properties.iter_mut().find(|(key, _)| key == name) {
        *existing = value;
    } else {
        tree.properties.push((name.into(), value));
    }
}

/// A normalized setting in 0..=1, or `fallback` when it is absent or not a number.
fn unit_or(value: Option<f64>, fallback: f64) -> f64 {
    value
        .filter(|value| value.is_finite())
        .map_or(fallback, |value| value.clamp(0.0, 1.0))
}

impl BlockParams {
    fn from_juce_tree(tree: &JuceTree) -> Result<Self, ChainError> {
        Ok(Self {
            enabled: juce_bool_prop(tree, "enabled").unwrap_or(true),
            normalize: juce_bool_prop(tree, "normalize").unwrap_or(true),
            slim_size: juce_number_prop(tree, "slimSize").unwrap_or(0.0),
            input_gain: juce_number_prop(tree, "inputGain").unwrap_or(0.5),
            output_gain: juce_number_prop(tree, "outputGain").unwrap_or(0.5),
            mix: juce_number_prop(tree, "mix").unwrap_or(1.0),
            eq: EqParams::from_juce_tree(child_tree(tree, "Eq"))?,
            ir_blend: unit_or(juce_number_prop(tree, "irBlend"), DEFAULT_IR_BLEND),
            ir_a_level: unit_or(juce_number_prop(tree, "irALevel"), DEFAULT_IR_LEVEL),
            ir_b_level: unit_or(juce_number_prop(tree, "irBLevel"), DEFAULT_IR_LEVEL),
            ir_b_invert: juce_bool_prop(tree, "irBInvert").unwrap_or(false),
        })
    }
}

impl EqParams {
    fn from_juce_tree(tree: Option<&JuceTree>) -> Result<Self, ChainError> {
        let Some(tree) = tree else {
            return Ok(reference_eq_defaults());
        };
        if tree.children.len() > MAX_EQ_BANDS {
            return Err(ChainError::TooManyEqBands {
                actual: tree.children.len(),
                max: MAX_EQ_BANDS,
            });
        }
        if tree.children.is_empty() {
            return Ok(Self {
                enabled: juce_bool_prop(tree, "enabled").unwrap_or(true),
                pre: juce_bool_prop(tree, "pre").unwrap_or(false),
                ..reference_eq_defaults()
            });
        }
        let mut bands = Vec::with_capacity(tree.children.len());
        for (index, band) in tree.children.iter().enumerate() {
            let mut kind =
                EqBandType::from_persisted(juce_string_prop(band, "type").unwrap_or("bell"));
            if index == 0 && kind == EqBandType::Bell {
                kind = EqBandType::Lowcut;
            } else if index + 1 == tree.children.len() && kind == EqBandType::Bell {
                kind = EqBandType::Highshelf;
            }
            bands.push(EqBand {
                kind,
                freq_hz: juce_number_prop(band, "freqHz").unwrap_or(1000.0),
                gain_db: juce_number_prop(band, "gainDb").unwrap_or(0.0),
                q: juce_number_prop(band, "q").unwrap_or(1.0),
            });
        }
        Ok(Self {
            enabled: juce_bool_prop(tree, "enabled").unwrap_or(true),
            pre: juce_bool_prop(tree, "pre").unwrap_or(false),
            bands,
        })
    }
}

fn reference_eq_defaults() -> EqParams {
    let types = [
        EqBandType::Lowcut,
        EqBandType::Bell,
        EqBandType::Bell,
        EqBandType::Bell,
        EqBandType::Bell,
        EqBandType::Highshelf,
    ];
    let frequencies = [20.0, 150.0, 500.0, 1500.0, 4000.0, 8000.0];
    EqParams {
        enabled: true,
        pre: false,
        bands: types
            .into_iter()
            .zip(frequencies)
            .map(|(kind, freq_hz)| EqBand {
                kind,
                freq_hz,
                gain_db: 0.0,
                q: if matches!(kind, EqBandType::Lowcut | EqBandType::Highcut) {
                    0.71
                } else {
                    1.0
                },
            })
            .collect(),
    }
}

fn parse_juce_lane(
    tree: Option<&JuceTree>,
    side: ChainSide,
    min_slots: usize,
) -> Result<Vec<ChainBlock>, ChainError> {
    let Some(tree) = tree else {
        return Ok(normalized_lane_with_min(Vec::new(), side, min_slots));
    };
    if tree.children.len() > MAX_LANE_BLOCKS {
        return Err(ChainError::TooManyBlocks {
            actual: tree.children.len(),
            max: MAX_LANE_BLOCKS,
        });
    }
    let mut blocks = Vec::with_capacity(tree.children.len());
    for block in &tree.children {
        blocks.push(ChainBlock::from_juce_tree(block)?);
    }
    Ok(normalized_lane_with_min(blocks, side, min_slots))
}

fn child_tree<'a>(tree: &'a JuceTree, name: &str) -> Option<&'a JuceTree> {
    tree.children.iter().find(|child| child.identifier == name)
}

fn property<'a>(tree: &'a JuceTree, name: &str) -> Option<&'a JuceValue> {
    tree.properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn juce_string_prop<'a>(tree: &'a JuceTree, name: &str) -> Option<&'a str> {
    match property(tree, name) {
        Some(JuceValue::String(value)) => Some(value),
        _ => None,
    }
}

fn juce_number_prop(tree: &JuceTree, name: &str) -> Option<f64> {
    match property(tree, name) {
        Some(JuceValue::Double(value)) => Some(*value),
        Some(JuceValue::Int(value)) => Some(f64::from(*value)),
        Some(JuceValue::Int64(value)) => Some(*value as f64),
        Some(JuceValue::String(value)) => value.parse().ok(),
        _ => None,
    }
}

fn juce_int_prop(tree: &JuceTree, name: &str) -> Option<i64> {
    match property(tree, name) {
        Some(JuceValue::Int(value)) => Some(i64::from(*value)),
        Some(JuceValue::Int64(value)) => Some(*value),
        Some(JuceValue::Double(value)) => Some(*value as i64),
        Some(JuceValue::String(value)) => value.parse().ok(),
        _ => None,
    }
}

fn juce_bool_prop(tree: &JuceTree, name: &str) -> Option<bool> {
    match property(tree, name) {
        Some(JuceValue::Bool(value)) => Some(*value),
        Some(JuceValue::Int(value)) => Some(*value != 0),
        Some(JuceValue::Int64(value)) => Some(*value != 0),
        _ => None,
    }
}

fn parse_lane(
    value: Option<&Value>,
    side: ChainSide,
    min_slots: usize,
) -> Result<Vec<ChainBlock>, ChainError> {
    let values = match value {
        Some(Value::Array(blocks)) => blocks,
        Some(_) => return Err(ChainError::InvalidDocument("chain lane must be an array")),
        None => &Vec::new(),
    };
    if values.len() > MAX_LANE_BLOCKS {
        return Err(ChainError::TooManyBlocks {
            actual: values.len(),
            max: MAX_LANE_BLOCKS,
        });
    }
    let mut blocks = Vec::with_capacity(values.len());
    for block in values {
        blocks.push(ChainBlock::from_value(block)?);
    }
    Ok(normalized_lane_with_min(blocks, side, min_slots))
}

fn normalized_lane(blocks: Vec<ChainBlock>, side: ChainSide) -> Vec<ChainBlock> {
    normalized_lane_with_min(blocks, side, MIN_MONO_LANE_SLOTS)
}

/// Puts the tones of one lane first, in their order, then the empty slots.
///
/// A lane has exactly one empty slot after its last tone, and at least `min_slots` slots. An empty
/// slot keeps its identity when it stays, so that a selected slot stays the same slot.
fn normalized_lane_with_min(
    blocks: Vec<ChainBlock>,
    side: ChainSide,
    min_slots: usize,
) -> Vec<ChainBlock> {
    let (mut lane, inserts): (Vec<_>, Vec<_>) =
        blocks.into_iter().partition(|block| !block.is_insert);
    let slots = (lane.len() + 1).max(min_slots);
    let mut spare = inserts.into_iter();
    let mut fresh_ids = None;
    while lane.len() < slots {
        if let Some(block) = spare.next() {
            lane.push(block);
            continue;
        }
        // All spare slots are in the lane now, so the lane holds each identity of the input.
        let fresh = fresh_ids.get_or_insert_with(|| {
            let names: BTreeSet<String> = lane.iter().map(|block| block.id.0.clone()).collect();
            (1_usize..)
                .map(move |number| format!("{}-insert-{number}", side.as_str()))
                .filter(move |name| !names.contains(name))
        });
        // why: `1_usize..` filtered by a finite set never ends, so this fallback cannot occur.
        let block = fresh.next().map_or_else(
            || ChainBlock::insert(format!("{}-insert", side.as_str())),
            ChainBlock::insert,
        );
        lane.push(block);
    }
    lane
}

/// Renames each insert slot whose identity an earlier block already has.
///
/// A chain can only be prepared when identities are unique across both lanes. A moved block
/// keeps its identity, so the numbered name of a new slot can otherwise collide with it.
fn unique_insert_ids(left: &mut [ChainBlock], right: &mut [ChainBlock]) {
    let mut names: BTreeSet<String> = left
        .iter()
        .chain(right.iter())
        .map(|block| block.id.0.clone())
        .collect();
    let mut seen: BTreeSet<String> = left
        .iter()
        .chain(right.iter())
        .filter(|block| !block.is_insert)
        .map(|block| block.id.0.clone())
        .collect();
    for (lane, side) in [(left, ChainSide::Left), (right, ChainSide::Right)] {
        for block in lane.iter_mut().filter(|block| block.is_insert) {
            if seen.insert(block.id.0.clone()) {
                continue;
            }
            let Some(fresh) = (1_usize..)
                .map(|number| format!("{}-insert-{number}", side.as_str()))
                .find(|name| !names.contains(name))
            else {
                continue;
            };
            names.insert(fresh.clone());
            seen.insert(fresh.clone());
            block.id = BlockId(fresh);
        }
    }
}

fn string_field<'a>(value: Option<&'a Value>, fallback: &'a str) -> &'a str {
    value.and_then(Value::as_str).unwrap_or(fallback)
}

fn number_field(value: Option<&Value>, fallback: f64) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(fallback)
}

fn bool_field(value: Option<&Value>, fallback: bool) -> bool {
    value.and_then(Value::as_bool).unwrap_or(fallback)
}

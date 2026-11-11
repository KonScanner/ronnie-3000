use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use ronie_dsp::{LatencyReport, PrepareSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{OracleError, reference_paths};

const OUTPUT_MAGIC: &[u8; 8] = b"RCHAIN01";
const MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 512 * 1024 * 1024;

/// Parameter changes applied immediately before one indexed callback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainStateChange {
    /// Zero-based callback at which this change takes effect.
    pub callback_index: usize,
    /// Normalized parameter values keyed by their stable host identifiers.
    pub parameters: BTreeMap<String, f64>,
    /// Whether the host preparation contract is reapplied after the change.
    pub reprepare: bool,
}

/// Neutral input consumed by the development-only full-chain runner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainScenario {
    /// Neutral scenario format version.
    pub schema_version: u32,
    /// Host and processing capabilities.
    pub spec: PrepareSpec,
    /// Exact callback lengths; their sum is the rendered frame count.
    pub callback_frames: Vec<usize>,
    /// Planar input samples in host channel order.
    pub input: Vec<f32>,
    /// Parameters applied before the initial preparation.
    pub parameters: BTreeMap<String, f64>,
    /// Ordered changes driven at callback boundaries.
    pub state_changes: Vec<ChainStateChange>,
    /// Arbitrary snapshot data retained without dropping unknown fields.
    pub snapshot: Value,
}

impl ChainScenario {
    fn validate(&self) -> Result<(usize, usize, usize), OracleError> {
        if self.schema_version != 1 {
            return Err(OracleError::InvalidCase(
                "unsupported chain scenario version",
            ));
        }
        if !self.spec.host_sample_rate.is_finite() || self.spec.host_sample_rate <= 0.0 {
            return Err(OracleError::InvalidCase(
                "host sample rate must be positive",
            ));
        }
        if self.spec.max_host_frames == 0 || !matches!(self.spec.oversampling, 1 | 2 | 4 | 8) {
            return Err(OracleError::InvalidCase(
                "invalid chain preparation dimensions",
            ));
        }
        if self.callback_frames.is_empty() || self.callback_frames.contains(&0) {
            return Err(OracleError::InvalidCase("callback lengths must be nonzero"));
        }
        let frames = self
            .callback_frames
            .iter()
            .try_fold(0usize, |total, frames| {
                total
                    .checked_add(*frames)
                    .ok_or(OracleError::InvalidCase("frame count overflow"))
            })?;
        let input_channels: usize = if self.spec.stereo_input { 2 } else { 1 };
        let output_channels: usize = if self.spec.stereo_output { 2 } else { 1 };
        let input_samples = input_channels
            .checked_mul(frames)
            .ok_or(OracleError::InvalidCase("input length overflow"))?;
        if self.input.len() != input_samples {
            return Err(OracleError::InvalidCase(
                "input does not match callback and channel dimensions",
            ));
        }
        if self.parameters.values().any(|value| !value.is_finite())
            || self
                .state_changes
                .iter()
                .flat_map(|change| change.parameters.values())
                .any(|value| !value.is_finite())
        {
            return Err(OracleError::InvalidCase("parameters must be finite"));
        }
        if self
            .state_changes
            .iter()
            .any(|change| change.callback_index >= self.callback_frames.len())
        {
            return Err(OracleError::InvalidCase(
                "state-change callback is out of range",
            ));
        }
        Ok((frames, input_channels, output_channels))
    }
}

/// Audio and non-secret diagnostics produced by a full-chain render.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainRender {
    /// Number of output channels.
    pub channels: usize,
    /// Number of frames per output channel.
    pub frames: usize,
    /// Planar f32 output in channel-major order.
    pub samples: Vec<f32>,
    /// Host-visible delay and tail.
    pub latency: LatencyReport,
    /// Final values of parameters selected by the scenario.
    pub parameters: BTreeMap<String, f64>,
    /// Processor diagnostics, with no account or credential data requested.
    pub diagnostic_state: Value,
}

#[derive(Deserialize)]
struct ChainMetadata {
    channels: usize,
    frames: usize,
    latency: LatencyReport,
    parameters: BTreeMap<String, f64>,
    diagnostic_state: Value,
}

/// Development-only adapter for the private full-chain Reference executable.
pub struct ChainOracle {
    executable: PathBuf,
    max_output_bytes: usize,
}

impl ChainOracle {
    /// Creates an adapter from the two configured private paths.
    ///
    /// # Errors
    ///
    /// Returns configuration errors when either path is absent or incomplete.
    pub fn new() -> Result<Self, OracleError> {
        let (_, build) = reference_paths()?.ok_or(OracleError::NotConfigured)?;
        Ok(Self::from_executable(build.join("ronie-reference-chain")))
    }

    /// Creates an adapter for a selected development runner.
    #[must_use]
    pub fn from_executable(executable: PathBuf) -> Self {
        Self {
            executable,
            max_output_bytes: MAX_OUTPUT_BYTES,
        }
    }

    /// Renders a neutral scenario through the full Reference processor.
    ///
    /// Standard output and error are discarded so private paths and diagnostics
    /// do not enter public evidence.
    ///
    /// # Errors
    ///
    /// Returns validation, process, output-bound, framing, or filesystem errors.
    pub fn render(&self, scenario: &ChainScenario) -> Result<ChainRender, OracleError> {
        let (frames, _, output_channels) = scenario.validate()?;
        let audio_bytes = frames
            .checked_mul(output_channels)
            .and_then(|samples| samples.checked_mul(size_of::<f32>()))
            .ok_or(OracleError::InvalidCase("output length overflow"))?;
        let output_size = audio_bytes
            .checked_add(MAX_METADATA_BYTES)
            .ok_or(OracleError::InvalidCase("output bound overflow"))?;
        if output_size > self.max_output_bytes {
            return Err(OracleError::StateOutputTooLarge {
                limit: self.max_output_bytes,
            });
        }

        let scratch = tempfile::tempdir()?;
        let input_path = scratch.path().join("scenario.json");
        let output_path = scratch.path().join("render.chain");
        let encoded = serde_json::to_vec(scenario)
            .map_err(|_| OracleError::InvalidCase("scenario is not serializable"))?;
        fs::write(&input_path, encoded)?;
        let status = Command::new(&self.executable)
            .arg(&input_path)
            .arg(&output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(OracleError::ChainProcessFailed(status));
        }
        let bytes = read_bounded(&output_path, self.max_output_bytes)?;
        decode_render(&bytes, frames, output_channels)
    }
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, OracleError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > limit as u64 {
        return Err(OracleError::StateOutputTooLarge { limit });
    }
    let bytes = fs::read(path)?;
    if bytes.len() > limit {
        return Err(OracleError::StateOutputTooLarge { limit });
    }
    Ok(bytes)
}

fn decode_render(
    bytes: &[u8],
    expected_frames: usize,
    expected_channels: usize,
) -> Result<ChainRender, OracleError> {
    if bytes.len() < 16 || &bytes[..8] != OUTPUT_MAGIC {
        return Err(OracleError::InvalidChainOutput("invalid output header"));
    }
    let metadata_bytes = u64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .map_err(|_| OracleError::InvalidChainOutput("truncated output metadata length"))?,
    );
    let metadata_bytes = usize::try_from(metadata_bytes)
        .map_err(|_| OracleError::InvalidChainOutput("output metadata length overflow"))?;
    if metadata_bytes > MAX_METADATA_BYTES {
        return Err(OracleError::InvalidChainOutput(
            "output metadata is too large",
        ));
    }
    let metadata_end = 16usize
        .checked_add(metadata_bytes)
        .ok_or(OracleError::InvalidChainOutput("output metadata overflow"))?;
    if metadata_end > bytes.len() {
        return Err(OracleError::InvalidChainOutput("truncated output metadata"));
    }
    let metadata: ChainMetadata = serde_json::from_slice(&bytes[16..metadata_end])
        .map_err(|_| OracleError::InvalidChainOutput("invalid output metadata"))?;
    if metadata.frames != expected_frames || metadata.channels != expected_channels {
        return Err(OracleError::InvalidChainOutput("output dimensions changed"));
    }
    let sample_bytes = &bytes[metadata_end..];
    let expected_samples = expected_frames
        .checked_mul(expected_channels)
        .ok_or(OracleError::InvalidChainOutput("output sample overflow"))?;
    if sample_bytes.len() != expected_samples * size_of::<f32>() {
        return Err(OracleError::InvalidOutputLength);
    }
    let samples = sample_bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect();
    Ok(ChainRender {
        channels: metadata.channels,
        frames: metadata.frames,
        samples,
        latency: metadata.latency,
        parameters: metadata.parameters,
        diagnostic_state: metadata.diagnostic_state,
    })
}

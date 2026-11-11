use ronie_core::InputMode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Host and chain capabilities fixed by one background preparation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PrepareSpec {
    /// Audio rate at the host boundary in hertz.
    pub host_sample_rate: f64,
    /// Maximum frames promised by the host for one callback.
    pub max_host_frames: usize,
    /// Chain oversampling factor: 1, 2, 4, or 8.
    pub oversampling: u8,
    /// Session input routing mode.
    #[serde(with = "input_mode_serde")]
    pub input_mode: InputMode,
    /// Whether the host supplies two input channels.
    pub stereo_input: bool,
    /// Whether the host can reproduce two output channels.
    pub stereo_output: bool,
}

/// Host-visible delay and decay information for a prepared chain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LatencyReport {
    /// Reported delay in host-rate samples.
    pub samples: u64,
    /// Reported output tail duration in seconds.
    pub tail_seconds: f64,
}

/// Bounded graph construction and processing failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChainError {
    /// Audio dimensions or channel indices are invalid.
    #[error("invalid audio layout: {0}")]
    InvalidLayout(&'static str),
    /// Background preparation failed.
    #[error("chain preparation failed: {0}")]
    Preparation(String),
    /// A completion belongs to a superseded preparation.
    #[error("stale chain generation {actual}; expected {expected}")]
    StaleGeneration {
        /// Generation accepted by the current chain.
        expected: u64,
        /// Generation supplied by the stale completion.
        actual: u64,
    },
    /// Real-time processing failed without corrupting the chain contract.
    #[error("chain processing failed: {0}")]
    Processing(&'static str),
}

mod input_mode_serde {
    use ronie_core::InputMode;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(mode: &InputMode, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(mode.as_str())
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<InputMode, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(InputMode::from_persisted(&value))
    }
}

use serde::Deserialize;
use serde_json::{Map, Value};

/// Model architectures accepted by the bounded document parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamArchitecture {
    /// WaveNet model, including A2-shaped models.
    WaveNet,
    /// Ordered container of complete child models.
    SlimmableContainer,
}

/// Support status for a NAM file version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSupport {
    /// The version is fully supported.
    Yes,
    /// The version is a later patch in the latest supported minor series.
    Partial,
}

/// Typed model metadata plus unrecognized fields for forward compatibility.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct NamMetadata {
    /// Model loudness in decibels, when supplied.
    #[serde(default)]
    pub loudness: Option<f64>,
    /// Calibration input level in dBu, when supplied.
    #[serde(default)]
    pub input_level_dbu: Option<f64>,
    /// Calibration output level in dBu, when supplied.
    #[serde(default)]
    pub output_level_dbu: Option<f64>,
    /// Unrecognized metadata fields retained without interpretation.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A validated NAM document with directly deserialized `f32` weights.
#[derive(Debug, Clone, PartialEq)]
pub struct NamDocument {
    /// Original version text from the document.
    pub version: String,
    /// Reference-compatible support classification.
    pub support: VersionSupport,
    /// Parsed model architecture.
    pub architecture: NamArchitecture,
    /// Expected sample rate, or `None` when the document does not declare it.
    pub sample_rate: Option<f64>,
    /// Typed and unrecognized model metadata.
    pub metadata: NamMetadata,
    /// Raw architecture configuration retained for later validation.
    pub config: Value,
    /// Top-level model weights parsed directly as `f32` values.
    pub weights: Vec<f32>,
}

mod decode;
mod engine;
mod nonuniform;
pub(crate) mod uniform;

pub use decode::compile_ir;
pub use engine::PreparedIr;

/// Audible IR category used for default mix and output padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrKind {
    /// Short cabinet-style response.
    Cab,
    /// Long room or reverb response, or an explicit `space` gear tag.
    Reverb,
}

impl IrKind {
    /// Default normalized wet mix for a newly loaded tone.
    #[must_use]
    pub const fn default_mix(self) -> f32 {
        match self {
            Self::Cab => 1.0,
            Self::Reverb => 0.5,
        }
    }

    /// Output gain applied by the block for this IR category.
    #[must_use]
    pub fn output_pad(self) -> f32 {
        match self {
            Self::Cab => 10.0_f32.powf(-18.0 / 20.0),
            Self::Reverb => 1.0,
        }
    }
}

/// Convolution engine selected from the pre-trim response length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrEngineKind {
    /// Uniform zero-latency partitions.
    Uniform,
    /// A short-partition head and a larger-partition delayed tail.
    NonUniform {
        /// Maximum samples in the head partition stream.
        head: usize,
    },
}

/// Immutable trimmed, resampled, and normalized IR preparation data.
#[derive(Debug)]
pub struct CompiledIr {
    /// One mono kernel or two channel-specific base-rate kernels.
    pub channels: Vec<Vec<f32>>,
    /// Trimmed kernel length after resampling to 48 kHz.
    pub base_len: usize,
    /// Audible category after gear and trimmed-length classification.
    pub kind: IrKind,
    /// Partition schedule selected before trimming.
    pub engine: IrEngineKind,
    /// Attenuation-only unit-energy normalization gain.
    pub normalization_gain: f32,
}

impl CompiledIr {
    /// Kernel duration in seconds at the base rate.
    #[must_use]
    pub fn tail_seconds(&self) -> f64 {
        self.base_len as f64 / 48_000.0
    }

    /// Default wet mix for a newly loaded block.
    #[must_use]
    pub const fn default_mix(&self) -> f32 {
        self.kind.default_mix()
    }

    /// Output pad for this IR category.
    #[must_use]
    pub fn output_pad(&self) -> f32 {
        self.kind.output_pad()
    }
}

/// Decodes WAV model bytes and compiles an immutable IR.
///
/// # Errors
///
/// Returns WAV validation or IR preparation errors.
pub fn compile_ir_bytes(
    bytes: &ronie_core::ModelBytes,
    gear: Option<&str>,
) -> Result<std::sync::Arc<CompiledIr>, crate::ChainError> {
    let document = ronie_core::decode_wav(bytes.as_ref())
        .map_err(|error| crate::ChainError::Preparation(format!("invalid IR WAV: {error}")))?;
    compile_ir(&document, gear)
}

//! A2 shape detection and immutable weight compilation.

mod compile;
mod engine;
mod process;
mod shape;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod simd;
mod state;

pub use compile::compile_a2;
pub use engine::A2Engine;
pub use process::A2Model;
pub use shape::detect_a2;
pub use state::A2HistoryInfo;

pub(crate) const NUM_LAYERS: usize = 23;
pub(crate) const HEAD_KERNEL_SIZE: usize = 16;
pub(crate) const KERNEL_SIZES: [usize; NUM_LAYERS] = [
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 15, 15, 6, 6, 6, 6, 6, 6, 6,
];
pub(crate) const DILATIONS: [usize; NUM_LAYERS] = [
    1, 3, 7, 17, 41, 101, 239, 1, 3, 7, 17, 41, 101, 239, 1, 13, 1, 3, 7, 17, 41, 101, 239,
];

/// Supported A2 channel specializations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2Channels {
    /// Three-channel A2 model.
    Nano,
    /// Eight-channel A2 model.
    Standard,
}

impl A2Channels {
    /// Returns the channel count.
    #[must_use]
    pub const fn count(self) -> usize {
        match self {
            Self::Nano => 3,
            Self::Standard => 8,
        }
    }

    /// Returns the exact weight count for this specialization.
    #[must_use]
    pub const fn expected_weight_count(self) -> usize {
        let channels = self.count();
        179 * channels * channels + 86 * channels + 2
    }
}

/// Immutable compiled weights for one A2 layer.
#[derive(Debug)]
pub struct CompiledA2Layer {
    pub(crate) kernel_size: usize,
    pub(crate) dilation: usize,
    pub(crate) conv_weights: Box<[f32]>,
    pub(crate) conv_bias: Box<[f32]>,
    pub(crate) mixin_weights: Box<[f32]>,
    pub(crate) layer1x1_weights: Box<[f32]>,
    pub(crate) layer1x1_bias: Box<[f32]>,
}

impl CompiledA2Layer {
    /// Convolution kernel size.
    #[must_use]
    pub const fn kernel_size(&self) -> usize {
        self.kernel_size
    }

    /// Convolution dilation.
    #[must_use]
    pub const fn dilation(&self) -> usize {
        self.dilation
    }

    /// Maximum historical frame offset used by this layer.
    #[must_use]
    pub const fn max_lookback(&self) -> usize {
        (self.kernel_size - 1) * self.dilation
    }

    /// Column-major-per-tap convolution weights.
    #[must_use]
    pub fn conv_weights(&self) -> &[f32] {
        &self.conv_weights
    }

    /// Convolution bias values.
    #[must_use]
    pub fn conv_bias(&self) -> &[f32] {
        &self.conv_bias
    }

    /// Input-mixin weights.
    #[must_use]
    pub fn mixin_weights(&self) -> &[f32] {
        &self.mixin_weights
    }

    /// Column-major residual 1x1 weights.
    #[must_use]
    pub fn layer1x1_weights(&self) -> &[f32] {
        &self.layer1x1_weights
    }

    /// Residual 1x1 bias values.
    #[must_use]
    pub fn layer1x1_bias(&self) -> &[f32] {
        &self.layer1x1_bias
    }
}

/// Immutable A2 weights and architecture data shared by engine instances.
#[derive(Debug)]
pub struct CompiledA2 {
    pub(crate) channels: A2Channels,
    pub(crate) rechannel_weights: Box<[f32]>,
    pub(crate) layers: Box<[CompiledA2Layer]>,
    pub(crate) head_weights: Box<[f32]>,
    pub(crate) head_bias: f32,
    pub(crate) head_scale: f32,
    pub(crate) expected_sample_rate: f64,
    pub(crate) metadata: ronie_core::NamMetadata,
}

impl CompiledA2 {
    /// Channel specialization.
    #[must_use]
    pub const fn channels(&self) -> A2Channels {
        self.channels
    }

    /// Input rechannel weights.
    #[must_use]
    pub fn rechannel_weights(&self) -> &[f32] {
        &self.rechannel_weights
    }

    /// All 23 compiled layers.
    #[must_use]
    pub fn layers(&self) -> &[CompiledA2Layer] {
        &self.layers
    }

    /// Column-major-per-tap head weights.
    #[must_use]
    pub fn head_weights(&self) -> &[f32] {
        &self.head_weights
    }

    /// Head bias.
    #[must_use]
    pub const fn head_bias(&self) -> f32 {
        self.head_bias
    }

    /// Trailing head scale.
    #[must_use]
    pub const fn head_scale(&self) -> f32 {
        self.head_scale
    }

    /// Expected model sample rate, or -1 when unknown.
    #[must_use]
    pub const fn expected_sample_rate(&self) -> f64 {
        self.expected_sample_rate
    }

    /// Number of zero frames required to prewarm this architecture.
    #[must_use]
    pub fn prewarm_samples(&self) -> usize {
        self.layers
            .iter()
            .map(CompiledA2Layer::max_lookback)
            .sum::<usize>()
            + HEAD_KERNEL_SIZE
            - 1
    }
}

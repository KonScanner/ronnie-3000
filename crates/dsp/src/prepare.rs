use std::sync::Arc;

use ronie_core::BlockId;

use crate::{
    ChainError, LatencyReport, PrepareSpec,
    analysis::{AutoBalance, AutoOffset, TunerDetector},
    block::SpectrumTap,
    input::InputStage,
    lane::LaneSnapshot,
    output::OutputStage,
    topology::{LaneSet, RtTopology},
};

/// A completely allocated candidate chain ready for atomic callback publication.
pub struct PreparedChainChange {
    /// Monotonically increasing preparation generation.
    pub generation: u64,
    /// Host and chain capabilities used by this candidate.
    pub spec: PrepareSpec,
    /// Independently owned lane processors and settings.
    pub blocks: LaneSet,
    /// Callback-owned routing and structural state.
    pub topology: RtTopology,
    /// Host-visible latency and tail report.
    pub latency: LatencyReport,
    pub(crate) input: InputStage,
    pub(crate) output: OutputStage,
    pub(crate) tuner: TunerDetector,
    pub(crate) auto_balance: AutoBalance,
    pub(crate) auto_offset: AutoOffset,
    pub(crate) probe_scratch: Vec<f32>,
    pub(crate) mono_scratch: Vec<f32>,
    pub(crate) host_scratch: Vec<f32>,
    pub(crate) resampler: crate::rate::ChainBoundaryResampler,
}

impl PreparedChainChange {
    /// Each block of this chain and the lock-free output of that block.
    ///
    /// A control thread reads this before it publishes the chain, because only the callback can
    /// reach the chain after that.
    #[must_use]
    pub fn spectrum_taps(&self) -> Vec<(BlockId, Arc<SpectrumTap>)> {
        self.blocks
            .left
            .spectrum_taps()
            .chain(self.blocks.right.spectrum_taps())
            .map(|(id, tap)| (id.clone(), tap))
            .collect()
    }

    /// Builds every processor and scratch allocation on the control thread.
    ///
    /// # Errors
    ///
    /// Returns the first invalid snapshot or processor preparation error. No
    /// active chain is modified by a failed candidate.
    pub fn new(
        generation: u64,
        spec: PrepareSpec,
        left: LaneSnapshot,
        right: LaneSnapshot,
        stereo_enabled: bool,
        latency: LatencyReport,
    ) -> Result<Self, ChainError> {
        if !spec.host_sample_rate.is_finite()
            || spec.host_sample_rate <= 0.0
            || spec.max_host_frames == 0
            || !matches!(spec.oversampling, 1 | 2 | 4 | 8)
        {
            return Err(ChainError::InvalidLayout("invalid chain preparation spec"));
        }
        let input = InputStage::new(spec.host_sample_rate, spec.max_host_frames)?;
        let output = OutputStage::new(spec.host_sample_rate, spec.max_host_frames)?;
        let tuner = TunerDetector::new(spec.host_sample_rate)?;
        let auto_offset = AutoOffset::new(48_000.0)?;
        let mut resampler = crate::rate::ChainBoundaryResampler::new();
        resampler.prepare(spec.host_sample_rate, 2, spec.max_host_frames)?;
        let chain_spec = PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: resampler.max_chain_frames(),
            ..spec
        };
        let ir_tail = left
            .models
            .iter()
            .chain(&right.models)
            .filter_map(|model| match model {
                Some(crate::block::BlockModel::Ir(ir)) => Some(ir.tail_seconds()),
                Some(crate::block::BlockModel::IrPair { a, b }) => {
                    Some(a.tail_seconds().max(b.tail_seconds()))
                }
                _ => None,
            })
            .fold(latency.tail_seconds, f64::max);
        let blocks = LaneSet::new(left, right, chain_spec, stereo_enabled, spec.input_mode)?;
        let topology = RtTopology::new(chain_spec);
        let scratch_samples = spec.max_host_frames.saturating_mul(2);
        Ok(Self {
            generation,
            spec,
            blocks,
            topology,
            latency: LatencyReport {
                samples: latency.samples.saturating_add(resampler.latency_samples()),
                tail_seconds: ir_tail,
            },
            input,
            output,
            tuner,
            auto_balance: AutoBalance::new(spec.host_sample_rate),
            auto_offset,
            probe_scratch: vec![0.0; chain_spec.max_host_frames],
            mono_scratch: vec![0.0; scratch_samples],
            host_scratch: vec![0.0; scratch_samples],
            resampler,
        })
    }
}

/// Six-band block EQ.
pub mod eq;
pub(crate) mod gain;
mod spectrum;

use std::{
    num::{NonZeroU8, NonZeroUsize},
    sync::Arc,
};

use ronie_core::BlockParams;
use spectrum::Spectrum;
pub use spectrum::{SpectrumAnalyzer, SpectrumTap};

use crate::{
    ChainError, CompiledModel, PhaseEngine, PlanarBuffer, PrepareSpec,
    ir::{CompiledIr, PairMix, PreparedIr, PreparedIrPair},
    oversampling::ChainOversampler,
};
use eq::{EqControls, EqEngine};
use gain::{LinearGain, db_to_gain, normalized_gain, peak_db};

/// Compiled model input for one block.
#[derive(Clone)]
pub enum BlockModel {
    /// NAM model or slimmable container.
    Nam(CompiledModel),
    /// Immutable compiled IR.
    Ir(Arc<CompiledIr>),
    /// Two compiled cab IRs blended inside one block.
    IrPair {
        /// The block's own IR.
        a: Arc<CompiledIr>,
        /// The IR blended in by `ir_blend`.
        b: Arc<CompiledIr>,
    },
}

/// Mutable model engine owned by one prepared block.
pub enum ModelProcessor {
    /// Phase/voice NAM engine.
    Nam(PhaseEngine),
    /// Prepared IR engine.
    Ir(Box<PreparedIr>),
    /// Two prepared IR engines and their mix.
    IrPair(Box<PreparedIrPair>),
}

/// Controls sampled at one callback boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockControls {
    /// Normalized input gain, 0.5 at unity.
    pub input_gain: f32,
    /// Normalized output gain, 0.5 at unity.
    pub output_gain: f32,
    /// Normalized wet mix.
    pub mix: f32,
    /// Whether the block is enabled.
    pub enabled: bool,
    /// NAM loudness normalization toggle.
    pub normalize: bool,
    /// Slimmable tier request.
    pub slim_size: f64,
    /// Six-band EQ controls.
    pub eq: EqControls,
    /// Apply input calibration metadata before NAM processing.
    pub calibrate_input: bool,
    /// User calibration reference in dBu.
    pub input_calibration_dbu: f32,
    /// Target listening loudness in dB.
    pub target_loudness_db: f32,
    /// Use calibrated output hand-off instead of loudness normalization.
    pub calibrated_handoff: bool,
    /// Process independent left and right NAM voices.
    pub dual_mono: bool,
    /// Whether an engine or structural swap is pending.
    pub swap_pending: bool,
    /// Whether the pending swap mutes only the wet term.
    pub swap_mute_wet: bool,
    /// Whether the EQ analyzer is open and accepting samples.
    pub spectrum_enabled: bool,
    /// IR pair crossfade: 0 plays only A, 1 plays only B.
    pub ir_blend: f32,
    /// Normalized level of IR A, 0.5 at unity.
    pub ir_a_level: f32,
    /// Normalized level of IR B, 0.5 at unity.
    pub ir_b_level: f32,
    /// Flip IR B's polarity.
    pub ir_b_invert: bool,
}

impl Default for BlockControls {
    fn default() -> Self {
        Self {
            input_gain: 0.5,
            output_gain: 0.5,
            mix: 1.0,
            enabled: true,
            normalize: true,
            slim_size: 0.0,
            eq: EqControls::default(),
            calibrate_input: false,
            input_calibration_dbu: 0.0,
            target_loudness_db: -18.0,
            calibrated_handoff: false,
            dual_mono: false,
            swap_pending: false,
            swap_mute_wet: false,
            spectrum_enabled: false,
            ir_blend: 0.5,
            ir_a_level: 0.5,
            ir_b_level: 0.5,
            ir_b_invert: false,
        }
    }
}

/// A normalized pair level, with a non-finite level at unity (0.5).
fn finite_level(level: f32) -> f32 {
    if level.is_finite() { level } else { 0.5 }
}

impl BlockControls {
    pub(crate) fn with_params(mut self, params: &BlockParams) -> Self {
        self.input_gain = params.input_gain as f32;
        self.output_gain = params.output_gain as f32;
        self.mix = params.mix as f32;
        self.enabled = params.enabled;
        self.normalize = params.normalize;
        self.slim_size = params.slim_size;
        self.eq = EqControls::from(&params.eq);
        self.ir_blend = params.ir_blend as f32;
        self.ir_a_level = params.ir_a_level as f32;
        self.ir_b_level = params.ir_b_level as f32;
        self.ir_b_invert = params.ir_b_invert;
        self
    }

    fn pair_mix(&self) -> PairMix {
        PairMix {
            blend: self.ir_blend,
            level_a: normalized_gain(finite_level(self.ir_a_level)),
            level_b: normalized_gain(finite_level(self.ir_b_level)),
            invert_b: self.ir_b_invert,
        }
    }
}

impl From<&BlockParams> for BlockControls {
    fn from(params: &BlockParams) -> Self {
        Self::default().with_params(params)
    }
}

/// Complete prepared processing state for one chain block.
pub struct PreparedBlock {
    processor: ModelProcessor,
    model: BlockModel,
    spec: PrepareSpec,
    ir_kind: Option<crate::ir::IrKind>,
    controls: BlockControls,
    dry_scratch: Vec<f32>,
    island: ChainOversampler,
    eq: EqEngine,
    input_smoother: LinearGain,
    output_smoother: LinearGain,
    mix_smoother: LinearGain,
    normalization_smoother: LinearGain,
    wet_fade: LinearGain,
    swap_mute: LinearGain,
    swap_done: bool,
    normalization_gain: f32,
    input_meter: f32,
    output_meter: f32,
    spectrum: Spectrum,
    runtime_disabled: bool,
    #[cfg(any(test, feature = "fault-injection"))]
    fail_next: bool,
}

/// Prepares a complete block and every allocation off the audio thread.
///
/// # Errors
///
/// Returns layout, model compilation, or preparation errors.
pub fn prepare_block(
    compiled: BlockModel,
    spec: PrepareSpec,
    params: BlockParams,
) -> Result<PreparedBlock, ChainError> {
    if !spec.host_sample_rate.is_finite()
        || spec.host_sample_rate <= 0.0
        || spec.max_host_frames == 0
        || !matches!(spec.oversampling, 1 | 2 | 4 | 8)
    {
        return Err(ChainError::InvalidLayout("invalid block preparation spec"));
    }
    let factor = spec.oversampling as usize;
    let chain_rate = spec.host_sample_rate * factor as f64;
    let chain_frames = spec
        .max_host_frames
        .checked_mul(factor)
        .ok_or(ChainError::InvalidLayout("block frame capacity overflow"))?;
    let mut controls = BlockControls::from(&params);
    let model = compiled.clone();
    let ir_kind = match &compiled {
        BlockModel::Ir(compiled) | BlockModel::IrPair { a: compiled, .. } => Some(compiled.kind),
        BlockModel::Nam(_) => None,
    };
    let (processor, normalization_gain) = match compiled {
        BlockModel::Nam(compiled) => {
            let mut engine = PhaseEngine::new(
                &compiled,
                NonZeroU8::new(spec.oversampling)
                    .ok_or(ChainError::InvalidLayout("zero oversampling factor"))?,
                NonZeroU8::new(2).ok_or(ChainError::InvalidLayout("zero voice count"))?,
            )
            .map_err(|error| ChainError::Preparation(error.to_string()))?;
            engine
                .prepare(
                    chain_rate,
                    NonZeroUsize::new(chain_frames)
                        .ok_or(ChainError::InvalidLayout("zero chain frame count"))?,
                    true,
                )
                .map_err(|error| ChainError::Preparation(error.to_string()))?;
            (ModelProcessor::Nam(engine), 1.0)
        }
        BlockModel::Ir(compiled) => {
            let mut engine = PreparedIr::new(Arc::clone(&compiled), spec.stereo_output);
            engine.prepare(spec.max_host_frames)?;
            if compiled.kind == crate::ir::IrKind::Reverb && params.mix == 1.0 {
                controls.mix = 0.5;
            }
            (
                ModelProcessor::Ir(Box::new(engine)),
                compiled.normalization_gain,
            )
        }
        BlockModel::IrPair { a, b } => {
            if a.kind == crate::ir::IrKind::Reverb && params.mix == 1.0 {
                controls.mix = 0.5;
            }
            let mut engine = PreparedIrPair::new(a, b, spec.stereo_output, controls.pair_mix());
            engine.prepare(spec.max_host_frames)?;
            // Each IR's own normalization is part of its mix gain.
            (ModelProcessor::IrPair(Box::new(engine)), 1.0)
        }
    };
    let eq = EqEngine::new(chain_rate, &controls.eq);
    let input_smoother = LinearGain::new(chain_rate, 0.05, normalized_gain(controls.input_gain));
    let output_smoother = LinearGain::new(chain_rate, 0.05, normalized_gain(controls.output_gain));
    let mix_smoother = LinearGain::new(chain_rate, 0.05, controls.mix);
    let normalization_smoother = LinearGain::new(chain_rate, 0.05, normalization_gain);
    let wet_fade = LinearGain::new(chain_rate, 0.025, 0.0);
    let swap_mute = LinearGain::new(chain_rate, 0.025, 1.0);
    Ok(PreparedBlock {
        processor,
        model,
        spec,
        ir_kind,
        controls,
        dry_scratch: vec![0.0; chain_frames * 2],
        island: ChainOversampler::new(spec.oversampling, spec.max_host_frames)?,
        eq,
        input_smoother,
        output_smoother,
        mix_smoother,
        normalization_smoother,
        wet_fade,
        swap_mute,
        swap_done: false,
        normalization_gain,
        input_meter: -60.0,
        output_meter: -60.0,
        spectrum: Spectrum::new(chain_rate),
        runtime_disabled: false,
        #[cfg(any(test, feature = "fault-injection"))]
        fail_next: false,
    })
}

impl PreparedBlock {
    /// Returns the immutable compiled model for snapshot preparation.
    #[must_use]
    pub fn model_clone(&self) -> BlockModel {
        self.model.clone()
    }

    /// Current control snapshot.
    #[must_use]
    pub const fn controls(&self) -> BlockControls {
        self.controls
    }

    /// Current model or IR normalization target.
    #[must_use]
    pub const fn normalization_gain(&self) -> f32 {
        self.normalization_gain
    }

    /// Post-input-gain/EQ meter in dB.
    #[must_use]
    pub const fn input_meter_db(&self) -> f32 {
        self.input_meter
    }

    /// Post-mix/output-gain meter in dB.
    #[must_use]
    pub const fn output_meter_db(&self) -> f32 {
        self.output_meter
    }

    /// Whether the pending fade reached its completion threshold.
    #[must_use]
    pub const fn swap_fade_done(&self) -> bool {
        self.swap_done
    }

    /// The lock-free output of this block, for a spectrum on another thread.
    #[must_use]
    pub fn spectrum_tap(&self) -> Arc<SpectrumTap> {
        self.spectrum.tap()
    }

    /// Returns the current 64-bin spectrum on the control thread.
    #[must_use]
    pub fn spectrum(&mut self, now_ms: i64) -> [f32; 64] {
        self.spectrum.get_spectrum(now_ms)
    }

    pub(crate) fn disable_and_restore_dry(&mut self, buffer: &mut PlanarBuffer<'_>) {
        let samples = buffer.channels() * buffer.frames();
        buffer
            .as_mut_slice()
            .copy_from_slice(&self.dry_scratch[..samples]);
        self.runtime_disabled = true;
        self.input_meter = -60.0;
        self.output_meter = -60.0;
    }

    #[cfg(any(test, feature = "fault-injection"))]
    pub(crate) fn fail_next_for_test(&mut self) {
        self.fail_next = true;
    }

    /// Processes one callback without allocating.
    ///
    /// # Errors
    ///
    /// Returns layout or model processing errors. Invalid callback dimensions
    /// are rejected before any control or audio state changes.
    pub fn process(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        controls: BlockControls,
    ) -> Result<(), ChainError> {
        self.process_with_pool(buffer, controls, None)
    }

    /// Processes one callback with optional independent phase/voice scheduling.
    ///
    /// # Errors
    ///
    /// Returns layout, model, or collected worker processing errors.
    pub fn process_with_pool(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        controls: BlockControls,
        pool: Option<&crate::rt::RtWorkerPool>,
    ) -> Result<(), ChainError> {
        let channels = buffer.channels();
        let frames = buffer.frames();
        let max_frames = self
            .spec
            .max_host_frames
            .checked_mul(self.spec.oversampling as usize)
            .ok_or(ChainError::InvalidLayout("block frame capacity overflow"))?;
        if !matches!(channels, 1 | 2)
            || frames > max_frames
            || self.dry_scratch.len() < channels * frames
        {
            return Err(ChainError::InvalidLayout("invalid block callback layout"));
        }
        if self.runtime_disabled {
            self.input_meter = -60.0;
            self.output_meter = -60.0;
            self.controls = controls;
            return Ok(());
        }
        self.spectrum.set_enabled(controls.spectrum_enabled);
        self.swap_done = false;
        let wants_wet = controls.enabled && !(controls.swap_pending && !controls.swap_mute_wet);
        self.wet_fade.set_target(if wants_wet { 1.0 } else { 0.0 });
        self.swap_mute
            .set_target(if controls.swap_pending && controls.swap_mute_wet {
                0.0
            } else {
                1.0
            });
        let wet_silent = !self.wet_fade.is_smoothing() && self.wet_fade.current() <= 0.001;
        if wet_silent {
            self.swap_done = true;
        }
        if !wants_wet && wet_silent {
            self.input_meter = -60.0;
            self.output_meter = -60.0;
            self.push_spectrum(buffer, controls.dual_mono)?;
            self.controls = controls;
            return Ok(());
        }

        let samples = channels * frames;
        self.dry_scratch[..samples].copy_from_slice(buffer.as_slice());
        #[cfg(any(test, feature = "fault-injection"))]
        if self.fail_next {
            self.fail_next = false;
            return Err(ChainError::Processing("injected model failure"));
        }
        self.eq.update_controls(&controls.eq);
        self.input_smoother
            .set_target(normalized_gain(controls.input_gain));
        let mut input_peak = 0.0_f32;
        for frame in 0..frames {
            let gain = self.input_smoother.next();
            for channel in 0..channels {
                let index = channel * frames + frame;
                let sample = &mut buffer.as_mut_slice()[index];
                *sample *= gain;
                input_peak = input_peak.max(sample.abs());
            }
        }
        if controls.eq.pre && self.eq.is_active() {
            self.eq.process(buffer.as_mut_slice(), channels);
            input_peak = buffer
                .as_slice()
                .iter()
                .map(|sample| sample.abs())
                .fold(0.0, f32::max);
        }
        self.input_meter = peak_db(input_peak);

        match &mut self.processor {
            ModelProcessor::Nam(engine) => {
                let model_input = engine.metadata().input_level_dbu;
                if controls.calibrate_input
                    && let Some(model_input) = model_input
                {
                    let gain = db_to_gain(controls.input_calibration_dbu - model_input as f32);
                    buffer
                        .as_mut_slice()
                        .iter_mut()
                        .for_each(|sample| *sample *= gain);
                }
                engine
                    .set_slimmable_size(controls.slim_size)
                    .map_err(|_| ChainError::Processing("slimmable tier selection failed"))?;
                if let Some(pool) = pool {
                    engine
                        .process_planar_with_pool(
                            buffer.as_mut_slice(),
                            channels,
                            controls.dual_mono,
                            pool,
                        )
                        .map_err(|_| ChainError::Processing("model processing failed"))?;
                } else {
                    engine
                        .process_planar(buffer.as_mut_slice(), channels, controls.dual_mono)
                        .map_err(|_| ChainError::Processing("model processing failed"))?;
                }
                let metadata = engine.metadata();
                let mut gain = 1.0;
                let mut calibrated = false;
                if controls.calibrated_handoff
                    && let Some(model_output) = metadata.output_level_dbu
                    && model_output.is_finite()
                    && (-60.0..=60.0).contains(&model_output)
                {
                    gain = db_to_gain(model_output as f32 - controls.input_calibration_dbu);
                    calibrated = true;
                }
                if !calibrated && controls.normalize {
                    let model_loudness = metadata
                        .loudness
                        .filter(|value| value.is_finite() && (-100.0..=0.0).contains(value))
                        .unwrap_or(controls.target_loudness_db as f64);
                    gain = db_to_gain(
                        (controls.target_loudness_db - model_loudness as f32).clamp(-12.0, 12.0),
                    );
                }
                self.normalization_gain = gain;
                self.normalization_smoother.set_target(gain);
                for sample in buffer.as_mut_slice() {
                    *sample *= self.normalization_smoother.next();
                }
            }
            ModelProcessor::Ir(engine) => {
                engine.process_planar_with_stereo(
                    buffer,
                    &mut self.island,
                    !controls.dual_mono && channels > 1 && self.spec.stereo_output,
                )?;
            }
            ModelProcessor::IrPair(engine) => {
                engine.set_mix(controls.pair_mix());
                engine.process_planar_with_stereo(
                    buffer,
                    &mut self.island,
                    !controls.dual_mono && channels > 1 && self.spec.stereo_output,
                )?;
            }
        }
        if !matches!(self.processor, ModelProcessor::Nam(_)) {
            self.normalization_gain = self.normalization_gain.clamp(0.0, 1.0);
            self.normalization_smoother
                .set_target(self.normalization_gain);
            for sample in buffer.as_mut_slice() {
                *sample *= self.normalization_smoother.next();
            }
        }
        if !controls.eq.pre && self.eq.is_active() {
            self.eq.process(buffer.as_mut_slice(), channels);
        }

        let cab_pad = self.ir_kind.map_or(1.0, crate::ir::IrKind::output_pad);
        self.output_smoother
            .set_target(normalized_gain(controls.output_gain));
        self.mix_smoother.set_target(controls.mix.clamp(0.0, 1.0));
        let mut output_peak = 0.0_f32;
        for frame in 0..frames {
            let wet_gain = self.swap_mute.next() * cab_pad;
            let out_gain = self.output_smoother.next();
            let fade = self.wet_fade.next();
            let mix = self.mix_smoother.next() * fade;
            let post_gain = 1.0 + (out_gain - 1.0) * fade;
            for channel in 0..channels {
                let index = channel * frames + frame;
                let wet = buffer.as_slice()[index] * wet_gain;
                let dry = self.dry_scratch[index];
                let output = (dry * (1.0 - mix) + wet * mix) * post_gain;
                buffer.as_mut_slice()[index] = output;
                output_peak = output_peak.max(output.abs());
            }
        }
        if controls.swap_pending {
            let fade = if controls.swap_mute_wet {
                &self.swap_mute
            } else {
                &self.wet_fade
            };
            if !fade.is_smoothing() && fade.current() <= 0.001 {
                self.swap_done = true;
            }
        }
        self.output_meter = peak_db(output_peak);
        self.push_spectrum(buffer, controls.dual_mono)?;
        self.controls = controls;
        Ok(())
    }

    fn push_spectrum(
        &mut self,
        buffer: &PlanarBuffer<'_>,
        dual_mono: bool,
    ) -> Result<(), ChainError> {
        if self.spectrum.accepts_samples() {
            let left = buffer.channel(0)?;
            let right = if buffer.channels() > 1 {
                Some(buffer.channel(1)?)
            } else {
                None
            };
            self.spectrum.push_samples(left, right, dual_mono);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::BlockControls;

    #[test]
    fn a_non_finite_pair_level_plays_at_unity() {
        for level in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let controls = BlockControls {
                ir_a_level: level,
                ir_b_level: level,
                ..BlockControls::default()
            };
            let mix = controls.pair_mix();
            assert!(
                (mix.level_a - 1.0).abs() < 1e-6 && (mix.level_b - 1.0).abs() < 1e-6,
                "{level}: {mix:?}"
            );
        }
    }
}

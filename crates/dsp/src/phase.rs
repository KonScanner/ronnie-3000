#![allow(unsafe_code)]

use ronie_core::{DspError, DspResult};
use std::num::{NonZeroU8, NonZeroUsize};

use crate::{
    CompiledModel, ModelEngine,
    rt::{Job, RtWorkerPool},
};

/// Phase-interleaved model engines with independent voice histories.
pub struct PhaseEngine {
    engines: Vec<ModelEngine>,
    scratch: Vec<SlotScratch>,
    factor: usize,
    voices: usize,
    phase_offset: usize,
    max_frames: usize,
}

struct SlotScratch {
    input: Vec<f64>,
    output: Vec<f64>,
}

impl SlotScratch {
    fn new(frames: usize) -> DspResult<Self> {
        Ok(Self {
            input: zero_f64(frames)?,
            output: zero_f64(frames)?,
        })
    }
}

impl PhaseEngine {
    /// Creates independent phase/voice engines from one compiled model.
    ///
    /// # Errors
    ///
    /// Rejects factors outside 1/2/4/8 and voice counts outside 1/2.
    pub fn new(compiled: &CompiledModel, factor: NonZeroU8, voices: NonZeroU8) -> DspResult<Self> {
        let factor = factor.get() as usize;
        let voices = voices.get() as usize;
        if !matches!(factor, 1 | 2 | 4 | 8) {
            return Err(DspError::InvalidOversamplingFactor);
        }
        if !matches!(voices, 1 | 2) {
            return Err(DspError::InvalidVoiceCount);
        }

        let mut engines = Vec::with_capacity(factor * voices);
        for _ in 0..factor * voices {
            engines.push(ModelEngine::new(compiled)?);
        }
        Ok(Self {
            engines,
            scratch: Vec::new(),
            factor,
            voices,
            phase_offset: 0,
            max_frames: 0,
        })
    }

    /// Allocates every engine and per-slot input/output buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if any engine or scratch allocation cannot be prepared.
    pub fn prepare(
        &mut self,
        sample_rate: f64,
        max_frames: NonZeroUsize,
        prewarm: bool,
    ) -> DspResult<()> {
        self.prepare_with(sample_rate, max_frames, prewarm, |model| {
            model.prepare(sample_rate, max_frames, prewarm)
        })
    }

    fn prepare_with(
        &mut self,
        _sample_rate: f64,
        max_frames: NonZeroUsize,
        _prewarm: bool,
        mut prepare_child: impl FnMut(&mut ModelEngine) -> DspResult<()>,
    ) -> DspResult<()> {
        self.max_frames = 0;
        self.scratch.clear();
        for engine in &mut self.engines {
            engine.invalidate();
        }
        let phase_max = max_frames
            .get()
            .checked_add(self.factor - 1)
            .ok_or(DspError::InvalidBufferSize)?
            / self.factor;
        let phase_max = NonZeroUsize::new(phase_max).ok_or(DspError::InvalidBufferSize)?;
        let mut scratch = Vec::with_capacity(self.engines.len());
        for _ in 0..self.engines.len() {
            scratch.push(SlotScratch::new(phase_max.get())?);
        }
        for engine in &mut self.engines {
            prepare_child(engine)?;
        }

        self.scratch = scratch;
        self.phase_offset = 0;
        self.max_frames = max_frames.get();
        Ok(())
    }

    /// Prepared maximum frames per channel, or zero before preparation.
    #[must_use]
    pub const fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Applies a slimmable tier request to every phase and voice.
    ///
    /// # Errors
    ///
    /// Returns an error before preparation or when a child tier cannot reset.
    pub fn set_slimmable_size(&mut self, value: f64) -> DspResult<bool> {
        let mut changed = false;
        for engine in &mut self.engines {
            changed |= engine.set_slimmable_size(value)?;
        }
        Ok(changed)
    }

    /// Metadata for the current primary model.
    #[must_use]
    pub fn metadata(&self) -> &ronie_core::NamMetadata {
        self.engines[0].metadata()
    }

    /// Returns one independent phase/voice engine by zero-based indices.
    #[must_use]
    pub fn slot(&self, voice: usize, phase: usize) -> Option<&ModelEngine> {
        if voice >= self.voices || phase >= self.factor {
            return None;
        }
        self.engines.get(voice * self.factor + phase)
    }

    /// Returns one mutable independent phase/voice engine by zero-based indices.
    #[must_use]
    pub fn slot_mut(&mut self, voice: usize, phase: usize) -> Option<&mut ModelEngine> {
        if voice >= self.voices || phase >= self.factor {
            return None;
        }
        self.engines.get_mut(voice * self.factor + phase)
    }

    /// Processes channel-major `f32` audio without allocating.
    ///
    /// # Errors
    ///
    /// Validates preparation, channels, voice configuration, buffer divisibility,
    /// and frame count before mutating audio or model state.
    pub fn process_planar(
        &mut self,
        buffer: &mut [f32],
        channels: usize,
        dual_mono: bool,
    ) -> DspResult<()> {
        self.process_planar_impl(buffer, channels, dual_mono, None)
    }

    /// Processes independent phase/voice jobs through a prepared worker pool.
    ///
    /// # Errors
    ///
    /// Returns layout errors before mutation or a collected worker failure.
    pub fn process_planar_with_pool(
        &mut self,
        buffer: &mut [f32],
        channels: usize,
        dual_mono: bool,
        pool: &RtWorkerPool,
    ) -> DspResult<()> {
        self.process_planar_impl(buffer, channels, dual_mono, Some(pool))
    }

    fn process_planar_impl(
        &mut self,
        buffer: &mut [f32],
        channels: usize,
        dual_mono: bool,
        pool: Option<&RtWorkerPool>,
    ) -> DspResult<()> {
        if self.max_frames == 0 {
            return Err(DspError::NotPrepared);
        }
        if !matches!(channels, 1 | 2) {
            return Err(DspError::InvalidChannelCount);
        }
        if dual_mono && (channels != 2 || self.voices != 2) {
            return Err(DspError::InvalidVoiceConfiguration);
        }
        if !buffer.len().is_multiple_of(channels) {
            return Err(DspError::InvalidPlanarLength);
        }
        let frames = buffer.len() / channels;
        if frames > self.max_frames {
            return Err(DspError::BlockTooLarge {
                max: self.max_frames,
                actual: frames,
            });
        }

        let active_voices = if dual_mono { self.voices } else { 1 };
        if let Some(pool) = pool {
            let mut contexts: [Option<PhaseJob<'_>>; 16] = std::array::from_fn(|_| None);
            let mut jobs = [Job::noop(); 16];
            let mut counts = [0_usize; 16];
            let mut job_count = 0;
            for voice in 0..active_voices {
                let channel = if dual_mono { voice } else { 0 };
                for phase in 0..self.factor {
                    let start = (phase + self.factor - self.phase_offset) % self.factor;
                    if start >= frames {
                        continue;
                    }
                    let count = (frames - start).div_ceil(self.factor);
                    let slot = voice * self.factor + phase;
                    let scratch = &mut self.scratch[slot];
                    for (index, frame) in (start..frames).step_by(self.factor).enumerate() {
                        scratch.input[index] = buffer[channel * frames + frame] as f64;
                    }
                    counts[slot] = count;
                }
            }
            for voice in 0..active_voices {
                for phase in 0..self.factor {
                    let slot = voice * self.factor + phase;
                    let count = counts[slot];
                    if count == 0 {
                        continue;
                    }
                    // SAFETY: every slot is unique, in range, and remains live
                    // until the join. No safe mutable slot borrow is outstanding.
                    let engine = unsafe { &mut *self.engines.as_mut_ptr().add(slot) };
                    // SAFETY: the scratch slot has the same unique-slot invariant.
                    let scratch = unsafe { &mut *self.scratch.as_mut_ptr().add(slot) };
                    contexts[job_count] = Some(PhaseJob {
                        engine,
                        input: &scratch.input[..count],
                        output: &mut scratch.output[..count],
                    });
                    if let Some(context) = contexts[job_count].as_mut() {
                        // SAFETY: the fixed context array remains live until the
                        // join, and every context owns a distinct engine and scratch.
                        jobs[job_count] = unsafe { Job::new(run_phase_job, context) };
                    }
                    job_count += 1;
                }
            }
            pool.fork_join(&mut jobs[..job_count])
                .map_err(|_| DspError::ProcessingFailed)?;
            for voice in 0..active_voices {
                let channel = if dual_mono { voice } else { 0 };
                for phase in 0..self.factor {
                    let start = (phase + self.factor - self.phase_offset) % self.factor;
                    let slot = voice * self.factor + phase;
                    for (index, frame) in (start..frames).step_by(self.factor).enumerate() {
                        if index < counts[slot] {
                            buffer[channel * frames + frame] =
                                self.scratch[slot].output[index] as f32;
                        }
                    }
                }
            }
        } else {
            for voice in 0..active_voices {
                let channel = if dual_mono { voice } else { 0 };
                for phase in 0..self.factor {
                    let start = (phase + self.factor - self.phase_offset) % self.factor;
                    if start >= frames {
                        continue;
                    }
                    let count = (frames - start).div_ceil(self.factor);
                    let slot = voice * self.factor + phase;
                    let scratch = &mut self.scratch[slot];
                    for (index, frame) in (start..frames).step_by(self.factor).enumerate() {
                        scratch.input[index] = buffer[channel * frames + frame] as f64;
                    }
                    self.engines[slot]
                        .process(&scratch.input[..count], &mut scratch.output[..count])?;
                    for (index, frame) in (start..frames).step_by(self.factor).enumerate() {
                        buffer[channel * frames + frame] = scratch.output[index] as f32;
                    }
                }
            }
        }

        if !dual_mono {
            for channel in 1..channels {
                let (source, destination) = buffer.split_at_mut(channel * frames);
                destination[..frames].copy_from_slice(&source[..frames]);
            }
        }
        self.phase_offset = (self.phase_offset + frames) % self.factor;
        Ok(())
    }
}

struct PhaseJob<'a> {
    engine: &'a mut ModelEngine,
    input: &'a [f64],
    output: &'a mut [f64],
}

fn run_phase_job(context: *mut ()) -> bool {
    // SAFETY: `PhaseEngine` constructs this pointer for a live, distinct job
    // context and joins every worker before the context can be reused.
    let job = unsafe { &mut *context.cast::<PhaseJob<'_>>() };
    job.engine.process(job.input, job.output).is_ok()
}

fn zero_f64(count: usize) -> DspResult<Vec<f64>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| DspError::InvalidBufferSize)?;
    values.resize(count, 0.0);
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn failed_reprepare_invalidates_every_slot() {
        let document =
            ronie_oracle::synthetic::synthetic_a2_document(3).expect("synthetic document");
        let compiled = CompiledModel::Single(Arc::new(
            crate::compile_a2(&document).expect("compiled model"),
        ));
        let mut engine = PhaseEngine::new(
            &compiled,
            NonZeroU8::new(2).expect("factor"),
            NonZeroU8::new(2).expect("voices"),
        )
        .expect("phase engine");
        engine
            .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
            .expect("initial preparation");
        let mut attempts = 0;
        let result = engine.prepare_with(
            48_000.0,
            NonZeroUsize::new(32).expect("nonzero"),
            false,
            |model| {
                attempts += 1;
                if attempts == 2 {
                    return Err(DspError::InvalidBufferSize);
                }
                model.prepare(48_000.0, NonZeroUsize::new(16).expect("nonzero"), false)
            },
        );

        assert!(matches!(result, Err(DspError::InvalidBufferSize)));
        assert_eq!(attempts, 2);
        assert_eq!(engine.max_frames(), 0);
        let mut buffer = [1.0_f32; 2];
        assert!(matches!(
            engine.process_planar(&mut buffer, 1, false),
            Err(DspError::NotPrepared)
        ));
        assert_eq!(buffer, [1.0_f32; 2]);
    }
}

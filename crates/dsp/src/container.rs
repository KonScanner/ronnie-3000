use ronie_core::{DspError, DspResult, NamArchitecture, NamDocument, parse_nam};
use std::{num::NonZeroUsize, sync::Arc};

use crate::{A2Model, CompiledA2, compile_a2};

/// Immutable compiled child and its upper tier threshold.
#[derive(Debug, Clone)]
pub struct CompiledSubmodel {
    /// First value below this threshold selects this child.
    pub max_value: f64,
    /// Shared immutable child weights.
    pub model: Arc<CompiledA2>,
}

/// Immutable compiled slimmable container.
#[derive(Debug, Clone)]
pub struct CompiledSlimmable {
    submodels: Vec<CompiledSubmodel>,
    expected_sample_rate: f64,
}

impl CompiledSlimmable {
    fn new(submodels: Vec<CompiledSubmodel>, expected_sample_rate: f64) -> DspResult<Self> {
        let tiers: Vec<(f64, f64)> = submodels
            .iter()
            .map(|submodel| (submodel.max_value, submodel.model.expected_sample_rate()))
            .collect();
        validate_container(&tiers, expected_sample_rate)?;
        Ok(Self {
            submodels,
            expected_sample_rate,
        })
    }

    /// Ordered compiled children.
    #[must_use]
    pub fn submodels(&self) -> &[CompiledSubmodel] {
        &self.submodels
    }

    /// Expected container sample rate, or -1 when unknown.
    #[must_use]
    pub const fn expected_sample_rate(&self) -> f64 {
        self.expected_sample_rate
    }
}

/// Immutable model or container used to create independent engines.
#[derive(Debug, Clone)]
pub enum CompiledModel {
    /// One compiled A2 model.
    Single(Arc<CompiledA2>),
    /// Ordered compiled container children.
    Slimmable(CompiledSlimmable),
}

/// Compiles one document or every child of a slimmable container exactly once.
///
/// # Errors
///
/// Returns an error for malformed container children, invalid A2 shapes,
/// invalid tier ordering, or incompatible known sample rates.
pub fn compile_model(document: &NamDocument) -> DspResult<CompiledModel> {
    match document.architecture {
        NamArchitecture::WaveNet => Ok(CompiledModel::Single(Arc::new(compile_a2(document)?))),
        NamArchitecture::SlimmableContainer => {
            let expected_sample_rate = document.sample_rate.unwrap_or(-1.0);
            let entries = document
                .config
                .get("submodels")
                .and_then(serde_json::Value::as_array)
                .ok_or(DspError::InvalidContainer)?;
            if entries.is_empty() {
                return Err(DspError::InvalidContainer);
            }

            let mut submodels = Vec::with_capacity(entries.len());
            for entry in entries {
                let max_value = entry
                    .get("max_value")
                    .and_then(serde_json::Value::as_f64)
                    .ok_or(DspError::InvalidContainer)?;
                let child = entry.get("model").ok_or(DspError::InvalidContainer)?;
                if child
                    .get("architecture")
                    .and_then(serde_json::Value::as_str)
                    != Some("WaveNet")
                {
                    return Err(DspError::InvalidContainer);
                }
                let child_bytes = serde_json::to_vec(child)?;
                let child_document = parse_nam(&child_bytes)?;
                let model = Arc::new(compile_a2(&child_document)?);
                submodels.push(CompiledSubmodel { max_value, model });
            }
            Ok(CompiledModel::Slimmable(CompiledSlimmable::new(
                submodels,
                expected_sample_rate,
            )?))
        }
    }
}

/// Runtime child with independent state.
pub struct Submodel {
    /// First value below this threshold selects this child.
    pub max_value: f64,
    /// Child engine.
    pub model: A2Model,
}

#[derive(Clone, Copy)]
struct ContainerPreparation {
    max_frames: usize,
    prewarm: bool,
}

/// Runtime slimmable container with allocation-free tier switching.
pub struct SlimmableContainer {
    submodels: Vec<Submodel>,
    active_index: usize,
    prepared: Option<ContainerPreparation>,
}

impl SlimmableContainer {
    /// Creates a container and validates ordering and sample rates.
    ///
    /// # Errors
    ///
    /// Returns an error for empty or unsorted tiers, insufficient final
    /// threshold, or incompatible known sample rates.
    pub fn new(submodels: Vec<Submodel>, expected_sample_rate: f64) -> DspResult<Self> {
        if submodels.is_empty() {
            return Err(DspError::InvalidContainer);
        }
        let tiers: Vec<(f64, f64)> = submodels
            .iter()
            .map(|submodel| (submodel.max_value, submodel.model.expected_sample_rate()))
            .collect();
        validate_container(&tiers, expected_sample_rate)?;
        let active_index = submodels.len() - 1;
        Ok(Self {
            submodels,
            active_index,
            prepared: None,
        })
    }

    fn prepare(
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
        mut prepare_child: impl FnMut(&mut A2Model) -> DspResult<()>,
    ) -> DspResult<()> {
        self.prepared = None;
        for submodel in &mut self.submodels {
            submodel.model.invalidate();
            prepare_child(&mut submodel.model)?;
        }
        self.prepared = Some(ContainerPreparation {
            max_frames: max_frames.get(),
            prewarm: _prewarm,
        });
        Ok(())
    }

    fn process(&mut self, input: &[f64], output: &mut [f64]) -> DspResult<()> {
        if self.prepared.is_none() {
            return Err(DspError::NotPrepared);
        }
        self.submodels[self.active_index]
            .model
            .process(input, output)
    }

    fn set_slimmable_size(&mut self, value: f64) -> DspResult<bool> {
        let prepared = self.prepared.ok_or(DspError::NotPrepared)?;
        let target = self
            .submodels
            .iter()
            .position(|submodel| value < submodel.max_value)
            .unwrap_or(self.submodels.len() - 1);
        if target == self.active_index {
            return Ok(false);
        }

        self.submodels[target]
            .model
            .reset_prepared(prepared.prewarm)?;
        self.active_index = target;
        Ok(true)
    }

    /// Active tier index.
    #[must_use]
    pub const fn active_index(&self) -> usize {
        self.active_index
    }

    /// Number of tiers with allocated state.
    #[must_use]
    pub fn prepared_tiers(&self) -> usize {
        self.submodels
            .iter()
            .filter(|submodel| submodel.model.max_frames() > 0)
            .count()
    }

    fn max_frames(&self) -> usize {
        self.prepared.map_or(0, |prepared| prepared.max_frames)
    }

    fn prewarm_samples(&self) -> usize {
        self.submodels[self.active_index].model.prewarm_samples()
    }

    fn active_metadata(&self) -> &ronie_core::NamMetadata {
        self.submodels[self.active_index].model.metadata()
    }
}

/// Runtime model with one lifecycle interface for single and container models.
// Keep the planned enum layout; all substantial history and scratch is heap-allocated.
#[allow(clippy::large_enum_variant)]
pub enum ModelEngine {
    /// One A2 model.
    Single(A2Model),
    /// Slimmable container.
    Slimmable(SlimmableContainer),
}

impl ModelEngine {
    /// Creates independent runtime state from shared compiled models.
    ///
    /// # Errors
    ///
    /// Returns an error when a child engine cannot be constructed.
    pub fn new(compiled: &CompiledModel) -> DspResult<Self> {
        match compiled {
            CompiledModel::Single(model) => Ok(Self::Single(A2Model::new(Arc::clone(model))?)),
            CompiledModel::Slimmable(container) => {
                let mut submodels = Vec::with_capacity(container.submodels.len());
                for child in &container.submodels {
                    submodels.push(Submodel {
                        max_value: child.max_value,
                        model: A2Model::new(Arc::clone(&child.model))?,
                    });
                }
                Ok(Self::Slimmable(SlimmableContainer::new(
                    submodels,
                    container.expected_sample_rate,
                )?))
            }
        }
    }

    pub(crate) fn invalidate(&mut self) {
        match self {
            Self::Single(model) => model.invalidate(),
            Self::Slimmable(container) => {
                container.prepared = None;
                for submodel in &mut container.submodels {
                    submodel.model.invalidate();
                }
            }
        }
    }

    /// Allocates every tier and scratch buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if any child preparation fails.
    pub fn prepare(
        &mut self,
        sample_rate: f64,
        max_frames: NonZeroUsize,
        prewarm: bool,
    ) -> DspResult<()> {
        match self {
            Self::Single(model) => model.prepare(sample_rate, max_frames, prewarm),
            Self::Slimmable(container) => container.prepare(sample_rate, max_frames, prewarm),
        }
    }

    /// Processes the active model without allocating.
    ///
    /// # Errors
    ///
    /// Returns an error before state mutation for invalid or unprepared calls.
    pub fn process(&mut self, input: &[f64], output: &mut [f64]) -> DspResult<()> {
        match self {
            Self::Single(model) => model.process(input, output),
            Self::Slimmable(container) => container.process(input, output),
        }
    }

    /// Selects a tier, resetting and prewarming its prepared state before activation.
    ///
    /// # Errors
    ///
    /// Returns [`DspError::NotPrepared`] before preparation or a reset error
    /// without changing the active tier.
    pub fn set_slimmable_size(&mut self, value: f64) -> DspResult<bool> {
        if self.max_frames() == 0 {
            return Err(DspError::NotPrepared);
        }
        match self {
            Self::Single(_) => Ok(false),
            Self::Slimmable(container) => container.set_slimmable_size(value),
        }
    }

    /// Prepared maximum callback frames, or zero before preparation.
    #[must_use]
    pub fn max_frames(&self) -> usize {
        match self {
            Self::Single(model) => model.max_frames(),
            Self::Slimmable(container) => container.max_frames(),
        }
    }

    /// Active model's prewarm frame count.
    #[must_use]
    pub fn prewarm_samples(&self) -> usize {
        match self {
            Self::Single(model) => model.prewarm_samples(),
            Self::Slimmable(container) => container.prewarm_samples(),
        }
    }

    /// Active tier index, or zero for a single model.
    #[must_use]
    pub fn active_index(&self) -> usize {
        match self {
            Self::Single(_) => 0,
            Self::Slimmable(container) => container.active_index(),
        }
    }

    /// Metadata for the active model.
    #[must_use]
    pub fn metadata(&self) -> &ronie_core::NamMetadata {
        match self {
            Self::Single(model) => model.metadata(),
            Self::Slimmable(container) => container.active_metadata(),
        }
    }
}

/// Compiles a document and creates an independent runtime engine.
///
/// # Errors
///
/// Returns compilation or engine-construction errors.
pub fn build_model(document: &NamDocument) -> DspResult<ModelEngine> {
    ModelEngine::new(&compile_model(document)?)
}

fn validate_container(tiers: &[(f64, f64)], expected_sample_rate: f64) -> DspResult<()> {
    let Some((final_max, _)) = tiers.last() else {
        return Err(DspError::InvalidContainer);
    };
    if tiers.windows(2).any(|pair| pair[1].0 <= pair[0].0) || *final_max < 1.0 {
        return Err(DspError::InvalidContainer);
    }
    for (_, sample_rate) in tiers {
        if *sample_rate != expected_sample_rate
            && *sample_rate != -1.0
            && expected_sample_rate != -1.0
        {
            return Err(DspError::SampleRateMismatch {
                expected: expected_sample_rate,
                actual: *sample_rate,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container() -> SlimmableContainer {
        let document =
            ronie_oracle::synthetic::synthetic_a2_document(3).expect("synthetic document");
        let model = Arc::new(compile_a2(&document).expect("compiled model"));
        let child = || A2Model::new(Arc::clone(&model)).expect("child engine");
        SlimmableContainer::new(
            vec![
                Submodel {
                    max_value: 0.5,
                    model: child(),
                },
                Submodel {
                    max_value: 1.0,
                    model: child(),
                },
            ],
            48_000.0,
        )
        .expect("container")
    }

    #[test]
    fn failed_reprepare_invalidates_every_tier() {
        let mut container = container();
        container
            .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
            .expect("initial preparation");
        let mut attempts = 0;
        let result = container.prepare_with(
            48_000.0,
            NonZeroUsize::new(32).expect("nonzero"),
            false,
            |model| {
                attempts += 1;
                if attempts == 2 {
                    return Err(DspError::InvalidBufferSize);
                }
                model.prepare(48_000.0, NonZeroUsize::new(32).expect("nonzero"), false)
            },
        );

        assert!(matches!(result, Err(DspError::InvalidBufferSize)));
        assert_eq!(attempts, 2);
        assert_eq!(container.max_frames(), 0);
        let input = [0.0; 1];
        let mut output = [0.0; 1];
        assert!(matches!(
            container.process(&input, &mut output),
            Err(DspError::NotPrepared)
        ));
    }
}

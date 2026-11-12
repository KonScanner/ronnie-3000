//! The chain controls that the audio thread makes from the host parameters.
//!
//! Host automation must reach the audio at the block where it changes, also in a render that runs
//! much faster than real time. So the audio thread makes the [`ChainControls`] itself, from the
//! host values and from a few session values that are not host parameters. This takes no lock and
//! allocates nothing.

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use ronie_core::{
    InputMode,
    params::{ParameterCatalog, ParameterId},
};
use ronie_dsp::rt_chain::ChainControls;
use ronie_plugin::{ProcessorBackend, UiBackend, catalog_default, chain_controls_from};

use crate::params::{RonnieParams, is_host_parameter};

/// Session values that the chain controls need, but that are not host parameters.
///
/// The worker writes them, and the audio thread reads them.
pub(crate) struct SessionExtras {
    input_mode: AtomicU8,
    /// Each catalog ID that is not a host parameter, with the bits of its session value.
    values: Vec<(&'static str, AtomicU32)>,
}

impl SessionExtras {
    pub(crate) fn new() -> Self {
        let values = ParameterCatalog::v1()
            .iter()
            .filter(|spec| !is_host_parameter(spec))
            .map(|spec| (spec.id.0, AtomicU32::new(spec.default.to_bits())))
            .collect();
        Self {
            input_mode: AtomicU8::new(input_mode_index(InputMode::Stereo)),
            values,
        }
    }

    /// Copies the values from the session. Call this on a control thread.
    pub(crate) fn publish(&self, backend: &ProcessorBackend, input_mode: InputMode) {
        self.input_mode
            .store(input_mode_index(input_mode), Ordering::Release);
        for (id, value) in &self.values {
            if let Some(session) = backend.parameter(ParameterId(id)) {
                value.store(session.0.to_bits(), Ordering::Release);
            }
        }
    }
}

const fn input_mode_index(mode: InputMode) -> u8 {
    match mode {
        InputMode::Stereo => 0,
        InputMode::Left => 1,
        InputMode::Right => 2,
        InputMode::DualMono => 3,
    }
}

const fn input_mode_of(index: u8) -> InputMode {
    match index {
        1 => InputMode::Left,
        2 => InputMode::Right,
        3 => InputMode::DualMono,
        _ => InputMode::Stereo,
    }
}

/// Where the value of one catalog ID comes from.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// The host parameter at this index.
    Host(usize),
    /// The session extra at this index.
    Extra(usize),
}

/// What the audio thread used for the last controls. It allocates only when it is made.
pub(crate) struct HostControls {
    host: Vec<f32>,
    extras: Vec<u32>,
    input_mode: u8,
    /// The source of each catalog ID, sorted by ID, so that a lookup does few comparisons.
    sources: Vec<(&'static str, Source)>,
}

impl HostControls {
    pub(crate) fn new(params: &RonnieParams, extras: &SessionExtras) -> Self {
        let mut sources: Vec<_> = params
            .bindings
            .iter()
            .enumerate()
            .map(|(index, binding)| (binding.spec.id.0, Source::Host(index)))
            .chain(
                extras
                    .values
                    .iter()
                    .enumerate()
                    .map(|(index, (id, _))| (*id, Source::Extra(index))),
            )
            .collect();
        sources.sort_unstable_by_key(|(id, _)| *id);
        Self {
            // No host value equals NaN, so the first update always makes controls.
            host: vec![f32::NAN; params.values.len()],
            extras: vec![0; extras.values.len()],
            input_mode: u8::MAX,
            sources,
        }
    }

    /// Forgets the last controls, so that the next update makes controls in any case.
    pub(crate) fn reset(&mut self) {
        self.host.fill(f32::NAN);
        self.input_mode = u8::MAX;
    }

    /// New controls when a host value or a session extra changed since the last call.
    ///
    /// This allocates nothing, so the audio thread can call it at the start of each block.
    pub(crate) fn update(
        &mut self,
        params: &RonnieParams,
        extras: &SessionExtras,
    ) -> Option<ChainControls> {
        let values = params.values.iter().map(crate::params::HostParam::value);
        self.update_values(values, params, extras)
    }

    /// [`Self::update`] with the host values given in the order of the host parameters.
    pub(crate) fn update_values(
        &mut self,
        values: impl Iterator<Item = f32>,
        params: &RonnieParams,
        extras: &SessionExtras,
    ) -> Option<ChainControls> {
        let mut changed = false;
        for (seen, value) in self.host.iter_mut().zip(values) {
            if value.to_bits() != seen.to_bits() {
                *seen = value;
                changed = true;
            }
        }
        for (seen, (_, value)) in self.extras.iter_mut().zip(&extras.values) {
            let bits = value.load(Ordering::Acquire);
            if bits != *seen {
                *seen = bits;
                changed = true;
            }
        }
        let input_mode = extras.input_mode.load(Ordering::Acquire);
        if input_mode != self.input_mode {
            self.input_mode = input_mode;
            changed = true;
        }
        if !changed {
            return None;
        }
        let value = |id: &'static str| {
            let source = self
                .sources
                .binary_search_by(|(known, _)| (*known).cmp(id))
                .ok()
                .map(|index| self.sources[index].1);
            match source {
                Some(Source::Host(index)) => params.bindings[index].to_session(self.host[index]).0,
                Some(Source::Extra(index)) => f32::from_bits(self.extras[index]),
                None => catalog_default(id),
            }
        };
        Some(chain_controls_from(
            input_mode_of(self.input_mode),
            value,
            true,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn each_catalog_id_finds_its_source() {
        let backend = Arc::new(ProcessorBackend::new(ronie_plugin::RonieProcessor::new(
            ronie_dsp::PrepareSpec {
                host_sample_rate: 48_000.0,
                max_host_frames: 256,
                oversampling: 1,
                input_mode: InputMode::Left,
                stereo_input: false,
                stereo_output: true,
            },
        )));
        let params = RonnieParams::new(backend);
        let extras = SessionExtras::new();
        let controls = HostControls::new(&params, &extras);

        for spec in ParameterCatalog::v1().iter() {
            let found = controls
                .sources
                .binary_search_by(|(known, _)| (*known).cmp(spec.id.0))
                .map(|index| controls.sources[index].1);
            match found {
                Ok(Source::Host(index)) => assert_eq!(params.bindings[index].spec.id, spec.id),
                Ok(Source::Extra(index)) => assert_eq!(extras.values[index].0, spec.id.0),
                Err(_) => panic!("{} has no source", spec.id.0),
            }
        }
        assert_eq!(controls.sources.len(), ParameterCatalog::v1().len());
    }
}

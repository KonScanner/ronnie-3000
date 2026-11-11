//! Real-time-safe model compilation and audio processing for Ronnie.

/// A2 shape detection and immutable weight compilation.
pub mod a2;
/// Tuner and automatic rig measurement workflows.
pub mod analysis;
/// Run-preserving benchmark measurement and statistics.
pub mod bench;
/// Complete per-block processing and analysis.
pub mod block;
/// Validated planar audio views.
pub mod buffer;
/// Shared immutable compiled-model cache.
pub mod cache;
/// Bounded control-to-audio chain commands.
pub mod command;
/// Slimmable model compilation and lifecycle.
pub mod container;
/// Shared preparation, latency, and graph error contracts.
pub mod graph;
/// Host bus, latency, and tail contracts.
pub mod host;
/// Post-chain stereo image and output matrix.
pub mod image;
/// Host-rate input gain, gate, and pitch processing.
pub mod input;
/// Prepared impulse-response decoding and convolution.
pub mod ir;
/// Independent real-time lane state and immutable lane snapshots.
pub mod lane;
/// DC blocking, tone, and main output processing.
pub mod output;
/// Minimum-phase chain and IR-island oversampling.
pub mod oversampling;
/// Phase-interleaved and dual-mono processing.
pub mod phase;
/// Complete off-thread chain preparation.
pub mod prepare;
/// Host-rate conversion and half-band filter stages.
pub mod rate;
/// Bounded off-thread engine retirement.
pub mod retire;
/// Input and output routing contracts.
pub mod routing;
/// Fixed-capacity real-time worker scheduling.
pub mod rt;
/// Callback-exclusive atomic chain ownership.
pub mod rt_chain;
/// Global three-band tone filters.
pub mod tone_stack;
/// Branch topology and structural history.
pub mod topology;
/// Compile-only local model validation.
pub mod validate;

pub use a2::{
    A2Channels, A2Engine, A2HistoryInfo, A2Model, CompiledA2, CompiledA2Layer, compile_a2,
    detect_a2,
};
pub use buffer::PlanarBuffer;
pub use cache::CompiledModelCache;
pub use container::{
    CompiledModel, CompiledSlimmable, CompiledSubmodel, ModelEngine, SlimmableContainer, Submodel,
    build_model, compile_model,
};
pub use graph::{ChainError, LatencyReport, PrepareSpec};
pub use phase::PhaseEngine;
pub use validate::validate_a2_document;

use clap as _;
use eyre as _;

#[cfg(test)]
use criterion as _;
#[cfg(test)]
use ronie_oracle as _;

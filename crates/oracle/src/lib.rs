//! Development-only fixtures and differential verification tools.

/// Neutral full-chain scenarios and private process rendering.
pub mod chain;
/// Differential cases, comparison policies, and reports.
pub mod compare;
/// Private Reference fixture configuration.
pub mod fixtures;
/// Independent mathematical A2 rendering.
pub mod math;
/// Private Reference process adapter.
pub mod reference;
/// Private Reference persistence and local-loading probes.
pub mod state;
/// Deterministic public model generation.
pub mod synthetic;

pub use chain::{ChainOracle, ChainRender, ChainScenario, ChainStateChange};
pub use compare::{
    DifferentialCase, DifferentialReport, DifferentialStream, FiniteEquivalence, ModelOracle,
    NonFiniteEquivalence, OracleError, compare_samples, run_differential,
};
pub use fixtures::reference_paths;
pub use math::PublicMathOracle;
pub use reference::ReferenceOracle;

use clap as _;
use eyre as _;

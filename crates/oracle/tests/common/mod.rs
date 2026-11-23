//! Shared configuration for the ignored oracle tests.
//!
//! Every ignored test in this crate that needs the private Reference starts by asking for its
//! adapter here. The helper returns `None` only when the private Reference is not configured at
//! all, because that is the one case where the test may skip on a public checkout. Any other
//! construction error is a broken configuration and panics with the error text and the two
//! environment variable names instead of skipping: a silent early return cannot tell an
//! incomplete configuration from a passing oracle.
//!
//! Each test file includes this module, so a helper that another file calls is dead code here.
#![allow(dead_code)]

use ronnie_oracle::state::StateOracle;
use ronnie_oracle::{ChainOracle, OracleError};

/// Returns the private Reference state adapter, or `None` when it is not configured.
///
/// # Panics
///
/// Panics with the error and the required environment variable names when the private
/// Reference is configured but invalid, for example when only one of the two variables is set.
#[must_use]
pub(crate) fn state_oracle() -> Option<StateOracle> {
    configured("state", StateOracle::new())
}

/// Returns the private full-chain Reference adapter, or `None` when it is not configured.
///
/// # Panics
///
/// Panics with the error and the required environment variable names when the private
/// Reference is configured but invalid, for example when only one of the two variables is set.
#[must_use]
pub(crate) fn chain_oracle() -> Option<ChainOracle> {
    configured("full-chain", ChainOracle::new())
}

fn configured<T>(kind: &str, result: Result<T, OracleError>) -> Option<T> {
    match result {
        Ok(oracle) => Some(oracle),
        Err(OracleError::NotConfigured) => None,
        Err(error) => panic!(
            "the private Reference {kind} oracle is misconfigured: {error}. The test skips \
             only when neither RONNIE_REFERENCE_ROOT nor RONNIE_REFERENCE_BUILD is set; set \
             both of them, or neither."
        ),
    }
}

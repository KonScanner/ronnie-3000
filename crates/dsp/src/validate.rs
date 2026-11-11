use ronie_core::{DspResult, NamDocument};

use crate::compile_model;

/// Compiles and drops a candidate to enforce the complete local A2/container gate.
/// No runtime engine or audio preparation state is created.
///
/// # Errors
///
/// Returns the same shape, child, weight-count, and container errors as compilation.
pub fn validate_a2_document(document: &NamDocument) -> DspResult<()> {
    drop(compile_model(document)?);
    Ok(())
}

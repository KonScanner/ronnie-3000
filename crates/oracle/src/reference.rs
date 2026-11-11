use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{DifferentialCase, ModelOracle, OracleError, reference_paths};

static OUTPUT_ID: AtomicU64 = AtomicU64::new(0);

/// Development-only adapter for the operator-selected Reference executable.
pub struct ReferenceOracle {
    executable: PathBuf,
}

impl ReferenceOracle {
    /// Creates the private adapter from configured source and build paths.
    ///
    /// # Errors
    ///
    /// Returns an error when private configuration is missing or incomplete.
    pub fn new() -> Result<Self, OracleError> {
        let (_, build) = reference_paths()?.ok_or(OracleError::NotConfigured)?;
        Ok(Self {
            executable: build.join("ronie-reference"),
        })
    }
}

impl ModelOracle for ReferenceOracle {
    fn render(&mut self, case: &DifferentialCase) -> Result<Vec<f64>, OracleError> {
        let model_file = case
            .model_file
            .as_ref()
            .ok_or(OracleError::MissingModelFile)?;
        let id = OUTPUT_ID.fetch_add(1, Ordering::Relaxed);
        let output_file =
            std::env::temp_dir().join(format!("ronie-reference-{}-{id}.raw", std::process::id()));
        let output = Command::new(&self.executable)
            .arg(model_file)
            .arg(case.stream.name())
            .arg(case.block_size.to_string())
            .arg(case.frame_count.to_string())
            .arg(case.seed.to_string())
            .arg(&output_file)
            .output()?;
        if !output.status.success() {
            return Err(OracleError::ProcessFailed(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }

        let bytes = std::fs::read(&output_file)?;
        let _ = std::fs::remove_file(output_file);
        if bytes.len() != case.frame_count * size_of::<f64>() {
            return Err(OracleError::InvalidOutputLength);
        }
        Ok(bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| f64::from_le_bytes(*chunk))
            .collect())
    }
}

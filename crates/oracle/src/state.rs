use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{OracleError, fixtures::reference_paths_from};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const SUPPORTED_COMMANDS: [&str; 7] = [
    "header-encode",
    "header-decode",
    "body-encode",
    "body-decode",
    "local-load",
    "preset-name",
    "token-merge",
];

/// One binary input and operation for the private Reference state runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCommand {
    operation: String,
    input: Vec<u8>,
}

impl StateCommand {
    /// Creates a command. [`StateOracle::run`] rejects unknown operation names.
    #[must_use]
    pub fn new(operation: impl Into<String>, input: Vec<u8>) -> Self {
        Self {
            operation: operation.into(),
            input,
        }
    }

    /// Encodes a preset header with the Reference implementation.
    #[must_use]
    pub fn header_encode(input: Vec<u8>) -> Self {
        Self::new("header-encode", input)
    }

    /// Decodes a preset header with the Reference implementation.
    #[must_use]
    pub fn header_decode(input: Vec<u8>) -> Self {
        Self::new("header-decode", input)
    }

    /// Encodes a session or preset body with the Reference implementation.
    #[must_use]
    pub fn body_encode(input: Vec<u8>) -> Self {
        Self::new("body-encode", input)
    }

    /// Decodes a session or preset body with the Reference implementation.
    #[must_use]
    pub fn body_decode(input: Vec<u8>) -> Self {
        Self::new("body-decode", input)
    }

    /// Runs a private local-loading vector through the Reference implementation.
    #[must_use]
    pub fn local_load(input: Vec<u8>) -> Self {
        Self::new("local-load", input)
    }

    /// Runs a preset filename vector through the Reference implementation.
    #[must_use]
    pub fn preset_name(input: Vec<u8>) -> Self {
        Self::new("preset-name", input)
    }

    /// Runs a private token-merge vector without exposing token values in errors.
    #[must_use]
    pub fn token_merge(input: Vec<u8>) -> Self {
        Self::new("token-merge", input)
    }
}

/// Development-only process adapter for Reference state and loading probes.
pub struct StateOracle {
    executable: PathBuf,
    max_output_bytes: usize,
}

impl StateOracle {
    /// Creates an adapter from the two private Reference environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error when private configuration is absent or incomplete.
    pub fn new() -> Result<Self, OracleError> {
        Self::with_environment(|name| std::env::var_os(name))
    }

    pub(crate) fn with_environment(
        get: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, OracleError> {
        let (_, build) = reference_paths_from(get)?.ok_or(OracleError::NotConfigured)?;
        Ok(Self::from_executable(
            build.join("ronie-reference-state"),
            DEFAULT_MAX_OUTPUT_BYTES,
        ))
    }

    /// Creates an adapter for a selected runner and output bound.
    #[must_use]
    pub fn from_executable(executable: PathBuf, max_output_bytes: usize) -> Self {
        Self {
            executable,
            max_output_bytes,
        }
    }

    /// Runs one probe and returns its binary output.
    ///
    /// Standard error is discarded so credentials and private diagnostics cannot
    /// enter public evidence. Output size is checked before and after reading.
    ///
    /// # Errors
    ///
    /// Returns configuration, command, process, output-bound, or filesystem errors.
    pub fn run(&self, command: &StateCommand) -> Result<Vec<u8>, OracleError> {
        if !SUPPORTED_COMMANDS.contains(&command.operation.as_str()) {
            return Err(OracleError::UnsupportedStateCommand);
        }

        let scratch = tempfile::tempdir()?;
        let input_path = scratch.path().join("input.bin");
        let output_path = scratch.path().join("output.bin");
        fs::write(&input_path, &command.input)?;
        let status = Command::new(&self.executable)
            .arg(&command.operation)
            .arg(&input_path)
            .arg(&output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(OracleError::StateProcessFailed(status));
        }

        let output = read_bounded(&output_path, self.max_output_bytes)?;
        Ok(output)
    }
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, OracleError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > limit as u64 {
        return Err(OracleError::StateOutputTooLarge { limit });
    }
    let bytes = fs::read(path)?;
    if bytes.len() > limit {
        return Err(OracleError::StateOutputTooLarge { limit });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    #[cfg(unix)]
    use tempfile::TempDir;

    use super::{StateCommand, StateOracle};
    use crate::{OracleError, fixtures::FixtureError};

    #[cfg(unix)]
    fn fake_oracle(script: &str, max_output_bytes: usize) -> (TempDir, StateOracle) {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let executable = directory.path().join("fake-reference-state");
        fs::write(&executable, script).expect("fake executable should be written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&executable)
                .expect("fake executable metadata should be readable")
                .permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&executable, permissions)
                .expect("fake executable should be executable");
        }
        wait_until_executable(&executable);
        let oracle = StateOracle::from_executable(executable, max_output_bytes);
        (directory, oracle)
    }

    /// A parallel test can fork while this test holds the script open for writing. The child
    /// keeps that write handle until its own exec, so a run in that window fails with
    /// `ETXTBSY`. New forks cannot get the handle after the write, so one run that starts
    /// proves that later runs also start. The fake scripts have no effect with no arguments.
    #[cfg(unix)]
    fn wait_until_executable(executable: &std::path::Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match std::process::Command::new(executable)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                Err(error)
                    if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::yield_now();
                }
                Err(error) => panic!("fake executable should start: {error}"),
                Ok(_) => return,
            }
        }
    }

    #[test]
    fn state_missing_private_configuration_is_rejected() {
        let result = StateOracle::with_environment(|_| None);

        assert!(matches!(result, Err(OracleError::NotConfigured)));
    }

    #[test]
    fn state_incomplete_private_configuration_is_rejected() {
        let result = StateOracle::with_environment(|name| {
            (name == "RONIE_REFERENCE_ROOT").then_some(PathBuf::from("/private/reference").into())
        });

        assert!(matches!(
            result,
            Err(OracleError::Fixture(FixtureError::IncompleteConfiguration))
        ));
    }

    #[test]
    fn state_unsupported_command_is_rejected_before_process_start() {
        let oracle = StateOracle::from_executable(PathBuf::from("/not/a/real/runner"), 1024);
        let command = StateCommand::new("unsupported", Vec::new());

        let result = oracle.run(&command);

        assert!(matches!(result, Err(OracleError::UnsupportedStateCommand)));
    }

    // The fake program is a shell script.
    #[cfg(unix)]
    #[test]
    fn state_oversized_output_is_rejected() {
        let (_directory, oracle) = fake_oracle("#!/bin/sh\nhead -c 33 /dev/zero > \"$3\"\n", 32);

        let result = oracle.run(&StateCommand::preset_name(b"example".to_vec()));

        assert!(
            matches!(result, Err(OracleError::StateOutputTooLarge { limit: 32 })),
            "{result:?}"
        );
    }

    // The fake program is a shell script.
    #[cfg(unix)]
    #[test]
    fn state_failed_process_does_not_expose_stderr() {
        let (_directory, oracle) =
            fake_oracle("#!/bin/sh\necho super-secret-token >&2\nexit 7\n", 1024);

        let error = oracle
            .run(&StateCommand::token_merge(b"private-input".to_vec()))
            .expect_err("a nonzero Reference exit should fail");

        assert!(matches!(
            error,
            OracleError::StateProcessFailed(ref status) if status.code() == Some(7)
        ));
        assert!(!error.to_string().contains("super-secret-token"));
        assert!(!error.to_string().contains("private-input"));
    }

    // The fake program is a shell script.
    #[cfg(unix)]
    #[test]
    fn state_runner_returns_output_bytes() {
        let (_directory, oracle) = fake_oracle(
            "#!/bin/sh\n[ \"$1\" = preset-name ] || exit 9\ncat \"$2\" > \"$3\"\n",
            1024,
        );

        let output = oracle
            .run(&StateCommand::preset_name(b"example payload".to_vec()))
            .expect("the fake Reference runner should succeed");

        assert_eq!(output, b"example payload");
    }

    #[test]
    fn state_manifest_requires_full_reference_evidence_for_verification() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = fs::read_to_string(root.join("fixtures/loading-manifest.json"))
            .expect("loading manifest should exist");
        let rows: serde_json::Value =
            serde_json::from_str(&manifest).expect("loading manifest should be valid JSON");
        let rows = rows.as_array().expect("manifest root should be an array");
        let required = [
            "catalog-acquisition",
            "local-acquisition",
            "wav-validation",
            "local-nam-validation",
            "stash-naming",
            "stash-lifecycle",
            "http-bearer",
            "http-anonymous-fallback",
            "http-timeouts-retries",
            "oauth-pkce",
            "oauth-refresh",
            "token-merge",
            "settings-merge",
            "juce-value-codec",
            "juce-tree-codec",
            "session-state",
            "legacy-migration",
            "offline-restoration",
            "preset-header",
            "preset-body",
            "preset-name",
            "preset-atomic-write",
            "preset-order",
            "host-program-list",
        ];
        for behavior in required {
            let row = rows
                .iter()
                .find(|row| row["behavior"] == behavior)
                .unwrap_or_else(|| panic!("missing manifest behavior {behavior}"));
            for field in [
                "owner",
                "reference",
                "fixture_sha256",
                "expected",
                "tolerance",
            ] {
                assert!(
                    row[field].as_str().is_some_and(|value| !value.is_empty()),
                    "{behavior} needs a nonempty {field}"
                );
            }
            assert_eq!(
                row["status"], "unresolved",
                "{behavior} still needs complete independent evidence"
            );
        }
    }
}

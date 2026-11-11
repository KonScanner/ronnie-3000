use std::{env, ffi::OsString, path::PathBuf};
use thiserror::Error;

const REFERENCE_ROOT: &str = "RONIE_REFERENCE_ROOT";
const REFERENCE_BUILD: &str = "RONIE_REFERENCE_BUILD";

/// Error returned when private Reference configuration is incomplete.
#[derive(Debug, Error)]
pub enum FixtureError {
    /// Exactly one of the two required environment variables was set.
    #[error("set both RONIE_REFERENCE_ROOT and RONIE_REFERENCE_BUILD, or neither")]
    IncompleteConfiguration,
}

/// Returns the private Reference source and build paths when both are configured.
///
/// This function reads only environment variables; it does not read files.
///
/// # Errors
///
/// Returns [`FixtureError::IncompleteConfiguration`] when exactly one path is set.
pub fn reference_paths() -> Result<Option<(PathBuf, PathBuf)>, FixtureError> {
    reference_paths_from(|name| env::var_os(name))
}

pub(crate) fn reference_paths_from(
    get: impl Fn(&str) -> Option<OsString>,
) -> Result<Option<(PathBuf, PathBuf)>, FixtureError> {
    match (get(REFERENCE_ROOT), get(REFERENCE_BUILD)) {
        (None, None) => Ok(None),
        (Some(root), Some(build)) => Ok(Some((PathBuf::from(root), PathBuf::from(build)))),
        _ => Err(FixtureError::IncompleteConfiguration),
    }
}

#[cfg(test)]
mod tests {
    use super::{FixtureError, reference_paths_from};
    use std::path::PathBuf;

    #[test]
    fn reference_paths_missing_environment_returns_none() {
        let result = reference_paths_from(|_| None);

        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn reference_paths_incomplete_environment_is_an_error() {
        let root_only = reference_paths_from(|name| {
            (name == "RONIE_REFERENCE_ROOT").then_some("/private/reference".into())
        });
        let build_only = reference_paths_from(|name| {
            (name == "RONIE_REFERENCE_BUILD").then_some("/private/build".into())
        });

        assert!(matches!(
            root_only,
            Err(FixtureError::IncompleteConfiguration)
        ));
        assert!(matches!(
            build_only,
            Err(FixtureError::IncompleteConfiguration)
        ));
    }

    #[test]
    fn reference_paths_complete_environment_returns_both_paths() {
        let result = reference_paths_from(|name| match name {
            "RONIE_REFERENCE_ROOT" => Some("/private/reference".into()),
            "RONIE_REFERENCE_BUILD" => Some("/private/build".into()),
            _ => None,
        });

        let (root, build) = result
            .expect("complete configuration should succeed")
            .expect("complete configuration should contain both paths");
        assert_eq!(root, PathBuf::from("/private/reference"));
        assert_eq!(build, PathBuf::from("/private/build"));
    }
}

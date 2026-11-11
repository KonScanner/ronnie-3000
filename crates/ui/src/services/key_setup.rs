//! The seam for the publishable key of the user.
//!
//! A build with no built-in key starts with no community service. When the host supplies a
//! [`KeySetup`], the sign-in controls open the key page instead of nothing. The user pastes a
//! key on that page, and the host saves it and builds the community service of the new
//! configuration. The controller then attaches that service with no restart.
//!
//! The key is a secret of the user. No error, log line, or view of this module holds the value.

use ronie_core::HttpTransport;

use super::{
    community::{CommunityAccess, CommunityService},
    community_worker::CommunityWorker,
    session::AuthFlowAdapter,
};

/// The start of each publishable key of the service.
pub const PUBLISHABLE_KEY_PREFIX: &str = "t3k_pub_";

/// Whether `value` has the form of a publishable key: the prefix, then one or more ASCII
/// letters, digits, `_`, or `-`.
#[must_use]
pub fn is_publishable_key(value: &str) -> bool {
    value
        .strip_prefix(PUBLISHABLE_KEY_PREFIX)
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

/// The two sides of one community service, with no transport in their type.
pub struct CommunityParts {
    /// The sign-in flow of the service.
    pub auth: Box<dyn AuthFlowAdapter>,
    /// The catalog, download, and token side of the service.
    pub access: Box<dyn CommunityAccess>,
}

impl<T: HttpTransport + Send + Sync + 'static> From<CommunityService<T>> for CommunityParts {
    fn from(service: CommunityService<T>) -> Self {
        let (auth, handle) = service.into_parts();
        Self {
            auth: Box::new(auth),
            access: Box::new(CommunityWorker::new(handle)),
        }
    }
}

/// Why a key change failed. The text never holds the key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeySetupError {
    /// The value does not have the form of a publishable key.
    #[error("This is not a publishable key. A publishable key starts with t3k_pub_.")]
    NotAKey,
    /// The key could not be saved or removed, or the service did not start.
    #[error("{0}")]
    Failed(String),
}

/// What a host does with the publishable key of the user.
///
/// Each method can read or write a file, so a frame must not call one.
pub trait KeySetup: Send {
    /// The page of the service that "Open TONE3000" opens.
    fn origin(&self) -> String;
    /// Whether the user saved a key.
    fn has_saved_key(&self) -> bool;
    /// Saves a key and builds the service of the new configuration.
    ///
    /// # Errors
    /// Returns [`KeySetupError::NotAKey`] and saves nothing when `key` is not a publishable
    /// key. Returns [`KeySetupError::Failed`] when the save or the service start fails.
    fn save(&mut self, key: &str) -> Result<Option<CommunityParts>, KeySetupError>;
    /// Removes the saved key and builds the service of the configuration that stays.
    ///
    /// # Errors
    /// Returns [`KeySetupError::Failed`] when the removal or the service start fails.
    fn remove(&mut self) -> Result<Option<CommunityParts>, KeySetupError>;
}

#[cfg(test)]
mod tests {
    use super::is_publishable_key;

    #[test]
    fn a_publishable_key_has_the_prefix_and_then_safe_characters() {
        for key in ["t3k_pub_a", "t3k_pub_test_fake", "t3k_pub_A-b_9"] {
            assert!(is_publishable_key(key), "{key}");
        }
        for value in [
            "",
            "t3k_pub_",
            "t3k_pub",
            "T3K_PUB_abc",
            "t3k_sec_abc",
            " t3k_pub_abc",
            "t3k_pub_abc ",
            "t3k_pub_a b",
            "t3k_pub_a.b",
            "t3k_pub_a/b",
            "t3k_pub_ä",
            "x t3k_pub_abc",
        ] {
            assert!(!is_publishable_key(value), "{value:?}");
        }
    }
}

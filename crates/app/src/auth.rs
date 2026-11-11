//! Production community construction with persistent session restoration.
//!
//! A launch takes its service values from the first source that has them:
//!
//! 1. the environment pair at launch (`RONIE_OAUTH_ORIGIN` and `RONIE_OAUTH_CLIENT_ID`),
//! 2. the key that the user saved in the app, with [`DEFAULT_ORIGIN`],
//! 3. the pair that the build holds,
//! 4. none, and then the app has no community service.
//!
//! With [`DEFAULT_ORIGIN`] and no `RONIE_MODEL_ORIGINS`, the public hosts in
//! [`DEFAULT_PUBLIC_ORIGINS`] can serve models and images.

use std::path::{Path, PathBuf};

use ronie_core::{
    OAuthConfig, OriginPolicy, ReqwestTransport, SettingsStore, UI_PREFERENCES_SETTINGS,
};
use ronie_ui::{
    backend::UiError,
    services::{
        community::CommunityService,
        key_setup::{CommunityParts, KeySetup, KeySetupError, is_publishable_key},
    },
};
use url::Url;

/// The origin of the service that a saved key uses.
pub const DEFAULT_ORIGIN: &str = "https://www.tone3000.com";
/// The public hosts of the default service for model files and images, in the form of
/// `RONIE_MODEL_ORIGINS`. They never receive a token.
pub const DEFAULT_PUBLIC_ORIGINS: &str = "https://assets.tone3000.com,https://files.tone3000.com";
/// The settings entry that holds the key of the user.
const USER_KEY_ENTRY: &str = "t3k_publishable_key";

/// The values in the environment of the build. `make build` takes them from `.env`. They are not
/// in the repository. A publishable key is safe in a client application.
const BUILT_IN: ServiceValues<'static> = ServiceValues {
    origin: option_env!("RONIE_OAUTH_ORIGIN"),
    client_id: option_env!("RONIE_OAUTH_CLIENT_ID"),
    model_origins: option_env!("RONIE_MODEL_ORIGINS"),
};

/// Builds the configured production service from the environment, or from the values that the
/// build holds.
///
/// # Errors
/// Returns invalid configuration or HTTP transport errors. An invalid value stops the start, and
/// the error never holds the value.
pub fn configured_community(
    root: &Path,
) -> Result<Option<CommunityService<ReqwestTransport>>, UiError> {
    let origin = environment("RONIE_OAUTH_ORIGIN")?;
    let client_id = environment("RONIE_OAUTH_CLIENT_ID")?;
    let model_origins = environment("RONIE_MODEL_ORIGINS")?;
    configured_community_with_launch(
        root,
        origin.as_deref(),
        client_id.as_deref(),
        model_origins.as_deref(),
    )
}

/// Builds the service from launch values, the saved key under `root`, and the values that the
/// build holds, in that order.
///
/// A saved key that cannot be read, or that is not a publishable key, counts as no key, so
/// that the user can save a new key in the app.
///
/// # Errors
/// Returns invalid configuration or HTTP transport errors. The error never holds a key.
pub fn configured_community_with_launch(
    root: &Path,
    origin: Option<&str>,
    client_id: Option<&str>,
    model_origins: Option<&str>,
) -> Result<Option<CommunityService<ReqwestTransport>>, UiError> {
    let saved = UserKeyStore::new(root).read().ok().flatten();
    let chosen = ServiceValues {
        origin,
        client_id,
        model_origins,
    }
    .choose(saved.as_deref(), BUILT_IN);
    configured_community_with_values(root, chosen.origin, chosen.client_id, chosen.model_origins)
}

/// The publishable key that the user saved in the app.
///
/// The key goes to the UI settings file, next to the tokens. Each write replaces that file
/// with a new file that only the owner can read, so the key gets the same protection as the
/// tokens.
#[derive(Debug, Clone)]
pub struct UserKeyStore {
    settings: SettingsStore,
}

impl UserKeyStore {
    /// Selects the UI settings file under the standalone settings directory `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            settings: SettingsStore::standalone(root, UI_PREFERENCES_SETTINGS),
        }
    }

    /// The saved key. A saved value that is not a publishable key counts as no key.
    ///
    /// # Errors
    /// Returns a read error. The error never holds the key.
    pub fn read(&self) -> Result<Option<String>, UiError> {
        let properties = self
            .settings
            .read()
            .map_err(|error| UiError::Control(format!("reading the saved key: {error}")))?;
        Ok(properties
            .get(USER_KEY_ENTRY)
            .filter(|key| is_publishable_key(key))
            .map(str::to_owned))
    }

    /// Saves `key` in place of an earlier key.
    ///
    /// # Errors
    /// Returns an error and saves nothing when `key` is not a publishable key. Returns a write
    /// error. No error holds the key.
    pub fn save(&self, key: &str) -> Result<(), UiError> {
        if !is_publishable_key(key) {
            return Err(UiError::Control(KeySetupError::NotAKey.to_string()));
        }
        self.settings
            .merge_key(USER_KEY_ENTRY, key)
            .map_err(|error| UiError::Control(format!("saving the key: {error}")))
    }

    /// Removes the saved key. This does nothing when no key is saved.
    ///
    /// # Errors
    /// Returns a write error.
    pub fn remove(&self) -> Result<(), UiError> {
        self.settings
            .remove_key(USER_KEY_ENTRY)
            .map_err(|error| UiError::Control(format!("removing the saved key: {error}")))
    }
}

/// The key setup of the standalone app. It saves the key under the settings directory and
/// builds the service of the new configuration, with the same rules as a launch.
#[derive(Debug, Clone)]
pub struct UserKeySetup {
    root: PathBuf,
    store: UserKeyStore,
}

impl UserKeySetup {
    /// Uses the standalone settings directory `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            store: UserKeyStore::new(root),
        }
    }

    fn service(&self) -> Result<Option<CommunityParts>, KeySetupError> {
        configured_community(&self.root)
            .map(|service| service.map(CommunityParts::from))
            .map_err(|error| KeySetupError::Failed(error.to_string()))
    }
}

impl KeySetup for UserKeySetup {
    fn origin(&self) -> String {
        DEFAULT_ORIGIN.to_owned()
    }

    fn has_saved_key(&self) -> bool {
        matches!(self.store.read(), Ok(Some(_)))
    }

    fn save(&mut self, key: &str) -> Result<Option<CommunityParts>, KeySetupError> {
        if !is_publishable_key(key) {
            return Err(KeySetupError::NotAKey);
        }
        self.store
            .save(key)
            .map_err(|error| KeySetupError::Failed(error.to_string()))?;
        self.service()
    }

    fn remove(&mut self) -> Result<Option<CommunityParts>, KeySetupError> {
        self.store
            .remove()
            .map_err(|error| KeySetupError::Failed(error.to_string()))?;
        self.service()
    }
}

/// Builds a production service from explicit values, or returns `None` when the origin and the
/// client ID are both absent.
///
/// The model-origin list is optional, and it does not need the other two values. This function
/// validates the list first, so an invalid list is an error in each configuration.
///
/// # Errors
/// Returns an error for incomplete, insecure, or invalid configuration.
pub fn configured_community_with_values(
    root: &Path,
    origin: Option<&str>,
    client_id: Option<&str>,
    model_origins: Option<&str>,
) -> Result<Option<CommunityService<ReqwestTransport>>, UiError> {
    let model_origins = parse_model_origins(model_origins)?;
    let (Some(origin), Some(client_id)) = (origin, client_id) else {
        if origin.is_none() && client_id.is_none() {
            return Ok(None);
        }
        return Err(UiError::Control(
            "RONIE_OAUTH_ORIGIN and RONIE_OAUTH_CLIENT_ID must be configured together".to_owned(),
        ));
    };
    if client_id.trim().is_empty() {
        return Err(UiError::Control(
            "RONIE_OAUTH_CLIENT_ID must not be empty".to_owned(),
        ));
    }
    let origin = Url::parse(origin)
        .map_err(|_| UiError::Control("RONIE_OAUTH_ORIGIN is not a valid URL".to_owned()))?;
    if origin.scheme() != "https"
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(UiError::Control(
            "RONIE_OAUTH_ORIGIN must be an HTTPS origin without credentials, query, or fragment"
                .to_owned(),
        ));
    }
    let policy = OriginPolicy::new(origin.clone(), model_origins);
    let transport = ReqwestTransport::new(policy.clone())
        .map_err(|error| UiError::Control(format!("creating community transport: {error}")))?;
    Ok(Some(CommunityService::new(
        OAuthConfig::new(origin, client_id),
        policy,
        transport,
        SettingsStore::standalone(root, UI_PREFERENCES_SETTINGS),
    )))
}

/// Parses the comma-separated list. An absent or blank value is an empty list.
fn parse_model_origins(value: Option<&str>) -> Result<Vec<Url>, UiError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    value
        .split(',')
        .enumerate()
        .map(|(index, entry)| {
            parse_model_origin(entry.trim()).ok_or_else(|| {
                // The value can hold a credential, so the error names only its position.
                UiError::Control(format!(
                    "RONIE_MODEL_ORIGINS entry {} must be an HTTPS origin without credentials, \
                     path, query, or fragment",
                    index + 1
                ))
            })
        })
        .collect()
}

fn parse_model_origin(entry: &str) -> Option<Url> {
    let url = Url::parse(entry).ok()?;
    let origin_only = url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    origin_only.then_some(url)
}

fn environment(name: &str) -> Result<Option<String>, UiError> {
    std::env::var_os(name)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| UiError::Control(format!("{name} must contain valid UTF-8")))
        })
        .transpose()
}

/// The service values that one source gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct ServiceValues<'a> {
    origin: Option<&'a str>,
    client_id: Option<&'a str>,
    model_origins: Option<&'a str>,
}

impl<'a> ServiceValues<'a> {
    /// Chooses what a launch uses: these launch values, then the saved key with the default
    /// origin, then the values of the build. The default origin then gets its public hosts,
    /// unless a model-origin list exists.
    fn choose(self, saved_key: Option<&'a str>, built_in: Self) -> Self {
        let chosen = if self.origin.is_some() || self.client_id.is_some() {
            self
        } else if let Some(key) = saved_key {
            Self {
                origin: Some(DEFAULT_ORIGIN),
                client_id: Some(key),
                model_origins: self.model_origins,
            }
        } else {
            self.over_built_in(built_in)
        };
        chosen.with_default_public_origins()
    }

    /// Adds the public hosts of the default service when the origin is the default origin and
    /// no model-origin list exists.
    fn with_default_public_origins(self) -> Self {
        let default_origin = self
            .origin
            .is_some_and(|origin| origin.trim_end_matches('/') == DEFAULT_ORIGIN);
        if default_origin && self.model_origins.is_none() {
            return Self {
                model_origins: Some(DEFAULT_PUBLIC_ORIGINS),
                ..self
            };
        }
        self
    }

    /// Chooses what a launch uses.
    ///
    /// A launch value for the origin or the client ID replaces both built-in values, so the
    /// built-in key never goes to an origin that somebody set by hand.
    fn over_built_in(self, built_in: Self) -> Self {
        if self.origin.is_some() || self.client_id.is_some() {
            return self;
        }
        Self {
            origin: built_in.origin,
            client_id: built_in.client_id,
            model_origins: self.model_origins.or(built_in.model_origins),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_ORIGIN, DEFAULT_PUBLIC_ORIGINS, ServiceValues};

    const SAVED_KEY: &str = "t3k_pub_test_fake";

    #[test]
    fn the_launch_pair_wins_over_a_saved_key() {
        let launch = ServiceValues {
            origin: Some("https://other.example"),
            client_id: Some("other-key"),
            model_origins: None,
        };

        assert_eq!(launch.choose(Some(SAVED_KEY), BUILT_IN), launch);
    }

    #[test]
    fn a_launch_origin_alone_never_receives_the_saved_key() {
        let launch = ServiceValues {
            origin: Some("https://other.example"),
            ..ServiceValues::default()
        };

        assert_eq!(launch.choose(Some(SAVED_KEY), BUILT_IN), launch);
    }

    #[test]
    fn a_saved_key_wins_over_the_built_in_pair_and_uses_the_default_service() {
        let chosen = ServiceValues::default().choose(Some(SAVED_KEY), BUILT_IN);

        assert_eq!(
            chosen,
            ServiceValues {
                origin: Some(DEFAULT_ORIGIN),
                client_id: Some(SAVED_KEY),
                model_origins: Some(DEFAULT_PUBLIC_ORIGINS),
            }
        );
    }

    #[test]
    fn launch_model_origins_replace_the_default_public_hosts() {
        let launch = ServiceValues {
            model_origins: Some("https://other-models.example"),
            ..ServiceValues::default()
        };

        let chosen = launch.choose(Some(SAVED_KEY), BUILT_IN);

        assert_eq!(chosen.client_id, Some(SAVED_KEY));
        assert_eq!(chosen.model_origins, launch.model_origins);
    }

    #[test]
    fn with_no_saved_key_the_built_in_pair_applies() {
        assert_eq!(ServiceValues::default().choose(None, BUILT_IN), BUILT_IN);
    }

    #[test]
    fn with_no_source_there_is_no_service() {
        assert_eq!(
            ServiceValues::default().choose(None, ServiceValues::default()),
            ServiceValues::default()
        );
    }

    #[test]
    fn the_default_origin_gets_its_public_hosts_only_without_a_list() {
        for origin in [DEFAULT_ORIGIN, "https://www.tone3000.com/"] {
            let launch = ServiceValues {
                origin: Some(origin),
                client_id: Some("launch-key"),
                model_origins: None,
            };
            assert_eq!(
                launch.choose(None, BUILT_IN).model_origins,
                Some(DEFAULT_PUBLIC_ORIGINS),
                "{origin}"
            );
        }
        let listed = ServiceValues {
            origin: Some(DEFAULT_ORIGIN),
            client_id: Some("launch-key"),
            model_origins: Some(""),
        };
        assert_eq!(listed.choose(None, BUILT_IN), listed);
        let other = ServiceValues {
            origin: Some("https://other.example"),
            client_id: Some("launch-key"),
            model_origins: None,
        };
        assert_eq!(other.choose(None, BUILT_IN), other);
    }

    const BUILT_IN: ServiceValues<'static> = ServiceValues {
        origin: Some("https://built.example"),
        client_id: Some("built-key"),
        model_origins: Some("https://built-models.example"),
    };

    #[test]
    fn a_launch_with_no_values_uses_the_built_in_values() {
        assert_eq!(ServiceValues::default().over_built_in(BUILT_IN), BUILT_IN);
    }

    #[test]
    fn a_launch_origin_never_receives_the_built_in_key() {
        let launch = ServiceValues {
            origin: Some("https://other.example"),
            ..ServiceValues::default()
        };

        let chosen = launch.over_built_in(BUILT_IN);

        assert_eq!(chosen, launch, "the missing key is an error at launch");
    }

    #[test]
    fn a_launch_key_does_not_join_the_built_in_origin() {
        let launch = ServiceValues {
            client_id: Some("other-key"),
            ..ServiceValues::default()
        };

        assert_eq!(launch.over_built_in(BUILT_IN), launch);
    }

    #[test]
    fn a_full_launch_pair_replaces_the_built_in_model_origins() {
        let launch = ServiceValues {
            origin: Some("https://other.example"),
            client_id: Some("other-key"),
            model_origins: None,
        };

        assert_eq!(launch.over_built_in(BUILT_IN), launch);
    }

    #[test]
    fn launch_model_origins_join_the_built_in_service() {
        let launch = ServiceValues {
            model_origins: Some("https://other-models.example"),
            ..ServiceValues::default()
        };

        let chosen = launch.over_built_in(BUILT_IN);

        assert_eq!(chosen.origin, BUILT_IN.origin);
        assert_eq!(chosen.client_id, BUILT_IN.client_id);
        assert_eq!(chosen.model_origins, launch.model_origins);
    }

    #[test]
    fn a_build_with_no_values_has_no_service() {
        assert_eq!(
            ServiceValues::default().over_built_in(ServiceValues::default()),
            ServiceValues::default()
        );
    }
}

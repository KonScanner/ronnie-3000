use std::sync::Arc;

use secrecy::SecretString;
use serde_json::Value;

use crate::{
    ApiError, HttpTransport, OAuthError, OAuthFlow, OAuthTokens, SessionError, SettingsStore,
    TokenProvider,
};

const TOKENS_KEY: &str = "t3k_tokens";
const CACHED_USER_KEY: &str = "t3k.cachedUser";
const REFRESH_LEAD_MS: i64 = 60_000;

/// Refresh adapter shared by OAuth and deterministic test transports.
pub trait TokenRefresher {
    /// Rotates one refresh token at a known current time.
    fn refresh(
        &self,
        refresh: SecretString,
        now_ms: i64,
    ) -> impl Future<Output = Result<OAuthTokens, OAuthError>> + Send;
}

impl<T: HttpTransport + Send + Sync> TokenRefresher for OAuthFlow<T> {
    async fn refresh(&self, refresh: SecretString, now_ms: i64) -> Result<OAuthTokens, OAuthError> {
        self.refresh_token(&refresh, now_ms).await
    }
}

/// Session persistence with single-flight refresh and cross-process token adoption.
pub struct SessionStore<R: TokenRefresher> {
    settings: SettingsStore,
    refresher: Arc<R>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
}

impl<R: TokenRefresher> Clone for SessionStore<R> {
    fn clone(&self) -> Self {
        Self {
            settings: self.settings.clone(),
            refresher: Arc::clone(&self.refresher),
            refresh_lock: Arc::clone(&self.refresh_lock),
        }
    }
}

impl<R: TokenRefresher> SessionStore<R> {
    /// Creates a session over shared settings and a refresh adapter.
    #[must_use]
    pub fn new(settings: SettingsStore, refresher: R) -> Self {
        Self {
            settings,
            refresher: Arc::new(refresher),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Returns the shared refresh adapter for lifecycle observation.
    #[must_use]
    pub fn refresher(&self) -> Arc<R> {
        Arc::clone(&self.refresher)
    }

    /// Persists one token pair through the atomic settings merge.
    ///
    /// # Errors
    ///
    /// Returns settings serialization, lock, or write errors.
    pub fn set_tokens(&self, tokens: OAuthTokens) -> Result<(), SessionError> {
        self.settings
            .merge_key(TOKENS_KEY, &tokens.to_json().to_string())?;
        Ok(())
    }

    /// Reads the currently stored token pair, treating invalid/missing data as signed out.
    ///
    /// # Errors
    ///
    /// Returns settings I/O or malformed-XML errors.
    pub fn read_tokens(&self) -> Result<Option<OAuthTokens>, SessionError> {
        let properties = self.settings.read()?;
        let Some(raw) = properties.get(TOKENS_KEY) else {
            return Ok(None);
        };
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return Ok(None);
        };
        Ok(OAuthTokens::from_json(&value, 0).ok())
    }

    /// Returns a fresh token, rotating at most once across concurrent callers.
    /// A newer token written by another process is adopted before and after refresh.
    ///
    /// # Errors
    ///
    /// Returns settings errors, [`SessionError::Offline`] after the Reference
    /// refresh-failure rules, or an OAuth error for an adopted-refresh failure.
    pub async fn access_token_at(&self, now_ms: i64) -> Result<SecretString, SessionError> {
        self.access_token_inner(now_ms, TokenNeed::Current).await
    }

    /// Forces one refresh after an upstream authentication rejection.
    ///
    /// # Errors
    ///
    /// Returns settings, refresh, or offline errors using the normal session rules.
    pub async fn refresh_access_token_at(&self, now_ms: i64) -> Result<SecretString, SessionError> {
        self.access_token_inner(now_ms, TokenNeed::Forced).await
    }

    /// Replaces an access token that upstream rejected.
    ///
    /// A fresh stored token that differs from `rejected` came from another caller or another
    /// process after the rejected request started, so callers rejected at the same time share
    /// one refresh.
    ///
    /// # Errors
    ///
    /// Returns settings, refresh, or offline errors using the normal session rules.
    pub async fn replace_rejected_token_at(
        &self,
        rejected: &SecretString,
        now_ms: i64,
    ) -> Result<SecretString, SessionError> {
        self.access_token_inner(now_ms, TokenNeed::Replacing(rejected))
            .await
    }

    async fn access_token_inner(
        &self,
        now_ms: i64,
        need: TokenNeed<'_>,
    ) -> Result<SecretString, SessionError> {
        let _guard = self.refresh_lock.lock().await;
        let properties = self.settings.read()?;
        let stored_raw = properties.get(TOKENS_KEY).map(str::to_owned);
        let stored = stored_raw.as_deref().and_then(|raw| {
            serde_json::from_str::<Value>(raw)
                .ok()
                .and_then(|value| OAuthTokens::from_json(&value, 0).ok())
        });
        if let Some(tokens) = stored
            .as_ref()
            .filter(|tokens| need.accepts(tokens, now_ms))
        {
            return Ok(tokens.access.clone());
        }
        let attempted = stored
            .map(|tokens| tokens.refresh)
            .ok_or(SessionError::Offline)?;
        match self.refresher.refresh(attempted.clone(), now_ms).await {
            Ok(tokens) => {
                let access = tokens.access.clone();
                let serialized = tokens.to_json().to_string();
                self.settings.compare_and_swap_key(
                    TOKENS_KEY,
                    stored_raw.as_deref(),
                    Some(&serialized),
                )?;
                Ok(access)
            }
            Err(error) => {
                if let Some(tokens) = self.read_tokens()?.filter(|tokens| {
                    use secrecy::ExposeSecret;
                    tokens.refresh.expose_secret() != attempted.expose_secret()
                        && fresh(tokens, now_ms)
                }) {
                    return Ok(tokens.access);
                }
                if error == OAuthError::RefreshFailed {
                    self.settings
                        .compare_and_swap_key(TOKENS_KEY, stored_raw.as_deref(), None)?;
                    Err(SessionError::Offline)
                } else {
                    Err(SessionError::OAuth(error))
                }
            }
        }
    }

    /// Stores non-secret cached profile data for offline display.
    ///
    /// # Errors
    ///
    /// Returns settings serialization, lock, or write errors.
    pub fn set_cached_user(&self, user: Value) -> Result<(), SessionError> {
        self.settings
            .merge_key(CACHED_USER_KEY, &user.to_string())?;
        Ok(())
    }

    /// Reads cached profile data even when tokens are unavailable or rejected.
    ///
    /// # Errors
    ///
    /// Returns settings I/O or malformed-XML errors.
    pub fn cached_user(&self) -> Result<Value, SessionError> {
        let properties = self.settings.read()?;
        let value = properties.get(CACHED_USER_KEY).map_or(Value::Null, |raw| {
            serde_json::from_str(raw).unwrap_or(Value::Null)
        });
        Ok(value)
    }

    /// Explicitly removes tokens only; cached profile data remains available offline.
    ///
    /// # Errors
    ///
    /// Returns lock, read, or atomic-write errors.
    pub fn logout(&self) -> Result<(), SessionError> {
        self.settings.remove_key(TOKENS_KEY)?;
        Ok(())
    }
}

/// The one token store is the token provider for the catalog and the downloads.
///
/// [`SessionError::Offline`] means that no usable session remains. The provider reports this as
/// no token, so that a caller can use a public model or ask the user to sign in. Other session
/// errors stay errors, because a timeout or a server failure does not sign the user out.
impl<R: TokenRefresher + Send + Sync> TokenProvider for SessionStore<R> {
    async fn access_token(&self, force_refresh: bool) -> Result<Option<SecretString>, ApiError> {
        let now_ms = current_time_ms();
        let result = if force_refresh {
            self.refresh_access_token_at(now_ms).await
        } else {
            self.access_token_at(now_ms).await
        };
        provided_token(result)
    }

    async fn refreshed_token(
        &self,
        rejected: &SecretString,
    ) -> Result<Option<SecretString>, ApiError> {
        provided_token(
            self.replace_rejected_token_at(rejected, current_time_ms())
                .await,
        )
    }
}

fn provided_token(
    result: Result<SecretString, SessionError>,
) -> Result<Option<SecretString>, ApiError> {
    match result {
        Ok(token) => Ok(Some(token)),
        Err(SessionError::Offline) => Ok(None),
        Err(error) => Err(ApiError::Session(error)),
    }
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// What one caller needs from the store.
#[derive(Clone, Copy)]
enum TokenNeed<'a> {
    /// Any fresh token.
    Current,
    /// A new token, whatever the store holds.
    Forced,
    /// A fresh token that is not the rejected one.
    Replacing(&'a SecretString),
}

impl TokenNeed<'_> {
    fn accepts(self, tokens: &OAuthTokens, now_ms: i64) -> bool {
        use secrecy::ExposeSecret;
        match self {
            Self::Current => fresh(tokens, now_ms),
            Self::Forced => false,
            Self::Replacing(rejected) => {
                fresh(tokens, now_ms) && tokens.access.expose_secret() != rejected.expose_secret()
            }
        }
    }
}

fn fresh(tokens: &OAuthTokens, now_ms: i64) -> bool {
    now_ms <= tokens.expires_at_ms.saturating_sub(REFRESH_LEAD_MS)
}

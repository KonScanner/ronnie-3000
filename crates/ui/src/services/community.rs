//! One construction for sign-in, the catalog, and the downloads.
//!
//! [`CommunityService`] builds one [`OAuthFlow`] and one [`SessionStore`] over one transport.
//! [`CommunityService::into_parts`] gives the sign-in side to the session seam and gives the
//! other side to background workers, so both sides read and rotate the same tokens.
//!
//! A construction starts no work and needs no runtime. The handle is cheap to clone.
//!
//! [`CommunityAccess`] is the seam for a controller, which has no transport in its type. The
//! seam takes the work as a value, so a new kind of work does not change the seam.

use std::sync::Arc;

use ronie_core::{
    ApiConfig, CatalogClient, DownloadClient, HttpTransport, OAuthConfig, OAuthFlow, OriginPolicy,
    SessionStore, SettingsStore,
};

use tokio::runtime::Handle;

use super::{
    auth::AuthService,
    community_worker::{CommunityEvent, CommunityScope, CommunityWork},
    images::ImageFetcher,
};

/// A configured community service before its two sides separate.
pub struct CommunityService<T: HttpTransport + Send + Sync + 'static> {
    auth: AuthService<T>,
    handle: CommunityHandle<T>,
}

impl<T: HttpTransport + Clone + Send + Sync + 'static> CommunityService<T> {
    /// Creates one flow and one token store over `transport` and `settings`.
    ///
    /// The clones of `transport` must share their state, and `policy` must be the policy of
    /// the transport.
    #[must_use]
    pub fn new(
        config: OAuthConfig,
        policy: OriginPolicy,
        transport: T,
        settings: SettingsStore,
    ) -> Self {
        let api = ApiConfig::new(config.origin.clone(), config.client_id.clone());
        let flow = OAuthFlow::new(config, transport.clone());
        let tokens = SessionStore::new(settings, flow.clone());
        Self {
            auth: AuthService::with_shared_session(flow, tokens.clone()),
            handle: CommunityHandle {
                catalog: CatalogClient::new(api, transport.clone()),
                downloads: DownloadClient::new(transport, policy),
                tokens,
            },
        }
    }
}

impl<T: HttpTransport + Send + Sync + 'static> CommunityService<T> {
    /// Separates the sign-in side from the side that background workers use.
    #[must_use]
    pub fn into_parts(self) -> (AuthService<T>, CommunityHandle<T>) {
        (self.auth, self.handle)
    }
}

/// The catalog, the downloads, and the shared token store for background workers.
pub struct CommunityHandle<T: HttpTransport + Send + Sync + 'static> {
    catalog: CatalogClient<T>,
    downloads: DownloadClient<T>,
    tokens: SessionStore<OAuthFlow<T>>,
}

impl<T: HttpTransport + Send + Sync + 'static> Clone for CommunityHandle<T> {
    fn clone(&self) -> Self {
        Self {
            catalog: self.catalog.clone(),
            downloads: self.downloads.clone(),
            tokens: self.tokens.clone(),
        }
    }
}

impl<T: HttpTransport + Send + Sync + 'static> CommunityHandle<T> {
    /// The catalog client.
    #[must_use]
    pub const fn catalog(&self) -> &CatalogClient<T> {
        &self.catalog
    }

    /// The model and image downloader.
    #[must_use]
    pub const fn downloads(&self) -> &DownloadClient<T> {
        &self.downloads
    }

    /// The token store that the sign-in side also uses. This is the token provider.
    #[must_use]
    pub const fn tokens(&self) -> &SessionStore<OAuthFlow<T>> {
        &self.tokens
    }

    /// Whether the token store holds tokens now. A failed read counts as no tokens.
    ///
    /// This reads a file, so a frame must not call it.
    #[must_use]
    pub fn has_stored_tokens(&self) -> bool {
        matches!(self.tokens.read_tokens(), Ok(Some(_)))
    }
}

/// What a controller holds of a community service.
pub trait CommunityAccess: Send {
    /// Whether the one token store holds tokens now. A failed read counts as no tokens.
    ///
    /// This reads a file, so a frame must not call it.
    fn has_stored_tokens(&self) -> bool;
    /// Starts one piece of work on `runtime` without blocking the caller.
    fn start(&mut self, runtime: &Handle, work: CommunityWork);
    /// Stops the delivery of the work in `scope`.
    fn cancel(&mut self, scope: CommunityScope);
    /// Takes the finished work without blocking the caller.
    fn poll(&mut self) -> Vec<CommunityEvent>;
    /// Creates the fetcher of the image workers. It uses the download client of this service and
    /// no token.
    fn image_fetcher(&self, runtime: Handle) -> Arc<dyn ImageFetcher>;
}

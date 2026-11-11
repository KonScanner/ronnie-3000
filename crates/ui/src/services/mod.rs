//! Host-neutral catalog, artwork, and load services.

pub mod auth;
pub(crate) mod bitmap;
pub mod catalog;
pub mod community;
pub mod community_worker;
pub mod connectivity;
pub mod images;
pub mod key_setup;
pub(crate) mod links;
pub mod load_flow;
pub mod release_check;
pub mod session;
pub mod update;

pub use auth::AuthService;
pub use catalog::{BrowserController, CatalogSeed, CatalogStatus, CatalogStore};
pub use community::{CommunityAccess, CommunityHandle, CommunityService};
pub use community_worker::{
    CATALOG_PAGE_SIZE, CommunityEvent, CommunityFailure, CommunityScope, CommunityWork,
    CommunityWorker, LoadRequest, MODEL_PAGE_SIZE, ModelList, ModelSwitchRequest, ModelsRequest,
    QueryReply, QueryRequest, TAXONOMY_PAGE_SIZE, TaxonomyRequest,
};
pub use connectivity::{ConnectionGate, ConnectionProblem};
pub use images::{
    ART_SIDE, AVATAR_SIDE, HttpImageFetcher, ImageCache, ImageFetcher, ImageRequest, RemoteImages,
};
pub use key_setup::{CommunityParts, KeySetup, KeySetupError, is_publishable_key};
pub use load_flow::{
    LoadFlow, LoadPhase, LoadTarget, LoadTicket, ModelSwitchFlow, ModelSwitchTicket,
};
pub use release_check::{
    GithubReleaseFeed, ReleaseAnswer, ReleaseFeed, ReleaseLog, UpdateSchedule,
};
pub use session::{AuthPhase, AuthSnapshot, Session, SessionStatus};
pub use update::{UpdateCheck, UpdateInfo};

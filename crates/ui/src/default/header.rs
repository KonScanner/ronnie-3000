//! Reference Default header, global actions, and egui chrome.

use std::sync::Arc;

use eframe::egui::{self, Align2, Color32};
use ronie_core::{BlockId, ChainSnapshot, HttpTransport, PresetInfo};

use crate::{
    accessibility::{Announcer, FocusManager},
    backend::{NavigationAction, UiAction, UiBackend, UiError},
};

use super::{
    block::BlockDetail,
    browser::{BrowserAction, BrowserView},
    chain::{ChainGallery, GallerySelection},
    faceplate::Faceplate,
    hints::HintController,
    overlays::{DismissReason, Overlay, OverlayController},
    popover::{Placement, Popover, Surface, Trigger},
    presets::PresetController,
};
use crate::services::{
    community::{CommunityAccess, CommunityService},
    community_worker::{
        CommunityEvent, CommunityFailure, CommunityScope, CommunityWork, CommunityWorker,
        LoadRequest, QueryRequest,
    },
    images::{ART_SIDE, AVATAR_SIDE, ImageCache, RemoteImages},
    key_setup::{CommunityParts, KeySetup, KeySetupError, is_publishable_key},
    links::open_web_url,
    load_flow::{LoadFlow, LoadPhase, LoadTicket, ModelSwitchFlow},
    session::{AuthFlowAdapter, Session, SessionEvent, SessionStatus},
};

/// Accessible name of the logo and name area in the header.
const HOME_LABEL: &str = "Back to main screen";
/// What the user reads when the token store holds no usable session.
const SESSION_ENDED: &str = "Your session ended. Sign in again to see your tones.";
/// The card of a failed load takes a click again, so the user reads how to retry.
const LOAD_RETRY_HINT: &str = "Select the tone to try again.";
/// The picker takes the same choice again, so the user reads how to retry.
const SWITCH_RETRY_HINT: &str = "Select the model to try again.";

/// Widest account name in the chip. The four header gaps can give this room and stay usable.
const ACCOUNT_NAME_MAX_WIDTH: f32 = 84.0;
/// Space that the account name needs in the chip, in addition to its own width.
const ACCOUNT_NAME_MARGIN: f32 = 6.0;
/// Start of the account name, measured from the left of the chip.
const ACCOUNT_NAME_LEFT: f32 = 36.0;

/// One image that the controller fetches for its own views.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ChromeImage {
    Artwork(BlockId),
    Avatar(BlockId),
    Account,
}

/// The two cached fields that a view can show. No other cached field reaches a view.
struct CachedAccount {
    name: Option<String>,
    avatar_url: Option<String>,
}

impl CachedAccount {
    fn from_user(user: &serde_json::Value) -> Self {
        Self {
            name: user_name(user).map(str::to_owned),
            avatar_url: user["avatar_url"].as_str().map(str::to_owned),
        }
    }
}

/// The best name of a public user, in the order that the service prefers.
fn user_name(user: &serde_json::Value) -> Option<&str> {
    ["display_name", "name", "username"]
        .into_iter()
        .find_map(|field| user[field].as_str().filter(|name| !name.trim().is_empty()))
}

/// Complete Default chrome controller. Processor mutations go through `UiBackend`.
pub struct ChromeController {
    backend: Arc<dyn UiBackend>,
    presets: PresetController,
    session: Session,
    community: Option<Box<dyn CommunityAccess>>,
    /// What the host does with the key of the user, when the host can take one.
    key_setup: Option<Box<dyn KeySetup>>,
    /// Whether the user saved a key. This is read when the setup arrives and after a change.
    key_saved: bool,
    /// The open key page.
    key_page: Option<super::key_setup::KeyPage>,
    /// Public fields of the signed-in user, after the service answered.
    public_user: Option<serde_json::Value>,
    /// What an earlier launch cached, for a session with no answer from the service yet.
    cached_account: Option<CachedAccount>,
    /// Whether the signed-in user still needs a refresh.
    user_refresh_due: bool,
    hints: HintController,
    overlays: OverlayController,
    announcements: Announcer,
    tuner_visible: bool,
    tuner: super::tuner::TunerFeed,
    tuner_view: super::tuner::TunerView,
    banners: super::banners::BannerStore,
    device_state: Option<super::banners::AudioDeviceState>,
    banner_visible: bool,
    window_chrome: crate::shell::Chrome,
    pending_window_steps: Vec<super::banners::WindowStep>,
    toast: super::toast::ToastController,
    modal_stack: super::modals::ModalStack,
    modal_scrim: super::modals::ModalScrim,
    modal_backdrop: Option<egui::TextureHandle>,
    /// Whether a dialog waits for a clean capture, which only a real window can take.
    dialogs_wait_for_backdrop: bool,
    dialog_backdrop_ready: bool,
    account_avatar: Option<egui::TextureHandle>,
    settings: Option<super::settings::SettingsController>,
    standalone: bool,
    faceplate: Faceplate,
    gallery: ChainGallery,
    detail: Option<BlockDetail>,
    browser: Option<BrowserView>,
    /// Closed browser that keeps its query, results, and scroll for the next open.
    parked_browser: Option<BrowserView>,
    load_flow: LoadFlow,
    model_switches: ModelSwitchFlow,
    /// Whether the worker can hold a model list or a switch of a detail.
    detail_work_live: bool,
    /// Shared cache of all fetched images. It starts at the first image.
    images: Option<ImageCache>,
    images_refused: bool,
    remote_images: Option<RemoteImages<ChromeImage>>,
    connection: crate::services::connectivity::ConnectionGate,
    updates: crate::services::update::UpdateCheck,
    overlay_trigger: Option<Trigger>,
}

impl Drop for ChromeController {
    /// The image workers end with their cache.
    fn drop(&mut self) {
        if let Some(images) = &self.images {
            images.close();
        }
    }
}

impl ChromeController {
    /// Creates Default chrome from backend state and preset metadata.
    #[must_use]
    pub fn new(
        backend: Arc<dyn UiBackend>,
        presets: Vec<PresetInfo>,
        authenticated: bool,
        hints_enabled: bool,
    ) -> Self {
        let session = Session::new(SessionStatus::from_signed_in(authenticated));
        Self::with_session(backend, presets, session, hints_enabled)
    }

    /// Creates Default chrome around one session.
    pub(crate) fn with_session(
        backend: Arc<dyn UiBackend>,
        presets: Vec<PresetInfo>,
        session: Session,
        hints_enabled: bool,
    ) -> Self {
        Self {
            presets: PresetController::new(backend.clone(), presets),
            session,
            community: None,
            key_setup: None,
            key_saved: false,
            key_page: None,
            public_user: None,
            cached_account: None,
            user_refresh_due: false,
            hints: HintController::new(hints_enabled),
            overlays: OverlayController::default(),
            announcements: Announcer::default(),
            tuner_visible: false,
            tuner: super::tuner::TunerFeed::default(),
            tuner_view: super::tuner::TunerView,
            banners: super::banners::BannerStore::new(),
            device_state: None,
            banner_visible: false,
            window_chrome: crate::shell::Chrome {
                banner: false,
                hints: hints_enabled,
            },
            pending_window_steps: Vec::new(),
            toast: super::toast::ToastController::default(),
            modal_stack: super::modals::ModalStack::default(),
            modal_scrim: super::modals::ModalScrim::default(),
            modal_backdrop: None,
            dialogs_wait_for_backdrop: false,
            dialog_backdrop_ready: false,
            account_avatar: None,
            settings: None,
            standalone: false,
            faceplate: Faceplate::new(backend.clone()),
            gallery: ChainGallery::new(backend.clone()),
            detail: None,
            browser: None,
            parked_browser: None,
            load_flow: LoadFlow::new(backend.clone()),
            model_switches: ModelSwitchFlow::new(backend.clone()),
            detail_work_live: false,
            images: None,
            images_refused: false,
            remote_images: None,
            connection: crate::services::connectivity::ConnectionGate::new(true),
            updates: crate::services::update::UpdateCheck::new(env!("CARGO_PKG_VERSION")),
            overlay_trigger: None,
            backend,
        }
    }

    /// Creates Chrome with a transport-erased authentication flow.
    #[must_use]
    pub fn with_auth_flow(
        backend: Arc<dyn UiBackend>,
        presets: Vec<PresetInfo>,
        authenticated: bool,
        hints_enabled: bool,
        auth: Box<dyn AuthFlowAdapter>,
    ) -> Self {
        let session = Session::with_flow(auth, SessionStatus::from_signed_in(authenticated));
        Self::with_session(backend, presets, session, hints_enabled)
    }

    /// Creates Chrome with the sign-in flow and the handle of one community service.
    #[must_use]
    pub fn with_community<T: HttpTransport + Send + Sync + 'static>(
        backend: Arc<dyn UiBackend>,
        presets: Vec<PresetInfo>,
        authenticated: bool,
        hints_enabled: bool,
        community: CommunityService<T>,
    ) -> Self {
        let (auth, handle) = community.into_parts();
        let mut chrome = Self::with_auth_flow(
            backend,
            presets,
            authenticated,
            hints_enabled,
            Box::new(auth),
        );
        if chrome.session.status().signed_in() {
            // This is the one read of the cached user. It is a file read, so no frame does it.
            chrome.cached_account = handle
                .tokens()
                .cached_user()
                .ok()
                .map(|user| CachedAccount::from_user(&user));
        }
        chrome.community = Some(Box::new(CommunityWorker::new(handle)));
        chrome.user_refresh_due = chrome.session.status().signed_in();
        chrome
    }

    /// The community service of this controller, when one is configured.
    #[must_use]
    pub fn community(&self) -> Option<&dyn CommunityAccess> {
        self.community.as_deref()
    }

    /// Gives the controller the key setup of the host.
    ///
    /// With no service, a sign-in then opens the key page, and a saved key starts the service.
    /// This reads a file, so a frame must not call it.
    pub fn use_key_setup(&mut self, setup: Box<dyn KeySetup>) {
        self.key_saved = setup.has_saved_key();
        self.key_setup = Some(setup);
        self.session.offer_key_setup(true);
    }

    /// Whether the user saved a key.
    #[must_use]
    pub const fn key_saved(&self) -> bool {
        self.key_saved
    }

    /// The open key page.
    #[must_use]
    pub const fn key_page(&self) -> Option<&super::key_setup::KeyPage> {
        self.key_page.as_ref()
    }

    /// The open key page, for a draft that a test or a scenario types.
    pub fn key_page_mut(&mut self) -> Option<&mut super::key_setup::KeyPage> {
        self.key_page.as_mut()
    }

    /// Opens the key page and closes the overlay that opened it.
    pub fn open_key_page(&mut self) {
        self.key_page = Some(super::key_setup::KeyPage::default());
        self.overlays.dismiss(DismissReason::Action);
    }

    /// Closes the key page. The draft goes away, and nothing is saved.
    pub fn close_key_page(&mut self) {
        self.key_page = None;
    }

    /// Saves the draft of the key page and starts the service of the key with no restart.
    ///
    /// A value that is not a publishable key saves nothing, and the page shows an error. After
    /// a save, the sign-in page replaces the key page.
    ///
    /// # Errors
    /// Returns the problem that the page also shows. The text never holds the key.
    pub fn save_key(&mut self) -> Result<(), UiError> {
        let (Some(page), Some(setup)) = (&mut self.key_page, &mut self.key_setup) else {
            return Err(UiError::InvalidAction("no key page is open".to_owned()));
        };
        let key = page.draft().trim();
        let result = if is_publishable_key(key) {
            setup.save(key)
        } else {
            Err(KeySetupError::NotAKey)
        };
        let parts = match result {
            Ok(parts) => parts,
            Err(error) => {
                let message = error.to_string();
                page.set_error(message.clone());
                return Err(UiError::Control(message));
            }
        };
        self.key_saved = true;
        self.key_page = None;
        match parts {
            Some(parts) => {
                self.attach_community(parts);
                if !self.session.status().signed_in() {
                    self.session.show_page();
                }
            }
            None => {
                let _ = self.sign_in_problem(super::key_setup::RESTART_TO_SIGN_IN.to_owned());
            }
        }
        Ok(())
    }

    /// Signs out, removes the saved key, and uses the service that stays, or none.
    ///
    /// # Errors
    /// Returns a sign-out or storage problem. A failed sign-out keeps the key.
    pub fn remove_key(&mut self) -> Result<(), UiError> {
        if self.key_setup.is_none() {
            return Err(UiError::InvalidAction("no key can be removed".to_owned()));
        }
        // The tokens belong to the key, so they go first.
        let before = self.session.status();
        self.session
            .sign_out()
            .map_err(|error| UiError::Control(error.to_string()))?;
        self.after_session_change(before);
        let removed = match &mut self.key_setup {
            Some(setup) => setup.remove(),
            None => Ok(None),
        };
        let parts = removed.map_err(|error| UiError::Control(error.to_string()))?;
        self.key_saved = false;
        match parts {
            Some(parts) => self.attach_community(parts),
            None => {
                let before = self.session.status();
                self.drop_community();
                self.session.detach_flow();
                self.after_session_change(before);
            }
        }
        Ok(())
    }

    /// Uses a service that started after the launch, in place of the service before it.
    fn attach_community(&mut self, parts: CommunityParts) {
        let before = self.session.status();
        self.drop_community();
        self.session.attach_flow(parts.auth);
        self.community = Some(parts.access);
        // The views of the old configuration have no worker and no images, so they start again.
        self.load_flow.abandon();
        for browser in self
            .browser
            .iter_mut()
            .chain(self.parked_browser.iter_mut())
        {
            browser.close();
        }
        self.browser = None;
        self.parked_browser = None;
        self.after_session_change(before);
        self.user_refresh_due = self.session.status().signed_in();
    }

    /// Stops the work and the images of the current service, and removes it.
    fn drop_community(&mut self) {
        if let Some(mut community) = self.community.take() {
            community.cancel(CommunityScope::Session);
        }
        if let Some(images) = self.images.take() {
            images.close();
        }
        self.remote_images = None;
        self.images_refused = false;
        self.detail_work_live = false;
        self.public_user = None;
        self.cached_account = None;
        self.account_avatar = None;
    }

    /// Public fields of the signed-in user, after the service answered.
    #[must_use]
    pub const fn public_user(&self) -> Option<&serde_json::Value> {
        self.public_user.as_ref()
    }

    /// The name that the account chip shows, on one line.
    ///
    /// A fresh user replaces the cached user. With no session, the chip shows no name.
    #[must_use]
    pub fn account_name(&self) -> Option<String> {
        if !self.session.status().signed_in() {
            return None;
        }
        let name = match &self.public_user {
            Some(user) => user_name(user),
            None => self.cached_account.as_ref()?.name.as_deref(),
        }?;
        Some(super::banners::one_line_text(name)).filter(|name| !name.is_empty())
    }

    /// The avatar address of the same user as [`Self::account_name`].
    fn account_avatar_url(&self) -> Option<&str> {
        match &self.public_user {
            Some(user) => user["avatar_url"].as_str(),
            None => self.cached_account.as_ref()?.avatar_url.as_deref(),
        }
    }

    /// Applies finished community work and starts the work that the browser asks for.
    ///
    /// This does not block and needs no UI, so it runs while the window is covered. With no
    /// community service, this does nothing.
    pub fn poll_community(&mut self) {
        let Some(community) = &mut self.community else {
            return;
        };
        // This comes before the poll, so that a closed detail gives nothing to the next detail.
        if self.detail_work_live
            && !self
                .detail
                .as_ref()
                .is_some_and(BlockDetail::has_model_work)
        {
            community.cancel(CommunityScope::Detail);
            self.detail_work_live = false;
        }
        for event in community.poll() {
            // A lost session cancels all work, so the rest of this batch is out of date.
            if !self.apply_community_event(event) {
                break;
            }
        }
        self.start_community_work();
    }

    /// Applies one event. Returns false when the event ended the session.
    fn apply_community_event(&mut self, event: CommunityEvent) -> bool {
        let store = self.browser.as_mut().map(BrowserView::store_mut);
        let failure = match event {
            CommunityEvent::Query { generation, result } => match result {
                Ok(reply) => {
                    if let Some(store) = store
                        && store.complete_query(generation, reply.page, reply.trending)
                    {
                        store.remember_result_creators();
                    }
                    None
                }
                Err(CommunityFailure::Failed(message)) => {
                    if let Some(store) = store {
                        store.fail_query(generation, message);
                    }
                    None
                }
                Err(failure) => Some(failure),
            },
            CommunityEvent::Taxonomy {
                generation,
                kind,
                result,
            } => match result {
                Ok(rows) => {
                    if let Some(store) = store {
                        store.complete_taxonomy(generation, kind, rows);
                    }
                    None
                }
                // The menu keeps the rows that it can derive from the results.
                Err(failure) => Some(failure),
            },
            CommunityEvent::Favorite { tone, result, .. } => {
                if let Some(store) = store {
                    store.finish_favorite(tone, result.is_ok());
                }
                result.err()
            }
            CommunityEvent::User { result, .. } => match result {
                Ok(user) => {
                    self.public_user = Some(user);
                    None
                }
                Err(failure) => Some(failure),
            },
            CommunityEvent::ToneLoad { generation, result } => match result {
                Ok(()) => {
                    self.load_flow.finish_remote(generation);
                    None
                }
                Err(CommunityFailure::Failed(message)) => {
                    if self.load_flow.fail_load(generation) {
                        self.hints
                            .set_toast(Some(format!("{message} {LOAD_RETRY_HINT}")));
                    }
                    None
                }
                Err(failure) => {
                    self.load_flow.fail_load(generation);
                    Some(failure)
                }
            },
            CommunityEvent::Models {
                generation,
                tone,
                result,
            } => match result {
                Ok(list) => {
                    if let Some(detail) = &mut self.detail {
                        detail.complete_models(generation, tone, list);
                    }
                    None
                }
                Err(CommunityFailure::Failed(message)) => {
                    if self
                        .detail
                        .as_mut()
                        .is_some_and(|detail| detail.fail_models(generation))
                    {
                        self.hints.set_toast(Some(message.to_owned()));
                    }
                    None
                }
                Err(failure) => {
                    if let Some(detail) = &mut self.detail {
                        detail.fail_models(generation);
                    }
                    Some(failure)
                }
            },
            CommunityEvent::ModelSwitch { generation, result } => {
                // The chain can have changed since the last frame, and then this switch has ended.
                if let Some(detail) = &mut self.detail {
                    detail.sync();
                }
                let detail = self.detail.as_mut();
                match result {
                    Ok(()) => {
                        if let Some(detail) = detail {
                            detail.finish_switch(generation);
                        }
                        None
                    }
                    Err(CommunityFailure::Failed(message)) => {
                        if detail.is_some_and(|detail| detail.fail_switch(generation)) {
                            self.hints
                                .set_toast(Some(format!("{message} {SWITCH_RETRY_HINT}")));
                        }
                        None
                    }
                    Err(failure) => {
                        if let Some(detail) = detail {
                            detail.fail_switch(generation);
                        }
                        Some(failure)
                    }
                }
            }
        };
        if failure != Some(CommunityFailure::SessionLost) {
            return true;
        }
        let before = self.session.status();
        self.session.expire();
        self.after_session_change(before);
        self.hints.set_toast(Some(SESSION_ENDED.to_owned()));
        false
    }

    /// Hands new work to the worker. A query starts when the store begins it.
    fn start_community_work(&mut self) {
        let signed_in = self.session.status().signed_in();
        let mut work = Vec::new();
        if signed_in && std::mem::take(&mut self.user_refresh_due) {
            work.push(CommunityWork::RefreshUser);
        }
        if let Some(browser) = &mut self.browser {
            let store = browser.store_mut();
            store.begin_owed_query();
            let query = store.take_unsent_query();
            let taxonomy = store.take_unsent_taxonomy().filter(|_| signed_in);
            if let Some(query) = query {
                if query.changed {
                    browser.set_scroll(0.0);
                }
                work.push(CommunityWork::Query(QueryRequest {
                    generation: query.generation,
                    query: query.query,
                    page: query.page,
                    signed_in,
                }));
            }
            work.extend(taxonomy.map(CommunityWork::Taxonomy));
        }
        if let Some(detail) = &mut self.detail {
            let list = detail.take_models_request(self.session.status());
            let switch = detail.take_switch_request();
            self.detail_work_live |= list.is_some() || switch.is_some();
            work.extend(list.map(CommunityWork::Models));
            work.extend(switch.map(CommunityWork::ModelSwitch));
        }
        if work.is_empty() {
            return;
        }
        match self.session.runtime() {
            Ok(runtime) => {
                if let Some(community) = &mut self.community {
                    for work in work {
                        community.start(&runtime, work);
                    }
                }
            }
            Err(problem) => {
                for work in work {
                    self.refuse_community_work(work, &problem.to_string());
                }
            }
        }
    }

    /// Ends the state of work that did not start, because no event will end it.
    fn refuse_community_work(&mut self, work: CommunityWork, problem: &str) {
        if let CommunityWork::ToneLoad(request) = &work {
            if self.load_flow.fail_load(request.ticket.generation()) {
                self.hints.set_toast(Some(problem.to_owned()));
            }
            return;
        }
        if let CommunityWork::Models(request) = &work {
            if let Some(detail) = &mut self.detail {
                detail.fail_models(request.generation);
            }
            return;
        }
        if let CommunityWork::ModelSwitch(request) = &work {
            if self
                .detail
                .as_mut()
                .is_some_and(|detail| detail.fail_switch(request.ticket.generation()))
            {
                self.hints.set_toast(Some(problem.to_owned()));
            }
            return;
        }
        let Some(store) = self.browser.as_mut().map(BrowserView::store_mut) else {
            return;
        };
        match work {
            CommunityWork::Query(request) => {
                store.fail_query(request.generation, problem);
            }
            CommunityWork::Favorite { tone, .. } => {
                store.finish_favorite(tone, false);
            }
            CommunityWork::Taxonomy(_)
            | CommunityWork::RefreshUser
            | CommunityWork::ToneLoad(_)
            | CommunityWork::Models(_)
            | CommunityWork::ModelSwitch(_) => {}
        }
    }

    /// Changes the favorite state of one tone in the open browser.
    ///
    /// The change shows at once, and a failed write restores the previous tone. A signed-out
    /// user with a community service gets the sign-in prompt.
    pub fn set_favorite(&mut self, tone: ronie_core::ToneId, favorite: bool) {
        let Some(browser) = &mut self.browser else {
            return;
        };
        let Some(community) = &mut self.community else {
            let store = browser.store_mut();
            store.begin_favorite(tone, favorite);
            store.finish_favorite(tone, true);
            return;
        };
        if !self.session.status().signed_in() {
            browser.set_sign_in_page(true);
            return;
        }
        if !browser.store_mut().begin_favorite(tone, favorite) {
            return;
        }
        let work = CommunityWork::Favorite { tone, favorite };
        match self.session.runtime() {
            Ok(runtime) => community.start(&runtime, work),
            Err(problem) => self.refuse_community_work(work, &problem.to_string()),
        }
    }

    /// Loads one tone that the user picked in the browser.
    ///
    /// With a community service, a catalog tone goes to a background worker. The chain changes
    /// only after its bytes arrive. With no service, or for a local tone, the bytes need no
    /// network and the tone lands at once.
    pub fn pick_tone(&mut self, tone: ronie_core::Tone) {
        if self.community.is_none() || tone.local {
            if let Ok(generation) = self.load_flow.begin_load(tone.clone()) {
                self.load_flow.complete_cached(generation, tone);
            }
            return;
        }
        if let Ok(ticket) = self.load_flow.begin_remote_load(tone.clone()) {
            self.start_tone_load(tone.id, ticket);
        }
    }

    /// Starts the last failed load again. This does nothing when no load failed.
    pub fn retry_load(&mut self) {
        if self.community.is_none() {
            return;
        }
        if let Ok((ticket, tone)) = self.load_flow.retry_remote() {
            self.start_tone_load(tone.id, ticket);
        }
    }

    fn start_tone_load(&mut self, tone: ronie_core::ToneId, ticket: LoadTicket) {
        let work = CommunityWork::ToneLoad(LoadRequest { tone, ticket });
        match self.session.runtime() {
            Ok(runtime) => {
                if let Some(community) = &mut self.community {
                    community.start(&runtime, work);
                }
            }
            Err(problem) => self.refuse_community_work(work, &problem.to_string()),
        }
    }

    /// Stops the work of a browser that closed.
    fn cancel_browser_work(&mut self) {
        if let Some(community) = &mut self.community {
            community.cancel(CommunityScope::Browser);
        }
    }

    /// The cache of fetched images, which starts here. A controller with no service has none,
    /// and a refused start is not tried again in each frame.
    fn image_cache(&mut self) -> Option<ImageCache> {
        if self.images.is_none() && !self.images_refused {
            let community = self.community.as_ref()?;
            match self.session.runtime() {
                Ok(runtime) => {
                    self.images = Some(ImageCache::new(community.image_fetcher(runtime)));
                }
                Err(_) => self.images_refused = true,
            }
        }
        self.images.clone()
    }

    /// Fetches the images of the chain, the open detail, and the account, and gives their
    /// textures to the views. A view shows its placeholder until an image arrives. A controller
    /// with no service does nothing here, so that its views keep what they were given.
    fn sync_remote_images(&mut self, context: &egui::Context, chain: &ChainSnapshot) {
        if self.community.is_none() {
            return;
        }
        let mut wanted = Vec::new();
        for block in chain.left.iter().chain(chain.right.iter()) {
            let Some(tone) = &block.tone else {
                continue;
            };
            if let Some(url) = tone.raw["images"][0].as_str() {
                wanted.push((
                    ChromeImage::Artwork(block.id.clone()),
                    url.to_owned(),
                    ART_SIDE,
                ));
            }
            if let Some(url) = tone.raw["user"]["avatar_url"].as_str() {
                wanted.push((
                    ChromeImage::Avatar(block.id.clone()),
                    url.to_owned(),
                    AVATAR_SIDE,
                ));
            }
        }
        if let Some(url) = self.account_avatar_url() {
            wanted.push((ChromeImage::Account, url.to_owned(), AVATAR_SIDE));
        }
        if self.remote_images.is_none() && !wanted.is_empty() {
            self.remote_images = self.image_cache().map(RemoteImages::new);
        }
        let Some(images) = &mut self.remote_images else {
            return;
        };
        images.retain(|key| wanted.iter().any(|(image, ..)| image == key));
        for (key, url, side) in wanted {
            images.request(key, &url, side);
        }
        images.poll(context);
        self.gallery.replace_artwork(
            chain
                .left
                .iter()
                .chain(chain.right.iter())
                .filter_map(|block| {
                    let texture = images.texture(&ChromeImage::Artwork(block.id.clone()))?;
                    Some((block.id.clone(), texture.clone()))
                })
                .collect(),
        );
        if let Some(detail) = &mut self.detail {
            let id = detail.block().id.clone();
            detail.set_remote_images(
                images.texture(&ChromeImage::Artwork(id.clone())).cloned(),
                images.texture(&ChromeImage::Avatar(id)).cloned(),
            );
        }
        self.account_avatar = images.texture(&ChromeImage::Account).cloned();
        if let (Some(browser), Some(avatar)) = (&mut self.browser, &self.account_avatar) {
            browser.set_profile_avatar(avatar.clone());
        }
    }

    /// The account avatar in the header.
    #[must_use]
    pub const fn account_avatar(&self) -> Option<&egui::TextureHandle> {
        self.account_avatar.as_ref()
    }

    /// Backend that this controller edits.
    pub(crate) fn backend(&self) -> &Arc<dyn UiBackend> {
        &self.backend
    }

    /// Persistent signal-chain gallery.
    #[must_use]
    pub fn gallery(&self) -> &ChainGallery {
        &self.gallery
    }

    /// Persistent signal-chain gallery for interaction and rendering.
    pub fn gallery_mut(&mut self) -> &mut ChainGallery {
        &mut self.gallery
    }

    /// Active block detail, when a tone tile is open.
    #[must_use]
    pub const fn detail(&self) -> Option<&BlockDetail> {
        self.detail.as_ref()
    }

    /// Mutable active block detail.
    pub fn detail_mut(&mut self) -> Option<&mut BlockDetail> {
        self.detail.as_mut()
    }

    /// Active tone browser, when an add or swap flow is open.
    #[must_use]
    pub const fn browser(&self) -> Option<&BrowserView> {
        self.browser.as_ref()
    }

    /// Mutable active tone browser.
    pub fn browser_mut(&mut self) -> Option<&mut BrowserView> {
        self.browser.as_mut()
    }

    /// Puts the open tone browser in a catalog state with no catalog. This is the narrow
    /// seeding seam for a state that no control can reach. Returns whether a browser is open.
    pub(crate) fn seed_catalog(&mut self, seed: crate::services::catalog::CatalogSeed) -> bool {
        let Some(browser) = &mut self.browser else {
            return false;
        };
        browser.store_mut().seed(seed);
        true
    }

    /// Phase of the load flow.
    #[must_use]
    pub const fn load_flow_phase(&self) -> LoadPhase {
        self.load_flow.phase()
    }

    /// Mutable pending load flow.
    pub fn load_flow_mut(&mut self) -> &mut LoadFlow {
        &mut self.load_flow
    }

    /// Opens tone selection for the gallery target and closes completed flows.
    pub fn sync_browser(&mut self) {
        // A finished phase stays set until the next selection, so it closes an open browser
        // one time. It must not act on a selection that the user makes later.
        if self.browser.is_some()
            && matches!(
                self.load_flow.phase(),
                LoadPhase::Ready | LoadPhase::Cancelled
            )
        {
            let mut closed = self.browser.take();
            if self.community.is_some() {
                // A cancelled load leaves the store open, and its worker now stops.
                if let Some(browser) = &mut closed {
                    browser.close();
                }
                self.cancel_browser_work();
            }
            if self.load_flow.phase() == LoadPhase::Cancelled {
                self.parked_browser = closed;
            }
            self.gallery.clear_selection();
            return;
        }
        if let GallerySelection::SelectTone {
            side,
            target_block_id,
        } = self.gallery.selection().clone()
        {
            if self
                .load_flow
                .pending_target()
                .is_none_or(|target| target.side != side || target.block_id != target_block_id.0)
            {
                self.load_flow.select(side, &target_block_id.0);
            }
            if self.browser.is_none() {
                let parked = self.parked_browser.take();
                let images = parked.is_none().then(|| self.image_cache()).flatten();
                self.browser = Some(parked.map_or_else(
                    || images.map_or_else(BrowserView::new, BrowserView::with_images),
                    |mut browser| {
                        browser.reopen();
                        browser
                    },
                ));
            }
            if let Some(browser) = &mut self.browser {
                browser.set_pending_tone(self.load_flow.loading_tone_id());
            }
        }
    }

    /// Returns to the main screen from the tone browser, a block detail, the tuner, or a
    /// sign-in page. The chain stays as it is, and a load in progress is dropped.
    pub fn go_to_main(&mut self) {
        // A mouse press closes an overlay first, and a keyboard activation does not.
        self.overlays.dismiss(DismissReason::Action);
        if self.auth_active() {
            self.close_auth();
        }
        if self.tuner_visible {
            self.set_tuner(false);
        }
        if let Some(browser) = &mut self.browser {
            browser.close();
            self.load_flow.close();
            self.parked_browser = self.browser.take();
            self.cancel_browser_work();
        }
        if let Some(detail) = &mut self.detail {
            detail.close();
            self.detail = None;
        }
        self.gallery.clear_selection();
    }

    /// Opens one block detail and selects it in the gallery.
    ///
    /// # Errors
    /// Returns an unknown-target error when the block does not exist.
    pub fn open_detail(&mut self, id: &str) -> Result<(), UiError> {
        let calibrate = self
            .backend
            .parameter(ronie_core::params::ParameterId("calibrateInput"))
            .is_some_and(|value| value.0 > 0.5);
        let mut detail = BlockDetail::new(self.backend.clone(), id, calibrate)?;
        if self.community.is_some() {
            detail.attach_model_switching(self.model_switches.clone());
        }
        self.detail = Some(detail);
        self.gallery.activate(id);
        Ok(())
    }

    /// Reconciles tile navigation, detail closure, and tone-swap requests.
    pub fn sync_detail(&mut self) {
        if let GallerySelection::BlockDetail(id) = self.gallery.selection().clone() {
            if self
                .detail
                .as_ref()
                .is_none_or(|detail| detail.block().id != id)
            {
                let _ = self.open_detail(&id.0);
            }
        } else {
            self.detail = None;
        }
        let swap = self
            .detail
            .as_mut()
            .and_then(BlockDetail::take_swap_request);
        if let Some(id) = swap {
            let chain = self.backend.chain_state(0).chain;
            let side = if chain.right.iter().any(|block| block.id == id) {
                ronie_core::ChainSide::Right
            } else {
                ronie_core::ChainSide::Left
            };
            self.gallery.select_tone(side, id);
            self.detail = None;
        }
        if self.detail.as_ref().is_some_and(BlockDetail::closed) {
            self.detail = None;
            self.gallery.clear_selection();
        }
    }

    /// Current overlay.
    #[must_use]
    pub fn overlay(&self) -> Overlay {
        self.overlays.overlay()
    }

    /// Current mounted settings takeover.
    #[must_use]
    pub const fn settings(&self) -> Option<&super::settings::SettingsController> {
        self.settings.as_ref()
    }

    /// Mutable mounted settings takeover.
    pub fn settings_mut(&mut self) -> Option<&mut super::settings::SettingsController> {
        self.settings.as_mut()
    }

    /// Selects whether this is a standalone editor with two settings pages.
    pub fn set_standalone(&mut self, standalone: bool) {
        self.standalone = standalone;
    }

    /// Whether this is a standalone editor.
    #[must_use]
    pub const fn standalone(&self) -> bool {
        self.standalone
    }

    /// Opens plugin settings.
    pub fn open_settings(&mut self) {
        self.settings = Some(super::settings::SettingsController::new(
            self.backend.clone(),
            self.standalone,
            super::settings::SettingsTab::Plugin,
        ));
        self.show_key_in_settings();
        self.overlays
            .dismiss(super::overlays::DismissReason::Action);
    }

    /// Opens standalone audio and MIDI settings.
    pub fn open_system_settings(&mut self) {
        self.standalone = true;
        self.settings = Some(super::settings::SettingsController::new(
            self.backend.clone(),
            true,
            super::settings::SettingsTab::System,
        ));
        self.show_key_in_settings();
        self.overlays
            .dismiss(super::overlays::DismissReason::Action);
    }

    /// Renders and closes the mounted settings takeover.
    pub fn sync_settings(&mut self, ui: &mut egui::Ui) {
        let viewport_height = ui.available_height();
        if self
            .settings
            .as_mut()
            .is_some_and(|settings| settings.show(ui, viewport_height))
        {
            self.settings = None;
        }
        let remove = self
            .settings
            .as_ref()
            .is_some_and(|settings| settings.system().take_remove_key_request());
        if remove {
            if let Err(error) = self.remove_key() {
                self.hints.set_toast(Some(error.to_string()));
            }
            self.show_key_in_settings();
        }
    }

    /// Tells the settings page whether the user saved a key.
    fn show_key_in_settings(&self) {
        if let Some(settings) = &self.settings {
            settings.system().set_key_saved(self.key_saved);
        }
    }

    /// Focus manager used by all chrome surfaces.
    #[must_use]
    pub fn focus(&self) -> &FocusManager {
        self.overlays.focus()
    }

    /// Preset metadata in store order.
    #[must_use]
    pub fn presets(&self) -> &[PresetInfo] {
        self.presets.presets()
    }

    /// Active preset name.
    #[must_use]
    pub fn active_preset_name(&self) -> Option<&str> {
        self.presets.active_preset_name()
    }

    /// Current toast text.
    #[must_use]
    pub fn toast(&self) -> Option<&str> {
        self.hints.toast()
    }

    /// Current prioritized banner state.
    #[must_use]
    pub const fn banners(&self) -> &super::banners::BannerStore {
        &self.banners
    }

    /// Restores update snoozes and dismissed banners from saved preferences.
    pub fn use_settings(&mut self, settings: ronie_core::SettingsStore) {
        self.updates = crate::services::update::UpdateCheck::with_settings(
            env!("CARGO_PKG_VERSION"),
            settings.clone(),
        );
        self.banners = super::banners::BannerStore::with_settings(settings);
    }

    /// Replaces the standalone device snapshot and re-evaluates banners.
    pub fn set_device_state(
        &mut self,
        state: Option<super::banners::AudioDeviceState>,
        now_ms: i64,
    ) {
        self.device_state = state;
        self.sync_banners(now_ms);
    }

    /// Replaces the audio chain faults and re-evaluates banners.
    pub fn set_chain_faults(&mut self, faults: Vec<crate::backend::ChainFault>, now_ms: i64) {
        self.banners.set_chain_faults(faults);
        self.sync_banners(now_ms);
    }

    /// Re-evaluates device banners and queues snapped native window steps.
    pub fn sync_banners(&mut self, now_ms: i64) {
        let before = self.window_chrome;
        self.banners.evaluate(self.device_state.as_ref(), now_ms);
        if let Some(announcement) = self.banners.take_announcement() {
            self.announce(announcement);
        }
        let after = crate::shell::Chrome {
            banner: self.banners.active().is_some(),
            hints: self.hints.enabled(),
        };
        let steps = super::banners::banner_window_transition(before, after, 1.0);
        for step in &steps {
            match step {
                super::banners::WindowStep::ShowBanner => self.banner_visible = true,
                super::banners::WindowStep::HideBanner => self.banner_visible = false,
                super::banners::WindowStep::Resize(_, _) => {}
            }
        }
        self.window_chrome = crate::shell::Chrome {
            banner: self.banner_visible,
            hints: self.hints.enabled(),
        };
        self.pending_window_steps.extend(steps);
    }

    /// Drains native resize/show/hide operations in their required order.
    pub fn take_window_steps(&mut self, scale: f64) -> Vec<super::banners::WindowStep> {
        let (width, height) = self.window_chrome.window_size(scale);
        self.pending_window_steps
            .drain(..)
            .map(|step| match step {
                super::banners::WindowStep::Resize(_, _) => {
                    super::banners::WindowStep::Resize(width, height)
                }
                other => other,
            })
            .collect()
    }

    /// Dispatches a banner action or dismissal through its typed boundary.
    ///
    /// # Errors
    /// Returns navigation, device, or preference errors.
    pub fn handle_banner_view_action(
        &mut self,
        action: super::banners::BannerViewAction,
        now_ms: i64,
    ) -> Result<(), UiError> {
        match action {
            super::banners::BannerViewAction::Action(action) => match action {
                super::banners::BannerAction::OpenSettings => {
                    self.open_system_settings();
                }
                super::banners::BannerAction::SwitchToAsio => {
                    self.backend.dispatch(UiAction::Device(
                        crate::backend::DeviceAction::SelectBackend("ASIO".to_owned()),
                    ))?;
                }
                super::banners::BannerAction::OpenMicSettings => {
                    self.backend
                        .dispatch(UiAction::Navigate(NavigationAction::OpenMicrophonePrivacy))?;
                }
                super::banners::BannerAction::None => {}
            },
            super::banners::BannerViewAction::Dismiss(id) => {
                self.banners
                    .dismiss(&id, now_ms)
                    .map_err(|error| UiError::Control(error.to_string()))?;
                self.sync_banners(now_ms);
            }
        }
        Ok(())
    }

    /// Currently visible toast after shared-clock expiry.
    #[must_use]
    pub fn visible_toast(&self) -> Option<&str> {
        self.toast.message()
    }

    /// Shows one timed toast through the shared accessible controller.
    pub fn show_toast(&mut self, message: impl Into<String>, now_ms: i64) {
        self.toast.show(message, now_ms);
    }

    /// Pins a toast until the active operation completes or is replaced.
    pub fn pin_toast(&mut self, message: impl Into<String>, now_ms: i64) {
        self.toast.pin(message, now_ms);
    }

    /// Feeds new toast requests into the timed controller and expires old ones.
    pub fn sync_toast(&mut self, now_ms: i64) {
        if let Some(message) = self.hints.toast().map(str::to_owned) {
            self.show_toast(message, now_ms);
            self.hints.set_toast(None);
        }
        self.toast.message_at(now_ms);
    }

    /// Whether a system dialog currently requires a clean cached backdrop.
    #[must_use]
    pub fn system_modal_visible(&self) -> bool {
        self.updates.notice().is_some() || self.connection.problem().is_some()
    }

    /// Makes a new dialog wait until the window holds a clean capture.
    pub fn wait_for_dialog_backdrops(&mut self) {
        self.dialogs_wait_for_backdrop = true;
    }

    /// Whether a dialog is open and stays off the screen, so that this pass can be captured.
    #[must_use]
    pub fn dialogs_held(&self) -> bool {
        self.dialogs_wait_for_backdrop && !self.dialog_backdrop_ready && self.system_modal_visible()
    }

    /// Whether a surface stays out of the frames that the window captures.
    #[must_use]
    pub fn capture_held(&self) -> bool {
        self.dialogs_held()
            || self
                .browser
                .as_ref()
                .is_some_and(BrowserView::busy_card_held)
    }

    /// Shows a held surface with what the controller holds, because no capture arrived.
    pub fn release_dialogs(&mut self) {
        self.dialog_backdrop_ready = true;
        if let Some(browser) = &mut self.browser {
            browser.hold_busy_overlay(false);
        }
    }

    /// Whether a system dialog paints in this pass.
    #[must_use]
    pub fn dialog_on_screen(&self) -> bool {
        self.system_modal_visible() && !self.dialogs_held()
    }

    /// The surface that now needs a clean backdrop.
    #[must_use]
    pub fn backdrop_user(&self) -> Option<super::modals::BackdropUser> {
        if self.system_modal_visible() {
            Some(super::modals::BackdropUser::Dialog)
        } else if self
            .browser
            .as_ref()
            .is_some_and(|browser| browser.pending_tone().is_some())
        {
            Some(super::modals::BackdropUser::BusyCard)
        } else {
            None
        }
    }

    /// The blurred, dialog-free backdrop of the system modals and of a busy tone card.
    #[must_use]
    pub const fn modal_backdrop(&self) -> Option<&egui::TextureHandle> {
        self.modal_backdrop.as_ref()
    }

    /// Drops the old backdrop, because the window now captures the one that replaces it.
    ///
    /// An older image shows another scroll position behind a card, and a dialog waits for the new
    /// image.
    pub fn begin_backdrop_capture(&mut self) {
        self.set_modal_backdrop(None);
        self.dialog_backdrop_ready = false;
        if let Some(browser) = &mut self.browser {
            browser.hold_busy_overlay(true);
        }
    }

    /// Replaces the blurred, dialog-free backdrop used by system modals.
    pub fn set_modal_backdrop(&mut self, texture: Option<egui::TextureHandle>) {
        if texture.is_some() {
            self.release_dialogs();
        }
        self.modal_backdrop = texture;
        if let Some(browser) = &mut self.browser {
            browser.set_busy_backdrop(self.modal_backdrop.clone());
        }
    }

    /// Supplies the signed-in account avatar used by native fixture and session flows.
    pub fn set_account_avatar(&mut self, texture: egui::TextureHandle) {
        self.account_avatar = Some(texture);
    }

    /// Restacks connection above update and paints their shared modal scrim.
    pub fn sync_modals(&mut self, ui: &mut egui::Ui) {
        self.modal_stack.restack(
            self.updates.notice().is_some(),
            self.connection.problem().is_some(),
        );
        if self.modal_stack.blocks_background_input() {
            self.modal_scrim.paint(ui, self.modal_backdrop.as_ref());
        }
    }

    /// Latest save error.
    #[must_use]
    pub fn save_error(&self) -> Option<&str> {
        self.presets.save_error()
    }

    /// The one session value, which views take when they render.
    #[must_use]
    pub const fn session(&self) -> SessionStatus {
        self.session.status()
    }

    /// Current connection gate.
    #[must_use]
    pub const fn connection(&self) -> &crate::services::connectivity::ConnectionGate {
        &self.connection
    }

    /// Records whether the system reports a network connection.
    pub fn set_online(&mut self, online: bool) {
        self.connection.set_online(online);
    }

    /// Current update offer and notice state.
    #[must_use]
    pub const fn updates(&self) -> &crate::services::update::UpdateCheck {
        &self.updates
    }

    /// Runs an online action or queues its identity behind the offline gate.
    pub fn require_connection(&mut self, action: impl Into<String>) -> bool {
        self.connection.require_connection(action)
    }

    /// Releases a queued action after the gate's retry backoff.
    pub fn retry_connection(&mut self, now_ms: i64) -> Option<String> {
        let action = self.connection.pending_action().map(str::to_owned);
        if !self.connection.retry(now_ms) {
            return None;
        }
        if let Some(action) = action {
            self.resume_connection_action(&action);
            return Some(action);
        }
        None
    }

    /// Applies a retry or dismiss action from the connection view.
    pub fn handle_connection_action(
        &mut self,
        action: super::connection::ConnectionAction,
        now_ms: i64,
    ) -> Option<String> {
        match action {
            super::connection::ConnectionAction::Retry => self.retry_connection(now_ms),
            super::connection::ConnectionAction::Dismiss => {
                self.connection.dismiss();
                None
            }
        }
    }

    /// Applies one best-effort update payload.
    pub fn apply_update_payload(&mut self, payload: serde_json::Value, now_ms: i64) {
        self.updates.apply_payload(payload, now_ms);
    }

    /// Applies a download or snooze action from the update view.
    ///
    /// # Errors
    /// Returns a preferences error when a snooze cannot be persisted.
    pub fn handle_update_action(
        &mut self,
        action: super::update::UpdateAction,
        now_ms: i64,
    ) -> Result<Option<String>, UiError> {
        match action {
            super::update::UpdateAction::Download(url) => Ok(Some(url)),
            super::update::UpdateAction::Remind(days) => {
                self.updates
                    .remind_later(days, now_ms)
                    .map_err(|error| UiError::Control(error.to_string()))?;
                Ok(None)
            }
        }
    }

    /// Renders and handles a visible connection problem.
    pub fn sync_connection(&mut self, ui: &mut egui::Ui, now_ms: i64) {
        if let Some(action) = super::connection::show(ui, &mut self.connection) {
            let _ = self.handle_connection_action(action, now_ms);
        }
    }

    /// Renders and handles an update notice, using only the system browser.
    pub fn sync_updates(&mut self, ui: &mut egui::Ui, now_ms: i64) {
        let Some(info) = self.updates.notice().cloned() else {
            return;
        };
        let Some(action) = super::update::show(ui, &info) else {
            return;
        };
        match self.handle_update_action(action, now_ms) {
            Ok(Some(url)) => open_web_url(ui.ctx(), url),
            Ok(None) => {}
            Err(error) => self.hints.set_toast(Some(error.to_string())),
        }
    }

    fn resume_connection_action(&mut self, action: &str) {
        match action {
            "browse" => {
                self.sync_browser();
            }
            "sign-in" => {
                let _ = self.sign_in();
            }
            "load" => {
                if self.community.is_some() {
                    self.retry_load();
                } else {
                    let _ = self.load_flow.retry();
                }
            }
            _ => {}
        }
    }

    /// Whether the automatic balance measurement runs.
    #[must_use]
    pub fn measuring(&self) -> bool {
        self.faceplate.measuring()
    }

    /// Whether the tuner is visible.
    #[must_use]
    pub fn tuner_visible(&self) -> bool {
        self.tuner_visible
    }

    /// Current tuner display state.
    #[must_use]
    pub const fn tuner(&self) -> &super::tuner::TunerFeed {
        &self.tuner
    }

    /// Polls raw-input readings only while the takeover is visible.
    pub fn sync_tuner(&mut self, now_ms: i64) {
        if self.tuner_visible {
            let reading = self.backend.tuner_reading();
            self.tuner.update(reading, now_ms);
        } else {
            self.tuner.reset();
        }
    }

    /// Current chrome dimensions used for live viewport scaling.
    #[must_use]
    pub fn window_chrome(&self) -> crate::shell::Chrome {
        self.window_chrome
    }

    /// Whether the hint strip is visible.
    #[must_use]
    pub fn hints_enabled(&self) -> bool {
        self.hints.enabled()
    }

    /// Contextual help in the hint strip.
    #[must_use]
    pub fn hint(&self) -> &str {
        self.hints.current()
    }

    /// Changes the contextual help shown in the hint strip.
    pub fn set_hint(&mut self, text: impl Into<String>) {
        self.hints.set_current(text);
    }

    /// Opens the account menu.
    pub fn open_account(&mut self) {
        self.overlays.open(Overlay::Account, "account");
    }

    /// Opens one advanced faceplate deck.
    pub fn open_deck(&mut self, kind: super::decks::DeckKind) {
        self.faceplate.open_deck(kind);
        if kind == super::decks::DeckKind::AutoBalance {
            self.pin_toast("Listening", 0);
        }
    }

    /// Whether the sign-in page or the key page replaces the signal chain.
    #[must_use]
    pub const fn auth_active(&self) -> bool {
        self.session.page_active() || self.key_page.is_some()
    }

    /// Shows an already initialized authentication flow without restarting it.
    pub fn activate_auth(&mut self) {
        self.session.show_page();
    }

    /// Starts system-browser PKCE sign-in and closes the account menu.
    ///
    /// # Errors
    /// Returns a sign-in problem that the toast also shows.
    pub fn sign_in(&mut self) -> Result<(), UiError> {
        if self.session.status() == SessionStatus::NoKey {
            self.open_key_page();
            return Ok(());
        }
        if let Err(problem) = self.session.start_browser() {
            return Err(self.sign_in_problem(problem.to_string()));
        }
        self.overlays.dismiss(DismissReason::Action);
        Ok(())
    }

    /// Shows a sign-in problem, because callers cannot show a returned error.
    fn sign_in_problem(&mut self, message: String) -> UiError {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as i64);
        self.toast.show(message.clone(), now_ms);
        self.hints.set_toast(Some(message.clone()));
        self.overlays.dismiss(DismissReason::Action);
        UiError::Control(message)
    }

    /// Drops the catalog results that belong to the previous session.
    fn after_session_change(&mut self, before: SessionStatus) {
        if self.session.status() == before {
            return;
        }
        let service = self.community.is_some();
        if let Some(community) = &mut self.community {
            // The generation moves first, so a compile that finishes now cannot publish.
            self.load_flow.abandon();
            if let Some(detail) = &mut self.detail {
                detail.abandon_model_work();
            }
            community.cancel(CommunityScope::Session);
            self.detail_work_live = false;
            self.public_user = None;
            // A later sign-in can be another account, so the cache does not come back.
            self.cached_account = None;
            self.account_avatar = None;
            self.user_refresh_due = self.session.status().signed_in();
        }
        // A closed browser shows its results again at the next open.
        for browser in self
            .browser
            .iter_mut()
            .chain(self.parked_browser.iter_mut())
        {
            browser.store_mut().drop_session_results();
            if service {
                browser.drop_session_filters();
            }
        }
    }

    /// Starts device-code sign-in without blocking the UI thread.
    ///
    /// # Errors
    /// Returns a sign-in problem that the toast also shows.
    pub fn start_device_sign_in(&mut self) -> Result<(), UiError> {
        if self.session.status() == SessionStatus::NoKey {
            self.open_key_page();
            return Ok(());
        }
        self.session
            .start_device()
            .map_err(|problem| self.sign_in_problem(problem.to_string()))
    }

    /// Applies a finished sign-in. This needs no UI, so it runs while the window is covered.
    pub fn poll_auth(&mut self) {
        let before = self.session.status();
        if let Some(SessionEvent::Problem(message)) = self.session.poll() {
            let _ = self.sign_in_problem(message);
        }
        self.after_session_change(before);
    }

    /// Applies completed auth results and opens the pending system-browser page.
    pub fn sync_auth(&mut self, ui: &mut egui::Ui) {
        self.poll_auth();
        self.poll_community();
        if let Some(url) = self.session.take_url_to_open() {
            open_web_url(ui.ctx(), url);
        }
        if let Some(page) = &mut self.key_page {
            match super::key_setup::show(ui, page) {
                Some(super::key_setup::KeyPageAction::OpenSite) => {
                    if let Some(setup) = &self.key_setup {
                        open_web_url(ui.ctx(), setup.origin());
                    }
                }
                Some(super::key_setup::KeyPageAction::Save) => {
                    let _ = self.save_key();
                }
                Some(super::key_setup::KeyPageAction::NotNow) => self.close_key_page(),
                None => {}
            }
            return;
        }
        let Some(snapshot) = self.session.page_snapshot() else {
            return;
        };
        let session = &mut self.session;
        match super::sign_in::show_snapshot(ui, snapshot, || session.copy_link()) {
            Some(super::sign_in::SignInAction::StartDevice) => {
                let _ = self.start_device_sign_in();
            }
            Some(super::sign_in::SignInAction::Retry) => {
                let _ = self.sign_in();
            }
            Some(super::sign_in::SignInAction::Close) => self.close_auth(),
            None => {}
        }
    }

    /// Cancels the current authentication flow and closes the key page.
    pub fn close_auth(&mut self) {
        self.session.cancel();
        self.key_page = None;
    }

    /// Signs out and closes the account menu.
    ///
    /// # Errors
    /// Returns an authentication or session-persistence error.
    pub fn sign_out(&mut self) -> Result<(), UiError> {
        let before = self.session.status();
        self.session
            .sign_out()
            .map_err(|error| UiError::Control(error.to_string()))?;
        self.after_session_change(before);
        self.overlays.dismiss(DismissReason::Action);
        Ok(())
    }

    /// Opens the save-preset editor.
    pub fn open_save(&mut self) {
        self.overlays.open(Overlay::SavePreset, "save");
    }

    /// Opens the preset browser.
    pub fn open_presets(&mut self) {
        self.overlays.open(Overlay::PresetBrowser, "presets");
    }

    /// Dismisses the current overlay and restores its anchor.
    pub fn dismiss(&mut self, reason: DismissReason) {
        self.overlays.dismiss(reason);
    }

    /// Changes the save draft.
    pub fn set_draft(&mut self, draft: impl Into<String>) {
        self.presets.set_draft(draft);
    }

    /// Saves a preset, announces success, and closes the editor.
    ///
    /// # Errors
    /// Returns validation or storage errors and leaves the editor open.
    pub fn save(&mut self) -> Result<(), UiError> {
        self.presets.save()?;
        self.overlays.dismiss(DismissReason::Action);
        self.hints.set_toast(Some("Preset Saved".to_owned()));
        self.announce("Preset Saved");
        Ok(())
    }

    /// Loads one preset and closes the browser.
    ///
    /// # Errors
    /// Returns a backend load error.
    pub fn load_preset(&mut self, id: &str) -> Result<(), UiError> {
        self.presets.load_preset(id)?;
        self.overlays.dismiss(DismissReason::Action);
        Ok(())
    }

    /// Renames one preset.
    ///
    /// # Errors
    /// Returns validation or storage errors.
    pub fn rename_preset(&mut self, id: &str, name: &str) -> Result<(), UiError> {
        self.presets.rename_preset(id, name)
    }

    /// Deletes one preset.
    ///
    /// # Errors
    /// Returns missing-target or storage errors.
    pub fn delete_preset(&mut self, id: &str) -> Result<(), UiError> {
        self.presets.delete_preset(id)
    }

    /// Moves one preset within its section.
    ///
    /// # Errors
    /// Returns invalid-move or storage errors.
    pub fn move_preset(&mut self, id: &str, delta: i32) -> Result<(), UiError> {
        self.presets.move_preset(id, delta)
    }

    /// Loads the next preset.
    ///
    /// # Errors
    /// Returns a backend load error.
    pub fn next_preset(&mut self) -> Result<(), UiError> {
        self.presets.next_preset()
    }

    /// Loads the previous preset.
    ///
    /// # Errors
    /// Returns a backend load error.
    pub fn previous_preset(&mut self) -> Result<(), UiError> {
        self.presets.previous_preset()
    }

    /// Changes PC badge visibility.
    pub fn set_show_pc(&mut self, show: bool) {
        self.presets.set_show_pc(show);
    }

    /// One-based PC badge.
    #[must_use]
    pub fn pc_badge(&self, index: usize) -> Option<String> {
        self.presets.pc_badge(index)
    }

    /// Changes stereo mode.
    ///
    /// # Errors
    /// Returns a backend chain error.
    pub fn set_stereo(&mut self, enabled: bool) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::SetStereoMode(enabled))?;
        Ok(())
    }

    /// Shows or hides the tuner.
    pub fn set_tuner(&mut self, visible: bool) {
        self.tuner_visible = visible;
        if !visible {
            self.tuner.reset();
        }
    }

    /// Opens the input-routing chooser above the faceplate trigger.
    pub fn open_input_mode(&mut self) {
        self.faceplate.open_input_mode();
    }

    /// Cycles input routing through the device boundary.
    ///
    /// # Errors
    /// Returns a backend device error.
    pub fn cycle_input_mode(&mut self) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::Device(
            super::super::backend::DeviceAction::CycleInputMode,
        ))?;
        Ok(())
    }

    /// Undoes one edit.
    ///
    /// # Errors
    /// Returns a backend history error.
    pub fn undo(&mut self) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::Undo)?;
        Ok(())
    }

    /// Redoes one edit.
    ///
    /// # Errors
    /// Returns a backend history error.
    pub fn redo(&mut self) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::Redo)?;
        Ok(())
    }

    /// Restores the default chain.
    ///
    /// # Errors
    /// Returns a backend chain error.
    pub fn reset(&mut self) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::ResetToDefault)?;
        Ok(())
    }

    /// Changes hint visibility.
    pub fn set_hints(&mut self, enabled: bool) {
        self.hints.set_enabled(enabled);
    }

    /// Queues an accessible announcement.
    pub fn announce(&mut self, message: impl Into<String>) {
        self.announcements.announce(message);
    }

    /// Drains accessible announcements.
    #[must_use]
    pub fn drain_announcements(&mut self) -> Vec<String> {
        let mut first = self.announcements.drain();
        first.extend(self.overlays.drain_announcements());
        first.extend(self.toast.drain_announcements());
        let mut unique = Vec::with_capacity(first.len());
        for message in first {
            if !unique.contains(&message) {
                unique.push(message);
            }
        }
        unique
    }

    /// Coalesces active-edit revisions for 400 ms.
    #[must_use]
    pub fn coalesce_edit(&mut self, now_ms: u64, revision: u64) -> u64 {
        self.presets.coalesce_edit(now_ms, revision)
    }
}

/// Renders the Default header, preset surfaces, account menu, hints, and toast.
pub fn show(ui: &mut egui::Ui, chrome: &mut ChromeController) {
    let background_events = chrome.system_modal_visible().then(|| {
        ui.ctx()
            .input_mut(|input| std::mem::take(&mut input.events))
    });
    if chrome.settings.is_some() {
        chrome.sync_settings(ui);
        close_hidden_drags(ui, chrome);
        if let Some(events) = background_events {
            ui.ctx().input_mut(|input| input.events = events);
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as i64);
        show_dialogs(ui, chrome, now_ms);
        show_toast(ui, chrome);
        return;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64);
    chrome.sync_banners(now_ms);
    chrome.sync_toast(now_ms);
    if chrome.banner_visible {
        let action = chrome.banners.active().cloned().and_then(|banner| {
            let mut action = None;
            egui::Panel::top("default-app-banner")
                .exact_size(super::banners::BANNER_HEIGHT as f32)
                .frame(egui::Frame::NONE.fill(Color32::BLACK))
                .show(ui, |ui| {
                    action = super::banners::show(ui, &banner);
                });
            action
        });
        if let Some(action) = action {
            let _ = chrome.handle_banner_view_action(action, now_ms);
        }
    }
    let state = chrome.backend.chain_state(0);
    chrome.sync_remote_images(ui.ctx(), &state.chain);
    show_header(ui, chrome, &state);
    show_overlay(ui, chrome);
    show_hints(ui, chrome);
    if chrome.browser.is_none() && !chrome.auth_active() {
        egui::Panel::bottom("default-faceplate")
            .exact_size(108.0)
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ui, |ui| chrome.faceplate.show(ui));
    }
    chrome.sync_auth(ui);
    if !chrome.auth_active() {
        if chrome.tuner_visible {
            chrome.sync_tuner(now_ms);
            let shift = super::tuning::shift(chrome.backend.as_ref());
            let close = chrome.tuner_view.show(ui, chrome.tuner.state(), shift);
            if close {
                chrome.set_tuner(false);
            }
        } else {
            chrome.sync_detail();
            chrome.sync_browser();
            let session = chrome.session();
            let action = chrome
                .browser
                .as_mut()
                .and_then(|browser| browser.show(ui, session));
            if let Some(action) = action {
                match action {
                    BrowserAction::Tone(tone) => chrome.pick_tone(tone),
                    BrowserAction::Favorite { tone, favorite } => {
                        chrome.set_favorite(tone, favorite);
                    }
                    BrowserAction::SignIn => {
                        let _ = chrome.sign_in();
                    }
                    BrowserAction::Close => chrome.load_flow.close(),
                }
            }
            chrome.sync_browser();
            if chrome.browser.is_none() {
                if let Some(detail) = &mut chrome.detail {
                    chrome.gallery.show_meters(ui);
                    detail.show(ui, session);
                    if detail.take_sign_in_request() {
                        let _ = chrome.sign_in();
                    }
                } else {
                    chrome.gallery.show(ui);
                }
            }
        }
    }
    close_hidden_drags(ui, chrome);
    if let Some(events) = background_events {
        ui.ctx().input_mut(|input| input.events = events);
    }
    show_dialogs(ui, chrome, now_ms);
    show_toast(ui, chrome);
}

/// Closes the knob drag of each page that this pass did not show.
///
/// A page such as the browser, the tuner, or the settings can cover the whole faceplate, chain
/// gallery, or block detail during a knob drag. The covered page then runs no frame of its own,
/// so the page switch closes its drag in the first pass that does not show it. A page that this
/// pass showed closed its own hidden drags already, and this call changes nothing for it.
fn close_hidden_drags(ui: &egui::Ui, chrome: &mut ChromeController) {
    let pass = ui.ctx().cumulative_pass_nr();
    chrome.faceplate.close_hidden_drags(pass);
    chrome.gallery.close_hidden_drags(pass);
    if let Some(detail) = &mut chrome.detail {
        detail.close_hidden_drags(pass);
    }
}

/// Paints the system dialogs, except in a pass that the window captures for their backdrop.
fn show_dialogs(ui: &mut egui::Ui, chrome: &mut ChromeController, now_ms: i64) {
    if !chrome.system_modal_visible() {
        chrome.dialog_backdrop_ready = false;
    }
    if chrome.dialogs_held() {
        return;
    }
    chrome.sync_modals(ui);
    chrome.sync_updates(ui, now_ms);
    chrome.sync_connection(ui, now_ms);
}

fn show_header(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &crate::backend::ChainViewState,
) {
    egui::Panel::top("default-header")
        .exact_size(64.0)
        .frame(egui::Frame::NONE.fill(Color32::BLACK))
        .show(ui, |ui| {
            let bounds = ui.available_rect_before_wrap();
            let painter = ui.painter_at(bounds);
            painter.line_segment(
                [
                    egui::pos2(bounds.left(), bounds.bottom()),
                    egui::pos2(bounds.right(), bounds.bottom()),
                ],
                egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
            );
            let logo = egui::Rect::from_min_size(
                egui::pos2(bounds.left() + 24.0, bounds.top() + 8.0),
                egui::Vec2::splat(48.0),
            );
            super::brand::draw_launcher_icon(ui, logo);
            let name = painter.text(
                egui::pos2(bounds.left() + 80.0, bounds.top() + 32.0),
                Align2::LEFT_CENTER,
                "RONNIE",
                egui::FontId::monospace(24.0),
                Color32::WHITE,
            );
            let home = ui
                .interact(
                    logo.union(name),
                    ui.scope_id().with("default-header-home"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            home.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, HOME_LABEL));
            if home.clicked() {
                chrome.go_to_main();
            }

            let preset_name = chrome.active_preset_name().unwrap_or("Presets").to_owned();
            let preset_active = chrome.active_preset_name().is_some();
            let account_name = chrome
                .account_name()
                .map(|name| account_name_galley(ui, name));
            let name_room = account_name.as_ref().map_or(0.0, |(_, galley)| {
                galley.size().x.ceil() + ACCOUNT_NAME_MARGIN
            });
            // The four gaps give the room to the chip, so the presets stay under their popovers.
            let gap = 40.0 - name_room / 4.0;
            let account = egui::Rect::from_min_size(
                egui::pos2(bounds.right() - 95.0 - name_room, bounds.top() + 12.0),
                egui::vec2(71.0 + name_room, 40.0),
            );
            let redo = egui::Rect::from_center_size(
                egui::pos2(account.left() - gap - 14.0, bounds.top() + 32.0),
                egui::Vec2::splat(28.0),
            );
            let undo = egui::Rect::from_center_size(
                egui::pos2(redo.left() - 16.0 - 14.0, bounds.top() + 32.0),
                egui::Vec2::splat(28.0),
            );
            let tuner = egui::Rect::from_center_size(
                egui::pos2(undo.left() - gap - 14.0, bounds.top() + 32.0),
                egui::Vec2::splat(28.0),
            );
            let stereo = egui::Rect::from_min_size(
                egui::pos2(tuner.left() - gap - 80.0, bounds.top() + 14.0),
                egui::vec2(80.0, 36.0),
            );
            let presets = egui::Rect::from_min_size(
                egui::pos2(stereo.left() - gap - 274.0, bounds.top() + 14.0),
                egui::vec2(274.0, 36.0),
            );

            let pill = egui::Rect::from_min_size(presets.min, egui::vec2(202.0, 36.0));
            painter.rect_filled(pill, 8.0, crate::theme::palette::CONTROL_FILL);
            let previous = header_hit(
                ui,
                "preset-previous",
                egui::Rect::from_min_size(
                    pill.left_top() + egui::vec2(4.0, 0.0),
                    egui::vec2(22.0, 36.0),
                ),
                "Previous preset",
                !chrome.presets().is_empty(),
            );
            let next_rect = egui::Rect::from_min_size(
                egui::pos2(pill.right() - 26.0, pill.top()),
                egui::vec2(22.0, 36.0),
            );
            let next = header_hit(
                ui,
                "preset-next",
                next_rect,
                "Next preset",
                !chrome.presets().is_empty(),
            );
            draw_chevron(&painter, previous.rect, false, chrome.presets().is_empty());
            draw_chevron(&painter, next.rect, true, chrome.presets().is_empty());
            if previous.clicked() {
                let _ = chrome.previous_preset();
            }
            if next.clicked() {
                let _ = chrome.next_preset();
            }
            let name_rect = egui::Rect::from_min_max(
                egui::pos2(pill.left() + 26.0, pill.top()),
                egui::pos2(next_rect.left(), pill.bottom()),
            );
            let browse = header_hit(ui, "preset-browse", name_rect, "Browse presets", true);
            painter.with_clip_rect(name_rect).text(
                name_rect.center(),
                Align2::CENTER_CENTER,
                preset_name,
                egui::FontId::proportional(14.0),
                if preset_active {
                    Color32::WHITE
                } else {
                    crate::theme::palette::TEXT_MUTED
                },
            );
            if browse.clicked() {
                if chrome.overlay() == Overlay::PresetBrowser {
                    chrome.dismiss(DismissReason::Action);
                } else {
                    chrome.open_presets();
                }
            }

            let save = egui::Rect::from_min_size(
                presets.left_top() + egui::vec2(210.0, 4.0),
                egui::Vec2::splat(28.0),
            );
            let new = egui::Rect::from_min_size(
                presets.left_top() + egui::vec2(246.0, 4.0),
                egui::Vec2::splat(28.0),
            );
            let save_response = header_hit(ui, "preset-save", save, "Save preset", true);
            if save_response.clicked() {
                if chrome.overlay() == Overlay::SavePreset {
                    chrome.dismiss(DismissReason::Action);
                } else {
                    chrome.open_save();
                }
            }
            if header_hit(ui, "preset-new", new, "New preset", true).clicked() {
                let _ = chrome.reset();
            }
            draw_save(&painter, save);
            draw_plus(&painter, new, Color32::WHITE);

            painter.rect_filled(stereo, 18.0, crate::theme::palette::SURFACE_RAISED);
            let mono_rect = egui::Rect::from_min_size(
                stereo.left_top() + egui::vec2(4.0, 0.0),
                egui::vec2(36.0, 36.0),
            );
            let stereo_rect = egui::Rect::from_min_size(
                stereo.left_top() + egui::vec2(40.0, 0.0),
                egui::vec2(36.0, 36.0),
            );
            let selected = if state.chain.stereo_enabled {
                stereo_rect
            } else {
                mono_rect
            };
            painter.rect_filled(
                selected.shrink2(egui::vec2(0.0, 4.0)),
                14.0,
                crate::theme::palette::TINT_FILL,
            );
            if header_hit(ui, "mono-mode", mono_rect, "Mono mode", true).clicked() {
                let _ = chrome.set_stereo(false);
            }
            if header_hit(ui, "stereo-mode", stereo_rect, "Stereo mode", true).clicked() {
                let _ = chrome.set_stereo(true);
            }
            painter.circle_stroke(
                mono_rect.center(),
                5.0,
                egui::Stroke::new(1.5, mode_color(!state.chain.stereo_enabled)),
            );
            for offset in [-2.7, 2.7] {
                painter.circle_stroke(
                    stereo_rect.center() + egui::vec2(offset, 0.0),
                    5.0,
                    egui::Stroke::new(1.5, mode_color(state.chain.stereo_enabled)),
                );
            }

            if header_hit(ui, "tuner", tuner, "Tuner", true).clicked() {
                chrome.set_tuner(!chrome.tuner_visible());
            }
            if chrome.tuner_visible() {
                painter.rect_filled(tuner, 4.0, crate::theme::palette::TINT_FILL);
            }
            let tuning_origin = tuner.center() - egui::Vec2::splat(9.0);
            let tuning_point = |x: f32, y: f32| tuning_origin + egui::vec2(x * 0.75, y * 0.75);
            let tuning_stroke = egui::Stroke::new(1.5, Color32::WHITE);
            for x in [8.0, 16.0] {
                painter.line_segment([tuning_point(x, 3.0), tuning_point(x, 10.0)], tuning_stroke);
            }
            let mut tuning_curve = Vec::new();
            for index in 0..=24 {
                let angle = std::f32::consts::PI - index as f32 / 24.0 * std::f32::consts::PI;
                tuning_curve.push(tuning_point(
                    12.0 + 4.0 * angle.cos(),
                    10.0 + 4.0 * angle.sin(),
                ));
            }
            painter.add(egui::epaint::PathShape::line(tuning_curve, tuning_stroke));
            painter.line_segment(
                [tuning_point(12.0, 14.0), tuning_point(12.0, 21.0)],
                tuning_stroke,
            );
            let undo_response = header_hit(ui, "undo", undo, "Undo", state.can_undo);
            let redo_response = header_hit(ui, "redo", redo, "Redo", state.can_redo);
            draw_history(&painter, undo, false, history_color(state.can_undo));
            draw_history(&painter, redo, true, history_color(state.can_redo));
            if undo_response.clicked() {
                let _ = chrome.undo();
            }
            if redo_response.clicked() {
                let _ = chrome.redo();
            }

            // The name registers below the chip, so that the chip takes the click on it.
            if let Some((name, galley)) = account_name {
                let top_left =
                    account.left_center() + egui::vec2(ACCOUNT_NAME_LEFT, -galley.size().y / 2.0);
                ui.interact(
                    egui::Rect::from_min_size(top_left, galley.size()),
                    ui.scope_id().with("account-name"),
                    egui::Sense::hover(),
                )
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, name.clone()));
                painter.galley(top_left, galley, Color32::WHITE);
            }
            let account_response = header_hit(ui, "account", account, "Account", true);
            if account_response.clicked() {
                if chrome.overlay() == Overlay::Account {
                    chrome.dismiss(DismissReason::Action);
                } else {
                    chrome.open_account();
                }
            }
            chrome.overlay_trigger = match chrome.overlay() {
                Overlay::None => None,
                Overlay::Account => Some(Trigger::from(&account_response)),
                Overlay::SavePreset => Some(Trigger::from(&save_response)),
                Overlay::PresetBrowser => Some(Trigger::from(&browse)),
            };
            painter.rect_stroke(
                account,
                20.0,
                egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                egui::StrokeKind::Inside,
            );
            for y in [-4.5, 0.0, 4.5] {
                let center = account.left_center() + egui::vec2(22.0, y);
                painter.line_segment(
                    [center - egui::vec2(6.0, 0.0), center + egui::vec2(6.0, 0.0)],
                    egui::Stroke::new(1.5, Color32::WHITE),
                );
            }
            let avatar = account.left_center() + egui::vec2(53.0 + name_room, 0.0);
            if chrome.session().signed_in()
                && let Some(texture) = &chrome.account_avatar
            {
                egui::Image::from_texture(texture)
                    .corner_radius(12.0)
                    .paint_at(
                        ui,
                        egui::Rect::from_center_size(avatar, egui::Vec2::splat(24.0)),
                    );
            } else {
                painter.circle_filled(avatar, 12.0, crate::theme::palette::TEXT_MUTED);
                painter.circle_filled(
                    avatar + egui::vec2(0.0, -3.5),
                    4.0,
                    crate::theme::palette::SURFACE_RAISED,
                );
                painter.circle_filled(
                    avatar + egui::vec2(0.0, 8.0),
                    6.5,
                    crate::theme::palette::SURFACE_RAISED,
                );
            }
        });
}

/// One line of the account name, which the width limit can end with an ellipsis.
fn account_name_galley(ui: &egui::Ui, name: String) -> (String, Arc<egui::Galley>) {
    let mut job = egui::text::LayoutJob::single_section(
        name.clone(),
        egui::text::TextFormat {
            font_id: egui::FontId::proportional(12.5),
            color: Color32::WHITE,
            ..Default::default()
        },
    );
    job.wrap.max_width = ACCOUNT_NAME_MAX_WIDTH;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    (name, galley)
}

fn header_hit(
    ui: &mut egui::Ui,
    id: &'static str,
    rect: egui::Rect,
    label: &'static str,
    enabled: bool,
) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(id),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    response
}

fn mode_color(selected: bool) -> Color32 {
    if selected {
        Color32::WHITE
    } else {
        crate::theme::palette::TEXT_MUTED
    }
}

fn history_color(enabled: bool) -> Color32 {
    if enabled {
        Color32::WHITE
    } else {
        Color32::from_rgba_unmultiplied(141, 141, 147, 115)
    }
}

fn draw_history(painter: &egui::Painter, rect: egui::Rect, right: bool, color: Color32) {
    let origin = rect.center() - egui::Vec2::splat(9.0);
    let point = |x: f32, y: f32| origin + egui::vec2(x * 0.75, y * 0.75);
    let stroke = egui::Stroke::new(1.5, color);
    let direction = if right { -1.0 } else { 1.0 };
    let arrow_x = if right { 20.0 } else { 4.0 };
    let tip_x = if right { 15.0 } else { 9.0 };
    painter.add(egui::epaint::PathShape::line(
        vec![point(tip_x, 14.0), point(arrow_x, 9.0), point(tip_x, 4.0)],
        stroke,
    ));
    let mut curve = vec![point(arrow_x, 9.0)];
    let center_x = if right { 9.5 } else { 14.5 };
    curve.push(point(center_x, 9.0));
    for index in 1..=32 {
        let angle = index as f32 / 32.0 * std::f32::consts::PI;
        curve.push(point(
            center_x + direction * 5.5 * angle.sin(),
            14.5 - 5.5 * angle.cos(),
        ));
    }
    curve.push(point(if right { 13.0 } else { 11.0 }, 20.0));
    painter.add(egui::epaint::PathShape::line(curve, stroke));
}

pub(super) fn draw_chevron(painter: &egui::Painter, rect: egui::Rect, right: bool, muted: bool) {
    let color = if muted {
        crate::theme::palette::TEXT_MUTED
    } else {
        Color32::WHITE
    };
    let scale = 14.0 / 24.0;
    let point = |x: f32, y: f32| rect.center() + egui::vec2((x - 12.0) * scale, (y - 12.0) * scale);
    let points = if right {
        [(9.0, 18.0), (15.0, 12.0), (9.0, 6.0)]
    } else {
        [(15.0, 18.0), (9.0, 12.0), (15.0, 6.0)]
    };
    painter.add(egui::epaint::PathShape::line(
        points.into_iter().map(|(x, y)| point(x, y)).collect(),
        egui::Stroke::new(2.0 * scale, color),
    ));
}

fn draw_plus(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let center = rect.center();
    let stroke = egui::Stroke::new(1.5, color);
    painter.line_segment(
        [
            center - egui::vec2(5.25, 0.0),
            center + egui::vec2(5.25, 0.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center - egui::vec2(0.0, 5.25),
            center + egui::vec2(0.0, 5.25),
        ],
        stroke,
    );
}

fn draw_save(painter: &egui::Painter, rect: egui::Rect) {
    let origin = rect.center() - egui::Vec2::splat(9.0);
    let point = |x: f32, y: f32| origin + egui::vec2(x * 0.75, y * 0.75);
    let stroke = egui::Stroke::new(1.5, Color32::WHITE);
    painter.add(egui::epaint::PathShape::line(
        vec![
            point(15.2, 3.0),
            point(16.6, 3.6),
            point(20.4, 7.4),
            point(21.0, 8.8),
            point(21.0, 19.0),
            point(19.0, 21.0),
            point(5.0, 21.0),
            point(3.0, 19.0),
            point(3.0, 5.0),
            point(5.0, 3.0),
            point(15.2, 3.0),
        ],
        stroke,
    ));
    painter.add(egui::epaint::PathShape::line(
        vec![
            point(17.0, 21.0),
            point(17.0, 14.0),
            point(16.0, 13.0),
            point(8.0, 13.0),
            point(7.0, 14.0),
            point(7.0, 21.0),
        ],
        stroke,
    ));
    painter.add(egui::epaint::PathShape::line(
        vec![
            point(7.0, 3.0),
            point(7.0, 7.0),
            point(8.0, 8.0),
            point(15.0, 8.0),
        ],
        stroke,
    ));
}

fn show_overlay(ui: &mut egui::Ui, chrome: &mut ChromeController) {
    let trigger = chrome.overlay_trigger;
    let popover = |name: &str, panel: egui::Rect, radius: f32| {
        Popover::new(
            ui.scope_id().with(name),
            Placement::At(panel.min),
            panel.size(),
            Surface::edged(radius),
        )
        .trigger(trigger)
    };
    let shown = match chrome.overlay() {
        Overlay::None => return,
        Overlay::Account => {
            let root = ui.available_rect_before_wrap();
            let session = chrome.session();
            let panel = egui::Rect::from_min_size(
                egui::pos2(root.right() - 214.0, root.top() - 4.0),
                egui::vec2(
                    190.0,
                    if session == SessionStatus::NoService {
                        56.0
                    } else {
                        94.0
                    },
                ),
            );
            popover("account-popover", panel, 12.0).show(ui, |ui, panel| {
                let row = |index| {
                    egui::Rect::from_min_size(
                        panel.min + egui::vec2(8.0, 8.0 + index as f32 * 38.0),
                        egui::vec2(174.0, 38.0),
                    )
                };
                if menu_row(ui, row(0), "Settings", MenuIcon::Settings).clicked() {
                    chrome.open_settings();
                    chrome.dismiss(DismissReason::Action);
                }
                if session.signed_in() {
                    if menu_row(ui, row(1), "Logout", MenuIcon::Logout).clicked() {
                        let _ = chrome.sign_out();
                    }
                } else if session.can_sign_in()
                    && menu_row(ui, row(1), "Sign in", MenuIcon::SignIn).clicked()
                {
                    let _ = chrome.sign_in();
                }
                (chrome.overlay() != Overlay::Account).then_some(())
            })
        }
        Overlay::SavePreset => {
            let root = ui.available_rect_before_wrap();
            let error = chrome.save_error().map(str::to_owned);
            let panel = egui::Rect::from_min_size(
                egui::pos2(root.center().x - 208.0, root.top() - 4.0),
                egui::vec2(280.0, if error.is_some() { 168.0 } else { 144.0 }),
            );
            popover("save-preset-popover", panel, 12.0).show(ui, |ui, panel| {
                ui.painter_at(panel).text(
                    panel.min + egui::vec2(16.0, 16.0),
                    Align2::LEFT_TOP,
                    "Save Preset",
                    egui::FontId::new(14.0, egui::FontFamily::Name("arimo-bold".into())),
                    Color32::WHITE,
                );
                let input = egui::Rect::from_min_size(
                    panel.min + egui::vec2(16.0, 44.0),
                    egui::vec2(248.0, 35.0),
                );
                ui.painter_at(input).rect_stroke(
                    input,
                    6.0,
                    egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                    egui::StrokeKind::Inside,
                );
                let mut draft = chrome.presets.draft().to_owned();
                let response = ui.put(
                    input,
                    egui::TextEdit::singleline(&mut draft)
                        .hint_text("Name")
                        .font(egui::FontId::proportional(13.0))
                        .margin(egui::Margin {
                            left: 8,
                            right: 8,
                            top: 10,
                            bottom: 6,
                        }),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Name".to_owned())
                });
                if response.changed() {
                    chrome.set_draft(draft);
                }
                let save = egui::Rect::from_min_size(
                    panel.min + egui::vec2(16.0, 91.0),
                    egui::vec2(248.0, 35.0),
                );
                if !chrome.presets.draft().trim().is_empty()
                    && pill_button(ui, save, "Save", true).clicked()
                {
                    let _ = chrome.save();
                }
                if let Some(error) = error {
                    ui.painter_at(panel).text(
                        panel.min + egui::vec2(16.0, 136.0),
                        Align2::LEFT_TOP,
                        error,
                        egui::FontId::proportional(12.0),
                        crate::theme::palette::FLAME,
                    );
                }
                (chrome.overlay() != Overlay::SavePreset).then_some(())
            })
        }
        Overlay::PresetBrowser => {
            let root = ui.available_rect_before_wrap();
            let query = chrome.presets.search().to_lowercase();
            let presets = chrome
                .presets
                .presets()
                .iter()
                .filter(|preset| preset.name.to_lowercase().contains(&query))
                .cloned()
                .collect::<Vec<_>>();
            let panel = egui::Rect::from_min_size(
                egui::pos2(root.center().x - 205.0, root.top() - 4.0),
                egui::vec2(360.0, 69.0 + (presets.len() + 2) as f32 * 32.0),
            );
            popover("preset-browser-popover", panel, 14.0).show(ui, |ui, panel| {
                let search_rect = egui::Rect::from_min_size(
                    panel.min + egui::vec2(13.0, 13.0),
                    egui::vec2(264.0, 33.0),
                );
                ui.painter_at(search_rect).rect_stroke(
                    search_rect,
                    8.0,
                    egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                    egui::StrokeKind::Inside,
                );
                let mut search = chrome.presets.search().to_owned();
                let editor_rect = egui::Rect::from_min_max(
                    search_rect.min + egui::vec2(32.0, 0.0),
                    search_rect.max - egui::vec2(12.0, 0.0),
                );
                let mut editor = ui.new_child(egui::UiBuilder::new().max_rect(editor_rect));
                let search_response = editor
                    .with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut search)
                                .hint_text(
                                    egui::RichText::new("Search presets")
                                        .color(crate::theme::palette::TEXT_MUTED),
                                )
                                .font(egui::FontId::proportional(13.0))
                                .align(Align2::LEFT_CENTER)
                                .frame(egui::Frame::NONE),
                        )
                    })
                    .inner;
                let search_icon = search_rect.left_center() + egui::vec2(19.0, 0.0);
                let search_painter = ui.painter_at(search_rect);
                search_painter.circle_stroke(
                    search_icon - egui::Vec2::splat(14.0 / 24.0),
                    14.0 * 8.0 / 24.0,
                    egui::Stroke::new(14.0 * 2.0 / 24.0, crate::theme::palette::TEXT_MUTED),
                );
                search_painter.line_segment(
                    [
                        search_icon + egui::vec2(4.65, 4.65) * (14.0 / 24.0),
                        search_icon + egui::vec2(9.0, 9.0) * (14.0 / 24.0),
                    ],
                    egui::Stroke::new(14.0 * 2.0 / 24.0, crate::theme::palette::TEXT_MUTED),
                );
                search_response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::TextInput,
                        true,
                        "Search presets".to_owned(),
                    )
                });
                if search_response.changed() {
                    chrome.presets.set_search(search);
                }
                if preset_action(
                    ui,
                    egui::Rect::from_min_size(
                        panel.min + egui::vec2(283.0, 15.0),
                        egui::Vec2::splat(29.0),
                    ),
                    PresetAction::Pc,
                )
                .clicked()
                {
                    let show = chrome.pc_badge(0).is_none();
                    chrome.set_show_pc(show);
                }
                if preset_action(
                    ui,
                    egui::Rect::from_min_size(
                        panel.min + egui::vec2(318.0, 15.0),
                        egui::Vec2::splat(29.0),
                    ),
                    PresetAction::Reorder,
                )
                .clicked()
                {
                    chrome.presets.toggle_reordering();
                }
                let active = chrome.presets.active_preset_name().map(str::to_owned);
                let mut slot = 0;
                for (heading, factory) in [("Your Presets", false), ("Ronnie", true)] {
                    let header = egui::Rect::from_min_size(
                        panel.min + egui::vec2(13.0, 56.0 + slot as f32 * 32.0),
                        egui::vec2(334.0, 32.0),
                    );
                    ui.painter_at(header).text(
                        header.left_center() + egui::vec2(4.0, 0.0),
                        Align2::LEFT_CENTER,
                        heading,
                        egui::FontId::new(14.0, egui::FontFamily::Name("arimo-bold".into())),
                        crate::theme::palette::TEXT_MUTED,
                    );
                    slot += 1;
                    for (index, preset) in presets
                        .iter()
                        .enumerate()
                        .filter(|(_, preset)| preset.factory == factory)
                    {
                        let row = egui::Rect::from_min_size(
                            panel.min + egui::vec2(13.0, 56.0 + slot as f32 * 32.0),
                            egui::vec2(334.0, 32.0),
                        );
                        if preset_row(
                            ui,
                            row,
                            &preset.name,
                            active.as_deref() == Some(preset.name.as_str()),
                        )
                        .clicked()
                        {
                            let _ = chrome.load_preset(&preset.id);
                        }
                        if let Some(badge) = chrome.pc_badge(index) {
                            ui.painter_at(row).text(
                                egui::pos2(row.right() - 12.0, row.center().y),
                                Align2::RIGHT_CENTER,
                                badge,
                                egui::FontId::monospace(11.0),
                                crate::theme::palette::TEXT_MUTED,
                            );
                        }
                        if !factory {
                            let actions = if chrome.presets.reordering() {
                                [(PresetAction::Up, -1), (PresetAction::Down, 1)].as_slice()
                            } else {
                                &[(PresetAction::Delete, 0), (PresetAction::Rename, 0)]
                            };
                            for (action_index, (action, delta)) in actions.iter().enumerate() {
                                let action_rect = egui::Rect::from_center_size(
                                    egui::pos2(
                                        row.right() - 14.0 - action_index as f32 * 28.0,
                                        row.center().y,
                                    ),
                                    egui::Vec2::splat(24.0),
                                );
                                if preset_action(ui, action_rect, *action).clicked() {
                                    match action {
                                        PresetAction::Rename => {
                                            let name = format!("{} renamed", preset.name);
                                            let _ = chrome.rename_preset(&preset.id, &name);
                                        }
                                        PresetAction::Delete => {
                                            let _ = chrome.delete_preset(&preset.id);
                                        }
                                        PresetAction::Up | PresetAction::Down => {
                                            let _ = chrome.move_preset(&preset.id, *delta);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        slot += 1;
                    }
                }
                (chrome.overlay() != Overlay::PresetBrowser).then_some(())
            })
        }
    };
    if let Some(reason) = shown.dismissed {
        chrome.dismiss(reason);
    }
}

fn preset_row(ui: &mut egui::Ui, rect: egui::Rect, name: &str, active: bool) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("preset-row", name)),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.to_owned()));
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 8.0, Color32::from_white_alpha(16));
    }
    if active {
        let center = egui::pos2(rect.left() + 12.0, rect.center().y);
        let point = |x: f32, y: f32| {
            center + egui::vec2((x - 12.0) * 14.0 / 24.0, (y - 12.0) * 14.0 / 24.0)
        };
        painter.add(egui::epaint::PathShape::line(
            vec![point(20.0, 6.0), point(9.0, 17.0), point(4.0, 12.0)],
            egui::Stroke::new(14.0 * 2.0 / 24.0, Color32::WHITE),
        ));
    }
    painter.text(
        egui::pos2(rect.left() + 28.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(14.0),
        if active {
            Color32::WHITE
        } else {
            crate::theme::palette::TEXT_MUTED
        },
    );
    response
}

#[derive(Debug, Clone, Copy)]
enum PresetAction {
    Pc,
    Reorder,
    Rename,
    Delete,
    Up,
    Down,
}

impl PresetAction {
    fn label(self) -> &'static str {
        match self {
            Self::Pc => "MIDI PC numbers",
            Self::Reorder => "Reorder presets",
            Self::Rename => "Rename",
            Self::Delete => "Delete",
            Self::Up => "Move preset up",
            Self::Down => "Move preset down",
        }
    }
}

fn preset_action(ui: &mut egui::Ui, rect: egui::Rect, action: PresetAction) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with((
            "preset-action",
            action.label(),
            rect.left() as i32,
            rect.top() as i32,
        )),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, action.label().to_owned())
    });
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 4.0, Color32::from_white_alpha(24));
    }
    let center = rect.center();
    let glyph = match action {
        PresetAction::Rename | PresetAction::Delete => 13.0,
        _ => 15.0,
    };
    let color = match action {
        PresetAction::Rename | PresetAction::Delete => Color32::WHITE,
        _ => crate::theme::palette::TEXT_MUTED,
    };
    let stroke = egui::Stroke::new(glyph * 2.0 / 24.0, color);
    let point =
        |x: f32, y: f32| center + egui::vec2((x - 12.0) * glyph / 24.0, (y - 12.0) * glyph / 24.0);
    match action {
        PresetAction::Pc => {
            painter.circle_stroke(center, 10.0 * glyph / 24.0, stroke);
            painter.add(egui::epaint::PathShape::line(
                vec![
                    point(15.0, 2.458),
                    point(15.0, 5.0),
                    point(14.8, 5.5),
                    point(14.4, 5.9),
                    point(14.0, 6.0),
                    point(10.0, 6.0),
                    point(9.6, 5.9),
                    point(9.2, 5.5),
                    point(9.0, 5.0),
                    point(9.0, 2.458),
                ],
                stroke,
            ));
            for (x, y) in [
                (12.0, 18.0),
                (16.0, 16.0),
                (18.0, 12.0),
                (6.0, 12.0),
                (8.0, 16.0),
            ] {
                painter.circle_filled(point(x, y), glyph / 24.0, color);
            }
        }
        PresetAction::Reorder => {
            for points in [
                vec![point(21.0, 16.0), point(17.0, 20.0), point(13.0, 16.0)],
                vec![point(17.0, 20.0), point(17.0, 4.0)],
                vec![point(3.0, 8.0), point(7.0, 4.0), point(11.0, 8.0)],
                vec![point(7.0, 4.0), point(7.0, 20.0)],
            ] {
                painter.add(egui::epaint::PathShape::line(points, stroke));
            }
        }
        PresetAction::Rename => {
            painter.add(egui::epaint::PathShape::line(
                vec![
                    point(21.174, 6.812),
                    point(20.9, 5.4),
                    point(20.2, 4.1),
                    point(19.2, 3.2),
                    point(18.2, 2.825),
                    point(17.188, 2.825),
                    point(3.842, 16.174),
                    point(3.342, 17.004),
                    point(2.021, 21.356),
                    point(2.644, 21.978),
                    point(6.997, 20.658),
                    point(7.827, 20.161),
                    point(21.174, 6.812),
                ],
                stroke,
            ));
            painter.line_segment([point(15.0, 5.0), point(19.0, 9.0)], stroke);
        }
        PresetAction::Delete => {
            painter.line_segment([point(10.0, 11.0), point(10.0, 17.0)], stroke);
            painter.line_segment([point(14.0, 11.0), point(14.0, 17.0)], stroke);
            painter.add(egui::epaint::PathShape::line(
                vec![
                    point(19.0, 6.0),
                    point(19.0, 18.8),
                    point(18.8, 19.5),
                    point(18.3, 20.0),
                    point(17.0, 20.0),
                    point(7.0, 20.0),
                    point(6.2, 20.0),
                    point(5.5, 19.6),
                    point(5.1, 19.0),
                    point(5.0, 6.0),
                ],
                stroke,
            ));
            painter.line_segment([point(3.0, 6.0), point(21.0, 6.0)], stroke);
            painter.add(egui::epaint::PathShape::line(
                vec![
                    point(8.0, 6.0),
                    point(8.0, 4.0),
                    point(8.3, 3.2),
                    point(9.0, 2.4),
                    point(10.0, 2.0),
                    point(14.0, 2.0),
                    point(15.0, 2.4),
                    point(15.7, 3.2),
                    point(16.0, 4.0),
                    point(16.0, 6.0),
                ],
                stroke,
            ));
        }
        PresetAction::Up | PresetAction::Down => {
            let direction = if matches!(action, PresetAction::Up) {
                -1.0
            } else {
                1.0
            };
            painter.line_segment([center + egui::vec2(-4.0, 2.0 * direction), center], stroke);
            painter.line_segment([center, center + egui::vec2(4.0, 2.0 * direction)], stroke);
        }
    }
    response
}

#[derive(Debug, Clone, Copy)]
enum MenuIcon {
    Settings,
    Logout,
    SignIn,
}

fn menu_row(ui: &mut egui::Ui, rect: egui::Rect, label: &str, icon: MenuIcon) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("popover-row", label)),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.to_owned()));
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 8.0, crate::theme::palette::HOVER_FILL);
    }
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 21.0, rect.center().y),
        egui::Vec2::splat(18.0),
    );
    let stroke = egui::Stroke::new(1.3, Color32::WHITE);
    match icon {
        MenuIcon::Settings => {
            let center = icon_rect.center();
            painter.circle_stroke(center, 3.0, stroke);
            for index in 0..8 {
                let angle = index as f32 * std::f32::consts::TAU / 8.0;
                let direction = egui::vec2(angle.cos(), angle.sin());
                painter.line_segment([center + direction * 5.0, center + direction * 8.0], stroke);
            }
        }
        MenuIcon::Logout | MenuIcon::SignIn => {
            let center = icon_rect.center();
            let door =
                egui::Rect::from_center_size(center - egui::vec2(3.0, 0.0), egui::vec2(9.0, 14.0));
            painter.rect_stroke(door, 1.0, stroke, egui::StrokeKind::Inside);
            let direction = if matches!(icon, MenuIcon::Logout) {
                1.0
            } else {
                -1.0
            };
            painter.line_segment(
                [
                    center - egui::vec2(4.0 * direction, 0.0),
                    center + egui::vec2(7.0 * direction, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(7.0 * direction, 0.0),
                    center + egui::vec2(3.0 * direction, -4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(7.0 * direction, 0.0),
                    center + egui::vec2(3.0 * direction, 4.0),
                ],
                stroke,
            );
        }
    }
    painter.text(
        egui::pos2(rect.left() + 42.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        Color32::WHITE,
    );
    response
}

fn pill_button(ui: &mut egui::Ui, rect: egui::Rect, label: &str, enabled: bool) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("pill", label)),
        egui::Sense::click(),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label.to_owned()));
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        rect.height() / 2.0,
        egui::Stroke::new(1.0, Color32::from_white_alpha(150)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        if enabled {
            Color32::WHITE
        } else {
            Color32::GRAY
        },
    );
    response
}

fn show_hints(ui: &mut egui::Ui, chrome: &mut ChromeController) {
    if !chrome.hints_enabled() {
        return;
    }
    let cpu = chrome.backend.meter_levels().cpu * 100.0;
    egui::Panel::bottom("default-hints")
        .exact_size(36.0)
        .frame(egui::Frame::NONE.fill(Color32::BLACK))
        .show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            let painter = ui.painter_at(rect);
            painter.line_segment(
                [rect.left_top(), rect.right_top()],
                egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
            );
            painter.text(
                egui::pos2(rect.left() + 24.0, rect.center().y),
                Align2::LEFT_CENTER,
                chrome.hints.current(),
                egui::FontId::proportional(13.0),
                crate::theme::palette::TEXT_SECONDARY,
            );
            let hide = egui::Rect::from_center_size(
                egui::pos2(rect.right() - 34.0, rect.center().y),
                egui::Vec2::splat(20.0),
            );
            let response =
                ui.interact(hide, ui.scope_id().with("hide-hints"), egui::Sense::click());
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Hide hints"));
            painter.text(
                hide.center(),
                Align2::CENTER_CENTER,
                "×",
                egui::FontId::proportional(18.0),
                Color32::WHITE,
            );
            if response.clicked() {
                chrome.set_hints(false);
            }
            painter.text(
                egui::pos2(rect.right() - 96.0, rect.center().y),
                Align2::CENTER_CENTER,
                format!("CPU {cpu:.1}%"),
                egui::FontId::proportional(12.0),
                crate::theme::palette::TEXT_SECONDARY,
            );
        });
}

fn show_toast(ui: &mut egui::Ui, chrome: &ChromeController) {
    if let Some(toast) = chrome.visible_toast() {
        let bottom_offset = 108.0
            + 24.0
            + if chrome.window_chrome.hints {
                36.0
            } else {
                0.0
            };
        super::toast::show(ui, toast, bottom_offset);
    }
}

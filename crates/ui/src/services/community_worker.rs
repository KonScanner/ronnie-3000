//! Background catalog work for one community service.
//!
//! [`CommunityWorker`] runs the catalog, taxonomy, favorite, and user requests on a runtime that
//! the caller names, and [`CommunityWorker::poll`] takes what finished without blocking. Only
//! the newest work of each kind stays live. A superseded or cancelled task has no live ticket,
//! so its output goes nowhere, even when the task finished before the poll.
//!
//! The token store does blocking file I/O, and a refresh rotates a token that the server
//! forgets. [`OffThreadTokens`] therefore runs each token lookup on a blocking thread. The
//! lookup does not stop the runtime threads, and an abort of its caller does not lose a rotated
//! token.
//!
//! A tone load and a model switch also change the audio chain, which a ticket from the load flow
//! guards. A cancellation that aborts a task cannot stop a compile, so the ticket has the last
//! word. Both kinds share one compile at a time.
//!
//! An event carries public results, generations, and fixed failure text. No event carries a
//! token, a response body, or a request URL. A model list holds the public model addresses that
//! the service gave.

use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

use ronie_core::{
    ApiError, DownloadError, FetchRequest, HttpError, HttpTransport, ModelBytes, ModelKind,
    ModelSource, Page, SecretString, Taxonomy, TaxonomyEntry, TokenProvider, Tone, ToneId,
    ToneModel, ToneQuery,
};
use serde_json::Value;
use tokio::{
    runtime::Handle,
    sync::Semaphore,
    task::{AbortHandle, JoinSet},
};

use crate::backend::UiError;

use super::{
    community::{CommunityAccess, CommunityHandle},
    images::{HttpImageFetcher, ImageFetcher},
    load_flow::{LoadTicket, ModelSwitchTicket},
};

/// Tones in one catalog page.
pub const CATALOG_PAGE_SIZE: u32 = 12;
/// Rows in one taxonomy page.
pub const TAXONOMY_PAGE_SIZE: u32 = 25;
/// Rows in one model list. A tone has at most this many models.
pub const MODEL_PAGE_SIZE: u32 = 300;

/// A catalog query has no architecture filter.
const ANY_ARCHITECTURE: i32 = -1;
/// The architecture of a NAM model that the audio engine can run.
const NAM_ARCHITECTURE: i32 = 2;
/// A tone load needs one model, and a later model list shows the others.
const TONE_LOAD_MODELS: u32 = 1;
/// Compiles at the same time. A load that waits for this has no compile to finish, so its abort
/// frees its bytes. This keeps the bytes in flight to one compile and one newest download.
const MAX_PREPARATIONS: usize = 1;
/// Most favorite writes in flight. A write cannot stop a newer write, so each one needs a limit.
const MAX_FAVORITE_WRITES: usize = 8;
/// A request that follows another request within this time is part of a burst.
const BURST_WINDOW: Duration = Duration::from_millis(300);
/// How long a request in a burst waits before it transmits. A newer request cancels the wait.
const BURST_QUIET: Duration = Duration::from_millis(120);

const SERVICE_UNREACHABLE: &str = "The community service did not answer.";
const SERVICE_REFUSED: &str = "The community service refused the request.";
const SERVICE_INVALID: &str = "The community service sent an answer that is not valid.";
const WORK_FAILED: &str = "The community request stopped.";
const TOO_MANY_WRITES: &str = "Too many favorite changes are in progress.";
const TONE_WRITE_BUSY: &str = "This tone has a favorite change in progress.";
const NO_MODEL: &str = "This tone has no model to load.";
const MODEL_ADDRESS_REFUSED: &str = "The model is not on an approved address.";
const MODEL_EMPTY: &str = "The model download was empty.";
const MODEL_TOO_LARGE: &str = "The model is larger than the limit.";
const MODEL_UNUSABLE: &str = "The model could not be prepared.";

/// Fields of a user that are public. The cache and the events keep no other field.
const PUBLIC_USER_FIELDS: [&str; 5] = ["id", "username", "display_name", "name", "avatar_url"];

/// One catalog page to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    /// Store generation that this request answers.
    pub generation: u64,
    /// Query to send. A signed-out request sends only its gear.
    pub query: ToneQuery,
    /// One-based page.
    pub page: u32,
    /// Whether to use the catalog. A signed-out user gets only the trending preview.
    pub signed_in: bool,
}

/// One taxonomy page to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxonomyRequest {
    /// Store generation that this request answers.
    pub generation: u64,
    /// Taxonomy family.
    pub kind: Taxonomy,
    /// Text that the menu search holds.
    pub text: String,
}

/// One remote tone to download and publish.
#[derive(Debug, Clone)]
pub struct LoadRequest {
    /// Tone to fetch. A catalog row is only a summary of it.
    pub tone: ToneId,
    /// The right to publish, which a cancellation takes away.
    pub ticket: LoadTicket,
}

/// The model list of one tone to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsRequest {
    /// Picker generation that this request answers.
    pub generation: u64,
    /// Tone that owns the models.
    pub tone: ToneId,
    /// Family of the tone, which selects the architecture filter.
    pub kind: ModelKind,
}

/// One remote model to download and publish in place of the active model.
#[derive(Debug, Clone)]
pub struct ModelSwitchRequest {
    /// Stored tone of the block.
    pub tone: Tone,
    /// Model to download.
    pub model: ToneModel,
    /// The right to publish, which a cancellation takes away.
    pub ticket: ModelSwitchTicket,
}

/// The models of one tone.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelList {
    /// At most one page of models.
    pub models: Vec<ToneModel>,
    /// Number of models that the service reports for the tone.
    pub total: u64,
}

/// One piece of work that a controller can start.
#[derive(Debug, Clone)]
pub enum CommunityWork {
    /// Fetch one catalog page.
    Query(QueryRequest),
    /// Fetch one taxonomy page.
    Taxonomy(TaxonomyRequest),
    /// Change the favorite state of one tone.
    Favorite {
        /// Tone to change.
        tone: ToneId,
        /// New state.
        favorite: bool,
    },
    /// Fetch and cache the public user.
    RefreshUser,
    /// Download one tone and publish it to the audio chain.
    ToneLoad(LoadRequest),
    /// Fetch the model list of one tone.
    Models(ModelsRequest),
    /// Download one model and publish it to the audio chain.
    ModelSwitch(ModelSwitchRequest),
}

/// Which work a cancellation stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityScope {
    /// The browser closed. A favorite write continues, but nothing reports its result.
    Browser,
    /// The block detail closed. Its model list and its switch stop.
    Detail,
    /// The session changed. All work stops.
    Session,
}

/// Why work did not finish. The text is fixed, so it holds no server content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityFailure {
    /// The token store holds no usable session.
    SessionLost,
    /// The session stays, and this work failed. The text is public.
    Failed(&'static str),
}

/// A finished catalog page.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryReply {
    /// Page of tones.
    pub page: Page<Tone>,
    /// Whether this page is the signed-out trending preview.
    pub trending: bool,
}

/// One piece of finished work.
#[derive(Debug, Clone, PartialEq)]
pub enum CommunityEvent {
    /// A catalog page.
    Query {
        /// Generation of the request.
        generation: u64,
        /// Page or failure.
        result: Result<QueryReply, CommunityFailure>,
    },
    /// A taxonomy page.
    Taxonomy {
        /// Generation of the request.
        generation: u64,
        /// Taxonomy family.
        kind: Taxonomy,
        /// Rows or failure.
        result: Result<Vec<TaxonomyEntry>, CommunityFailure>,
    },
    /// A favorite write.
    Favorite {
        /// Ticket of the write.
        generation: u64,
        /// Tone that the write changed.
        tone: ToneId,
        /// Whether the write held.
        result: Result<(), CommunityFailure>,
    },
    /// The public user.
    User {
        /// Ticket of the request.
        generation: u64,
        /// Public fields or failure.
        result: Result<Value, CommunityFailure>,
    },
    /// A tone load. The chain changed before a success arrives here.
    ToneLoad {
        /// Generation of the load.
        generation: u64,
        /// Whether the load published.
        result: Result<(), CommunityFailure>,
    },
    /// The model list of one tone.
    Models {
        /// Generation of the request.
        generation: u64,
        /// Tone that owns the models.
        tone: ToneId,
        /// Models or failure.
        result: Result<ModelList, CommunityFailure>,
    },
    /// A model switch. The chain changed before a success arrives here.
    ModelSwitch {
        /// Generation of the switch.
        generation: u64,
        /// Whether the switch published.
        result: Result<(), CommunityFailure>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkKind {
    Query,
    Taxonomy,
    Favorite,
    User,
    ToneLoad,
    Models,
    ModelSwitch,
}

/// Work whose output is still wanted.
struct LiveWork {
    ticket: u64,
    kind: WorkKind,
    abort: AbortHandle,
    /// What the controller receives if the task panics, so that no state waits forever.
    on_panic: CommunityEvent,
    /// The tone of a favorite write, so that a tone has one write at a time.
    tone: Option<ToneId>,
    /// Whether the browser closed. A detached write still counts, but nobody hears its answer.
    detached: bool,
}

/// The output of one task. The write of the user cache reports nothing.
type TaskOutput = (u64, Option<CommunityEvent>);

/// Bounded background work over one community handle.
pub struct CommunityWorker<T: HttpTransport + Send + Sync + 'static> {
    handle: CommunityHandle<T>,
    runtime: Option<Handle>,
    tasks: JoinSet<TaskOutput>,
    live: Vec<LiveWork>,
    ready: Vec<CommunityEvent>,
    next_ticket: u64,
    load_ticket: Option<LoadTicket>,
    switch_ticket: Option<ModelSwitchTicket>,
    preparations: Arc<Semaphore>,
    last_query: Option<Instant>,
    last_taxonomy: Option<Instant>,
}

impl<T: HttpTransport + Send + Sync + 'static> CommunityWorker<T> {
    /// Creates a worker that starts no work and needs no runtime.
    #[must_use]
    pub fn new(handle: CommunityHandle<T>) -> Self {
        Self {
            handle,
            runtime: None,
            tasks: JoinSet::new(),
            live: Vec::new(),
            ready: Vec::new(),
            next_ticket: 1,
            load_ticket: None,
            switch_ticket: None,
            preparations: Arc::new(Semaphore::new(MAX_PREPARATIONS)),
            last_query: None,
            last_taxonomy: None,
        }
    }

    /// Fetches one catalog page and supersedes the previous query.
    pub fn start_query(&mut self, runtime: &Handle, request: QueryRequest) {
        self.retire(WorkKind::Query);
        let quiet = burst_quiet(&mut self.last_query);
        let generation = request.generation;
        let handle = self.handle.clone();
        self.spawn(
            runtime,
            WorkKind::Query,
            CommunityEvent::Query {
                generation,
                result: Err(CommunityFailure::Failed(WORK_FAILED)),
            },
            async move {
                tokio::time::sleep(quiet).await;
                CommunityEvent::Query {
                    generation,
                    result: run_query(&handle, request).await.map_err(failure),
                }
            },
        );
    }

    /// Fetches one taxonomy page and supersedes the previous taxonomy request.
    pub fn start_taxonomy(&mut self, runtime: &Handle, request: TaxonomyRequest) {
        self.retire(WorkKind::Taxonomy);
        let quiet = burst_quiet(&mut self.last_taxonomy);
        let TaxonomyRequest {
            generation,
            kind,
            text,
        } = request;
        let handle = self.handle.clone();
        self.spawn(
            runtime,
            WorkKind::Taxonomy,
            CommunityEvent::Taxonomy {
                generation,
                kind,
                result: Err(CommunityFailure::Failed(WORK_FAILED)),
            },
            async move {
                tokio::time::sleep(quiet).await;
                let tokens = OffThreadTokens(handle.tokens().clone());
                let result = handle
                    .catalog()
                    .list_taxonomy(kind, &text, TAXONOMY_PAGE_SIZE, &tokens)
                    .await;
                CommunityEvent::Taxonomy {
                    generation,
                    kind,
                    result: result.map(|page| page.data).map_err(failure),
                }
            },
        );
    }

    /// Writes one favorite state. A write over the limit fails at once.
    pub fn start_favorite(&mut self, runtime: &Handle, tone: ToneId, favorite: bool) {
        self.drain();
        let generation = self.next_ticket;
        // A detached write counts until it finishes, so a close cannot restart the limit.
        let writes = self
            .live
            .iter()
            .filter(|work| work.kind == WorkKind::Favorite)
            .count();
        let busy = self
            .live
            .iter()
            .any(|work| work.kind == WorkKind::Favorite && work.tone == Some(tone));
        let refusal = if busy {
            Some(TONE_WRITE_BUSY)
        } else if writes >= MAX_FAVORITE_WRITES {
            Some(TOO_MANY_WRITES)
        } else {
            None
        };
        if let Some(message) = refusal {
            self.next_ticket += 1;
            self.ready.push(CommunityEvent::Favorite {
                generation,
                tone,
                result: Err(CommunityFailure::Failed(message)),
            });
            return;
        }
        let handle = self.handle.clone();
        self.spawn(
            runtime,
            WorkKind::Favorite,
            CommunityEvent::Favorite {
                generation,
                tone,
                result: Err(CommunityFailure::Failed(WORK_FAILED)),
            },
            async move {
                let tokens = OffThreadTokens(handle.tokens().clone());
                let result = handle.catalog().set_favorite(tone, favorite, &tokens).await;
                CommunityEvent::Favorite {
                    generation,
                    tone,
                    result: result.map_err(failure),
                }
            },
        );
        if let Some(work) = self.live.last_mut() {
            work.tone = Some(tone);
        }
    }

    /// Fetches the public user and supersedes the previous user request.
    ///
    /// The cache changes only after a poll accepts the answer, so a late answer cannot change
    /// it after a sign-out.
    pub fn refresh_user(&mut self, runtime: &Handle) {
        self.retire(WorkKind::User);
        let generation = self.next_ticket;
        let handle = self.handle.clone();
        self.spawn(
            runtime,
            WorkKind::User,
            CommunityEvent::User {
                generation,
                result: Err(CommunityFailure::Failed(WORK_FAILED)),
            },
            async move {
                let tokens = OffThreadTokens(handle.tokens().clone());
                let result = handle.catalog().get_user(&tokens).await;
                CommunityEvent::User {
                    generation,
                    result: result.map(|user| public_user(&user)).map_err(failure),
                }
            },
        );
    }

    /// Downloads one tone and publishes it, and supersedes the previous load.
    ///
    /// The bytes reach the audio chain on a blocking thread of `runtime`. The event comes after
    /// the audio chain is prepared.
    pub fn start_tone_load(&mut self, runtime: &Handle, request: LoadRequest) {
        self.retire(WorkKind::ToneLoad);
        let LoadRequest { tone, ticket } = request;
        let generation = ticket.generation();
        self.load_ticket = Some(ticket.clone());
        let handle = self.handle.clone();
        let preparations = self.preparations.clone();
        self.spawn(
            runtime,
            WorkKind::ToneLoad,
            CommunityEvent::ToneLoad {
                generation,
                result: Err(CommunityFailure::Failed(WORK_FAILED)),
            },
            async move {
                CommunityEvent::ToneLoad {
                    generation,
                    result: run_tone_load(&handle, tone, ticket, preparations).await,
                }
            },
        );
    }

    /// Fetches the model list of one tone and supersedes the previous list.
    pub fn start_models(&mut self, runtime: &Handle, request: ModelsRequest) {
        self.retire(WorkKind::Models);
        let ModelsRequest {
            generation,
            tone,
            kind,
        } = request;
        let handle = self.handle.clone();
        self.spawn(
            runtime,
            WorkKind::Models,
            CommunityEvent::Models {
                generation,
                tone,
                result: Err(CommunityFailure::Failed(WORK_FAILED)),
            },
            async move {
                let tokens = OffThreadTokens(handle.tokens().clone());
                let result = handle
                    .catalog()
                    .list_models(tone, MODEL_PAGE_SIZE, model_architecture(kind), &tokens)
                    .await;
                CommunityEvent::Models {
                    generation,
                    tone,
                    result: result
                        .map(|page| ModelList {
                            models: page.data,
                            total: page.total,
                        })
                        .map_err(failure),
                }
            },
        );
    }

    /// Downloads one model and publishes it, and supersedes the previous switch.
    ///
    /// The bytes reach the audio chain on a blocking thread of `runtime`. The event comes after
    /// the audio chain is prepared.
    pub fn start_model_switch(&mut self, runtime: &Handle, request: ModelSwitchRequest) {
        self.retire(WorkKind::ModelSwitch);
        let generation = request.ticket.generation();
        self.switch_ticket = Some(request.ticket.clone());
        let handle = self.handle.clone();
        let preparations = self.preparations.clone();
        self.spawn(
            runtime,
            WorkKind::ModelSwitch,
            CommunityEvent::ModelSwitch {
                generation,
                result: Err(CommunityFailure::Failed(WORK_FAILED)),
            },
            async move {
                CommunityEvent::ModelSwitch {
                    generation,
                    result: run_model_switch(&handle, request, preparations).await,
                }
            },
        );
    }

    /// Stops the delivery of the work in `scope`.
    pub fn cancel(&mut self, scope: CommunityScope) {
        match scope {
            CommunityScope::Detail => {
                self.retire(WorkKind::Models);
                self.retire(WorkKind::ModelSwitch);
            }
            CommunityScope::Browser => {
                self.retire(WorkKind::Query);
                self.retire(WorkKind::Taxonomy);
                self.retire(WorkKind::ToneLoad);
                // A write that left cannot return, so it finishes with nobody to tell.
                // It stays counted until it finishes.
                for work in &mut self.live {
                    if work.kind == WorkKind::Favorite {
                        work.detached = true;
                    }
                }
                self.ready
                    .retain(|event| !matches!(event, CommunityEvent::Favorite { .. }));
            }
            CommunityScope::Session => {
                self.revoke_load();
                self.revoke_switch();
                for work in self.live.drain(..) {
                    work.abort.abort();
                }
                self.ready.clear();
            }
        }
    }

    /// Takes the finished work. This does not block and needs no runtime context.
    pub fn poll(&mut self) -> Vec<CommunityEvent> {
        self.drain();
        std::mem::take(&mut self.ready)
    }

    /// Stops the live work of one kind and forgets its undelivered output.
    fn retire(&mut self, kind: WorkKind) {
        match kind {
            WorkKind::ToneLoad => self.revoke_load(),
            WorkKind::ModelSwitch => self.revoke_switch(),
            _ => {}
        }
        self.live.retain(|work| {
            if work.kind == kind {
                work.abort.abort();
            }
            work.kind != kind
        });
        self.ready.retain(|event| {
            kind != match event {
                CommunityEvent::Query { .. } => WorkKind::Query,
                CommunityEvent::Taxonomy { .. } => WorkKind::Taxonomy,
                CommunityEvent::Favorite { .. } => WorkKind::Favorite,
                CommunityEvent::User { .. } => WorkKind::User,
                CommunityEvent::ToneLoad { .. } => WorkKind::ToneLoad,
                CommunityEvent::Models { .. } => WorkKind::Models,
                CommunityEvent::ModelSwitch { .. } => WorkKind::ModelSwitch,
            }
        });
    }

    /// An abort cannot stop a compile, so the load must also lose the right to publish.
    fn revoke_load(&mut self) {
        if let Some(ticket) = self.load_ticket.take() {
            ticket.revoke();
        }
    }

    fn revoke_switch(&mut self) {
        if let Some(ticket) = self.switch_ticket.take() {
            ticket.revoke();
        }
    }

    fn spawn(
        &mut self,
        runtime: &Handle,
        kind: WorkKind,
        on_panic: CommunityEvent,
        work: impl Future<Output = CommunityEvent> + Send + 'static,
    ) {
        // An aborted task stays in the set until it is joined, so this keeps the set small.
        self.drain();
        self.runtime = Some(runtime.clone());
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let abort = self
            .tasks
            .spawn_on(async move { (ticket, Some(work.await)) }, runtime);
        self.live.push(LiveWork {
            ticket,
            kind,
            abort,
            on_panic,
            tone: None,
            detached: false,
        });
    }

    fn drain(&mut self) {
        while let Some(joined) = self.tasks.try_join_next() {
            match joined {
                Ok((ticket, Some(event))) => {
                    if let Some(index) = self.live.iter().position(|work| work.ticket == ticket) {
                        let work = self.live.swap_remove(index);
                        if !work.detached {
                            self.accept(event);
                        }
                    }
                }
                Ok((_, None)) => {}
                Err(error) if error.is_panic() => {
                    let id = error.id();
                    if let Some(index) = self.live.iter().position(|work| work.abort.id() == id) {
                        let work = self.live.swap_remove(index);
                        if !work.detached {
                            self.ready.push(work.on_panic);
                        }
                    }
                }
                Err(_) => {}
            }
        }
    }

    fn accept(&mut self, event: CommunityEvent) {
        if let CommunityEvent::User {
            result: Ok(user), ..
        } = &event
        {
            self.cache_user(user.clone());
        }
        self.ready.push(event);
    }

    /// Writes the cache on a blocking thread. The cache is a convenience, so a failure is quiet.
    fn cache_user(&mut self, user: Value) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        let tokens = self.handle.tokens().clone();
        self.tasks.spawn_blocking_on(
            move || {
                let _ = tokens.set_cached_user(user);
                (0, None)
            },
            runtime,
        );
    }
}

impl<T: HttpTransport + Send + Sync + 'static> CommunityAccess for CommunityWorker<T> {
    fn has_stored_tokens(&self) -> bool {
        self.handle.has_stored_tokens()
    }

    fn start(&mut self, runtime: &Handle, work: CommunityWork) {
        match work {
            CommunityWork::Query(request) => self.start_query(runtime, request),
            CommunityWork::Taxonomy(request) => self.start_taxonomy(runtime, request),
            CommunityWork::Favorite { tone, favorite } => {
                self.start_favorite(runtime, tone, favorite);
            }
            CommunityWork::RefreshUser => self.refresh_user(runtime),
            CommunityWork::ToneLoad(request) => self.start_tone_load(runtime, request),
            CommunityWork::Models(request) => self.start_models(runtime, request),
            CommunityWork::ModelSwitch(request) => self.start_model_switch(runtime, request),
        }
    }

    fn cancel(&mut self, scope: CommunityScope) {
        Self::cancel(self, scope);
    }

    fn poll(&mut self) -> Vec<CommunityEvent> {
        Self::poll(self)
    }

    fn image_fetcher(&self, runtime: Handle) -> Arc<dyn ImageFetcher> {
        Arc::new(HttpImageFetcher::new(
            self.handle.downloads().clone(),
            runtime,
        ))
    }
}

async fn run_query<T: HttpTransport + Send + Sync + 'static>(
    handle: &CommunityHandle<T>,
    request: QueryRequest,
) -> Result<QueryReply, ApiError> {
    let QueryRequest {
        query,
        page,
        signed_in,
        ..
    } = request;
    if !signed_in {
        let page = handle
            .catalog()
            .list_trending(Some(&query.gear), None)
            .await?;
        return Ok(QueryReply {
            page,
            trending: true,
        });
    }
    let tokens = OffThreadTokens(handle.tokens().clone());
    let page = handle
        .catalog()
        .list_tones(query, page, CATALOG_PAGE_SIZE, ANY_ARCHITECTURE, &tokens)
        .await?;
    Ok(QueryReply {
        page,
        trending: false,
    })
}

/// Fetches one tone and its first compatible model, downloads the bytes, and publishes them.
///
/// A failure at any step leaves the chain as it was. Nothing is sent to a model address that the
/// origin policy of the download client refuses.
async fn run_tone_load<T: HttpTransport + Send + Sync + 'static>(
    handle: &CommunityHandle<T>,
    tone_id: ToneId,
    ticket: LoadTicket,
    preparations: Arc<Semaphore>,
) -> Result<(), CommunityFailure> {
    let tokens = OffThreadTokens(handle.tokens().clone());
    let tone = handle
        .catalog()
        .get_tone(tone_id, &tokens)
        .await
        .map_err(failure)?;
    if tone.id != tone_id {
        return Err(CommunityFailure::Failed(SERVICE_INVALID));
    }
    let page = handle
        .catalog()
        .list_models(
            tone_id,
            TONE_LOAD_MODELS,
            model_architecture(tone.kind),
            &tokens,
        )
        .await
        .map_err(failure)?;
    let model = page
        .data
        .into_iter()
        .next()
        .ok_or(CommunityFailure::Failed(NO_MODEL))?;
    let bytes = download_catalog_model(handle, &model, &tokens).await?;
    let model_id = model.id;
    let tone = stored_tone(tone, model)?;
    publish_with_one_compile(preparations, move || ticket.publish(tone, model_id, bytes)).await
}

/// Downloads one model of a tone and publishes it in place of the active model.
///
/// A failure at any step leaves the chain as it was.
async fn run_model_switch<T: HttpTransport + Send + Sync + 'static>(
    handle: &CommunityHandle<T>,
    request: ModelSwitchRequest,
    preparations: Arc<Semaphore>,
) -> Result<(), CommunityFailure> {
    let ModelSwitchRequest {
        tone,
        model,
        ticket,
    } = request;
    let tokens = OffThreadTokens(handle.tokens().clone());
    let bytes = download_catalog_model(handle, &model, &tokens).await?;
    let model_id = model.id;
    let tone = stored_tone(tone, model)?;
    publish_with_one_compile(preparations, move || ticket.publish(tone, model_id, bytes)).await
}

const fn model_architecture(kind: ModelKind) -> i32 {
    match kind {
        ModelKind::Nam => NAM_ARCHITECTURE,
        ModelKind::Ir => ANY_ARCHITECTURE,
    }
}

/// Downloads the bytes of one model. Nothing is sent to an address that the origin policy of the
/// download client refuses.
async fn download_catalog_model<T: HttpTransport + Send + Sync + 'static>(
    handle: &CommunityHandle<T>,
    model: &ToneModel,
    tokens: &OffThreadTokens<ronie_core::SessionStore<ronie_core::OAuthFlow<T>>>,
) -> Result<ModelBytes, CommunityFailure> {
    let source = match &model.source {
        Some(source @ ModelSource::Catalog { .. }) => source.clone(),
        // The service cannot name a file on this computer.
        Some(ModelSource::Local { .. }) => {
            return Err(CommunityFailure::Failed(MODEL_ADDRESS_REFUSED));
        }
        None => return Err(CommunityFailure::Failed(NO_MODEL)),
    };
    let downloaded = handle
        .downloads()
        .download_model_with_tokens(
            FetchRequest {
                source,
                bearer: None,
            },
            tokens,
        )
        .await;
    match downloaded {
        Ok(bytes) => Ok(bytes),
        Err(DownloadError::Unauthorized) => Err(refused_download(tokens).await),
        Err(error) => Err(download_failure(&error)),
    }
}

/// The tone as a block stores it, which holds only `model` of its catalog models.
fn stored_tone(mut tone: Tone, model: ToneModel) -> Result<Tone, CommunityFailure> {
    let model_id = model.id;
    tone.models = vec![model];
    tone.for_storage(model_id)
        .map_err(|_| CommunityFailure::Failed(SERVICE_INVALID))
}

/// Runs one publication on a blocking thread, after the compile of each other publication.
async fn publish_with_one_compile(
    preparations: Arc<Semaphore>,
    publish: impl FnOnce() -> Result<bool, UiError> + Send + 'static,
) -> Result<(), CommunityFailure> {
    let permit = preparations
        .acquire_owned()
        .await
        .map_err(|_| CommunityFailure::Failed(WORK_FAILED))?;
    let published = tokio::task::spawn_blocking(move || {
        // An abort of this task leaves the compile running, so the compile keeps the permit.
        let _permit = permit;
        publish()
    })
    .await
    .map_err(|_| CommunityFailure::Failed(WORK_FAILED))?
    .map_err(|_| CommunityFailure::Failed(MODEL_UNUSABLE))?;
    if published {
        Ok(())
    } else {
        Err(CommunityFailure::Failed(WORK_FAILED))
    }
}

/// A model refused after the one refresh. Only a store with no token means that the session is
/// gone, so a refusal of this one model does not sign the user out.
async fn refused_download<P: TokenProvider + Clone + Send + Sync + 'static>(
    tokens: &OffThreadTokens<P>,
) -> CommunityFailure {
    match tokens.access_token(false).await {
        Ok(None) => CommunityFailure::SessionLost,
        Ok(Some(_)) | Err(_) => CommunityFailure::Failed(SERVICE_REFUSED),
    }
}

fn download_failure(error: &DownloadError) -> CommunityFailure {
    CommunityFailure::Failed(match error {
        DownloadError::UnsafeOrigin => MODEL_ADDRESS_REFUSED,
        DownloadError::EmptyDownload => MODEL_EMPTY,
        DownloadError::ResponseTooLarge { .. } => MODEL_TOO_LARGE,
        DownloadError::HttpStatus(_) | DownloadError::Unauthorized => SERVICE_REFUSED,
        _ => SERVICE_UNREACHABLE,
    })
}

/// How long a new request waits, and the record of its start.
fn burst_quiet(last: &mut Option<Instant>) -> Duration {
    let now = Instant::now();
    let in_burst = last.is_some_and(|last| now.duration_since(last) < BURST_WINDOW);
    *last = Some(now);
    if in_burst {
        BURST_QUIET
    } else {
        Duration::ZERO
    }
}

/// Maps an error to public text. Only a missing token means that the session is gone.
fn failure(error: ApiError) -> CommunityFailure {
    match error {
        ApiError::Unauthorized => CommunityFailure::SessionLost,
        ApiError::Http(_) | ApiError::Session(_) => CommunityFailure::Failed(SERVICE_UNREACHABLE),
        ApiError::HttpStatus(_) => CommunityFailure::Failed(SERVICE_REFUSED),
        ApiError::Json(_)
        | ApiError::Tone(_)
        | ApiError::InvalidDocument(_)
        | ApiError::ResponseTooLarge { .. } => CommunityFailure::Failed(SERVICE_INVALID),
        // A worker holds one live query of each kind, so no other request can supersede it.
        ApiError::Superseded { .. } => CommunityFailure::Failed(WORK_FAILED),
    }
}

fn public_user(user: &Value) -> Value {
    let public = PUBLIC_USER_FIELDS
        .into_iter()
        .filter_map(|field| Some((field.to_owned(), user.get(field)?.clone())))
        .collect();
    Value::Object(public)
}

/// A token provider that runs each lookup on a blocking thread.
///
/// The lookup reads and writes a settings file, and it can wait for the lock of that file. A
/// blocking thread also finishes when the caller is aborted. This matters for a refresh, because
/// the server forgets the old refresh token when it answers, so the store must save the answer.
#[derive(Clone)]
struct OffThreadTokens<P>(P);

impl<P: TokenProvider + Clone + Send + Sync + 'static> TokenProvider for OffThreadTokens<P> {
    async fn access_token(&self, force_refresh: bool) -> Result<Option<SecretString>, ApiError> {
        let provider = self.0.clone();
        off_runtime_thread(move || async move { provider.access_token(force_refresh).await }).await
    }

    async fn refreshed_token(
        &self,
        rejected: &SecretString,
    ) -> Result<Option<SecretString>, ApiError> {
        let provider = self.0.clone();
        let rejected = rejected.clone();
        off_runtime_thread(move || async move { provider.refreshed_token(&rejected).await }).await
    }
}

async fn off_runtime_thread<Lookup, Answer>(
    lookup: Lookup,
) -> Result<Option<SecretString>, ApiError>
where
    Lookup: FnOnce() -> Answer + Send + 'static,
    Answer: Future<Output = Result<Option<SecretString>, ApiError>>,
{
    let runtime = Handle::current();
    tokio::task::spawn_blocking(move || runtime.block_on(lookup()))
        .await
        .map_err(|_| ApiError::Http(HttpError::Transport))?
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        time::Duration,
    };

    use ronie_core::{
        ApiError, OAuthConfig, OriginPolicy, RecordedTransport, SecretString, SettingsStore,
        TokenProvider,
    };
    use url::Url;

    use super::{CommunityEvent, CommunityFailure, CommunityWorker, OffThreadTokens, WorkKind};
    use crate::services::community::CommunityService;

    /// A provider that blocks its thread until another task runs on the runtime.
    #[derive(Clone)]
    struct BlockingProvider(Arc<Barrier>);

    impl TokenProvider for BlockingProvider {
        async fn access_token(
            &self,
            _force_refresh: bool,
        ) -> Result<Option<SecretString>, ApiError> {
            self.0.wait();
            Ok(None)
        }
    }

    #[test]
    fn a_task_that_panics_ends_its_state_with_a_failure() {
        let directory = tempfile::tempdir().expect("settings root");
        let origin = Url::parse("https://api.example").expect("origin");
        let service = CommunityService::new(
            OAuthConfig::new(origin.clone(), "public-client"),
            OriginPolicy::new(origin, []),
            RecordedTransport::new(Vec::new()),
            SettingsStore::standalone(directory.path(), "settings"),
        );
        let (_auth, handle) = service.into_parts();
        let mut worker = CommunityWorker::new(handle);
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let on_panic = CommunityEvent::Query {
            generation: 4,
            result: Err(CommunityFailure::Failed("stopped")),
        };

        worker.spawn(
            runtime.handle(),
            WorkKind::Query,
            on_panic.clone(),
            async move { panic!("a task failure") },
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let events = loop {
            let events = worker.poll();
            if !events.is_empty() {
                break events;
            }
            assert!(std::time::Instant::now() < deadline, "no failure");
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(events, [on_panic]);
    }

    #[test]
    fn a_token_lookup_leaves_the_one_runtime_thread_free_for_other_work() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let barrier = Arc::new(Barrier::new(2));
        let tokens = OffThreadTokens(BlockingProvider(barrier.clone()));
        let (done, mut finished) = tokio::sync::oneshot::channel();
        // A lookup that blocks the worker thread would stop the task that releases it.
        runtime.spawn(async move {
            let _ = done.send(tokens.access_token(false).await.is_ok());
        });
        let other_work = runtime.spawn(async move { barrier.wait() });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match finished.try_recv() {
                Ok(succeeded) => {
                    assert!(succeeded);
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    if std::time::Instant::now() > deadline {
                        // The two tasks wait for each other forever, so do not wait for them.
                        std::mem::forget(other_work);
                        runtime.shutdown_background();
                        panic!("the lookup stopped the runtime thread");
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("{error}"),
            }
        }
    }
}

//! Shared browser query, result, profile, and favorite state.

use std::collections::BTreeMap;

use ronie_core::{Page, Taxonomy, TaxonomyEntry, Tone, ToneId, ToneQuery};

pub use ronie_core::Profile;

use super::community_worker::TaxonomyRequest;

/// Filter menu navigation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterMenu {
    /// Sort options.
    Sort,
    /// Tag options.
    Tags,
    /// Make options.
    Makes,
    /// Creator options.
    Creators,
    /// Format options.
    Format,
    /// Profile options.
    Profile,
}

/// Most creator avatars that one store remembers.
const MAX_CREATOR_AVATARS: usize = 1024;

/// Readiness of the current catalog result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogStatus {
    /// No request is active and no error is held.
    Idle,
    /// A request is in flight.
    Loading,
    /// The latest result is available, including an empty result.
    Ready,
    /// The latest request failed.
    Error,
}

/// A catalog state that only a live catalog can otherwise produce.
///
/// A scenario that has no network uses this seam. No control can put the store in these
/// states, because each one needs a request that a catalog answers.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogSeed {
    /// The query that the results belong to.
    Query(ToneQuery),
    /// A request that finished with this page.
    Page {
        /// Page of results.
        page: Page<Tone>,
        /// Whether the page shows trending tones to a signed-out user.
        trending: bool,
    },
    /// A request that failed with this message.
    Failed(String),
    /// A request that is still in flight.
    Pending,
}

#[derive(Clone)]
struct FavoriteOverride {
    previous: serde_json::Value,
}

/// A query that no worker holds yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsentQuery {
    /// Generation that the answer must carry.
    pub(crate) generation: u64,
    /// Query to send.
    pub(crate) query: ToneQuery,
    /// One-based page.
    pub(crate) page: u32,
    /// Whether this asks for another query or page than the previous request.
    pub(crate) changed: bool,
}

/// Browser state preserved across close/reopen and shared by Default and Flow.
pub struct CatalogStore {
    query: ToneQuery,
    result: Option<Page<Tone>>,
    result_is_trending: bool,
    status: CatalogStatus,
    error: Option<String>,
    generation: u64,
    closed: bool,
    favorites: BTreeMap<ToneId, FavoriteOverride>,
    creator_avatars: BTreeMap<String, String>,
    /// Newest generation that a worker holds.
    sent_generation: u64,
    /// Query and page of the newest request.
    sent_request: Option<(ToneQuery, u64)>,
    /// Whether a close stopped work, so that the preserved result may be out of date.
    refetch_on_return: bool,
    taxonomy: Vec<(Taxonomy, Vec<TaxonomyEntry>)>,
    taxonomy_generation: u64,
    unsent_taxonomy: Option<TaxonomyRequest>,
}

impl Default for CatalogStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            query: ToneQuery::default(),
            result: None,
            result_is_trending: false,
            status: CatalogStatus::Idle,
            error: None,
            generation: 0,
            closed: false,
            favorites: BTreeMap::new(),
            creator_avatars: BTreeMap::new(),
            sent_generation: 0,
            sent_request: None,
            refetch_on_return: false,
            taxonomy: Vec::new(),
            taxonomy_generation: 0,
            unsent_taxonomy: None,
        }
    }

    /// Current query.
    #[must_use]
    pub const fn query(&self) -> &ToneQuery {
        &self.query
    }

    /// Mutable query used by filter controls.
    pub fn query_mut(&mut self) -> &mut ToneQuery {
        &mut self.query
    }

    /// Current result, preserved after close.
    #[must_use]
    pub const fn result(&self) -> Option<&Page<Tone>> {
        self.result.as_ref()
    }

    /// Whether the result is the signed-out trending preview.
    #[must_use]
    pub const fn result_is_trending(&self) -> bool {
        self.result_is_trending
    }

    /// Current generation used to reject stale replies.
    #[must_use]
    pub const fn current_generation(&self) -> u64 {
        self.generation
    }

    /// Current status.
    #[must_use]
    pub const fn status(&self) -> CatalogStatus {
        self.status
    }

    /// Latest request error.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether a ready result has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.status == CatalogStatus::Ready
            && self
                .result
                .as_ref()
                .is_some_and(|page| page.data.is_empty())
    }

    /// Starts a new result generation.
    pub fn begin_query(&mut self) -> u64 {
        self.generation += 1;
        self.closed = false;
        self.status = CatalogStatus::Loading;
        self.error = None;
        self.generation
    }

    /// Applies only the newest page reply.
    pub fn complete_query(&mut self, generation: u64, result: Page<Tone>, trending: bool) -> bool {
        if self.closed || generation != self.generation {
            return false;
        }
        self.result = Some(result);
        self.result_is_trending = trending;
        self.status = CatalogStatus::Ready;
        self.error = None;
        // The new rows hold the state of the server, so an old copy must not return.
        self.favorites.clear();
        true
    }

    /// Records a failure only for the active generation.
    pub fn fail_query(&mut self, generation: u64, error: impl Into<String>) -> bool {
        if self.closed || generation != self.generation {
            return false;
        }
        self.error = Some(error.into());
        self.status = CatalogStatus::Error;
        true
    }

    /// Applies one seed in place of a catalog reply.
    pub(crate) fn seed(&mut self, seed: CatalogSeed) {
        match seed {
            CatalogSeed::Query(query) => self.query = query,
            CatalogSeed::Page { page, trending } => {
                let generation = self.begin_query();
                self.complete_query(generation, page, trending);
            }
            CatalogSeed::Failed(error) => {
                let generation = self.begin_query();
                self.fail_query(generation, error);
            }
            CatalogSeed::Pending => {
                self.begin_query();
            }
        }
    }

    /// Starts the query that a community service owes this store: the first query of a store
    /// that has no result, or the query that a close stopped.
    pub(crate) fn begin_owed_query(&mut self) {
        if self.closed {
            return;
        }
        let unanswered = self.status == CatalogStatus::Idle && self.result.is_none();
        if std::mem::take(&mut self.refetch_on_return) || unanswered {
            self.begin_query();
        }
    }

    /// Takes the newest query when no worker holds it, and chooses its page.
    pub(crate) fn take_unsent_query(&mut self) -> Option<UnsentQuery> {
        if self.closed
            || self.status != CatalogStatus::Loading
            || self.generation == self.sent_generation
        {
            return None;
        }
        self.sent_generation = self.generation;
        let same_query = self
            .sent_request
            .as_ref()
            .is_some_and(|(query, _)| *query == self.query);
        if !same_query && let Some(result) = &mut self.result {
            result.page = 1;
        }
        let page = self.result.as_ref().map_or(1, |result| result.page.max(1));
        let request = (self.query.clone(), page);
        let changed = self.sent_request.as_ref() != Some(&request);
        self.sent_request = Some(request);
        Some(UnsentQuery {
            generation: self.generation,
            query: self.query.clone(),
            page: u32::try_from(page).unwrap_or(1),
            changed,
        })
    }

    /// Turns to a different page when it exists and starts its request.
    pub fn turn_page(&mut self, page: u64) -> bool {
        let Some(result) = &mut self.result else {
            return false;
        };
        if page == 0 || page > result.total_pages || page == result.page {
            return false;
        }
        result.page = page;
        self.begin_query();
        true
    }

    /// Advances to the next page when it exists.
    pub fn next_page(&mut self) -> bool {
        // A server chooses the page number, so it can be the largest number.
        let Some(page) = self
            .result
            .as_ref()
            .and_then(|result| result.page.checked_add(1))
        else {
            return false;
        };
        self.turn_page(page)
    }

    /// Returns to the previous page when it exists.
    pub fn previous_page(&mut self) -> bool {
        // A server chooses the page number, so it can be zero.
        let Some(page) = self
            .result
            .as_ref()
            .and_then(|result| result.page.checked_sub(1))
        else {
            return false;
        };
        self.turn_page(page)
    }

    /// Drops results and in-flight queries, because a new session sees other tones.
    pub fn drop_session_results(&mut self) {
        self.result = None;
        self.result_is_trending = false;
        self.generation += 1;
        self.status = CatalogStatus::Idle;
        self.error = None;
        self.favorites.clear();
        self.sent_request = None;
        self.refetch_on_return = false;
        self.taxonomy.clear();
        self.taxonomy_generation += 1;
        self.unsent_taxonomy = None;
    }

    /// Fetched rows of one taxonomy family.
    #[must_use]
    pub fn taxonomy(&self, kind: Taxonomy) -> Option<&[TaxonomyEntry]> {
        self.taxonomy
            .iter()
            .find(|(held, _)| *held == kind)
            .map(|(_, rows)| rows.as_slice())
    }

    /// Asks for the rows of one taxonomy family. A newer request supersedes this one.
    pub fn request_taxonomy(&mut self, kind: Taxonomy, text: &str) {
        self.taxonomy_generation += 1;
        self.unsent_taxonomy = Some(TaxonomyRequest {
            generation: self.taxonomy_generation,
            kind,
            text: text.to_owned(),
        });
    }

    /// Takes the newest taxonomy request when no worker holds it.
    pub(crate) fn take_unsent_taxonomy(&mut self) -> Option<TaxonomyRequest> {
        self.unsent_taxonomy.take()
    }

    /// Applies only the newest taxonomy reply.
    pub fn complete_taxonomy(
        &mut self,
        generation: u64,
        kind: Taxonomy,
        rows: Vec<TaxonomyEntry>,
    ) -> bool {
        if self.closed || generation != self.taxonomy_generation {
            return false;
        }
        if kind == Taxonomy::Creators {
            for row in &rows {
                let name = row.raw["username"].as_str().unwrap_or(&row.name);
                if let Some(url) = row.raw["avatar_url"].as_str() {
                    remember_creator(&mut self.creator_avatars, name, url);
                }
            }
        }
        self.taxonomy.retain(|(held, _)| *held != kind);
        self.taxonomy.push((kind, rows));
        true
    }

    /// Pins the avatars of the creators in the current result.
    pub fn remember_result_creators(&mut self) {
        for tone in self.result.iter().flat_map(|page| &page.data) {
            let user = &tone.raw["user"];
            if let (Some(name), Some(url)) =
                (user["username"].as_str(), user["avatar_url"].as_str())
            {
                remember_creator(&mut self.creator_avatars, name, url);
            }
        }
    }

    /// Selects a gear chip.
    pub fn set_gear(&mut self, gear: impl Into<String>) {
        self.query.gear = gear.into();
    }

    /// Selects a model format.
    pub fn set_format(&mut self, format: impl Into<String>) {
        self.query.format = format.into();
    }

    /// Selects a profile stream.
    pub fn set_profile(&mut self, profile: Profile) {
        self.query.profile = profile;
    }

    /// Toggles calibrated-only filtering.
    pub fn set_calibrated(&mut self, calibrated: bool) {
        self.query.calibrated = calibrated;
    }

    /// Whether profile streams park catalog-only filters.
    #[must_use]
    pub fn catalog_filters_parked(&self) -> bool {
        self.query.profile != Profile::None
    }

    /// Whether a filter menu can open in the current profile.
    #[must_use]
    pub fn filter_enabled(&self, _menu: FilterMenu) -> bool {
        !self.catalog_filters_parked()
    }

    /// Applies an optimistic favorite and count change. Returns whether the change applied.
    ///
    /// A tone has one write at a time, so a change of a tone with an active write does nothing.
    pub fn begin_favorite(&mut self, id: ToneId, on: bool) -> bool {
        if self.favorites.contains_key(&id) {
            return false;
        }
        let Some(tone) = self
            .result
            .as_mut()
            .and_then(|page| page.data.iter_mut().find(|tone| tone.id == id))
        else {
            return false;
        };
        let previous = tone.raw.clone();
        let old_on = tone.raw["is_favorite"].as_bool().unwrap_or(false);
        let old_count = tone.raw["favorites_count"].as_u64().unwrap_or(0);
        let count = if on == old_on {
            old_count
        } else if on {
            old_count.saturating_add(1)
        } else {
            old_count.saturating_sub(1)
        };
        tone.raw["is_favorite"] = serde_json::json!(on);
        tone.raw["favorites_count"] = serde_json::json!(count);
        self.favorites.insert(id, FavoriteOverride { previous });
        true
    }

    /// Commits success or rolls back a failed favorite update.
    ///
    /// Returns false when the tone has no active change: a newer result or a close replaced it.
    pub fn finish_favorite(&mut self, id: ToneId, success: bool) -> bool {
        let Some(override_state) = self.favorites.remove(&id) else {
            return false;
        };
        if !success {
            self.restore_favorite(id, override_state);
        }
        true
    }

    fn restore_favorite(&mut self, id: ToneId, override_state: FavoriteOverride) {
        if let Some(tone) = self
            .result
            .as_mut()
            .and_then(|page| page.data.iter_mut().find(|tone| tone.id == id))
        {
            tone.raw = override_state.previous;
        }
    }

    /// Pins a creator avatar by name even after later lookups omit it.
    pub fn set_creator_avatar(&mut self, name: impl Into<String>, url: impl Into<String>) {
        self.creator_avatars.insert(name.into(), url.into());
    }

    /// Current pinned avatar URL.
    #[must_use]
    pub fn creator_avatar(&self, name: &str) -> Option<&str> {
        self.creator_avatars.get(name).map(String::as_str)
    }

    /// Cancels callbacks while preserving the last result for return.
    ///
    /// The preserved result holds no unconfirmed favorite, because a late answer cannot publish.
    pub fn close(&mut self) {
        let stopped_work = matches!(self.status, CatalogStatus::Loading | CatalogStatus::Error)
            || !self.favorites.is_empty();
        self.refetch_on_return |= stopped_work;
        for (id, override_state) in std::mem::take(&mut self.favorites) {
            self.restore_favorite(id, override_state);
        }
        self.closed = true;
        self.generation += 1;
        self.status = CatalogStatus::Idle;
        self.taxonomy_generation += 1;
        self.unsent_taxonomy = None;
    }

    /// Reopens the preserved state without starting a new request.
    pub fn reopen(&mut self) {
        self.closed = false;
        if self.result.is_some() {
            self.status = CatalogStatus::Ready;
        }
    }
}

/// Adds one avatar. A server chooses the names, so the map has a limit.
fn remember_creator(avatars: &mut BTreeMap<String, String>, name: &str, url: &str) {
    if avatars.len() < MAX_CREATOR_AVATARS || avatars.contains_key(name) {
        avatars.insert(name.to_owned(), url.to_owned());
    }
}

/// Browser visibility and scroll state.
pub struct BrowserController {
    store: CatalogStore,
    scroll: f32,
    open: bool,
}

impl Default for BrowserController {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserController {
    /// Creates a browser with an empty result store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: CatalogStore::new(),
            scroll: 0.0,
            open: true,
        }
    }

    /// Shared catalog state.
    #[must_use]
    pub const fn store(&self) -> &CatalogStore {
        &self.store
    }

    /// Mutable catalog state.
    pub fn store_mut(&mut self) -> &mut CatalogStore {
        &mut self.store
    }

    /// Current scroll position.
    #[must_use]
    pub const fn scroll(&self) -> f32 {
        self.scroll
    }

    /// Saves the grid scroll position.
    pub fn set_scroll(&mut self, scroll: f32) {
        self.scroll = scroll.max(0.0);
    }

    /// Closes the browser and cancels in-flight results.
    pub fn close(&mut self) {
        self.open = false;
        self.store.close();
    }

    /// Reopens the preserved result and scroll position.
    pub fn reopen(&mut self) {
        self.open = true;
        self.store.reopen();
    }

    /// Whether the browser is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }
}

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use serde_json::Value;
use url::Url;

use crate::{
    ApiError, HttpMethod, HttpRequest, HttpResponse, HttpTransport, ResponseKind, SecretString,
    Tone, ToneError, ToneId, ToneModel,
};

const API_PREFIX: &str = "/api/v1";

/// Immutable catalog client configuration.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Configured upstream origin.
    pub origin: Url,
    /// Public build key, never a user token.
    pub publishable_key: String,
    /// Header/connect timeout.
    pub connect_timeout: std::time::Duration,
    /// Complete response-body deadline.
    pub body_deadline: std::time::Duration,
}

impl ApiConfig {
    /// Creates the Reference-compatible 30/120-second configuration.
    #[must_use]
    pub fn new(origin: Url, publishable_key: impl Into<String>) -> Self {
        Self {
            origin,
            publishable_key: publishable_key.into(),
            connect_timeout: std::time::Duration::from_secs(30),
            body_deadline: std::time::Duration::from_secs(120),
        }
    }
}

/// Optional bearer provider with an explicit force-refresh handoff.
pub trait TokenProvider {
    /// Returns the current token, or refreshes it when `force_refresh` is true.
    fn access_token(
        &self,
        force_refresh: bool,
    ) -> impl Future<Output = Result<Option<SecretString>, ApiError>> + Send;

    /// Returns a token to replace one that upstream rejected.
    ///
    /// A provider that can compare tokens overrides this method, so that callers rejected at
    /// the same time share one refresh. Any other provider forces a refresh.
    fn refreshed_token(
        &self,
        rejected: &SecretString,
    ) -> impl Future<Output = Result<Option<SecretString>, ApiError>> + Send {
        let _ = rejected;
        self.access_token(true)
    }
}

/// Profile-specific catalog stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Profile {
    /// General search.
    #[default]
    None,
    /// Recently used models.
    Downloaded,
    /// Favorite tones.
    Favorited,
    /// Tones created by the signed-in user.
    Created,
}

impl Profile {
    fn endpoint(self) -> &'static str {
        match self {
            Self::None => "search",
            Self::Downloaded => "downloaded",
            Self::Favorited => "favorited",
            Self::Created => "created",
        }
    }
}

/// Catalog sort selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneSort {
    /// Search relevance.
    BestMatch,
    /// Trending.
    Trending,
    /// All-time downloads.
    Popular,
    /// Newest first.
    Newest,
    /// Oldest first.
    Oldest,
}

impl ToneSort {
    fn id(self) -> &'static str {
        match self {
            Self::BestMatch => "best-match",
            Self::Trending => "trending",
            Self::Popular => "downloads-all-time",
            Self::Newest => "newest",
            Self::Oldest => "oldest",
        }
    }
}

/// Taxonomy lookup family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taxonomy {
    /// Tags.
    Tags,
    /// Makes and models.
    Makes,
    /// Creator accounts.
    Creators,
}

impl Taxonomy {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Tags => "tags",
            Self::Makes => "makes",
            Self::Creators => "users",
        }
    }
}

/// Complete tone-browser query with Reference request encoding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToneQuery {
    /// Free-text title search.
    pub text: String,
    /// Optional explicit sort.
    pub sort: Option<ToneSort>,
    /// Gear identifier.
    pub gear: String,
    /// `nam`, `ir`, or empty for any.
    pub format: String,
    /// Selected tag names.
    pub tags: Vec<String>,
    /// Selected make names.
    pub makes: Vec<String>,
    /// Selected creator names.
    pub creators: Vec<String>,
    /// Calibration filter, suppressed for IR gear/format.
    pub calibrated: bool,
    /// Verified-only filter.
    pub verified: bool,
    /// Profile stream.
    pub profile: Profile,
}

impl ToneQuery {
    /// Whether calibration applies to this gear and format.
    #[must_use]
    pub fn calibrated_applies(&self) -> bool {
        self.gear != "cab" && self.gear != "space" && self.format != "ir"
    }

    /// Builds the exact v1 request path for one page.
    #[must_use]
    pub fn request_path(&self, page: u32, page_size: u32, architecture: i32) -> String {
        let mut query = QueryBuilder::default();
        query.add("page", page.to_string());
        query.add("page_size", page_size.to_string());
        query.add_escaped("query", self.text.trim());
        let base = format!("{API_PREFIX}/tones/{}", self.profile.endpoint());
        if self.profile != Profile::None {
            query.add_escaped("gear", &self.gear);
            return format!("{base}{query}");
        }
        if let Some(sort) = self.sort {
            query.add("sort", sort.id());
        }
        query.add_escaped("gears", &self.gear);
        query.add("format", &self.format);
        query.add_joined("tags", &self.tags, "_");
        query.add_joined("makes", &self.makes, "_");
        query.add_joined("creators", &self.creators, ",");
        if self.calibrated && self.calibrated_applies() {
            query.add("calibrated", "true");
        }
        if self.verified {
            query.add("verified", "true");
        }
        if architecture >= 0 {
            query.add("architecture", architecture.to_string());
        }
        format!("{base}{query}")
    }
}

#[derive(Default)]
struct QueryBuilder {
    parts: Vec<String>,
}

impl QueryBuilder {
    fn add(&mut self, key: &str, value: impl AsRef<str>) {
        let value = value.as_ref();
        if !value.is_empty() {
            self.parts.push(format!("{key}={value}"));
        }
    }

    fn add_escaped(&mut self, key: &str, value: &str) {
        if !value.is_empty() {
            self.parts.push(format!("{key}={}", escape(value)));
        }
    }

    fn add_joined(&mut self, key: &str, values: &[String], separator: &str) {
        if !values.is_empty() {
            let joined = values
                .iter()
                .map(|value| escape(value))
                .collect::<Vec<_>>()
                .join(separator);
            self.parts.push(format!("{key}={joined}"));
        }
    }
}

impl std::fmt::Display for QueryBuilder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.parts.is_empty() {
            return Ok(());
        }
        write!(formatter, "?{}", self.parts.join("&"))
    }
}

/// Generic v1 paginated payload.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    /// Page rows.
    pub data: Vec<T>,
    /// One-based page number.
    pub page: u64,
    /// Requested page size.
    pub page_size: u64,
    /// Total row count.
    pub total: u64,
    /// Total page count.
    pub total_pages: u64,
}

impl<T> Page<T> {
    fn from_value(
        value: Value,
        mut parse: impl FnMut(Value) -> Result<T, ApiError>,
    ) -> Result<Self, ApiError> {
        let rows = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or(ApiError::InvalidDocument("page data must be an array"))?;
        if rows.len() > 300 {
            return Err(ApiError::InvalidDocument("page exceeds 300 rows"));
        }
        let mut data = Vec::with_capacity(rows.len());
        for row in rows {
            data.push(parse(row.clone())?);
        }
        Ok(Self {
            page: number(value.get("page"), 1),
            page_size: number(value.get("page_size"), data.len() as u64),
            total: number(value.get("total"), data.len() as u64),
            total_pages: number(value.get("total_pages"), 1),
            data,
        })
    }
}

/// Taxonomy row with complete unrecognized JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct TaxonomyEntry {
    /// Taxonomy identity.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Complete original JSON.
    pub raw: Value,
}

/// Async catalog client with page and taxonomy generation guards.
pub struct CatalogClient<T: HttpTransport> {
    config: Arc<ApiConfig>,
    transport: Arc<T>,
    latest_page: Arc<AtomicU64>,
    latest_taxonomy: Arc<AtomicU64>,
}

impl<T: HttpTransport> Clone for CatalogClient<T> {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            transport: Arc::clone(&self.transport),
            latest_page: Arc::clone(&self.latest_page),
            latest_taxonomy: Arc::clone(&self.latest_taxonomy),
        }
    }
}

impl<T: HttpTransport> CatalogClient<T> {
    /// Creates a client over shared immutable configuration and transport.
    #[must_use]
    pub fn new(config: ApiConfig, transport: T) -> Self {
        Self {
            config: Arc::new(config),
            transport: Arc::new(transport),
            latest_page: Arc::new(AtomicU64::new(0)),
            latest_taxonomy: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Loads one tone with a one-time 401 refresh handoff.
    pub async fn get_tone(
        &self,
        id: ToneId,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<Tone, ApiError> {
        let response = self
            .authenticated(&format!("{API_PREFIX}/tones/{id}", id = id.0), tokens)
            .await?;
        let value = self.parse(response)?;
        Ok(Tone::from_value(value)?)
    }

    /// Loads the signed-in user's profile.
    pub async fn get_user(
        &self,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<Value, ApiError> {
        let response = self
            .authenticated(&format!("{API_PREFIX}/user"), tokens)
            .await?;
        self.parse(response)
    }

    /// Updates favorite state without a blind retry of the write.
    pub async fn set_favorite(
        &self,
        id: ToneId,
        favorite: bool,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<(), ApiError> {
        let method = if favorite {
            HttpMethod::Put
        } else {
            HttpMethod::Delete
        };
        let response = self
            .authenticated_method(
                method,
                &format!("{API_PREFIX}/tones/{}/favorite", id.0),
                tokens,
            )
            .await?;
        require_success(response.status)?;
        Ok(())
    }

    /// Loads one generation-guarded tone page.
    pub async fn list_tones(
        &self,
        query: ToneQuery,
        page: u32,
        page_size: u32,
        architecture: i32,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<Page<Tone>, ApiError> {
        let generation = self.latest_page.fetch_add(1, Ordering::SeqCst) + 1;
        let result = self
            .list_tones_inner(&query, page, page_size, architecture, tokens)
            .await;
        if self.latest_page.load(Ordering::SeqCst) != generation {
            return Err(ApiError::Superseded { generation });
        }
        result
    }

    async fn list_tones_inner(
        &self,
        query: &ToneQuery,
        page: u32,
        page_size: u32,
        architecture: i32,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<Page<Tone>, ApiError> {
        let path = query.request_path(page, page_size, architecture);
        let response = self.authenticated(&path, tokens).await?;
        let value = self.parse(response)?;
        Page::from_value(value, |value| Ok(Tone::from_value(value)?))
    }

    /// Loads the public or bearer-authenticated trending feed.
    pub async fn list_trending(
        &self,
        gear: Option<&str>,
        bearer: Option<SecretString>,
    ) -> Result<Page<Tone>, ApiError> {
        let path = match gear.filter(|gear| !gear.is_empty()) {
            Some(gear) => format!("{API_PREFIX}/tones/trending?gear={}", escape(gear)),
            None => format!("{API_PREFIX}/tones/trending"),
        };
        let response = self
            .transport
            .execute(HttpRequest {
                headers: bearer_headers(bearer),
                ..self.request(self.url(&path)?, ResponseKind::Json)
            })
            .await
            .map_err(map_http)?;
        require_success(response.status)?;
        let value = self.parse(response)?;
        Page::from_value(value, |value| Ok(Tone::from_value(value)?))
    }

    /// Loads one generation-guarded taxonomy page.
    pub async fn list_taxonomy(
        &self,
        kind: Taxonomy,
        query: &str,
        page_size: u32,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<Page<TaxonomyEntry>, ApiError> {
        let generation = self.latest_taxonomy.fetch_add(1, Ordering::SeqCst) + 1;
        let result = self
            .list_taxonomy_inner(kind, query, page_size, tokens)
            .await;
        if self.latest_taxonomy.load(Ordering::SeqCst) != generation {
            return Err(ApiError::Superseded { generation });
        }
        result
    }

    async fn list_taxonomy_inner(
        &self,
        kind: Taxonomy,
        query: &str,
        page_size: u32,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<Page<TaxonomyEntry>, ApiError> {
        let mut path = format!(
            "{API_PREFIX}/{}?page=1&page_size={page_size}",
            kind.endpoint()
        );
        if !query.is_empty() {
            path.push_str(&format!("&query={}", escape(query)));
        }
        let response = self.authenticated(&path, tokens).await?;
        let value = self.parse(response)?;
        Page::from_value(value, |value| {
            let id = value.get("id").and_then(Value::as_i64).unwrap_or(0);
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Ok(TaxonomyEntry {
                id,
                name,
                raw: value,
            })
        })
    }

    /// Loads the typed models for one tone.
    pub async fn list_models(
        &self,
        tone_id: ToneId,
        page_size: u32,
        architecture: i32,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<Page<ToneModel>, ApiError> {
        let mut path = format!(
            "{API_PREFIX}/models?tone_id={}&page=1&page_size={page_size}",
            tone_id.0
        );
        if architecture >= 0 {
            path.push_str(&format!("&architecture={architecture}"));
        }
        let response = self.authenticated(&path, tokens).await?;
        let value = self.parse(response)?;
        Page::from_value(value, |value| Ok(ToneModel::from_value(&value, false)?))
    }

    /// Fetches the version payload with optional authentication and device headers.
    pub async fn fetch_plugin_version(
        &self,
        device_id: &str,
        local_version: &str,
        bearer: Option<SecretString>,
    ) -> Result<Value, ApiError> {
        let mut headers = bearer_headers(bearer);
        if !device_id.is_empty() {
            headers.push(("X-Device-Id".into(), device_id.into()));
        }
        if !local_version.is_empty() {
            headers.push(("X-Plugin-Version".into(), local_version.into()));
        }
        let response = self
            .transport
            .execute(HttpRequest {
                headers,
                ..self.request(
                    self.url(&format!("{API_PREFIX}/plugin/version"))?,
                    ResponseKind::Json,
                )
            })
            .await
            .map_err(map_http)?;
        require_success(response.status)?;
        self.parse(response)
    }

    async fn authenticated(
        &self,
        path: &str,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<HttpResponse, ApiError> {
        self.authenticated_method(HttpMethod::Get, path, tokens)
            .await
    }

    async fn authenticated_method(
        &self,
        method: HttpMethod,
        path: &str,
        tokens: &(impl TokenProvider + ?Sized),
    ) -> Result<HttpResponse, ApiError> {
        let token = tokens
            .access_token(false)
            .await?
            .ok_or(ApiError::Unauthorized)?;
        let first = self.send_with_token(method, path, token.clone()).await?;
        if first.status != 401 {
            return require_success(first.status).map(|_| first);
        }
        let fresh = tokens
            .refreshed_token(&token)
            .await?
            .ok_or(ApiError::Unauthorized)?;
        let second = self.send_with_token(method, path, fresh).await?;
        require_success(second.status).map(|_| second)
    }

    async fn send_with_token(
        &self,
        method: HttpMethod,
        path: &str,
        token: SecretString,
    ) -> Result<HttpResponse, ApiError> {
        self.transport
            .execute(HttpRequest {
                method,
                headers: bearer_headers(Some(token)),
                content_type: (method != HttpMethod::Get).then(|| "application/json".into()),
                ..self.request(self.url(path)?, ResponseKind::Json)
            })
            .await
            .map_err(map_http)
    }

    fn request(&self, url: Url, response_kind: ResponseKind) -> HttpRequest {
        HttpRequest {
            connect_timeout: Some(self.config.connect_timeout),
            body_deadline: Some(self.config.body_deadline),
            ..HttpRequest::get(url, response_kind)
        }
    }

    fn parse(&self, response: HttpResponse) -> Result<Value, ApiError> {
        Ok(serde_json::from_slice(&response.body)?)
    }

    fn url(&self, path: &str) -> Result<Url, ApiError> {
        let base = self.config.origin.as_str().trim_end_matches('/');
        Url::parse(&format!("{base}{path}"))
            .map_err(|_| ApiError::InvalidDocument("invalid API URL"))
    }
}

fn bearer_headers(bearer: Option<SecretString>) -> Vec<(String, String)> {
    use secrecy::ExposeSecret;
    bearer.map_or_else(Vec::new, |token| {
        vec![(
            "Authorization".into(),
            format!("Bearer {}", token.expose_secret()),
        )]
    })
}

fn require_success(status: u16) -> Result<(), ApiError> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(ApiError::HttpStatus(status))
    }
}

fn map_http(error: crate::HttpError) -> ApiError {
    match error {
        crate::HttpError::ResponseTooLarge { max } => ApiError::ResponseTooLarge { max },
        other => ApiError::Http(other),
    }
}

fn number(value: Option<&Value>, fallback: u64) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(fallback)
}

fn escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

impl From<ToneError> for ApiError {
    fn from(value: ToneError) -> Self {
        Self::Tone(value)
    }
}

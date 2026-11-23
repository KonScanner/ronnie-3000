use std::{fmt, sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::SecretString;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinSet,
    time::timeout,
};
use url::Url;

use crate::{HttpMethod, HttpRequest, HttpResponse, HttpTransport, OAuthError, ResponseKind};

const MAX_CALLBACK_BYTES: usize = 8192;
/// Browsers open spare sockets, so a few connections can wait at the same time.
const MAX_PENDING_CALLBACKS: usize = 8;

/// PKCE verifier, S256 challenge, and anti-CSRF state.
#[derive(Clone)]
pub struct Pkce {
    /// Secret code verifier.
    pub verifier: String,
    /// Public S256 challenge.
    pub challenge: String,
    /// Random callback state.
    pub state: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pkce")
            .field("verifier", &"[redacted]")
            .field("challenge", &self.challenge)
            .field("state", &"[redacted]")
            .finish()
    }
}

impl Pkce {
    /// Generates 32 random verifier bytes and 16 random state bytes, base64url-encoded.
    ///
    /// # Errors
    ///
    /// Returns [`OAuthError::Random`] when the OS random source fails.
    pub fn generate() -> Result<Self, OAuthError> {
        let mut verifier = [0_u8; 32];
        let mut state = [0_u8; 16];
        getrandom::fill(&mut verifier).map_err(|_| OAuthError::Random)?;
        getrandom::fill(&mut state).map_err(|_| OAuthError::Random)?;
        let verifier = URL_SAFE_NO_PAD.encode(verifier);
        let challenge = Self::challenge_for(&verifier);
        Ok(Self {
            verifier,
            challenge,
            state: URL_SAFE_NO_PAD.encode(state),
        })
    }

    /// Computes base64url(SHA-256(verifier)) without padding.
    #[must_use]
    pub fn challenge_for(verifier: &str) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    }
}

/// OAuth endpoint configuration and public client identity.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// Upstream origin.
    pub origin: Url,
    /// Public client key.
    pub client_id: String,
}

impl OAuthConfig {
    /// Creates the v1 browser/device endpoint configuration.
    #[must_use]
    pub fn new(origin: Url, client_id: impl Into<String>) -> Self {
        Self {
            origin,
            client_id: client_id.into(),
        }
    }
}

/// Redacted access/refresh token pair and absolute expiry.
#[derive(Clone)]
pub struct OAuthTokens {
    /// Secret access token.
    pub access: SecretString,
    /// Secret refresh token.
    pub refresh: SecretString,
    /// Expiry in Unix milliseconds.
    pub expires_at_ms: i64,
}

impl fmt::Debug for OAuthTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthTokens")
            .field("access", &"[redacted]")
            .field("refresh", &"[redacted]")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

impl OAuthTokens {
    /// Whether these tokens can still give an access token at a known time.
    #[must_use]
    pub fn usable_at(&self, now_ms: i64) -> bool {
        use secrecy::ExposeSecret;
        now_ms < self.expires_at_ms || !self.refresh.expose_secret().is_empty()
    }

    /// Converts tokens to the persisted JSON shape.
    #[must_use]
    pub fn to_json(&self) -> Value {
        use secrecy::ExposeSecret;
        serde_json::json!({
            "access_token": self.access.expose_secret(),
            "refresh_token": self.refresh.expose_secret(),
            "expires_at": self.expires_at_ms,
        })
    }

    /// Parses the persisted or endpoint JSON shape at a known current time.
    ///
    /// # Errors
    ///
    /// Returns [`OAuthError::InvalidTokenResponse`] when required fields are absent or invalid.
    pub fn from_json(value: &Value, now_ms: i64) -> Result<Self, OAuthError> {
        let access = value
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(OAuthError::InvalidTokenResponse)?;
        let refresh = value
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expires_at_ms = if let Some(expires) = value.get("expires_at").and_then(Value::as_i64) {
            expires
        } else {
            let seconds = value
                .get("expires_in")
                .and_then(Value::as_f64)
                .ok_or(OAuthError::InvalidTokenResponse)?;
            now_ms
                .checked_add((seconds * 1000.0) as i64)
                .ok_or(OAuthError::InvalidTokenResponse)?
        };
        Ok(Self {
            access: SecretString::from(access),
            refresh: SecretString::from(refresh),
            expires_at_ms,
        })
    }
}

/// Public device-flow metadata plus a redacted device code.
#[derive(Clone)]
pub struct DeviceAuthorization {
    /// Secret device code used during polling.
    pub device_code: SecretString,
    /// User-entered code.
    pub user_code: String,
    /// Verification page.
    pub verification_uri: String,
    /// Verification page with the user code included, falling back to `verification_uri`.
    pub verification_uri_complete: String,
    /// Device-code lifetime in seconds.
    pub expires_in_s: u64,
    /// Polling interval in seconds, at least one.
    pub interval_s: u64,
}

impl fmt::Debug for DeviceAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceAuthorization")
            .field("device_code", &"[redacted]")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &self.verification_uri_complete)
            .field("expires_in_s", &self.expires_in_s)
            .field("interval_s", &self.interval_s)
            .finish()
    }
}

/// Parsed redirect result before code exchange.
#[derive(Clone, PartialEq, Eq)]
pub enum CallbackOutcome {
    /// Authorization code ready for exchange.
    Code(String),
    /// User cancelled without a code.
    Canceled,
}

impl fmt::Debug for CallbackOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(_) => formatter.write_str("Code([redacted])"),
            Self::Canceled => formatter.write_str("Canceled"),
        }
    }
}

/// Parses a callback query and validates its single-use state.
///
/// # Errors
///
/// Returns state, server, cancellation, or missing-code errors.
pub fn parse_callback(query: &str, expected_state: &str) -> Result<CallbackOutcome, OAuthError> {
    let query = query.strip_prefix('?').unwrap_or(query);
    let mut code = None;
    let mut state = None;
    let mut canceled = false;
    let mut server_error = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "canceled" => canceled = value == "true",
            "error" => server_error = Some(value.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Err(OAuthError::StateMismatch);
    }
    if canceled && code.as_deref().unwrap_or_default().is_empty() {
        return Ok(CallbackOutcome::Canceled);
    }
    if let Some(error) = server_error {
        return Err(OAuthError::Callback(error));
    }
    let code = code
        .filter(|code| !code.is_empty())
        .ok_or(OAuthError::MissingCode)?;
    Ok(CallbackOutcome::Code(code))
}

/// OAuth browser/device client with a bounded loopback listener.
pub struct OAuthFlow<T: HttpTransport> {
    config: Arc<OAuthConfig>,
    transport: Arc<T>,
}

impl<T: HttpTransport> Clone for OAuthFlow<T> {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            transport: Arc::clone(&self.transport),
        }
    }
}

impl<T: HttpTransport> OAuthFlow<T> {
    /// Creates an OAuth client over a recorded or production transport.
    #[must_use]
    pub fn new(config: OAuthConfig, transport: T) -> Self {
        Self {
            config: Arc::new(config),
            transport: Arc::new(transport),
        }
    }

    /// Starts a single-callback listener on `127.0.0.1:0`, with a `localhost:<port>` redirect.
    ///
    /// # Errors
    ///
    /// Returns random-source or loopback I/O errors.
    pub async fn start_loopback(&self) -> Result<LoopbackFlow, OAuthError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| OAuthError::Loopback)?;
        let port = listener
            .local_addr()
            .map_err(|_| OAuthError::Loopback)?
            .port();
        Ok(LoopbackFlow {
            listener: Some(listener),
            pkce: Pkce::generate()?,
            redirect_uri: format!("http://localhost:{port}/"),
            config: Arc::clone(&self.config),
        })
    }

    /// Exchanges an authorization code using the Reference form fields.
    pub async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
        now_ms: i64,
    ) -> Result<OAuthTokens, OAuthError> {
        let response = self
            .post_form(
                "/api/v1/oauth/token",
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("code_verifier", verifier),
                    ("redirect_uri", redirect_uri),
                    ("client_id", &self.config.client_id),
                ],
            )
            .await?;
        self.tokens(response, now_ms, true).await
    }

    /// Refreshes a token pair using the same token endpoint.
    pub async fn refresh_token(
        &self,
        refresh: &SecretString,
        now_ms: i64,
    ) -> Result<OAuthTokens, OAuthError> {
        use secrecy::ExposeSecret;
        let response = self
            .post_form(
                "/api/v1/oauth/token",
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh.expose_secret()),
                    ("client_id", &self.config.client_id),
                ],
            )
            .await?;
        // These answers say nothing about the refresh token, so they must not end the session.
        if response.status == 429 || (500..600).contains(&response.status) {
            return Err(OAuthError::ServerFailure(response.status));
        }
        self.tokens(response, now_ms, false).await
    }

    /// Starts the device-authorization flow.
    pub async fn request_device_authorization(&self) -> Result<DeviceAuthorization, OAuthError> {
        let response = self
            .post_form(
                "/api/v1/oauth/device_authorization",
                &[("client_id", self.config.client_id.as_str())],
            )
            .await?;
        if !(200..300).contains(&response.status) {
            return Err(OAuthError::DeviceAuthorizationFailed);
        }
        let value: Value =
            serde_json::from_slice(&response.body).map_err(|_| OAuthError::InvalidTokenResponse)?;
        let device_code = required_string(&value, "device_code")?;
        let user_code = required_string(&value, "user_code")?;
        let verification_uri = required_string(&value, "verification_uri")?;
        let verification_uri_complete = value
            .get("verification_uri_complete")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(&verification_uri)
            .to_owned();
        Ok(DeviceAuthorization {
            device_code: SecretString::from(device_code),
            user_code,
            verification_uri,
            verification_uri_complete,
            expires_in_s: value
                .get("expires_in")
                .and_then(Value::as_u64)
                .unwrap_or(600),
            interval_s: value
                .get("interval")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .max(1),
        })
    }

    /// Polls the device-code token endpoint once.
    pub async fn poll_device_token(
        &self,
        device_code: &SecretString,
        now_ms: i64,
    ) -> Result<OAuthTokens, OAuthError> {
        use secrecy::ExposeSecret;
        let response = self
            .post_form(
                "/api/v1/oauth/token",
                &[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", device_code.expose_secret()),
                    ("client_id", &self.config.client_id),
                ],
            )
            .await?;
        // The body of a rate limit or a gateway failure is often not JSON, and it says nothing
        // about the code.
        if response.status == 429 || (500..600).contains(&response.status) {
            return Err(OAuthError::ServerFailure(response.status));
        }
        self.tokens(response, now_ms, false).await
    }

    async fn tokens(
        &self,
        response: HttpResponse,
        now_ms: i64,
        exchange: bool,
    ) -> Result<OAuthTokens, OAuthError> {
        let value: Value =
            serde_json::from_slice(&response.body).map_err(|_| OAuthError::InvalidTokenResponse)?;
        if !(200..300).contains(&response.status) {
            let error = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Err(match error {
                "expired_token" => OAuthError::ExpiredDeviceCode,
                "authorization_pending" if !exchange => OAuthError::AuthorizationPending,
                "slow_down" if !exchange => OAuthError::SlowDown,
                _ if exchange => OAuthError::ExchangeFailed,
                _ => OAuthError::RefreshFailed,
            });
        }
        OAuthTokens::from_json(&value, now_ms)
    }

    async fn post_form(
        &self,
        path: &str,
        fields: &[(&str, &str)],
    ) -> Result<HttpResponse, OAuthError> {
        let body = fields
            .iter()
            .map(|(key, value)| format!("{}={}", form_escape(key), form_escape(value)))
            .collect::<Vec<_>>()
            .join("&");
        self.transport
            .execute(HttpRequest {
                method: HttpMethod::Post,
                url: self.url(path)?,
                headers: Vec::new(),
                body: Some(body.into_bytes()),
                content_type: Some("application/x-www-form-urlencoded".into()),
                response_kind: ResponseKind::Json,
                connect_timeout: None,
                body_deadline: None,
            })
            .await
            .map_err(OAuthError::Http)
    }

    fn url(&self, path: &str) -> Result<Url, OAuthError> {
        let base = self.config.origin.as_str().trim_end_matches('/');
        Url::parse(&format!("{base}{path}")).map_err(|_| OAuthError::InvalidCallback)
    }
}

/// One authorization-code callback and its single-use verifier.
pub struct LoopbackCallback {
    /// Authorization code.
    pub code: String,
    /// PKCE verifier.
    pub verifier: String,
    /// Loopback redirect URI.
    pub redirect_uri: String,
}

impl fmt::Debug for LoopbackCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoopbackCallback")
            .field("code", &"[redacted]")
            .field("verifier", &"[redacted]")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

/// One ephemeral loopback flow and its single-use PKCE state.
pub struct LoopbackFlow {
    listener: Option<TcpListener>,
    pkce: Pkce,
    /// Loopback redirect URI.
    pub redirect_uri: String,
    config: Arc<OAuthConfig>,
}

impl LoopbackFlow {
    /// Current anti-CSRF state.
    #[must_use]
    pub fn state(&self) -> &str {
        &self.pkce.state
    }

    /// Reference browser authorization URL including the menubar flag.
    #[must_use]
    pub fn authorization_url(&self) -> String {
        let fields = [
            ("client_id", self.config.client_id.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("response_type", "code"),
            ("code_challenge", self.pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", self.pkce.state.as_str()),
            ("menubar", "true"),
        ];
        let query = fields
            .iter()
            .map(|(key, value)| format!("{key}={}", form_escape(value)))
            .collect::<Vec<_>>()
            .join("&");
        format!(
            "{}/api/v1/oauth/authorize?{query}",
            self.config.origin.as_str().trim_end_matches('/')
        )
    }

    /// Waits for one callback without an external cancellation source.
    pub async fn wait_for_callback(self) -> Result<LoopbackCallback, OAuthError> {
        let (_sender, canceled) = watch::channel(false);
        self.wait_for_callback_with_cancel(canceled).await
    }

    /// Waits for one bounded callback and shuts down after completion or cancellation.
    pub async fn wait_for_callback_with_cancel(
        mut self,
        mut canceled: watch::Receiver<bool>,
    ) -> Result<LoopbackCallback, OAuthError> {
        if *canceled.borrow() {
            return Err(OAuthError::Cancelled);
        }
        let listener = self.listener.take().ok_or(OAuthError::Cancelled)?;
        let mut connections: JoinSet<(TcpStream, Result<String, OAuthError>)> = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = canceled.changed() => return Err(OAuthError::Cancelled),
                Some(joined) = connections.join_next() => {
                    // A silent or malformed connection must not end the sign-in flow.
                    let Ok((mut stream, Ok(request))) = joined else {
                        continue;
                    };
                    if let Some(result) = self.answer_callback(&mut stream, &request).await {
                        return result;
                    }
                }
                accepted = listener.accept(), if connections.len() < MAX_PENDING_CALLBACKS => {
                    let (mut stream, _) = accepted.map_err(|_| OAuthError::Loopback)?;
                    connections.spawn(async move {
                        let request = read_callback(&mut stream).await;
                        (stream, request)
                    });
                }
            }
        }
    }

    /// Answers one request, and returns a result only for a real callback.
    ///
    /// Browsers also ask for other paths, such as `/favicon.ico`, and those requests
    /// must not end the flow. A failed response write is ignored, because the callback
    /// data is already complete and the browser can close its socket at any time.
    async fn answer_callback(
        &self,
        stream: &mut TcpStream,
        request: &str,
    ) -> Option<Result<LoopbackCallback, OAuthError>> {
        let mut parts = request.lines().next()?.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let target = parts.next().unwrap_or_default();
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if method != "GET" || path != "/" || query.is_empty() {
            let _ = write_response(stream, CallbackPage::NotFound).await;
            return None;
        }
        let outcome = parse_callback(query, &self.pkce.state);
        // Any local process can reach this port. A request without the state of this sign-in
        // did not come from the service, so it must not end the sign-in.
        if matches!(outcome, Err(OAuthError::StateMismatch)) {
            let _ = write_response(stream, CallbackPage::NotThisSignIn).await;
            return None;
        }
        let page = match &outcome {
            Ok(CallbackOutcome::Code(_)) => CallbackPage::Complete,
            Ok(CallbackOutcome::Canceled) => CallbackPage::Canceled,
            Err(_) => CallbackPage::Failed,
        };
        let _ = write_response(stream, page).await;
        Some(match outcome {
            Ok(CallbackOutcome::Code(code)) => Ok(LoopbackCallback {
                code,
                verifier: self.pkce.verifier.clone(),
                redirect_uri: self.redirect_uri.clone(),
            }),
            Ok(CallbackOutcome::Canceled) => Err(OAuthError::Cancelled),
            Err(error) => Err(error),
        })
    }
}

async fn read_callback(stream: &mut TcpStream) -> Result<String, OAuthError> {
    let mut bytes = Vec::new();
    let result = timeout(Duration::from_secs(3), async {
        let mut chunk = [0_u8; 1024];
        loop {
            let count = stream
                .read(&mut chunk)
                .await
                .map_err(|_| OAuthError::Loopback)?;
            if count == 0 {
                return Err(OAuthError::InvalidCallback);
            }
            if bytes.len().saturating_add(count) > MAX_CALLBACK_BYTES {
                return Err(OAuthError::InvalidCallback);
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                return Ok(());
            }
        }
    })
    .await;
    if result.is_err() {
        return Err(OAuthError::CallbackTimeout);
    }
    String::from_utf8(bytes).map_err(|_| OAuthError::InvalidCallback)
}

/// Plain-text page that the system browser shows after the redirect.
#[derive(Clone, Copy)]
enum CallbackPage {
    Complete,
    Canceled,
    Failed,
    NotThisSignIn,
    NotFound,
}

async fn write_response(stream: &mut TcpStream, page: CallbackPage) -> Result<(), OAuthError> {
    let (status, body) = match page {
        CallbackPage::Complete => (
            "200 OK",
            "Ronnie sign-in complete. You can close this window.",
        ),
        CallbackPage::Canceled => (
            "200 OK",
            "Ronnie sign-in was canceled. You can close this window.",
        ),
        CallbackPage::Failed => (
            "400 Bad Request",
            "Ronnie sign-in failed. Return to Ronnie and try again.",
        ),
        CallbackPage::NotThisSignIn => (
            "400 Bad Request",
            "This page is not part of the current Ronnie sign-in. Return to Ronnie.",
        ),
        CallbackPage::NotFound => ("404 Not Found", "Not found."),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|_| OAuthError::Loopback)
}

fn required_string(value: &Value, field: &str) -> Result<String, OAuthError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(OAuthError::InvalidTokenResponse)
}

fn form_escape(value: &str) -> String {
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

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::redirect::Policy;
use tokio::{sync::Notify, time::timeout};
use url::Url;

use crate::HttpError;

const DEFAULT_JSON_LIMIT: usize = 8 * 1024 * 1024;
const DEFAULT_MODEL_LIMIT: usize = 50 * 1024 * 1024;
const DEFAULT_IMAGE_LIMIT: usize = 10 * 1024 * 1024;
const MAX_REDIRECT_HISTORY: usize = 5;

/// Configured upstream and explicitly allowed public origins for model and image traffic.
#[derive(Debug, Clone)]
pub struct OriginPolicy {
    upstream: String,
    public: Vec<String>,
}

impl OriginPolicy {
    /// Creates an origin allowlist from parsed URLs.
    #[must_use]
    pub fn new(upstream: Url, public: impl IntoIterator<Item = Url>) -> Self {
        Self {
            upstream: origin_key(&upstream),
            public: public.into_iter().map(|url| origin_key(&url)).collect(),
        }
    }

    /// Requires HTTPS, no credentials in the URL, and a configured upstream or public origin.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::UnsafeOrigin`] for any other URL.
    pub fn validate(&self, url: &Url) -> Result<(), HttpError> {
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(HttpError::UnsafeOrigin);
        }
        let origin = origin_key(url);
        if origin == self.upstream || self.public.iter().any(|allowed| allowed == &origin) {
            Ok(())
        } else {
            Err(HttpError::UnsafeOrigin)
        }
    }

    /// Returns whether a redirect target remains on an allowed origin.
    #[must_use]
    pub fn permits_redirect(&self, url: &Url) -> bool {
        self.validate(url).is_ok()
    }

    /// Returns whether a request to this URL may carry a credential.
    ///
    /// Only the upstream origin may. The other allowed origins are public, so they need none.
    #[must_use]
    pub fn permits_credentials(&self, url: &Url) -> bool {
        self.validate(url).is_ok() && origin_key(url) == self.upstream
    }
}

/// HTTP methods used by the Reference v1 API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// Read.
    Get,
    /// Create or exchange.
    Post,
    /// Replace.
    Put,
    /// Delete.
    Delete,
}

/// Response body class with an independent byte bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseKind {
    /// Catalog JSON.
    Json,
    /// Model bytes.
    Model,
    /// Encoded image.
    Image,
}

/// One transport request with redacted diagnostic formatting.
#[derive(Clone)]
pub struct HttpRequest {
    /// Request method.
    pub method: HttpMethod,
    /// Absolute URL.
    pub url: Url,
    /// Ordered request headers.
    pub headers: Vec<(String, String)>,
    /// Optional request body.
    pub body: Option<Vec<u8>>,
    /// Optional content type.
    pub content_type: Option<String>,
    /// Response body class.
    pub response_kind: ResponseKind,
    /// Optional per-request connect/header deadline.
    pub connect_timeout: Option<Duration>,
    /// Optional per-request response-body deadline.
    pub body_deadline: Option<Duration>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = self.redacted();
        formatter
            .debug_struct("HttpRequest")
            .field("method", &redacted.method)
            .field("url", &redacted.url)
            .field("headers", &redacted.headers)
            .field("body", &redacted.body)
            .field("content_type", &redacted.content_type)
            .field("response_kind", &redacted.response_kind)
            .field("connect_timeout", &redacted.connect_timeout)
            .field("body_deadline", &redacted.body_deadline)
            .finish()
    }
}

impl HttpRequest {
    /// Creates a bodyless GET request.
    #[must_use]
    pub fn get(url: Url, response_kind: ResponseKind) -> Self {
        Self {
            method: HttpMethod::Get,
            url,
            headers: Vec::new(),
            body: None,
            content_type: None,
            response_kind,
            connect_timeout: None,
            body_deadline: None,
        }
    }

    fn redacted(&self) -> Self {
        let mut request = self.clone();
        for (name, value) in &mut request.headers {
            if matches!(
                name.as_str(),
                "Authorization" | "Proxy-Authorization" | "Cookie"
            ) {
                *value = if name == "Authorization" {
                    "Bearer [redacted]".into()
                } else {
                    "[redacted]".into()
                };
            }
        }
        if request.url.path().contains("oauth") {
            request.body = None;
        }
        request
    }
}

/// Complete bounded transport response.
#[derive(Clone)]
pub struct HttpResponse {
    /// Numeric HTTP status.
    pub status: u16,
    /// Response bytes.
    pub body: Vec<u8>,
}

impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish()
    }
}

impl HttpResponse {
    /// Creates a response from raw bytes.
    #[must_use]
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body }
    }

    /// Creates a JSON response fixture.
    #[must_use]
    pub fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(&value).unwrap_or_default(),
        }
    }
}

/// Async transport contract shared by production and recorded fixtures.
pub trait HttpTransport {
    /// Executes one bounded request without logging credentials or credential-bearing URLs.
    fn execute(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send;
}

#[derive(Default)]
struct RecordedState {
    responses: VecDeque<Result<HttpResponse, HttpError>>,
    requests: Vec<HttpRequest>,
    first: bool,
}

/// Local scripted transport with redacted request evidence and optional first-call gating.
#[derive(Clone)]
pub struct RecordedTransport {
    state: Arc<Mutex<RecordedState>>,
    started: Option<Arc<Notify>>,
    release: Option<Arc<Notify>>,
    json_limit: usize,
    model_limit: usize,
    image_limit: usize,
}

impl RecordedTransport {
    /// Creates a transport that returns each scripted result in order.
    #[must_use]
    pub fn new(responses: Vec<Result<HttpResponse, HttpError>>) -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordedState {
                responses: responses.into(),
                requests: Vec::new(),
                first: true,
            })),
            started: None,
            release: None,
            json_limit: DEFAULT_JSON_LIMIT,
            model_limit: DEFAULT_MODEL_LIMIT,
            image_limit: DEFAULT_IMAGE_LIMIT,
        }
    }

    /// Creates a transport that pauses its first response until `release` is notified.
    #[must_use]
    pub fn gated(
        responses: Vec<Result<HttpResponse, HttpError>>,
        started: Arc<Notify>,
        release: Arc<Notify>,
    ) -> Self {
        let mut transport = Self::new(responses);
        transport.started = Some(started);
        transport.release = Some(release);
        transport
    }

    /// Sets a small JSON bound for adversarial fixtures.
    #[must_use]
    pub fn with_json_limit(responses: Vec<Result<HttpResponse, HttpError>>, limit: usize) -> Self {
        let mut transport = Self::new(responses);
        transport.json_limit = limit;
        transport
    }

    /// Sets a small model bound for adversarial fixtures.
    #[must_use]
    pub fn with_model_limit(responses: Vec<Result<HttpResponse, HttpError>>, limit: usize) -> Self {
        let mut transport = Self::new(responses);
        transport.model_limit = limit;
        transport
    }

    /// Returns secret-redacted requests in execution order.
    #[must_use]
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.lock().requests.clone()
    }

    fn lock(&self) -> MutexGuard<'_, RecordedState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl HttpTransport for RecordedTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let (first, result) = {
            let mut state = self.lock();
            state.requests.push(request.redacted());
            let first = state.first;
            state.first = false;
            let result = state
                .responses
                .pop_front()
                .unwrap_or(Err(HttpError::Transport));
            (first, result)
        };
        if first {
            if let Some(started) = &self.started {
                started.notify_one();
            }
            if let Some(release) = &self.release {
                if let Some(deadline) = request.body_deadline {
                    timeout(deadline, release.notified())
                        .await
                        .map_err(|_| HttpError::BodyTimeout)?;
                } else {
                    release.notified().await;
                }
            }
        }
        let response = result?;
        let limit = match request.response_kind {
            ResponseKind::Json => self.json_limit,
            ResponseKind::Model => self.model_limit,
            ResponseKind::Image => self.image_limit,
        };
        if response.body.len() > limit {
            return Err(HttpError::ResponseTooLarge { max: limit });
        }
        Ok(response)
    }
}

/// Production reqwest/rustls transport with constrained origins and response deadlines.
///
/// Clones share the two clients and one policy.
#[derive(Clone)]
pub struct ReqwestTransport {
    /// Follows a redirect to each allowed origin, for a plain read.
    client: reqwest::Client,
    /// Follows a redirect only to an origin that can receive a credential, for all other requests.
    credential_client: reqwest::Client,
    policy: Arc<OriginPolicy>,
    connect_timeout: Duration,
    body_deadline: Duration,
}

impl ReqwestTransport {
    /// Builds a production transport with 30-second header/connect and 120-second body bounds.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::Transport`] if the HTTP client cannot be built.
    pub fn new(policy: OriginPolicy) -> Result<Self, HttpError> {
        Self::with_timeouts(policy, Duration::from_secs(30), Duration::from_secs(120))
    }

    /// Builds a transport with explicit deadlines for deterministic verification.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::Transport`] if the HTTP client cannot be built.
    pub fn with_timeouts(
        policy: OriginPolicy,
        connect_timeout: Duration,
        body_deadline: Duration,
    ) -> Result<Self, HttpError> {
        let policy = Arc::new(policy);
        let client_with = |redirect| {
            reqwest::Client::builder()
                .connect_timeout(connect_timeout)
                .redirect(redirect)
                .build()
                .map_err(|_| HttpError::Transport)
        };
        Ok(Self {
            client: client_with(redirect_policy(Arc::clone(&policy)))?,
            credential_client: client_with(credential_redirect_policy(Arc::clone(&policy)))?,
            policy,
            connect_timeout,
            body_deadline,
        })
    }
}

fn redirect_policy(policy: Arc<OriginPolicy>) -> Policy {
    redirect_with(move |url| policy.permits_redirect(url))
}

fn credential_redirect_policy(policy: Arc<OriginPolicy>) -> Policy {
    redirect_with(move |url| policy.permits_credentials(url))
}

/// Whether a redirect of this request can go to a public origin.
///
/// The client removes the credential headers when a redirect changes the origin, so a plain read
/// keeps no credential there. A redirect policy cannot see the request, and a 307 or a 308 sends
/// a body again, which can hold a code or a token. Each other request therefore uses a client
/// with the stricter policy.
fn may_follow_to_public_origin(request: &HttpRequest) -> bool {
    request.method == HttpMethod::Get && request.body.is_none()
}

/// The client removes the authorization header when a redirect changes the origin.
fn redirect_with(permits: impl Fn(&Url) -> bool + Send + Sync + 'static) -> Policy {
    Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECT_HISTORY {
            attempt.stop()
        } else if permits(attempt.url()) {
            attempt.follow()
        } else {
            let note = refused_redirect_note(attempt.url());
            attempt.error(note)
        }
    })
}

/// A credential header or a form body. A form body holds a code or a token.
fn carries_credentials(request: &HttpRequest) -> bool {
    request.body.is_some()
        || request.headers.iter().any(|(name, _)| {
            ["Authorization", "Proxy-Authorization", "Cookie"]
                .iter()
                .any(|credential| name.eq_ignore_ascii_case(credential))
        })
}

impl HttpTransport for ReqwestTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.policy.validate(&request.url)?;
        if carries_credentials(&request) && !self.policy.permits_credentials(&request.url) {
            return Err(HttpError::UnsafeOrigin);
        }
        self.send(request).await
    }
}

impl ReqwestTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let client = if may_follow_to_public_origin(&request) {
            &self.client
        } else {
            &self.credential_client
        };
        let method = match request.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };
        let mut builder = client.request(method, request.url);
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        if let Some(content_type) = request.content_type {
            builder = builder.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let limit = match request.response_kind {
            ResponseKind::Json => DEFAULT_JSON_LIMIT,
            ResponseKind::Model => DEFAULT_MODEL_LIMIT,
            ResponseKind::Image => DEFAULT_IMAGE_LIMIT,
        };
        receive_with_deadlines(
            request.connect_timeout.unwrap_or(self.connect_timeout),
            request.body_deadline.unwrap_or(self.body_deadline),
            builder.send(),
            limit,
        )
        .await
    }
}

/// The terminal note for a refused redirect. It names only the host, because the rest of the
/// address can hold a signature.
fn refused_redirect_note(target: &Url) -> String {
    let host = target.host_str().unwrap_or("with no name");
    format!(
        "Ronnie refused a redirect to the host {host}. To allow this host, add its HTTPS origin to RONIE_MODEL_ORIGINS."
    )
}

async fn receive_with_deadlines(
    connect_timeout: Duration,
    body_deadline: Duration,
    send: impl Future<Output = Result<reqwest::Response, reqwest::Error>>,
    limit: usize,
) -> Result<HttpResponse, HttpError> {
    let response = timeout(connect_timeout, send)
        .await
        .map_err(|_| HttpError::ConnectTimeout)?
        .map_err(|error| {
            // Only the redirect policy gives this error. A retry cannot change the answer.
            if error.is_redirect() {
                // The policy put the note here, because only the policy sees the target.
                if let Some(note) = std::error::Error::source(&error) {
                    eprintln!("{note}");
                }
                HttpError::UnsafeOrigin
            } else {
                HttpError::Transport
            }
        })?;
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(HttpError::ResponseTooLarge { max: limit });
    }
    let body = timeout(body_deadline, read_response_body(response, limit))
        .await
        .map_err(|_| HttpError::BodyTimeout)??;
    Ok(HttpResponse { status, body })
}

async fn read_response_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, HttpError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| HttpError::Transport)?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(HttpError::ResponseTooLarge { max: limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn origin_key(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    async fn stalled_server(send_headers: bool) -> (std::net::SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept fixture");
            let mut request = [0_u8; 4096];
            let _ = socket
                .read(&mut request)
                .await
                .expect("read fixture request");
            if send_headers {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
                    .await
                    .expect("write fixture headers");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        (address, task)
    }

    /// Answers each request with a 302, and sends the request text to the test.
    async fn redirecting_server(
        location: String,
    ) -> (
        std::net::SocketAddr,
        tokio::sync::mpsc::UnboundedReceiver<String>,
        JoinHandle<()>,
    ) {
        redirecting_server_with(302, location).await
    }

    /// Answers each request with this redirect status, and sends the request text to the test.
    async fn redirecting_server_with(
        status: u16,
        location: String,
    ) -> (
        std::net::SocketAddr,
        tokio::sync::mpsc::UnboundedReceiver<String>,
        JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.expect("accept fixture");
                let mut request = [0_u8; 4096];
                let count = socket.read(&mut request).await.expect("read fixture");
                let _ = sender.send(String::from_utf8_lossy(&request[..count]).into_owned());
                let response = format!(
                    "HTTP/1.1 {status} Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("write fixture");
            }
        });
        (address, receiver, task)
    }

    /// Answers with an empty success, and reports each request to the test.
    async fn final_server() -> (
        std::net::SocketAddr,
        tokio::sync::mpsc::UnboundedReceiver<String>,
        JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.expect("accept fixture");
                let mut request = Vec::new();
                // A client can write the headers and the body apart, so read until a short pause.
                let mut chunk = [0_u8; 4096];
                while let Ok(Ok(count @ 1..)) =
                    timeout(Duration::from_millis(50), socket.read(&mut chunk)).await
                {
                    request.extend_from_slice(&chunk[..count]);
                }
                let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            }
        });
        (address, receiver, task)
    }

    fn client_with(redirect: Policy) -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(redirect)
            .build()
            .expect("client")
    }

    fn policy_for(upstream: &str, others: &[&str]) -> Arc<OriginPolicy> {
        Arc::new(OriginPolicy::new(
            Url::parse(upstream).expect("upstream"),
            others.iter().map(|url| Url::parse(url).expect("origin")),
        ))
    }

    async fn send_with_bearer(
        client: &reqwest::Client,
        url: String,
    ) -> Result<HttpResponse, HttpError> {
        receive_with_deadlines(
            Duration::from_secs(5),
            Duration::from_secs(5),
            client
                .get(url)
                .header("Authorization", "Bearer secret-token")
                .send(),
            16,
        )
        .await
    }

    #[tokio::test]
    async fn a_redirect_to_an_unapproved_origin_is_refused_before_transmission() {
        let (target, mut target_requests, target_task) = final_server().await;
        // The target uses HTTPS, so only its origin can cause this refusal.
        let (first, mut first_requests, first_task) =
            redirecting_server(format!("https://{target}/model.nam")).await;
        let client = client_with(redirect_policy(policy_for("https://api.example", &[])));

        let result = send_with_bearer(&client, format!("http://{first}/model.nam")).await;

        assert!(matches!(result, Err(HttpError::UnsafeOrigin)), "{result:?}");
        assert!(
            first_requests
                .recv()
                .await
                .expect("first request")
                .contains("secret-token"),
            "the first hop is the fixture for the upstream"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            target_requests.try_recv().is_err(),
            "the target saw a request"
        );
        first_task.abort();
        target_task.abort();
    }

    #[tokio::test]
    async fn a_refused_redirect_names_only_the_host_of_its_target() {
        let (target, _target_requests, target_task) = final_server().await;
        // The name differs from the first hop, so the note can only come from the target.
        let address = format!(
            "https://localhost:{}/secret/model.nam?signature=hidden",
            target.port()
        );
        let (first, _first_requests, first_task) = redirecting_server(address).await;
        let client = client_with(redirect_policy(policy_for("https://api.example", &[])));

        let error = client
            .get(format!("http://{first}/model.nam"))
            .send()
            .await
            .expect_err("the redirect is refused");

        assert!(error.is_redirect());
        let note = std::error::Error::source(&error).expect("note").to_string();
        assert!(note.contains("the host localhost."), "{note}");
        for hidden in ["secret", "signature", "hidden", "127.0.0.1"] {
            assert!(!note.contains(hidden), "{note}");
        }
        first_task.abort();
        target_task.abort();
    }

    #[tokio::test]
    async fn a_redirect_that_leaves_https_is_refused_even_on_an_approved_host() {
        let (target, mut target_requests, target_task) = final_server().await;
        let (first, _first_requests, first_task) =
            redirecting_server(format!("http://{target}/model.nam")).await;
        let approved = format!("https://{target}");
        let client = client_with(redirect_policy(policy_for(&approved, &[])));

        let result = send_with_bearer(&client, format!("http://{first}/model.nam")).await;

        assert!(matches!(result, Err(HttpError::UnsafeOrigin)), "{result:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            target_requests.try_recv().is_err(),
            "the target saw a request"
        );
        first_task.abort();
        target_task.abort();
    }

    fn token_post(url: String) -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: Url::parse(&url).expect("URL"),
            headers: Vec::new(),
            body: Some(b"grant_type=refresh_token&refresh_token=secret-refresh".to_vec()),
            content_type: Some("application/x-www-form-urlencoded".into()),
            response_kind: ResponseKind::Json,
            connect_timeout: Some(Duration::from_secs(5)),
            body_deadline: Some(Duration::from_secs(5)),
        }
    }

    /// The rule for a form body relies on this behavior of the client: a 307 and a 308 send the
    /// body again, also to another origin.
    #[tokio::test]
    async fn a_followed_307_or_308_sends_a_form_body_to_the_other_origin() {
        for status in [307, 308] {
            let (target, mut target_requests, target_task) = final_server().await;
            let (first, _first_requests, first_task) =
                redirecting_server_with(status, format!("http://{target}/token")).await;
            let client = client_with(redirect_with(|_| true));

            let result = client
                .post(format!("http://{first}/token"))
                .body("refresh_token=secret-refresh")
                .send()
                .await;

            assert_eq!(result.expect("followed").status(), 200, "{status}");
            let seen = target_requests.recv().await.expect("target request");
            assert!(seen.starts_with("POST "), "{status}: {seen}");
            assert!(seen.contains("secret-refresh"), "{status}: {seen}");
            first_task.abort();
            target_task.abort();
        }
    }

    #[tokio::test]
    async fn a_form_body_is_not_redirected_to_a_public_origin() {
        for status in [301, 302, 303, 307, 308] {
            let (target, mut target_requests, target_task) = final_server().await;
            let (first, _first_requests, first_task) =
                redirecting_server_with(status, format!("https://{target}/token")).await;
            let approved = format!("https://{target}");
            let transport = ReqwestTransport::new(Arc::unwrap_or_clone(policy_for(
                "https://api.example",
                &[&approved],
            )))
            .expect("transport");

            let result = transport
                .send(token_post(format!("http://{first}/token")))
                .await;

            assert!(
                matches!(result, Err(HttpError::UnsafeOrigin)),
                "{status}: {result:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                target_requests.try_recv().is_err(),
                "{status}: the public origin saw a request"
            );
            first_task.abort();
            target_task.abort();
        }
    }

    /// The fixture has no TLS, so the handshake of a followed redirect fails, and it fails as a
    /// transport error and not as a refused origin.
    #[tokio::test]
    async fn a_form_body_can_follow_a_redirect_on_the_upstream_origin() {
        let (target, mut target_requests, target_task) = final_server().await;
        let (first, _first_requests, first_task) =
            redirecting_server_with(307, format!("https://{target}/token")).await;
        let upstream = format!("https://{target}");
        let transport = ReqwestTransport::new(Arc::unwrap_or_clone(policy_for(&upstream, &[])))
            .expect("transport");

        let result = transport
            .send(token_post(format!("http://{first}/token")))
            .await;

        assert!(matches!(result, Err(HttpError::Transport)), "{result:?}");
        target_requests.recv().await.expect("the handshake arrived");
        first_task.abort();
        target_task.abort();
    }

    #[tokio::test]
    async fn a_model_get_with_a_bearer_follows_302_and_307_to_a_public_origin() {
        for status in [302, 307] {
            let (target, mut target_requests, target_task) = final_server().await;
            let (first, _first_requests, first_task) =
                redirecting_server_with(status, format!("https://{target}/model.nam")).await;
            let approved = format!("https://{target}");
            let transport = ReqwestTransport::new(Arc::unwrap_or_clone(policy_for(
                "https://api.example",
                &[&approved],
            )))
            .expect("transport");
            let request = HttpRequest {
                headers: vec![("Authorization".into(), "Bearer secret-token".into())],
                connect_timeout: Some(Duration::from_secs(5)),
                ..HttpRequest::get(
                    Url::parse(&format!("http://{first}/model.nam")).expect("URL"),
                    ResponseKind::Model,
                )
            };

            let result = transport.send(request).await;

            assert!(
                matches!(result, Err(HttpError::Transport)),
                "{status}: the redirect must be followed, {result:?}"
            );
            target_requests.recv().await.expect("the handshake arrived");
            first_task.abort();
            target_task.abort();
        }
    }

    /// The bearer rule for a redirect to a public origin relies on this behavior of the client.
    #[tokio::test]
    async fn a_followed_redirect_to_another_origin_carries_no_bearer() {
        for status in [302, 307] {
            let (target, mut target_requests, target_task) = final_server().await;
            let (first, mut first_requests, first_task) =
                redirecting_server_with(status, format!("http://{target}/model.nam")).await;
            let client = client_with(redirect_with(|_| true));

            let result = send_with_bearer(&client, format!("http://{first}/model.nam")).await;

            assert_eq!(result.expect("followed").status, 200);
            assert!(
                first_requests
                    .recv()
                    .await
                    .expect("first request")
                    .contains("secret-token")
            );
            assert!(
                !target_requests
                    .recv()
                    .await
                    .expect("target request")
                    .to_ascii_lowercase()
                    .contains("authorization")
            );
            first_task.abort();
            target_task.abort();
        }
    }

    #[tokio::test]
    async fn a_redirect_chain_stops_after_five_requests() {
        // The fixture redirects to itself, so that only the limit ends this chain.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let hops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task = tokio::spawn({
            let hops = Arc::clone(&hops);
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.expect("accept fixture");
                    let mut request = [0_u8; 4096];
                    let _ = socket.read(&mut request).await.expect("read fixture");
                    hops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let response = format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{address}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    socket
                        .write_all(response.as_bytes())
                        .await
                        .expect("write fixture");
                }
            }
        });
        let client = client_with(redirect_with(|_| true));

        let result = send_with_bearer(&client, format!("http://{address}/first")).await;

        assert_eq!(result.expect("stopped").status, 302);
        assert_eq!(hops.load(std::sync::atomic::Ordering::SeqCst), 5);
        task.abort();
    }

    #[test]
    fn a_credential_is_a_header_or_a_body_in_any_spelling() {
        let url = Url::parse("https://api.example/").expect("URL");
        let anonymous = HttpRequest::get(url.clone(), ResponseKind::Model);
        assert!(!carries_credentials(&anonymous));
        for name in [
            "Authorization",
            "authorization",
            "COOKIE",
            "Proxy-Authorization",
        ] {
            let request = HttpRequest {
                headers: vec![(name.to_owned(), "value".to_owned())],
                ..HttpRequest::get(url.clone(), ResponseKind::Model)
            };
            assert!(carries_credentials(&request), "{name}");
        }
        let form = HttpRequest {
            body: Some(b"refresh_token=value".to_vec()),
            ..HttpRequest::get(url, ResponseKind::Json)
        };
        assert!(carries_credentials(&form));
    }

    #[tokio::test]
    async fn header_stall_uses_connect_timeout() {
        let (address, server) = stalled_server(false).await;
        let client = reqwest::Client::new();
        let url = format!("http://{address}");

        let result = receive_with_deadlines(
            Duration::from_millis(20),
            Duration::from_secs(1),
            client.get(url).send(),
            16,
        )
        .await;

        assert!(matches!(result, Err(HttpError::ConnectTimeout)));
        server.abort();
    }

    #[tokio::test]
    async fn body_stall_uses_body_deadline() {
        let (address, server) = stalled_server(true).await;
        let client = reqwest::Client::new();
        let url = format!("http://{address}");

        let result = receive_with_deadlines(
            Duration::from_secs(1),
            Duration::from_millis(20),
            client.get(url).send(),
            16,
        )
        .await;

        assert!(
            matches!(result, Err(HttpError::BodyTimeout)),
            "actual result: {result:?}"
        );
        server.abort();
    }
}

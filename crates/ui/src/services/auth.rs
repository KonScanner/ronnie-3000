//! Production adapter at the session seam: PKCE browser and device sign-in with redacted
//! public state. This adapter writes the tokens to the one store and keeps no copy.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ronie_core::{
    DeviceAuthorization, HttpTransport, OAuthError, OAuthFlow, OAuthTokens, SessionStore,
    SettingsStore,
};
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
    task::JoinHandle,
};

use super::{
    links::is_web_url,
    session::{AuthFlowAdapter, AuthPhase, AuthSnapshot, FlowEvent},
};

/// One step that a flow task reports to its service.
enum FlowMessage {
    BrowserReady(String),
    BrowserFailed(OAuthError),
    DeviceIssued(DeviceAuthorization),
    DeviceRequestFailed(OAuthError),
    Finished(Result<OAuthTokens, OAuthError>),
}

/// A flow task reports at most two steps.
const FLOW_MESSAGES: usize = 2;

/// Longest wait between two device polls. This also bounds the interval of a server.
const MAX_POLL_DELAY: Duration = Duration::from_secs(60);
/// Longest device-code lifetime that a sign-in page waits for.
const MAX_DEVICE_LIFETIME: Duration = Duration::from_secs(3_600);
/// Extra wait that each `slow_down` answer adds, from RFC 8628.
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);

/// Browser and device flow owner with explicit close cancellation.
pub struct AuthService<T: HttpTransport + Send + Sync + 'static> {
    flow: OAuthFlow<T>,
    phase: AuthPhase,
    copied: bool,
    authorize_url: Option<String>,
    device: Option<DeviceAuthorization>,
    error: Option<String>,
    messages: Option<mpsc::Receiver<FlowMessage>>,
    task: Option<JoinHandle<()>>,
    cancel: Option<watch::Sender<bool>>,
    authenticated: bool,
    session: Option<SessionStore<OAuthFlow<T>>>,
    /// Whether the current browser flow exchanges its code. Each flow has its own flag, because
    /// an aborted task can still set its flag after the next flow starts.
    returning: Arc<AtomicBool>,
}

impl<T: HttpTransport + Send + Sync + 'static> AuthService<T> {
    /// Creates a signed-out service without persistence.
    #[must_use]
    pub fn new(flow: OAuthFlow<T>) -> Self {
        Self::from_session(flow, None)
    }

    /// Creates a service that restores and saves tokens through UI preferences.
    #[must_use]
    pub fn with_session(flow: OAuthFlow<T>, settings: SettingsStore) -> Self {
        let session = SessionStore::new(settings, flow.clone());
        Self::with_shared_session(flow, session)
    }

    /// Creates a service over a token store that other parts of the application also use.
    #[must_use]
    pub fn with_shared_session(flow: OAuthFlow<T>, session: SessionStore<OAuthFlow<T>>) -> Self {
        Self::from_session(flow, Some(session))
    }

    fn from_session(flow: OAuthFlow<T>, session: Option<SessionStore<OAuthFlow<T>>>) -> Self {
        let authenticated = session
            .as_ref()
            .and_then(|session| session.read_tokens().ok().flatten())
            .is_some_and(|tokens| tokens.usable_at(now_ms()));
        let phase = if authenticated {
            AuthPhase::Authenticated
        } else {
            AuthPhase::Idle
        };
        Self {
            flow,
            phase,
            copied: false,
            authorize_url: None,
            device: None,
            error: None,
            messages: None,
            task: None,
            cancel: None,
            authenticated,
            session,
            returning: Arc::default(),
        }
    }

    fn complete_result(
        &mut self,
        result: Result<OAuthTokens, OAuthError>,
    ) -> Result<(), OAuthError> {
        match result {
            Ok(tokens) => self.apply_tokens(tokens),
            Err(error) => {
                self.fail(public_message(&error));
                if self.device.is_some() {
                    self.phase = AuthPhase::DeviceFailed;
                }
                Err(error)
            }
        }
    }

    /// Marks a public error phase.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.returning.store(false, Ordering::Release);
        self.phase = AuthPhase::Error;
        self.error = Some(error.into());
    }

    /// Whether tokens were acquired.
    #[must_use]
    pub const fn authenticated(&self) -> bool {
        self.authenticated
    }

    fn apply_tokens(&mut self, tokens: OAuthTokens) -> Result<(), OAuthError> {
        if let Some(session) = &self.session
            && let Err(error) = session.set_tokens(tokens)
        {
            let error = OAuthError::Session(error.to_string());
            self.fail(public_message(&error));
            return Err(error);
        }
        self.returning.store(false, Ordering::Release);
        self.authenticated = true;
        self.phase = AuthPhase::Authenticated;
        self.error = None;
        self.task = None;
        Ok(())
    }
}

impl<T: HttpTransport + Send + Sync + 'static> AuthFlowAdapter for AuthService<T> {
    /// Public, non-secret snapshot.
    fn snapshot(&self) -> AuthSnapshot {
        let phase =
            if self.phase == AuthPhase::BrowserWaiting && self.returning.load(Ordering::Acquire) {
                AuthPhase::Returning
            } else {
                self.phase
            };
        AuthSnapshot {
            phase,
            copied: self.copied,
            user_code: self.device.as_ref().map(|device| device.user_code.clone()),
            verification_uri: self
                .device
                .as_ref()
                .map(|device| device.verification_uri_complete.clone()),
            browser_problem: None,
            pending_intent: "browse".to_owned(),
            error: self.error.clone(),
        }
    }

    fn start_browser(&mut self, runtime: &Handle) {
        self.close();
        self.phase = AuthPhase::BrowserWaiting;
        self.copied = false;
        let (cancel, canceled) = watch::channel(false);
        self.cancel = Some(cancel);
        let (sender, receiver) = mpsc::channel(FLOW_MESSAGES);
        self.messages = Some(receiver);
        let flow = self.flow.clone();
        self.returning = Arc::default();
        let returning = self.returning.clone();
        self.task = Some(runtime.spawn(async move {
            let loopback = match flow.start_loopback().await {
                Ok(loopback) => loopback,
                Err(error) => {
                    let _ = sender.send(FlowMessage::BrowserFailed(error)).await;
                    return;
                }
            };
            let redirect_uri = loopback.redirect_uri.clone();
            let ready = FlowMessage::BrowserReady(loopback.authorization_url());
            if sender.send(ready).await.is_err() {
                return;
            }
            let result = match loopback.wait_for_callback_with_cancel(canceled).await {
                Ok(callback) => {
                    returning.store(true, Ordering::Release);
                    flow.exchange_code(&callback.code, &callback.verifier, &redirect_uri, now_ms())
                        .await
                }
                Err(error) => Err(error),
            };
            let _ = sender.send(FlowMessage::Finished(result)).await;
        }));
    }

    fn start_device(&mut self, runtime: &Handle) {
        self.close();
        self.phase = AuthPhase::DeviceRequesting;
        self.copied = false;
        let flow = self.flow.clone();
        let (sender, receiver) = mpsc::channel(FLOW_MESSAGES);
        self.messages = Some(receiver);
        self.task = Some(runtime.spawn(async move {
            let device = match flow.request_device_authorization().await {
                Ok(device) => device,
                Err(error) => {
                    let _ = sender.send(FlowMessage::DeviceRequestFailed(error)).await;
                    return;
                }
            };
            // The session opens this page with no click, so a refused page ends the flow here.
            if !is_web_url(&device.verification_uri_complete) {
                let refused =
                    FlowMessage::DeviceRequestFailed(OAuthError::DeviceAuthorizationFailed);
                let _ = sender.send(refused).await;
                return;
            }
            if sender
                .send(FlowMessage::DeviceIssued(device.clone()))
                .await
                .is_err()
            {
                return;
            }
            let mut clock = TokioClock(tokio::time::Instant::now());
            let result = poll_device_until_finished(&flow, &device, &mut clock).await;
            let _ = sender.send(FlowMessage::Finished(result)).await;
        }));
    }

    fn poll(&mut self) -> Option<FlowEvent> {
        let message = match self.messages.as_mut()?.try_recv() {
            Ok(message) => message,
            Err(mpsc::error::TryRecvError::Empty) => return None,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                FlowMessage::Finished(Err(OAuthError::Cancelled))
            }
        };
        match message {
            FlowMessage::BrowserReady(url) => {
                self.authorize_url = Some(url.clone());
                Some(FlowEvent::BrowserReady(url))
            }
            FlowMessage::BrowserFailed(error) => {
                self.close();
                Some(FlowEvent::StartFailed(error))
            }
            FlowMessage::DeviceIssued(device) => {
                self.device = Some(device);
                self.phase = AuthPhase::DeviceWaiting;
                None
            }
            FlowMessage::DeviceRequestFailed(error) => {
                self.messages = None;
                self.fail(public_message(&error));
                // "New code" must retry this flow, not start the browser flow.
                self.phase = AuthPhase::DeviceFailed;
                Some(FlowEvent::Finished(Err(error)))
            }
            FlowMessage::Finished(result) => {
                self.messages = None;
                Some(FlowEvent::Finished(self.complete_result(result)))
            }
        }
    }

    /// Returns the authorization URL and records copy feedback without displaying it.
    fn copy_link(&mut self) -> Option<String> {
        // The device-failure page offers this link after the browser flow closed.
        let url = self.authorize_url.clone().or_else(|| {
            self.device
                .as_ref()
                .map(|device| device.verification_uri_complete.clone())
        })?;
        self.copied = true;
        Some(url)
    }

    /// Removes persisted credentials while retaining the cached public user.
    fn sign_out(&mut self) -> Result<(), OAuthError> {
        if let Some(session) = &self.session {
            session
                .logout()
                .map_err(|error| OAuthError::Session(error.to_string()))?;
        }
        self.close();
        self.authenticated = false;
        Ok(())
    }

    /// Closes the flow and cancels the listener and result delivery.
    fn close(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.returning.store(false, Ordering::Release);
        self.messages = None;
        self.authorize_url = None;
        self.device = None;
        self.phase = AuthPhase::Idle;
        self.error = None;
    }
}

impl<T: HttpTransport + Send + Sync + 'static> Drop for AuthService<T> {
    fn drop(&mut self) {
        self.close();
    }
}

/// Time for one device flow. The tests use a clock that does not wait.
trait DeviceClock: Send {
    /// Time since the device code was issued.
    fn elapsed(&self) -> Duration;
    /// Waits for `wait`.
    fn sleep(&mut self, wait: Duration) -> impl Future<Output = ()> + Send;
}

/// The clock of a real device flow.
struct TokioClock(tokio::time::Instant);

impl DeviceClock for TokioClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&mut self, wait: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(wait)
    }
}

/// Polls the token endpoint at the interval that the server asks for, until the code expires.
///
/// A transport failure, a rate limit, or a server failure does not end the flow. The wait doubles
/// for each failure in a row, up to [`MAX_POLL_DELAY`].
async fn poll_device_until_finished<T: HttpTransport + Send + Sync + 'static>(
    flow: &OAuthFlow<T>,
    device: &DeviceAuthorization,
    clock: &mut impl DeviceClock,
) -> Result<OAuthTokens, OAuthError> {
    let lifetime = Duration::from_secs(device.expires_in_s).min(MAX_DEVICE_LIFETIME);
    let mut interval = Duration::from_secs(device.interval_s.max(1)).min(MAX_POLL_DELAY);
    let mut retry = interval;
    loop {
        // A poll runs before each check of the deadline, so a server can give the last word.
        let result = tokio::select! {
            biased;
            result = flow.poll_device_token(&device.device_code, now_ms()) => result,
            () = clock.sleep(lifetime.saturating_sub(clock.elapsed())) => {
                return Err(OAuthError::ExpiredDeviceCode);
            }
        };
        let wait = match result {
            Err(OAuthError::AuthorizationPending) => {
                retry = interval;
                interval
            }
            Err(OAuthError::SlowDown) => {
                interval = interval.saturating_add(SLOW_DOWN_STEP).min(MAX_POLL_DELAY);
                retry = interval;
                interval
            }
            Err(OAuthError::Http(_) | OAuthError::ServerFailure(_)) => {
                retry = retry.saturating_mul(2).min(MAX_POLL_DELAY);
                retry
            }
            result => return result,
        };
        let remaining = lifetime.saturating_sub(clock.elapsed());
        if remaining.is_zero() {
            return Err(OAuthError::ExpiredDeviceCode);
        }
        clock.sleep(wait.min(remaining)).await;
    }
}

/// Text for the sign-in page. A transport error alone does not tell the user what to do.
fn public_message(error: &OAuthError) -> String {
    match error {
        OAuthError::Http(_) => {
            "Could not reach the sign-in service. Check your connection and try again.".to_owned()
        }
        other => other.to_string(),
    }
}

/// Current Unix time in milliseconds.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ronie_core::{HttpError, HttpResponse, OAuthConfig, RecordedTransport, SecretString};
    use tokio::sync::Notify;

    use super::*;

    /// A clock that records each wait and returns at once.
    #[derive(Default)]
    struct FakeClock {
        now: Duration,
        waits: Vec<Duration>,
    }

    impl DeviceClock for FakeClock {
        fn elapsed(&self) -> Duration {
            self.now
        }

        async fn sleep(&mut self, wait: Duration) {
            // A flow without a deadline would run here for ever.
            assert!(self.waits.len() < 1_000, "the flow did not stop");
            self.now += wait;
            self.waits.push(wait);
        }
    }

    fn seconds(values: &[u64]) -> Vec<Duration> {
        values.iter().copied().map(Duration::from_secs).collect()
    }

    fn flow_over(transport: RecordedTransport) -> OAuthFlow<RecordedTransport> {
        OAuthFlow::new(
            OAuthConfig::new(
                url::Url::parse("https://api.example").expect("origin"),
                "id",
            ),
            transport,
        )
    }

    fn flow(responses: Vec<Result<HttpResponse, HttpError>>) -> OAuthFlow<RecordedTransport> {
        flow_over(RecordedTransport::new(responses))
    }

    fn device(expires_in_s: u64, interval_s: u64) -> DeviceAuthorization {
        DeviceAuthorization {
            device_code: SecretString::from("device-secret"),
            user_code: "ABCD".to_owned(),
            verification_uri: "https://api.example/device".to_owned(),
            verification_uri_complete: "https://api.example/device".to_owned(),
            expires_in_s,
            interval_s,
        }
    }

    fn tokens() -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse::json(
            200,
            serde_json::json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600}),
        ))
    }

    fn oauth_error(code: &str) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse::json(
            400,
            serde_json::json!({ "error": code }),
        ))
    }

    /// A gateway failure, and its body is not JSON.
    fn bad_gateway() -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status: 503,
            body: b"<html>Service Unavailable</html>".to_vec(),
        })
    }

    async fn run(
        responses: Vec<Result<HttpResponse, HttpError>>,
        device: &DeviceAuthorization,
    ) -> (Result<OAuthTokens, OAuthError>, FakeClock) {
        let mut clock = FakeClock::default();
        let result = poll_device_until_finished(&flow(responses), device, &mut clock).await;
        (result, clock)
    }

    #[tokio::test]
    async fn a_timeout_does_not_end_the_flow_and_the_retry_waits_longer() {
        let responses = vec![Err(HttpError::ConnectTimeout), tokens()];
        let (result, clock) = run(responses, &device(600, 5)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(clock.waits, seconds(&[10]));
    }

    #[tokio::test]
    async fn a_server_failure_without_json_does_not_end_the_flow() {
        let responses = vec![bad_gateway(), tokens()];
        let (result, clock) = run(responses, &device(600, 5)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(clock.waits, seconds(&[10]));
    }

    #[tokio::test]
    async fn a_rate_limit_does_not_end_the_flow_and_the_retry_waits_longer() {
        let responses = vec![
            Ok(HttpResponse::json(
                429,
                serde_json::json!({"error": "rate_limited"}),
            )),
            Ok(HttpResponse {
                status: 429,
                body: b"Too Many Requests".to_vec(),
            }),
            tokens(),
        ];
        let (result, clock) = run(responses, &device(600, 5)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(clock.waits, seconds(&[10, 20]));
    }

    #[tokio::test]
    async fn the_retry_wait_doubles_to_a_cap_and_a_normal_answer_resets_it() {
        let responses = vec![
            Err(HttpError::Transport),
            bad_gateway(),
            Err(HttpError::BodyTimeout),
            bad_gateway(),
            oauth_error("authorization_pending"),
            Err(HttpError::Transport),
            tokens(),
        ];
        let (result, clock) = run(responses, &device(600, 5)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(clock.waits, seconds(&[10, 20, 40, 60, 5, 10]));
    }

    #[tokio::test]
    async fn a_server_that_stays_pending_stops_at_the_deadline() {
        let responses = (0..100)
            .map(|_| oauth_error("authorization_pending"))
            .collect();
        let (result, clock) = run(responses, &device(11, 5)).await;
        assert!(
            matches!(result, Err(OAuthError::ExpiredDeviceCode)),
            "{result:?}"
        );
        assert_eq!(clock.waits, seconds(&[5, 5, 1]));
    }

    #[tokio::test]
    async fn retries_also_stop_at_the_deadline() {
        // The recorded transport fails after its last response, so that these retries never end.
        let (result, clock) = run(Vec::new(), &device(25, 5)).await;
        assert!(
            matches!(result, Err(OAuthError::ExpiredDeviceCode)),
            "{result:?}"
        );
        assert_eq!(clock.waits, seconds(&[10, 15]));
    }

    #[tokio::test]
    async fn a_request_that_hangs_stops_at_the_deadline() {
        // No one releases this gate, so that the first request never finishes.
        let transport = RecordedTransport::gated(
            vec![tokens()],
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
        );
        let mut clock = FakeClock::default();
        let result =
            poll_device_until_finished(&flow_over(transport), &device(30, 5), &mut clock).await;
        assert!(
            matches!(result, Err(OAuthError::ExpiredDeviceCode)),
            "{result:?}"
        );
        assert_eq!(clock.waits, seconds(&[30]));
    }

    #[tokio::test]
    async fn a_very_large_interval_is_bounded_and_does_not_panic() {
        let responses = vec![
            oauth_error("slow_down"),
            oauth_error("authorization_pending"),
            Err(HttpError::Transport),
            tokens(),
        ];
        let (result, clock) = run(responses, &device(u64::MAX, u64::MAX)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(clock.waits, seconds(&[60, 60, 60]));

        let responses = vec![oauth_error("authorization_pending"), tokens()];
        let (result, clock) = run(responses, &device(u64::MAX, u64::MAX)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(clock.waits, seconds(&[60]));
    }

    #[tokio::test]
    async fn slow_down_adds_to_the_interval() {
        let responses = vec![
            oauth_error("slow_down"),
            oauth_error("authorization_pending"),
            tokens(),
        ];
        let (result, clock) = run(responses, &device(600, 5)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(clock.waits, seconds(&[10, 10]));
    }

    #[tokio::test]
    async fn a_real_oauth_error_in_a_client_failure_is_final() {
        let responses = vec![oauth_error("access_denied"), tokens()];
        let (result, clock) = run(responses, &device(600, 5)).await;
        assert!(
            matches!(result, Err(OAuthError::RefreshFailed)),
            "{result:?}"
        );
        assert!(clock.waits.is_empty());

        let responses = vec![oauth_error("expired_token"), tokens()];
        let (result, _) = run(responses, &device(600, 5)).await;
        assert!(
            matches!(result, Err(OAuthError::ExpiredDeviceCode)),
            "{result:?}"
        );
    }

    #[test]
    fn a_late_store_from_the_previous_flow_does_not_reach_the_next_flow() {
        let mut service = AuthService::new(flow(Vec::new()));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        service.start_browser(runtime.handle());
        let previous = service.returning.clone();

        service.start_browser(runtime.handle());
        // The abort of a task does not stop it at once.
        previous.store(true, Ordering::Release);

        assert_eq!(service.snapshot().phase, AuthPhase::BrowserWaiting);
    }
}

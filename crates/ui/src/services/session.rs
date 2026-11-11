//! The session module: one owner for the sign-in flow, its runtime, and the tokens.
//!
//! Callers start a flow with [`Session::start_browser`] or [`Session::start_device`], poll it
//! with [`Session::poll`], and read one value from [`Session::status`]. Views receive that
//! value when they render and store no copy.
//!
//! This module owns the runtime. It exists only after a flow or other community work starts,
//! and the work gets its [`Handle`] as an argument, so no caller needs a runtime context. This
//! holds on the native UI thread, on a plain thread, and inside a test runtime.
//!
//! [`AuthFlowAdapter`] is the seam. [`crate::services::auth::AuthService`] is the production
//! adapter, and it writes the tokens to the one store. The capture scenarios have their own
//! adapter.

use ronie_core::OAuthError;
use thiserror::Error;
use tokio::runtime::{Handle, Runtime};

/// The one session value. Views take it when they render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// No usable tokens.
    SignedOut,
    /// A finished flow or a restored session supplied tokens.
    SignedIn,
    /// No usable tokens and no flow, so a sign-in cannot work and nothing may offer one.
    NoService,
    /// No flow yet, because the user has no key. A sign-in opens the key page.
    NoKey,
}

impl SessionStatus {
    /// Converts an account flag from a scenario or a host.
    #[must_use]
    pub const fn from_signed_in(signed_in: bool) -> Self {
        if signed_in {
            Self::SignedIn
        } else {
            Self::SignedOut
        }
    }

    /// Whether the user is signed in.
    #[must_use]
    pub const fn signed_in(self) -> bool {
        matches!(self, Self::SignedIn)
    }

    /// Whether a view can offer a sign-in: a flow exists or a key can make one, and the user is
    /// signed out.
    #[must_use]
    pub const fn can_sign_in(self) -> bool {
        matches!(self, Self::SignedOut | Self::NoKey)
    }

    /// Whether a community service exists, so catalog work can get an answer.
    #[must_use]
    pub const fn has_service(self) -> bool {
        matches!(self, Self::SignedOut | Self::SignedIn)
    }
}

/// Why a flow did not start.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The host configured no flow.
    #[error("Sign-in is not available, because no community service is configured.")]
    Unconfigured,
    /// The runtime that a flow needs did not build.
    #[error("Sign-in failed: could not start the sign-in runtime ({0})")]
    Runtime(#[source] std::io::Error),
}

/// What one [`Session::poll`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// The flow finished and the status is now [`SessionStatus::SignedIn`].
    SignedIn,
    /// The flow failed, and the sign-in page shows the failure.
    FlowFailed,
    /// The flow did not start, so no page can show the failure. The text is for a toast.
    Problem(String),
}

/// One progress report from an [`AuthFlowAdapter`].
#[derive(Debug)]
pub enum FlowEvent {
    /// The browser flow is listening. The system browser must open this URL.
    BrowserReady(String),
    /// The browser flow did not start, and the flow is closed again.
    StartFailed(OAuthError),
    /// The flow finished.
    Finished(Result<(), OAuthError>),
}

/// Public authentication phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthPhase {
    /// No active flow.
    Idle,
    /// Waiting for the system browser callback.
    BrowserWaiting,
    /// Requesting a device code.
    DeviceRequesting,
    /// Waiting for the user to complete device authorization.
    DeviceWaiting,
    /// Device authorization failed or expired and needs a new code.
    DeviceFailed,
    /// Exchanging or refreshing credentials.
    Returning,
    /// Authenticated.
    Authenticated,
    /// Flow failed and can be retried or dismissed.
    Error,
}

/// Public, non-secret authentication view state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSnapshot {
    /// Current phase.
    pub phase: AuthPhase,
    /// Copy-link confirmation.
    pub copied: bool,
    /// User-entered device code, never the polling secret.
    pub user_code: Option<String>,
    /// Public verification page.
    pub verification_uri: Option<String>,
    /// Browser-open failure shown during browser authorization.
    pub browser_problem: Option<String>,
    /// Preserved destination after sign-in.
    pub pending_intent: String,
    /// Public error phase details.
    pub error: Option<String>,
}

/// Sign-in flow behind the session. This is the seam for the production and scenario adapters.
///
/// No method needs a runtime context. A method that starts work takes the handle to start it on.
pub trait AuthFlowAdapter: Send {
    /// Public, non-secret state of the flow.
    fn snapshot(&self) -> AuthSnapshot;
    /// Starts browser PKCE on `runtime` without blocking the caller.
    fn start_browser(&mut self, runtime: &Handle);
    /// Starts device authorization on `runtime` without blocking the caller.
    fn start_device(&mut self, runtime: &Handle);
    /// Applies at most one finished step without blocking the caller.
    fn poll(&mut self) -> Option<FlowEvent>;
    /// Copies the authorization URL.
    fn copy_link(&mut self) -> Option<String>;
    /// Removes persisted credentials and closes the flow.
    ///
    /// # Errors
    /// Returns a session error and keeps the credentials if the logout cannot be saved.
    fn sign_out(&mut self) -> Result<(), OAuthError>;
    /// Closes the flow and cancels its work.
    fn close(&mut self);
}

/// Owner of the sign-in flow, its runtime, and the session status.
pub struct Session {
    flow: Option<Box<dyn AuthFlowAdapter>>,
    runtime: Option<Runtime>,
    build_runtime: fn() -> std::io::Result<Runtime>,
    status: SessionStatus,
    /// Whether a key page can make a flow when none exists.
    key_setup: bool,
    page_active: bool,
    url_to_open: Option<String>,
    opened_verification_uri: Option<String>,
}

impl Session {
    /// Creates a session that has no flow. A signed-out status reads as
    /// [`SessionStatus::NoService`], and a sign-in reports a problem.
    #[must_use]
    pub fn new(status: SessionStatus) -> Self {
        Self {
            flow: None,
            runtime: None,
            build_runtime: build_flow_runtime,
            status,
            key_setup: false,
            page_active: false,
            url_to_open: None,
            opened_verification_uri: None,
        }
    }

    /// Creates a session around one flow. A flow that restored its tokens signs the user in.
    #[must_use]
    pub fn with_flow(flow: Box<dyn AuthFlowAdapter>, status: SessionStatus) -> Self {
        let restored = flow.snapshot().phase == AuthPhase::Authenticated;
        let mut session = Self::new(if restored {
            SessionStatus::SignedIn
        } else {
            status
        });
        session.flow = Some(flow);
        session
    }

    /// The one value that views read.
    #[must_use]
    pub const fn status(&self) -> SessionStatus {
        match self.status {
            SessionStatus::SignedOut if self.flow.is_none() => {
                if self.key_setup {
                    SessionStatus::NoKey
                } else {
                    SessionStatus::NoService
                }
            }
            status => status,
        }
    }

    /// Selects whether a session with no flow offers the key page.
    pub fn offer_key_setup(&mut self, offered: bool) {
        self.key_setup = offered;
    }

    /// Replaces the flow, for a service that started after the launch. A flow that restored
    /// its tokens signs the user in. The runtime stays.
    pub fn attach_flow(&mut self, flow: Box<dyn AuthFlowAdapter>) {
        if let Some(previous) = &mut self.flow {
            previous.close();
        }
        self.leave_page();
        self.status = if flow.snapshot().phase == AuthPhase::Authenticated {
            SessionStatus::SignedIn
        } else {
            SessionStatus::SignedOut
        };
        self.flow = Some(flow);
    }

    /// Closes and removes the flow, for a service that stopped. The user is signed out.
    pub fn detach_flow(&mut self) {
        if let Some(mut flow) = self.flow.take() {
            flow.close();
        }
        self.leave_page();
        self.status = SessionStatus::SignedOut;
    }

    /// Starts the system-browser flow without blocking the caller.
    ///
    /// # Errors
    /// Returns an error when no flow is configured or when the runtime does not build.
    pub fn start_browser(&mut self) -> Result<(), SessionError> {
        let runtime = self.runtime()?;
        if let Some(flow) = &mut self.flow {
            flow.start_browser(&runtime);
        }
        self.show_started_page();
        Ok(())
    }

    /// Starts the device flow without blocking the caller.
    ///
    /// # Errors
    /// Returns an error when no flow is configured or when the runtime does not build.
    pub fn start_device(&mut self) -> Result<(), SessionError> {
        let runtime = self.runtime()?;
        if let Some(flow) = &mut self.flow {
            flow.start_device(&runtime);
        }
        self.show_started_page();
        Ok(())
    }

    /// Applies at most one finished step. This needs no UI, so it runs for an occluded window.
    pub fn poll(&mut self) -> Option<SessionEvent> {
        match self.flow.as_mut()?.poll()? {
            FlowEvent::BrowserReady(url) => {
                self.url_to_open = Some(url);
                None
            }
            FlowEvent::StartFailed(error) => {
                self.page_active = false;
                Some(SessionEvent::Problem(format!("Sign-in failed: {error}")))
            }
            FlowEvent::Finished(Ok(())) => {
                self.status = SessionStatus::SignedIn;
                self.page_active = false;
                Some(SessionEvent::SignedIn)
            }
            FlowEvent::Finished(Err(_)) => {
                self.page_active = true;
                Some(SessionEvent::FlowFailed)
            }
        }
    }

    /// Takes the page that the system browser must open, for a frame to pass on.
    pub fn take_url_to_open(&mut self) -> Option<String> {
        if self.page_active
            && let Some(flow) = &self.flow
        {
            let uri = flow.snapshot().verification_uri;
            if uri != self.opened_verification_uri {
                self.opened_verification_uri.clone_from(&uri);
                if uri.is_some() {
                    self.url_to_open = uri;
                }
            }
        }
        self.url_to_open.take()
    }

    /// Whether the sign-in page replaces the signal chain.
    #[must_use]
    pub const fn page_active(&self) -> bool {
        self.page_active
    }

    /// Shows the page of a flow that is already in a phase, without restarting it.
    pub fn show_page(&mut self) {
        self.page_active = true;
    }

    /// Public state for the sign-in page, when a page is active and a flow exists.
    #[must_use]
    pub fn page_snapshot(&self) -> Option<AuthSnapshot> {
        self.flow
            .as_ref()
            .filter(|_| self.page_active)
            .map(|flow| flow.snapshot())
    }

    /// Copies the authorization URL of the active flow.
    pub fn copy_link(&mut self) -> Option<String> {
        self.flow.as_mut().and_then(|flow| flow.copy_link())
    }

    /// Cancels the active flow and leaves the page. This keeps the status.
    pub fn cancel(&mut self) {
        if let Some(flow) = &mut self.flow {
            flow.close();
        }
        self.leave_page();
    }

    /// Removes the credentials, closes any flow, and changes the status to signed out.
    ///
    /// # Errors
    /// Returns a session error and keeps the status if the logout cannot be saved.
    pub fn sign_out(&mut self) -> Result<(), OAuthError> {
        if let Some(flow) = &mut self.flow {
            flow.sign_out()?;
        }
        // The adapter closed the flow, so its page and its URL must not stay.
        self.leave_page();
        self.status = SessionStatus::SignedOut;
        Ok(())
    }

    /// Changes the status to signed out because the token store holds no usable session.
    ///
    /// The store already removed its tokens, so this removes nothing. A newer token from another
    /// process stays. A sign-in that the user started continues, and a finished one signs in.
    pub fn expire(&mut self) {
        self.status = SessionStatus::SignedOut;
        if self.page_active {
            return;
        }
        if let Some(flow) = &mut self.flow {
            flow.close();
        }
        self.leave_page();
    }

    fn leave_page(&mut self) {
        self.page_active = false;
        self.url_to_open = None;
        self.opened_verification_uri = None;
    }

    /// The runtime handle for new work. The runtime is built on the first start.
    ///
    /// # Errors
    /// Returns an error when no flow is configured or when the runtime does not build.
    pub(crate) fn runtime(&mut self) -> Result<Handle, SessionError> {
        if self.flow.is_none() {
            return Err(SessionError::Unconfigured);
        }
        if let Some(runtime) = &self.runtime {
            return Ok(runtime.handle().clone());
        }
        let runtime = (self.build_runtime)().map_err(SessionError::Runtime)?;
        let handle = runtime.handle().clone();
        self.runtime = Some(runtime);
        Ok(handle)
    }

    fn show_started_page(&mut self) {
        self.url_to_open = None;
        self.opened_verification_uri = None;
        self.page_active = true;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The flow cancels its work first.
        self.flow = None;
        // A plain drop of a runtime panics on a thread that is inside another runtime.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// One flow runs at a time and catalog work only waits for the network, so one worker thread is
/// sufficient. Blocking file work runs on the blocking threads of this runtime.
fn build_flow_runtime() -> std::io::Result<Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("ronie-session")
        .enable_all()
        .build()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{backend::MockBackend, default::ChromeController, testbed::load_scenarios};

    /// A flow that does nothing, so a test sees only what the session does.
    struct IdleFlow {
        next: Option<FlowEvent>,
    }

    impl AuthFlowAdapter for IdleFlow {
        fn snapshot(&self) -> AuthSnapshot {
            AuthSnapshot {
                phase: AuthPhase::Idle,
                copied: false,
                user_code: None,
                verification_uri: None,
                browser_problem: None,
                pending_intent: "browse".to_owned(),
                error: None,
            }
        }

        fn start_browser(&mut self, _runtime: &Handle) {}

        fn start_device(&mut self, _runtime: &Handle) {}

        fn poll(&mut self) -> Option<FlowEvent> {
            self.next.take()
        }

        fn copy_link(&mut self) -> Option<String> {
            None
        }

        fn sign_out(&mut self) -> Result<(), OAuthError> {
            Ok(())
        }

        fn close(&mut self) {}
    }

    fn idle_session(next: Option<FlowEvent>) -> Session {
        Session::with_flow(Box::new(IdleFlow { next }), SessionStatus::SignedOut)
    }

    fn no_runtime() -> std::io::Result<Runtime> {
        Err(std::io::Error::other("no threads left"))
    }

    fn chrome(session: Session) -> ChromeController {
        let scenario = load_scenarios(include_str!("../../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "main-mono")
            .expect("main scenario");
        let backend = Arc::new(MockBackend::from_scenario(&scenario));
        ChromeController::with_session(backend, Vec::new(), session, true)
    }

    #[test]
    fn a_session_with_no_flow_and_no_tokens_has_no_service() {
        let mut session = Session::new(SessionStatus::SignedOut);
        assert_eq!(session.status(), SessionStatus::NoService);
        assert!(!session.status().signed_in());
        assert!(!session.status().can_sign_in());
        assert_eq!(idle_session(None).status(), SessionStatus::SignedOut);
        assert!(idle_session(None).status().can_sign_in());
        assert!(
            !session
                .start_browser()
                .expect_err("no flow")
                .to_string()
                .contains("RONIE_")
        );

        let mut restored = Session::new(SessionStatus::SignedIn);
        assert_eq!(restored.status(), SessionStatus::SignedIn);
        restored.sign_out().expect("sign out");
        assert_eq!(restored.status(), SessionStatus::NoService);
        let mut expired = Session::new(SessionStatus::SignedIn);
        expired.expire();
        assert_eq!(expired.status(), SessionStatus::NoService);
    }

    #[test]
    fn no_runtime_exists_before_a_flow_starts() {
        let mut unconfigured = Session::new(SessionStatus::SignedOut);
        assert!(matches!(
            unconfigured.start_browser(),
            Err(SessionError::Unconfigured)
        ));
        assert!(unconfigured.runtime.is_none());

        let mut configured = idle_session(None);
        assert!(
            configured.runtime.is_none(),
            "a flow alone needs no threads"
        );
        assert!(configured.poll().is_none());
        assert!(configured.runtime.is_none(), "a poll needs no threads");
        configured.start_browser().expect("start");
        assert!(configured.runtime.is_some());
    }

    #[test]
    fn a_runtime_that_does_not_build_is_a_sign_in_problem() {
        let mut session = idle_session(None);
        session.build_runtime = no_runtime;
        let mut chrome = chrome(session);

        let error = chrome.sign_in().expect_err("no runtime");
        assert!(error.to_string().contains("no threads left"), "{error}");
        assert!(
            chrome
                .toast()
                .is_some_and(|toast| toast.contains("no threads left")),
            "the user must see why the sign-in did not start"
        );
        assert!(!chrome.auth_active(), "no page can show this problem");
        assert!(chrome.start_device_sign_in().is_err());
    }

    #[test]
    fn a_browser_flow_that_does_not_start_is_a_sign_in_problem() {
        let mut chrome = chrome(idle_session(Some(FlowEvent::StartFailed(
            OAuthError::Loopback,
        ))));
        chrome.sign_in().expect("the start itself does not block");
        assert!(chrome.auth_active());

        chrome.poll_auth();

        assert!(!chrome.auth_active());
        assert!(
            chrome
                .toast()
                .is_some_and(|toast| toast.starts_with("Sign-in failed: "))
        );
    }

    #[test]
    fn a_sign_out_leaves_the_page_and_the_url_of_a_running_flow() {
        let url = "https://api.example/authorize".to_owned();
        let flow = IdleFlow {
            next: Some(FlowEvent::BrowserReady(url)),
        };
        let mut session = Session::with_flow(Box::new(flow), SessionStatus::SignedIn);
        session.start_browser().expect("start");
        assert_eq!(session.poll(), None, "the flow is ready");

        session.sign_out().expect("sign out");

        assert_eq!(session.status(), SessionStatus::SignedOut);
        assert!(!session.page_active(), "the flow of this page is closed");
        assert_eq!(session.take_url_to_open(), None, "nothing listens there");
    }

    #[test]
    fn an_expired_session_is_signed_out_and_keeps_no_credential_call() {
        let mut session =
            Session::with_flow(Box::new(IdleFlow { next: None }), SessionStatus::SignedIn);

        session.expire();

        assert_eq!(session.status(), SessionStatus::SignedOut);
        assert!(!session.page_active());
    }

    #[test]
    fn an_expired_session_keeps_a_sign_in_that_the_user_started() {
        let mut session =
            Session::with_flow(Box::new(IdleFlow { next: None }), SessionStatus::SignedIn);
        session.start_device().expect("start");

        session.expire();

        assert_eq!(session.status(), SessionStatus::SignedOut);
        assert!(session.page_active(), "the running flow keeps its page");
    }

    #[test]
    fn a_failed_flow_keeps_the_page_and_a_finished_flow_leaves_it() {
        let mut failed = idle_session(Some(FlowEvent::Finished(Err(OAuthError::Cancelled))));
        assert_eq!(failed.poll(), Some(SessionEvent::FlowFailed));
        assert!(failed.page_active());
        assert_eq!(failed.status(), SessionStatus::SignedOut);

        let mut finished = idle_session(Some(FlowEvent::Finished(Ok(()))));
        finished.show_page();
        assert_eq!(finished.poll(), Some(SessionEvent::SignedIn));
        assert!(!finished.page_active());
        assert_eq!(finished.status(), SessionStatus::SignedIn);
    }
}

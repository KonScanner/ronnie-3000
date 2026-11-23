//! Offline action gating, confirmed secure probes, and retry backoff.

/// Visible connection problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionProblem {
    /// No network interface is available.
    Offline,
    /// A secure connection could not be established.
    Insecure,
}

/// Connection gate with one queued action and confirmed-failure handling.
pub struct ConnectionGate {
    online: bool,
    problem: Option<ConnectionProblem>,
    pending_action: Option<String>,
    next_retry_ms: i64,
    backoff_ms: i64,
}

impl ConnectionGate {
    /// Creates a gate with the current connectivity state.
    #[must_use]
    pub fn new(online: bool) -> Self {
        Self {
            online,
            problem: None,
            pending_action: None,
            next_retry_ms: 0,
            backoff_ms: 1000,
        }
    }

    /// Updates connectivity without exposing a transient warning by itself.
    pub fn set_online(&mut self, online: bool) {
        self.online = online;
    }

    /// Executes an action online or queues it behind the offline gate.
    pub fn require_connection(&mut self, action: impl Into<String>) -> bool {
        let action = action.into();
        if !self.online {
            self.pending_action = Some(action);
            self.problem = Some(ConnectionProblem::Offline);
            return false;
        }
        self.pending_action = None;
        true
    }

    /// Retries a queued action after its backoff deadline.
    pub fn retry(&mut self, now_ms: i64) -> bool {
        if !self.online {
            return false;
        }
        if self.next_retry_ms == 0 {
            self.next_retry_ms = now_ms + self.backoff_ms;
            return false;
        }
        if now_ms < self.next_retry_ms {
            return false;
        }
        if self.pending_action.is_none() {
            return false;
        }
        self.pending_action = None;
        self.problem = None;
        self.next_retry_ms = now_ms + self.backoff_ms;
        self.backoff_ms = (self.backoff_ms * 2).min(60_000);
        true
    }

    /// Records secure-probe results; a second failure is required before warning.
    pub fn record_probe(&mut self, secure: Option<bool>, now_ms: i64) {
        match secure {
            Some(true) => {
                if self.problem == Some(ConnectionProblem::Insecure) {
                    self.problem = None;
                }
                self.next_retry_ms = now_ms + self.backoff_ms;
            }
            Some(false) => {
                if self.next_retry_ms == 0 {
                    self.next_retry_ms = now_ms + self.backoff_ms;
                    return;
                }
                if self.online && self.problem.is_none() {
                    self.problem = Some(ConnectionProblem::Insecure);
                }
            }
            None => {}
        }
    }

    /// Current visible problem.
    #[must_use]
    pub const fn problem(&self) -> Option<ConnectionProblem> {
        self.problem
    }

    /// Pending action identity, if queued.
    #[must_use]
    pub fn pending_action(&self) -> Option<&str> {
        self.pending_action.as_deref()
    }

    /// Dismisses the warning and drops the queued action.
    pub fn dismiss(&mut self) {
        self.pending_action = None;
        self.problem = None;
    }
}

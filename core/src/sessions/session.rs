//! Host-side session state machine.
//!
//! ```text
//! Requested -> AwaitingApproval -> Negotiating -> Connected <-> Reconnecting
//!      \______________\________________\______________\__________> Ended
//! ```
//! Invalid transitions are rejected, so e.g. media can never start for a
//! session the host user has not approved.

use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum SessionState {
    Requested,
    AwaitingApproval,
    Negotiating,
    Connected,
    Reconnecting,
    Ended(EndReason),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    RejectedByHost,
    ApprovalTimeout,
    ClientDisconnected,
    HostStopped,
    KickedByHost,
    Expired,
    ConnectionFailed(String),
    Error(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid session transition {from:?} -> {to:?}")]
pub struct TransitionError {
    pub from: SessionState,
    pub to: SessionState,
}

#[derive(Debug)]
pub struct Session {
    pub id: String,
    state: SessionState,
    created: Instant,
    approved_at: Option<Instant>,
    max_duration: Duration,
    approval_timeout: Duration,
}

pub const DEFAULT_MAX_SESSION: Duration = Duration::from_secs(12 * 3600);
pub const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);

impl Session {
    pub fn new(id: String) -> Self {
        Self::with_limits(id, DEFAULT_MAX_SESSION, DEFAULT_APPROVAL_TIMEOUT, Instant::now())
    }

    pub fn with_limits(id: String, max_duration: Duration, approval_timeout: Duration, now: Instant) -> Self {
        Session { id, state: SessionState::Requested, created: now, approved_at: None, max_duration, approval_timeout }
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.state, SessionState::Ended(_))
    }

    /// Media may flow and input may be accepted only in these states.
    pub fn is_authorized(&self) -> bool {
        matches!(self.state, SessionState::Negotiating | SessionState::Connected | SessionState::Reconnecting)
    }

    pub fn transition(&mut self, to: SessionState) -> Result<(), TransitionError> {
        self.transition_at(to, Instant::now())
    }

    pub fn transition_at(&mut self, to: SessionState, now: Instant) -> Result<(), TransitionError> {
        use SessionState::*;
        let ok = match (&self.state, &to) {
            (Ended(_), _) => false,
            (_, Ended(_)) => true,
            (Requested, AwaitingApproval) => true,
            (AwaitingApproval, Negotiating) => true,
            (Negotiating, Connected) => true,
            (Connected, Reconnecting) => true,
            (Reconnecting, Negotiating) => true,
            (Reconnecting, Connected) => true,
            _ => false,
        };
        if !ok {
            return Err(TransitionError { from: self.state.clone(), to });
        }
        if to == Negotiating && self.approved_at.is_none() {
            self.approved_at = Some(now);
        }
        tracing::debug!(session = %self.id, from = ?self.state, to = ?to, "session transition");
        self.state = to;
        Ok(())
    }

    /// Apply time-based limits; returns the reason if the session just ended.
    pub fn check_timeouts(&mut self, now: Instant) -> Option<EndReason> {
        let reason = match self.state {
            SessionState::AwaitingApproval if now.duration_since(self.created) > self.approval_timeout => {
                Some(EndReason::ApprovalTimeout)
            }
            SessionState::Ended(_) => None,
            _ => match self.approved_at {
                Some(t) if now.duration_since(t) > self.max_duration => Some(EndReason::Expired),
                _ => None,
            },
        };
        if let Some(r) = &reason {
            self.state = SessionState::Ended(r.clone());
        }
        reason
    }

    pub fn expires_at(&self) -> Option<Instant> {
        self.approved_at.map(|t| t + self.max_duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_and_reconnect() {
        let mut s = Session::new("a".into());
        assert!(!s.is_authorized());
        s.transition(SessionState::AwaitingApproval).unwrap();
        assert!(!s.is_authorized());
        s.transition(SessionState::Negotiating).unwrap();
        assert!(s.is_authorized());
        s.transition(SessionState::Connected).unwrap();
        s.transition(SessionState::Reconnecting).unwrap();
        s.transition(SessionState::Negotiating).unwrap();
        s.transition(SessionState::Connected).unwrap();
        s.transition(SessionState::Ended(EndReason::ClientDisconnected)).unwrap();
        assert!(!s.is_active());
        assert!(!s.is_authorized());
    }

    #[test]
    fn cannot_skip_approval() {
        let mut s = Session::new("a".into());
        assert!(s.transition(SessionState::Negotiating).is_err());
        assert!(s.transition(SessionState::Connected).is_err());
        s.transition(SessionState::AwaitingApproval).unwrap();
        assert!(s.transition(SessionState::Connected).is_err());
    }

    #[test]
    fn ended_is_terminal() {
        let mut s = Session::new("a".into());
        s.transition(SessionState::Ended(EndReason::RejectedByHost)).unwrap();
        assert!(s.transition(SessionState::AwaitingApproval).is_err());
        assert!(s.transition(SessionState::Ended(EndReason::HostStopped)).is_err());
    }

    #[test]
    fn approval_timeout_and_expiry() {
        let t0 = Instant::now();
        let mut s = Session::with_limits("a".into(), Duration::from_secs(100), Duration::from_secs(10), t0);
        s.transition_at(SessionState::AwaitingApproval, t0).unwrap();
        assert_eq!(s.check_timeouts(t0 + Duration::from_secs(5)), None);
        assert_eq!(s.check_timeouts(t0 + Duration::from_secs(11)), Some(EndReason::ApprovalTimeout));

        let mut s = Session::with_limits("b".into(), Duration::from_secs(100), Duration::from_secs(10), t0);
        s.transition_at(SessionState::AwaitingApproval, t0).unwrap();
        s.transition_at(SessionState::Negotiating, t0 + Duration::from_secs(5)).unwrap();
        s.transition_at(SessionState::Connected, t0 + Duration::from_secs(6)).unwrap();
        assert_eq!(s.check_timeouts(t0 + Duration::from_secs(50)), None);
        assert_eq!(s.check_timeouts(t0 + Duration::from_secs(106)), Some(EndReason::Expired));
        assert!(!s.is_authorized());
    }
}

//! Trait-boundary error type for `TimerService`.

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(C)]
pub enum TimerError {
    #[error("client not found: {0}")]
    ClientNotFound(String),

    #[error("engagement not found: {0}")]
    EngagementNotFound(String),

    #[error("work session not found: {0}")]
    WorkSessionNotFound(String),

    #[error("tag not found: {0}")]
    TagNotFound(String),

    /// User tried to start a timer while another session is
    /// still running. The active-timer invariant is "at most
    /// one open WorkSession per user".
    #[error("timer already running for user: {0}")]
    AlreadyRunning(String),

    /// Tried to stop a timer that's not actually open.
    #[error("no active timer for user: {0}")]
    NoActiveTimer(String),

    /// Currency mismatch — e.g. logging a session against an
    /// engagement whose currency differs from the rate row's.
    #[error("currency mismatch: {message}")]
    CurrencyMismatch { message: String },

    #[error("invalid: {0}")]
    Invalid(String),

    /// Cross-org access denied.
    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("backend: {0}")]
    Backend(String),
}

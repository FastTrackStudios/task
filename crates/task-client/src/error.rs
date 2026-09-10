//! What can go wrong reaching a Task org, as a type rather than a
//! string.
//!
//! The CLI used to build its failures directly as `eyre::Report`s
//! carrying a `CliError` marker, because it was the only consumer and
//! the only thing it wanted from a failure was an exit code. A library
//! cannot assume that: an external app may want to retry a
//! [`Error::Connect`] against a different endpoint, prompt for a login
//! on [`Error::Session`], and treat [`Error::NotHosted`] as "create the
//! org" rather than as an error at all. So the variants are the
//! distinctions a caller can actually act on, and nothing finer.
//!
//! Every variant renders a full sentence, because the CLI prints these
//! verbatim and an unhelpful `Display` becomes an unhelpful error
//! message at the terminal.

use std::fmt;

/// A failure establishing a service client.
#[derive(Debug)]
pub enum Error {
    /// The dial itself failed — nothing answered, TLS refused, the
    /// handshake was rejected. The transport target is included because
    /// "connection refused" without a URL is the least useful error
    /// message in computing, and because resolution has several inputs
    /// (flag, env, session, default) so the caller frequently does not
    /// know which one won.
    Connect { url: String, cause: String },
    /// The in-process backend failed to boot, or the router refused the
    /// establish. Distinct from [`Error::Connect`] because no network
    /// was involved: retrying against another endpoint cannot help.
    Embedded { what: String, cause: String },
    /// A per-org URL that has no `/org/<slug>/vox` shape, handed to a
    /// path that needs the slug (embedded mode has no URL to route
    /// with — the slug *is* the routing).
    NoSlug { url: String },
    /// The embedded backend booted, but does not host that org: the
    /// slug has no directory under the local data root. Actionable —
    /// `task org init`, or point at a server that does host it.
    NotHosted { slug: String },
    /// Reading or interpreting the stored session failed. Carries the
    /// underlying report because the session module's own errors are
    /// already written to be read by a human (see
    /// [`crate::session`] on the inline-token trap).
    Session(eyre::Report),
}

impl Error {
    /// True for failures a different endpoint might fix. The CLI maps
    /// this onto its `Connection` exit class (6); an app with a
    /// fallback endpoint list can use it to decide whether to advance.
    #[must_use]
    pub fn is_transport(&self) -> bool {
        matches!(self, Error::Connect { .. })
    }

    /// The underlying cause, without this crate's framing. The CLI
    /// re-frames failures in its own taxonomy and would otherwise
    /// print the endpoint twice.
    #[must_use]
    pub fn cause(&self) -> Option<&str> {
        match self {
            Error::Connect { cause, .. } | Error::Embedded { cause, .. } => Some(cause),
            _ => None,
        }
    }

    /// The endpoint involved, when there was one.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        match self {
            Error::Connect { url, .. } | Error::NoSlug { url } => Some(url),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Connect { url, cause } => write!(f, "connect `{url}`: {cause}"),
            Error::Embedded { what, cause } => write!(f, "embedded {what}: {cause}"),
            Error::NoSlug { url } => {
                write!(
                    f,
                    "can't recover an org slug from `{url}` for embedded mode"
                )
            }
            Error::NotHosted { slug } => {
                write!(f, "org `{slug}` is not hosted by the local data root")
            }
            Error::Session(e) => write!(f, "session: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// The crate's result alias.
pub type Result<T> = std::result::Result<T, Error>;

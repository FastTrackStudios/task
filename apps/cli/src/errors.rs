//! Error taxonomy + rendering for the CLI boundary.
//!
//! The CLI's helpers (`establish_*`, the `resolve_*` family) tag the
//! failures they understand with a [`CliError`] — a typed marker that
//! rides inside the `eyre::Report` chain. `main()` walks the chain
//! once, renders a stable one-line `error: <what failed> (<entity>):
//! <cause>` (plus a `hint:` line when actionable), and exits with a
//! taxonomy code instead of eyre's debug dump.
//!
//! Exit codes (stable, scriptable):
//!
//! | code | meaning                              |
//! |------|--------------------------------------|
//! | 0    | success                              |
//! | 1    | generic / untagged failure           |
//! | 2    | usage error (clap default + tagged)  |
//! | 4    | entity not found                     |
//! | 5    | conflict / ambiguous reference       |
//! | 6    | connection / transport failure       |

use std::fmt;

/// Failure class → process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Bad invocation the parser couldn't catch (mirrors clap's 2).
    Usage,
    /// The referenced entity doesn't exist.
    NotFound,
    /// Conflicting state or an ambiguous reference.
    Conflict,
    /// Couldn't reach the backend at all.
    Connection,
}

impl Kind {
    pub fn exit_code(self) -> i32 {
        match self {
            Kind::Usage => 2,
            Kind::NotFound => 4,
            Kind::Conflict => 5,
            Kind::Connection => 6,
        }
    }
}

/// Typed error carried in an eyre chain. Display renders the stable
/// `<what failed> (<entity>): <cause>` line; the hint is rendered
/// separately by [`exit_with`] so it lands on its own line.
#[derive(Debug)]
pub struct CliError {
    pub kind: Kind,
    /// What the CLI was doing, e.g. `resolve issue`.
    what: String,
    /// The entity reference involved, e.g. `` `abc123` ``.
    entity: Option<String>,
    /// Underlying cause, e.g. the matcher / RPC failure text.
    cause: Option<String>,
    /// One actionable next step, e.g. `pass --force to steal`.
    hint: Option<String>,
}

impl CliError {
    pub fn new(kind: Kind, what: impl Into<String>) -> Self {
        Self {
            kind,
            what: what.into(),
            entity: None,
            cause: None,
            hint: None,
        }
    }

    #[must_use]
    pub fn entity(mut self, entity: impl fmt::Display) -> Self {
        self.entity = Some(format!("`{entity}`"));
        self
    }

    #[must_use]
    pub fn cause(mut self, cause: impl fmt::Display) -> Self {
        let c = cause.to_string();
        if !c.is_empty() {
            self.cause = Some(c);
        }
        self
    }

    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Wrap into an `eyre::Report` so it can be `?`-propagated.
    pub fn report(self) -> eyre::Report {
        eyre::Report::new(self)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.what)?;
        if let Some(e) = &self.entity {
            write!(f, " ({e})")?;
        }
        if let Some(c) = &self.cause {
            write!(f, ": {c}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CliError {}

/// `Kind::NotFound` constructor — `not_found("resolve issue", id)`.
pub fn not_found(what: impl Into<String>, entity: impl fmt::Display) -> CliError {
    CliError::new(Kind::NotFound, what).entity(entity)
}

/// `Kind::Conflict` constructor (also covers ambiguous references).
pub fn conflict(what: impl Into<String>, entity: impl fmt::Display) -> CliError {
    CliError::new(Kind::Conflict, what).entity(entity)
}

/// `Kind::Connection` constructor.
pub fn connection(what: impl Into<String>) -> CliError {
    CliError::new(Kind::Connection, what)
}

/// `Kind::Usage` constructor.
pub fn usage(what: impl Into<String>) -> CliError {
    CliError::new(Kind::Usage, what)
}

/// Find the typed marker anywhere in the report chain.
fn marker(report: &eyre::Report) -> Option<&CliError> {
    report.chain().find_map(|c| c.downcast_ref::<CliError>())
}

/// Exit code a report maps to (1 when untagged).
pub fn exit_code(report: &eyre::Report) -> i32 {
    marker(report).map_or(1, |m| m.kind.exit_code())
}

/// Render a failed run to stderr and exit with the taxonomy code.
/// The boundary `main()` calls this on any `Err`.
pub fn exit_with(report: &eyre::Report) -> ! {
    let mut chain = report.chain();
    if let Some(top) = chain.next() {
        eprintln!("error: {top}");
    }
    for cause in chain {
        // The marker's Display already folded its cause into the
        // line above when it's the top; deeper duplicates are rare
        // and still informative, so print the chain as-is.
        eprintln!("  caused by: {cause}");
    }
    if let Some(h) = marker(report).and_then(|m| m.hint.as_deref()) {
        eprintln!("hint: {h}");
    }
    std::process::exit(exit_code(report));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_full_shape() {
        let e = not_found("resolve issue", "abc123").cause("no issue matches");
        assert_eq!(e.to_string(), "resolve issue (`abc123`): no issue matches");
    }

    #[test]
    fn display_minimal_shape() {
        let e = connection("connect `ws://x/vox`");
        assert_eq!(e.to_string(), "connect `ws://x/vox`");
    }

    #[test]
    fn empty_cause_is_dropped() {
        let e = usage("parse --cycle").cause("");
        assert_eq!(e.to_string(), "parse --cycle");
    }

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(Kind::Usage.exit_code(), 2);
        assert_eq!(Kind::NotFound.exit_code(), 4);
        assert_eq!(Kind::Conflict.exit_code(), 5);
        assert_eq!(Kind::Connection.exit_code(), 6);
    }

    #[test]
    fn marker_found_through_wrapping() {
        use eyre::WrapErr as _;
        let r: eyre::Report = conflict("claim issue", "abc").report();
        let wrapped = Err::<(), _>(r).wrap_err("outer context").unwrap_err();
        assert_eq!(exit_code(&wrapped), 5);
    }

    #[test]
    fn untagged_report_is_generic() {
        let r = eyre::eyre!("plain failure");
        assert_eq!(exit_code(&r), 1);
    }
}

//! Runner capabilities, scope, and routing.
//!
//! A **runner** is a registered, trusted process that executes agent
//! work. It is not a new entity — it is an [`crate::backend::AgentBackend`]
//! that declares what it can do, so routing is a capability match
//! rather than a switch on a backend kind.
//!
//! # Why `Build` is separate from `Shell`
//!
//! The server-side runtime must be able to clone a repository and
//! read real source — triage cannot answer "is this already
//! implemented?" from a summary. It must *not* be able to start a
//! 160-crate compile on the box that also serves the API.
//!
//! Splitting the two makes that a mechanical fact rather than a
//! convention: the server's runner declares [`Capability::Records`],
//! [`Capability::Shell`] and its repositories, and simply omits
//! [`Capability::Build`]. Nothing has to remember the rule.
//!
//! # Closed vocabulary
//!
//! Capabilities are a closed set. An unrecognised capability is
//! rejected at registration rather than stored and silently never
//! matched — a runner that thinks it advertised something it did not
//! is worse than one that failed to start.

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One thing a runner can do.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(C)]
pub enum Capability {
    /// Read and write Task entities. Every runner has this.
    Records,
    /// Run shell commands.
    Shell,
    /// Run compilations and other heavy builds. Deliberately
    /// distinct from [`Self::Shell`].
    Build,
    /// Holds a clone of this repository, as `owner/name`.
    Repo(String),
}

impl Capability {
    /// Parse the wire form: `records`, `shell`, `build`, or
    /// `repo:<owner>/<name>`.
    ///
    /// # Errors
    ///
    /// [`CapabilityError`] when the token is not in the closed set,
    /// or a `repo:` token names nothing.
    pub fn parse(s: &str) -> Result<Self, CapabilityError> {
        let s = s.trim();
        match s.to_ascii_lowercase().as_str() {
            "records" => Ok(Self::Records),
            "shell" => Ok(Self::Shell),
            "build" => Ok(Self::Build),
            _ => match s.split_once(':') {
                Some((k, repo)) if k.eq_ignore_ascii_case("repo") && !repo.trim().is_empty() => {
                    Ok(Self::Repo(repo.trim().to_string()))
                }
                _ => Err(CapabilityError::Unknown(s.to_string())),
            },
        }
    }

    /// The wire form.
    #[must_use]
    pub fn as_string(&self) -> String {
        match self {
            Self::Records => "records".into(),
            Self::Shell => "shell".into(),
            Self::Build => "build".into(),
            Self::Repo(r) => format!("repo:{r}"),
        }
    }
}

/// Why a capability token was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    /// Not in the closed vocabulary.
    #[error("unknown capability `{0}`: expected one of records, shell, build, repo:<owner>/<name>")]
    Unknown(String),
}

/// Parse a whole capability list, rejecting the first bad token.
///
/// # Errors
///
/// [`CapabilityError`] for the first token outside the closed set.
pub fn parse_capabilities<I, S>(tokens: I) -> Result<Vec<Capability>, CapabilityError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    tokens
        .into_iter()
        .map(|t| Capability::parse(t.as_ref()))
        .collect()
}

/// Which work a runner is allowed to see.
///
/// Both lists empty means "everything this runner is authenticated
/// for" — the single-machine default. Narrowing either list narrows
/// what the runner is offered, so a shared box never sees work from
/// an org it should not touch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct RunnerScope {
    /// Org slugs this runner serves. Empty = any.
    pub orgs: Vec<String>,
    /// Project ids this runner serves. Empty = any project within
    /// the scoped orgs.
    pub projects: Vec<Uuid>,
}

impl RunnerScope {
    /// Every org, every project.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self::default()
    }

    /// Does this scope admit work in `org` under `project`?
    #[must_use]
    pub fn admits(&self, org: &str, project: Option<Uuid>) -> bool {
        let org_ok = self.orgs.is_empty() || self.orgs.iter().any(|o| o == org);
        if !org_ok {
            return false;
        }
        if self.projects.is_empty() {
            return true;
        }
        project.is_some_and(|p| self.projects.contains(&p))
    }
}

/// What a ticket needs from whoever runs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct TicketRequirements {
    /// Capabilities the runner must have, all of them.
    pub capabilities: Vec<Capability>,
    /// Org the ticket lives in.
    pub org: String,
    /// Project the ticket belongs to, when it has one.
    pub project: Option<Uuid>,
}

/// A runner's advertised profile, as routing sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct RunnerProfile {
    /// Matches [`crate::backend::AgentBackend::id`].
    pub id: String,
    pub capabilities: Vec<Capability>,
    pub scope: RunnerScope,
    /// How many tickets this runner will hold at once. `0` means the
    /// runner is registered but taking nothing — the way to drain a
    /// box without deregistering it.
    pub max_concurrent: u32,
}

/// Why a runner cannot take a ticket.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unroutable {
    /// The runner lacks a capability the ticket requires.
    #[error("missing capability `{0}`")]
    MissingCapability(String),
    /// The ticket's org or project is outside the runner's scope.
    #[error("out of scope")]
    OutOfScope,
    /// The runner is already holding `max_concurrent` tickets.
    #[error("at capacity")]
    AtCapacity,
}

impl RunnerProfile {
    /// Can this runner take that ticket, given how many it already
    /// holds?
    ///
    /// # Errors
    ///
    /// The first reason it cannot.
    pub fn can_take(&self, req: &TicketRequirements, in_flight: u32) -> Result<(), Unroutable> {
        if !self.scope.admits(&req.org, req.project) {
            return Err(Unroutable::OutOfScope);
        }
        for needed in &req.capabilities {
            if !self.capabilities.contains(needed) {
                return Err(Unroutable::MissingCapability(needed.as_string()));
            }
        }
        if in_flight >= self.max_concurrent {
            return Err(Unroutable::AtCapacity);
        }
        Ok(())
    }
}

/// The capability no registered runner offers, if any.
///
/// A ticket nothing can take is a reported condition, not a ticket
/// that sits in the queue forever looking available. Returns the
/// first such capability so the caller can name it.
#[must_use]
pub fn unsatisfiable_capability(
    req: &TicketRequirements,
    runners: &[RunnerProfile],
) -> Option<String> {
    req.capabilities
        .iter()
        .find(|needed| {
            !runners
                .iter()
                .any(|r| r.scope.admits(&req.org, req.project) && r.capabilities.contains(needed))
        })
        .map(Capability::as_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner(id: &str, caps: &[Capability], max: u32) -> RunnerProfile {
        RunnerProfile {
            id: id.into(),
            capabilities: caps.to_vec(),
            scope: RunnerScope::unrestricted(),
            max_concurrent: max,
        }
    }

    fn needs(caps: &[Capability]) -> TicketRequirements {
        TicketRequirements {
            capabilities: caps.to_vec(),
            org: "fasttrackstudios".into(),
            project: None,
        }
    }

    fn battleship() -> RunnerProfile {
        runner(
            "thebattleship",
            &[
                Capability::Records,
                Capability::Shell,
                Capability::Build,
                Capability::Repo("FastTrackStudios/FastTrackStudio".into()),
            ],
            4,
        )
    }

    /// The server-side runtime: reads source, never compiles.
    fn hermes() -> RunnerProfile {
        runner(
            "hermes",
            &[
                Capability::Records,
                Capability::Shell,
                Capability::Repo("FastTrackStudios/FastTrackStudio".into()),
            ],
            2,
        )
    }

    #[test]
    fn the_closed_vocabulary_round_trips() {
        for c in [
            Capability::Records,
            Capability::Shell,
            Capability::Build,
            Capability::Repo("owner/name".into()),
        ] {
            assert_eq!(Capability::parse(&c.as_string()), Ok(c));
        }
    }

    #[test]
    fn an_unknown_capability_is_rejected_at_registration() {
        for bad in ["compile", "repo:", "repo", "", "gpu"] {
            assert!(Capability::parse(bad).is_err(), "`{bad}` should be refused");
        }
        let err = parse_capabilities(["records", "teleport"]).unwrap_err();
        assert_eq!(err, CapabilityError::Unknown("teleport".into()));
        assert!(err.to_string().contains("repo:<owner>/<name>"));
    }

    #[test]
    fn capability_parsing_is_case_insensitive_and_trims() {
        assert_eq!(Capability::parse(" Records "), Ok(Capability::Records));
        assert_eq!(
            Capability::parse("Repo: owner/name "),
            Ok(Capability::Repo("owner/name".into()))
        );
    }

    #[test]
    fn a_runner_takes_work_it_has_every_capability_for() {
        assert_eq!(
            battleship().can_take(&needs(&[Capability::Build, Capability::Shell]), 0),
            Ok(())
        );
    }

    #[test]
    fn a_ticket_requiring_a_build_never_reaches_a_runner_without_it() {
        // The load-bearing test: this is what keeps compilation off
        // the server, and it is a fact about the model, not a rule
        // someone has to remember.
        assert_eq!(
            hermes().can_take(&needs(&[Capability::Build]), 0),
            Err(Unroutable::MissingCapability("build".into()))
        );
    }

    #[test]
    fn the_server_runner_can_still_read_source_for_triage() {
        let req = needs(&[
            Capability::Records,
            Capability::Shell,
            Capability::Repo("FastTrackStudios/FastTrackStudio".into()),
        ]);
        assert_eq!(hermes().can_take(&req, 0), Ok(()));
    }

    #[test]
    fn a_missing_repo_clone_is_a_missing_capability() {
        let req = needs(&[Capability::Repo("other/repo".into())]);
        assert_eq!(
            battleship().can_take(&req, 0),
            Err(Unroutable::MissingCapability("repo:other/repo".into()))
        );
    }

    #[test]
    fn a_runner_at_its_limit_is_offered_nothing_further() {
        let r = battleship();
        assert_eq!(r.can_take(&needs(&[]), 3), Ok(()));
        assert_eq!(r.can_take(&needs(&[]), 4), Err(Unroutable::AtCapacity));
        assert_eq!(r.can_take(&needs(&[]), 9), Err(Unroutable::AtCapacity));
    }

    #[test]
    fn zero_concurrency_drains_a_runner_without_deregistering_it() {
        let mut r = battleship();
        r.max_concurrent = 0;
        assert_eq!(r.can_take(&needs(&[]), 0), Err(Unroutable::AtCapacity));
    }

    #[test]
    fn an_org_scoped_runner_never_sees_another_orgs_work() {
        let mut r = battleship();
        r.scope = RunnerScope {
            orgs: vec!["fasttrackstudios".into()],
            projects: vec![],
        };
        assert_eq!(r.can_take(&needs(&[]), 0), Ok(()));

        let mut elsewhere = needs(&[]);
        elsewhere.org = "cbu".into();
        assert_eq!(r.can_take(&elsewhere, 0), Err(Unroutable::OutOfScope));
    }

    #[test]
    fn a_runner_can_serve_several_orgs() {
        let mut r = battleship();
        r.scope = RunnerScope {
            orgs: vec!["fasttrackstudios".into(), "tombrooksmusic".into()],
            projects: vec![],
        };
        for org in ["fasttrackstudios", "tombrooksmusic"] {
            let mut req = needs(&[]);
            req.org = org.into();
            assert_eq!(r.can_take(&req, 0), Ok(()), "{org} should be admitted");
        }
        let mut third = needs(&[]);
        third.org = "cbu".into();
        assert_eq!(r.can_take(&third, 0), Err(Unroutable::OutOfScope));
    }

    #[test]
    fn a_project_scoped_runner_never_sees_another_projects_work() {
        let mine = Uuid::new_v4();
        let mut r = battleship();
        r.scope = RunnerScope {
            orgs: vec![],
            projects: vec![mine],
        };

        let mut req = needs(&[]);
        req.project = Some(mine);
        assert_eq!(r.can_take(&req, 0), Ok(()));

        req.project = Some(Uuid::new_v4());
        assert_eq!(r.can_take(&req, 0), Err(Unroutable::OutOfScope));

        // A ticket with no project cannot satisfy a project scope.
        req.project = None;
        assert_eq!(r.can_take(&req, 0), Err(Unroutable::OutOfScope));
    }

    #[test]
    fn scope_is_checked_before_capabilities() {
        // Out-of-scope work must not leak which capabilities a
        // runner is missing.
        let mut r = hermes();
        r.scope = RunnerScope {
            orgs: vec!["other".into()],
            projects: vec![],
        };
        assert_eq!(
            r.can_take(&needs(&[Capability::Build]), 0),
            Err(Unroutable::OutOfScope)
        );
    }

    #[test]
    fn a_ticket_no_runner_satisfies_names_the_missing_capability() {
        let req = needs(&[Capability::Build]);
        assert_eq!(
            unsatisfiable_capability(&req, &[hermes()]),
            Some("build".into())
        );
        assert_eq!(
            unsatisfiable_capability(&req, &[hermes(), battleship()]),
            None
        );
    }

    #[test]
    fn capacity_alone_does_not_make_a_ticket_unroutable() {
        // A busy runner is a wait, not a dead end. Only a capability
        // nobody has is worth reporting.
        let mut r = battleship();
        r.max_concurrent = 0;
        let req = needs(&[Capability::Build]);
        assert_eq!(unsatisfiable_capability(&req, &[r]), None);
    }

    #[test]
    fn an_empty_registry_makes_everything_unsatisfiable() {
        let req = needs(&[Capability::Records]);
        assert_eq!(unsatisfiable_capability(&req, &[]), Some("records".into()));
    }

    #[test]
    fn a_ticket_requiring_nothing_is_always_routable() {
        assert_eq!(unsatisfiable_capability(&needs(&[]), &[]), None);
    }
}

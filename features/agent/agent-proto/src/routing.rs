//! Turning a ticket into a routing question.
//!
//! [`crate::runner`] answers "can this runner take that ticket?"
//! against a [`TicketRequirements`]. This module builds that
//! requirement from a real ticket, and answers the two questions the
//! claim loop asks:
//!
//! - **What can I take?** — [`takeable`], the runner's view of the
//!   queue.
//! - **What can nobody take?** — [`unroutable`], the operator's view.
//!   A ticket no live runner satisfies must be *reported*, not left
//!   sitting in the queue looking available.
//!
//! Kept in `agent-proto` rather than the task crates because the
//! vocabulary being matched — capabilities, scope — is the runner's,
//! and this crate is where a client and a runner already agree on it.

use uuid::Uuid;

use crate::runner::{Capability, CapabilityError, RunnerProfile, TicketRequirements, Unroutable};

/// The bits of a ticket routing cares about.
///
/// A tiny borrowed view rather than a dependency on the task crate:
/// callers on either side of the wire assemble it from whatever
/// their own ticket type is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TicketRef<'a> {
    pub id: Uuid,
    /// Capability tokens as stored on the ticket.
    pub capabilities: &'a [String],
    pub org: &'a str,
    pub project: Option<Uuid>,
}

/// Build the routing requirement for a ticket.
///
/// # Errors
///
/// [`CapabilityError`] when the ticket names a capability outside the
/// closed vocabulary — a typo on a ticket must surface as a bad
/// ticket, not as work that silently never routes.
pub fn requirements(ticket: TicketRef<'_>) -> Result<TicketRequirements, CapabilityError> {
    Ok(TicketRequirements {
        capabilities: ticket
            .capabilities
            .iter()
            .map(|c| Capability::parse(c))
            .collect::<Result<Vec<_>, _>>()?,
        org: ticket.org.to_string(),
        project: ticket.project,
    })
}

/// One ticket a runner cannot take, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub ticket: Uuid,
    pub reason: Unroutable,
}

/// Which of these tickets this runner may take right now, in the
/// order given.
///
/// `in_flight` is how many the runner already holds. Tickets whose
/// capability list does not parse are skipped — they are bad tickets,
/// surfaced by [`malformed`], not routing failures.
#[must_use]
pub fn takeable(
    runner: &RunnerProfile,
    tickets: &[TicketRef<'_>],
    in_flight: u32,
) -> Vec<Uuid> {
    let mut held = in_flight;
    let mut out = Vec::new();
    for t in tickets {
        let Ok(req) = requirements(*t) else { continue };
        if runner.can_take(&req, held).is_ok() {
            out.push(t.id);
            // Taking one fills a slot, so a runner with one free
            // slot is offered exactly one ticket.
            held += 1;
        }
    }
    out
}

/// Why this runner is refusing each ticket it cannot take.
///
/// The diagnostic counterpart to [`takeable`] — "why is my runner
/// idle?" is otherwise unanswerable.
#[must_use]
pub fn refusals(
    runner: &RunnerProfile,
    tickets: &[TicketRef<'_>],
    in_flight: u32,
) -> Vec<Refusal> {
    tickets
        .iter()
        .filter_map(|t| {
            let req = requirements(*t).ok()?;
            runner.can_take(&req, in_flight).err().map(|reason| Refusal {
                ticket: t.id,
                reason,
            })
        })
        .collect()
}

/// Tickets no runner in `runners` can ever take, with the capability
/// that nobody offers.
///
/// Capacity is deliberately not a reason: a busy runner is a wait,
/// not a dead end. Only a capability nothing advertises — or a scope
/// nothing covers — makes a ticket genuinely stuck.
#[must_use]
pub fn unroutable(tickets: &[TicketRef<'_>], runners: &[RunnerProfile]) -> Vec<(Uuid, Stuck)> {
    tickets
        .iter()
        .filter_map(|t| {
            let req = requirements(*t).ok()?;
            // An empty fleet is its own diagnosis. Reporting it as a
            // scope problem sends you to edit runner scopes when the
            // real answer is that nothing has heartbeated.
            if runners.is_empty() {
                return Some((t.id, Stuck::NoLiveRunners));
            }
            if !runners.iter().any(|r| r.scope.admits(&req.org, req.project)) {
                return Some((t.id, Stuck::OutOfEveryScope));
            }
            crate::runner::unsatisfiable_capability(&req, runners)
                .map(|cap| (t.id, Stuck::NobodyOffers(cap)))
        })
        .collect()
}

/// Why nothing in the fleet can take a ticket.
///
/// Three different problems with three different fixes: start a
/// runner, widen a scope, or add a machine that can do the thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stuck {
    /// No runner is currently live. Nothing has heartbeated.
    NoLiveRunners,
    /// Live runners exist, but none is scoped to this ticket's org
    /// or project.
    OutOfEveryScope,
    /// In-scope runners exist, but none advertises this capability.
    NobodyOffers(String),
}

impl core::fmt::Display for Stuck {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoLiveRunners => f.write_str("no live runners — has anything heartbeated?"),
            Self::OutOfEveryScope => {
                f.write_str("no live runner is scoped to this org/project")
            }
            Self::NobodyOffers(cap) => write!(f, "no live runner offers `{cap}`"),
        }
    }
}

/// Tickets whose capability list does not parse, with the bad token.
///
/// Separate from [`unroutable`] because the fix is different: a
/// malformed ticket needs editing, an unroutable one needs a machine.
#[must_use]
pub fn malformed(tickets: &[TicketRef<'_>]) -> Vec<(Uuid, String)> {
    tickets
        .iter()
        .filter_map(|t| match requirements(*t) {
            Ok(_) => None,
            Err(e) => Some((t.id, e.to_string())),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RunnerScope;

    fn profile(id: &str, caps: Vec<Capability>, max: u32) -> RunnerProfile {
        RunnerProfile {
            id: id.into(),
            capabilities: caps,
            scope: RunnerScope::unrestricted(),
            max_concurrent: max,
        }
    }

    fn battleship(max: u32) -> RunnerProfile {
        profile(
            "THEBATTLESHIP",
            vec![Capability::Records, Capability::Shell, Capability::Build],
            max,
        )
    }

    fn hermes() -> RunnerProfile {
        profile("hermes", vec![Capability::Records, Capability::Shell], 2)
    }

    const NONE: &[String] = &[];

    fn ticket<'a>(caps: &'a [String]) -> TicketRef<'a> {
        TicketRef {
            id: Uuid::new_v4(),
            capabilities: caps,
            org: "fasttrackstudios",
            project: None,
        }
    }

    fn caps(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_ticket_with_no_requirements_routes_to_anyone() {
        let t = ticket(NONE);
        assert_eq!(takeable(&hermes(), &[t], 0), vec![t.id]);
    }

    #[test]
    fn a_build_ticket_reaches_only_a_runner_that_can_build() {
        let c = caps(&["build"]);
        let t = ticket(&c);
        assert_eq!(takeable(&battleship(4), &[t], 0), vec![t.id]);
        assert!(takeable(&hermes(), &[t], 0).is_empty());
    }

    #[test]
    fn refusals_say_why_a_runner_is_idle() {
        let c = caps(&["build"]);
        let t = ticket(&c);
        assert_eq!(
            refusals(&hermes(), &[t], 0),
            vec![Refusal {
                ticket: t.id,
                reason: Unroutable::MissingCapability("build".into())
            }]
        );
    }

    #[test]
    fn a_runner_is_offered_only_as_many_tickets_as_it_has_free_slots() {
        let r = battleship(2);
        let t1 = ticket(NONE);
        let t2 = ticket(NONE);
        let t3 = ticket(NONE);
        assert_eq!(takeable(&r, &[t1, t2, t3], 0).len(), 2);
        assert_eq!(takeable(&r, &[t1, t2, t3], 1).len(), 1);
        assert!(takeable(&r, &[t1, t2, t3], 2).is_empty());
    }

    #[test]
    fn ticket_order_is_preserved() {
        let r = battleship(4);
        let t1 = ticket(NONE);
        let t2 = ticket(NONE);
        assert_eq!(takeable(&r, &[t1, t2], 0), vec![t1.id, t2.id]);
    }

    #[test]
    fn a_ticket_no_live_runner_satisfies_is_reported_with_the_missing_capability() {
        let c = caps(&["build"]);
        let t = ticket(&c);
        assert_eq!(
            unroutable(&[t], &[hermes()]),
            vec![(t.id, Stuck::NobodyOffers("build".into()))]
        );
        assert!(unroutable(&[t], &[hermes(), battleship(4)]).is_empty());
    }

    #[test]
    fn an_empty_fleet_is_diagnosed_as_no_live_runners_not_as_a_scope_problem() {
        // Found the hard way: with every runner stale, the scope
        // branch fired and sent you off to edit runner scopes when
        // the real answer was that nothing had heartbeated.
        let c = caps(&["build"]);
        let t = ticket(&c);
        assert_eq!(unroutable(&[t], &[]), vec![(t.id, Stuck::NoLiveRunners)]);
        assert!(
            Stuck::NoLiveRunners.to_string().contains("heartbeat"),
            "the message must point at the actual fix"
        );
    }

    #[test]
    fn a_busy_runner_does_not_make_a_ticket_unroutable() {
        let c = caps(&["build"]);
        let t = ticket(&c);
        assert!(
            unroutable(&[t], &[battleship(0)]).is_empty(),
            "capacity is a wait, not a dead end"
        );
    }

    #[test]
    fn an_org_nothing_serves_is_reported_as_a_scope_problem() {
        let mut r = battleship(4);
        r.scope = RunnerScope {
            orgs: vec!["somewhere-else".into()],
            projects: vec![],
        };
        let t = ticket(NONE);
        assert_eq!(unroutable(&[t], &[r]), vec![(t.id, Stuck::OutOfEveryScope)]);
    }

    #[test]
    fn the_three_reasons_are_distinguishable() {
        // Each has a different fix — start a runner, widen a scope,
        // add a machine — so they must never collapse into one
        // message.
        let mut narrow = battleship(4);
        narrow.scope = RunnerScope {
            orgs: vec!["somewhere-else".into()],
            projects: vec![],
        };
        let c = caps(&["build"]);
        let t = ticket(&c);
        assert_eq!(unroutable(&[t], &[])[0].1, Stuck::NoLiveRunners);
        assert_eq!(unroutable(&[t], &[narrow])[0].1, Stuck::OutOfEveryScope);
        assert_eq!(
            unroutable(&[t], &[hermes()])[0].1,
            Stuck::NobodyOffers("build".into())
        );
    }

    #[test]
    fn a_typo_on_a_ticket_is_malformed_not_unroutable() {
        let c = caps(&["biuld"]);
        let t = ticket(&c);

        // It must not silently sit in the queue looking takeable...
        assert!(takeable(&battleship(4), &[t], 0).is_empty());
        // ...must not be blamed on the fleet...
        assert!(unroutable(&[t], &[battleship(4)]).is_empty());
        // ...and must be reported as the bad ticket it is.
        let bad = malformed(&[t]);
        assert_eq!(bad.len(), 1);
        assert!(bad[0].1.contains("biuld"), "{bad:?}");
    }

    #[test]
    fn a_well_formed_ticket_is_not_malformed() {
        let c = caps(&["build", "repo:FastTrackStudios/FastTrackStudio"]);
        assert!(malformed(&[ticket(&c)]).is_empty());
    }
}

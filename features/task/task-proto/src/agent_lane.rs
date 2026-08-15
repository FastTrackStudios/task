//! The agent lane's triage vocabulary — four labels, no state
//! machine.
//!
//! An issue in the agent lane carries at most one **triage label**,
//! and the four are deliberately distinct because each lands in a
//! different place with a different human action:
//!
//! - [`TriageLabel::ReadyForAgent`] — an agent should take this.
//! - [`TriageLabel::ReadyForHuman`] — this was never agent-able; a
//!   person does it.
//! - [`TriageLabel::NeedsInput`] — an agent parked mid-run and asked
//!   a question. The session is resumable; answering continues it.
//! - [`TriageLabel::NeedsReview`] — an agent finished and its verify
//!   command went green. A branch is waiting.
//!
//! Collapsing the last three into one label would make the
//! per-project agent surface unbuildable: "answer me", "do this
//! yourself" and "merge this" are three queues, not one.
//!
//! **Untriaged is the absence of all four** — see [`is_untriaged`].
//! There is no stored flag, because a stored flag can drift from the
//! labels it claims to summarise.
//!
//! Distinct from [`crate::filing::is_unfiled`], which asks whether a
//! task hangs off anything at all. That is about filing; this is
//! about whether an agent has evaluated the request.
//!
//! Labels live in `TaskInfo::tags`, so they are queryable through the
//! ordinary tag surface with no new storage.

use crate::model::TaskInfo;

/// One of the four agent-lane triage labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TriageLabel {
    /// Fully specified; an agent should pick this up.
    ReadyForAgent,
    /// Needs a person — judgment, external access, or a decision an
    /// agent must not make.
    ReadyForHuman,
    /// An agent stopped and asked a question. Resumable.
    NeedsInput,
    /// An agent finished green. A branch is waiting for review.
    NeedsReview,
}

/// Every triage label, in the order a queue should present them:
/// what blocks a human first, then what an agent can take.
pub const ALL: [TriageLabel; 4] = [
    TriageLabel::NeedsInput,
    TriageLabel::NeedsReview,
    TriageLabel::ReadyForHuman,
    TriageLabel::ReadyForAgent,
];

impl TriageLabel {
    /// The tag string as it is stored on a task.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadyForAgent => "ready-for-agent",
            Self::ReadyForHuman => "ready-for-human",
            Self::NeedsInput => "needs-input",
            Self::NeedsReview => "needs-review",
        }
    }

    /// Parse a tag string. Case-insensitive and tolerant of a leading
    /// `#`, because tags arrive from a CLI flag, a frontmatter list
    /// and a UI chip, and all three have been seen to differ.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#').to_ascii_lowercase();
        ALL.into_iter().find(|l| l.as_str() == s)
    }

    /// Does this label mean a human is being waited on?
    ///
    /// True for the three human-facing states. Used to answer "is
    /// anything blocking me?" without enumerating them at each call
    /// site — the question the per-project surface is built around.
    #[must_use]
    pub fn blocks_human(self) -> bool {
        !matches!(self, Self::ReadyForAgent)
    }
}

/// Every triage label present on a task, sorted and deduplicated.
///
/// Returns a vector rather than an `Option` because nothing prevents
/// two labels being applied by hand; callers that need exactly one
/// use [`triage_label`], which reports the conflict instead of
/// silently picking a winner.
#[must_use]
pub fn triage_labels(t: &TaskInfo) -> Vec<TriageLabel> {
    let mut found: Vec<TriageLabel> = t
        .tags
        .iter()
        .filter_map(|s| TriageLabel::parse(s))
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// The task's single triage label.
///
/// `Ok(None)` means untriaged. `Err` carries every label found when
/// more than one is present — a conflict the caller must surface
/// rather than resolve, because guessing which of `needs-input` and
/// `needs-review` was meant would route the issue to the wrong queue.
///
/// # Errors
///
/// Returns the full label list when a task carries more than one.
pub fn triage_label(t: &TaskInfo) -> Result<Option<TriageLabel>, Vec<TriageLabel>> {
    let found = triage_labels(t);
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found[0])),
        _ => Err(found),
    }
}

/// Has anything triaged this issue yet?
///
/// Untriaged is the *absence* of all four labels — there is no stored
/// flag to fall out of step with them.
#[must_use]
pub fn is_untriaged(t: &TaskInfo) -> bool {
    triage_labels(t).is_empty()
}

/// Does this issue carry the given triage label?
#[must_use]
pub fn has_triage_label(t: &TaskInfo, label: TriageLabel) -> bool {
    t.tags.iter().any(|s| TriageLabel::parse(s) == Some(label))
}

/// Why an issue cannot be marked [`TriageLabel::ReadyForAgent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAgentReady {
    /// Nothing in the ticket-then-project chain declares a verify
    /// command, so an agent would have no way to know it was done.
    NoVerifyCommand,
}

impl NotAgentReady {
    /// A message naming the reason, for a CLI or a UI toast.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::NoVerifyCommand => {
                "no verify command: set `verifyCommand` on the ticket or its project \
                 — an agent needs an exit code to know it is done"
            }
        }
    }
}

impl core::fmt::Display for NotAgentReady {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for NotAgentReady {}

/// May this issue be marked ready for an agent?
///
/// `resolved_verify` is the outcome of walking the ticket override
/// and then the project chain — see `project_proto::verify::resolve`.
/// The chain walk needs project data, which this wasm-clean crate
/// deliberately does not depend on, so the caller resolves and this
/// function judges.
///
/// This is a **gate, not a warning**. A ticket with no verify command
/// is one whose completion nobody can check, and an agent lane built
/// on unverifiable tickets is a review queue with extra steps.
///
/// # Errors
///
/// [`NotAgentReady::NoVerifyCommand`] when nothing resolved.
pub fn check_agent_ready(resolved_verify: Option<&str>) -> Result<(), NotAgentReady> {
    match resolved_verify.map(str::trim) {
        Some(cmd) if !cmd.is_empty() => Ok(()),
        _ => Err(NotAgentReady::NoVerifyCommand),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tagged(tags: &[&str]) -> TaskInfo {
        let mut t = TaskInfo::new("x");
        t.tags = tags
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>()
            .into();
        t
    }

    #[test]
    fn the_four_labels_round_trip_through_their_strings() {
        for label in ALL {
            assert_eq!(TriageLabel::parse(label.as_str()), Some(label));
        }
    }

    #[test]
    fn parsing_tolerates_case_whitespace_and_a_leading_hash() {
        assert_eq!(
            TriageLabel::parse("  #Ready-For-Agent "),
            Some(TriageLabel::ReadyForAgent)
        );
    }

    #[test]
    fn an_unrelated_tag_is_not_a_triage_label() {
        assert_eq!(TriageLabel::parse("rust"), None);
        assert!(is_untriaged(&tagged(&["rust", "audio"])));
    }

    #[test]
    fn untriaged_is_the_absence_of_all_four() {
        assert!(is_untriaged(&tagged(&[])));
        for label in ALL {
            assert!(
                !is_untriaged(&tagged(&[label.as_str()])),
                "{label:?} should count as triaged"
            );
        }
    }

    #[test]
    fn each_label_is_queryable_on_its_own() {
        // The property that matters: applying one label must not make
        // any *other* label's query match. This is what keeps
        // needs-input and needs-review in separate panels.
        for applied in ALL {
            let t = tagged(&[applied.as_str()]);
            for probe in ALL {
                assert_eq!(
                    has_triage_label(&t, probe),
                    probe == applied,
                    "{probe:?} matched a task labelled {applied:?}"
                );
            }
        }
    }

    #[test]
    fn needs_input_and_needs_review_are_never_conflated() {
        let parked = tagged(&["needs-input"]);
        let finished = tagged(&["needs-review"]);
        assert!(has_triage_label(&parked, TriageLabel::NeedsInput));
        assert!(!has_triage_label(&parked, TriageLabel::NeedsReview));
        assert!(has_triage_label(&finished, TriageLabel::NeedsReview));
        assert!(!has_triage_label(&finished, TriageLabel::NeedsInput));
    }

    #[test]
    fn a_single_label_resolves_and_two_conflict() {
        let one = tagged(&["ready-for-agent", "rust"]);
        assert_eq!(triage_label(&one), Ok(Some(TriageLabel::ReadyForAgent)));

        let none = tagged(&["rust"]);
        assert_eq!(triage_label(&none), Ok(None));

        let two = tagged(&["needs-input", "needs-review"]);
        let Err(conflict) = triage_label(&two) else {
            panic!("two labels must be reported as a conflict, not resolved");
        };
        assert_eq!(
            conflict,
            vec![TriageLabel::NeedsInput, TriageLabel::NeedsReview]
        );
    }

    #[test]
    fn a_repeated_label_is_not_a_conflict() {
        let t = tagged(&["needs-input", "#Needs-Input"]);
        assert_eq!(triage_label(&t), Ok(Some(TriageLabel::NeedsInput)));
    }

    #[test]
    fn only_ready_for_agent_does_not_block_a_human() {
        assert!(!TriageLabel::ReadyForAgent.blocks_human());
        for label in [
            TriageLabel::ReadyForHuman,
            TriageLabel::NeedsInput,
            TriageLabel::NeedsReview,
        ] {
            assert!(label.blocks_human(), "{label:?} should block a human");
        }
    }

    #[test]
    fn a_resolved_verify_command_opens_the_agent_ready_gate() {
        assert_eq!(check_agent_ready(Some("cargo check -p task")), Ok(()));
    }

    #[test]
    fn no_verify_command_closes_the_gate_and_names_the_reason() {
        for missing in [None, Some(""), Some("   ")] {
            assert_eq!(
                check_agent_ready(missing),
                Err(NotAgentReady::NoVerifyCommand),
                "{missing:?} should not be agent-ready"
            );
        }
        assert!(
            NotAgentReady::NoVerifyCommand
                .reason()
                .contains("verify command"),
            "the refusal must name the reason"
        );
    }

    #[test]
    fn filing_and_triage_are_independent_questions() {
        // A task can be filed to a project and still untriaged, and a
        // bare unfiled capture can be labelled. The two predicates
        // must not imply each other.
        let mut filed_untriaged = TaskInfo::new("x");
        filed_untriaged.project_id = Some(uuid::Uuid::new_v4());
        assert!(!crate::filing::is_unfiled(&filed_untriaged));
        assert!(is_untriaged(&filed_untriaged));

        let unfiled_triaged = tagged(&["ready-for-agent"]);
        assert!(crate::filing::is_unfiled(&unfiled_triaged));
        assert!(!is_untriaged(&unfiled_triaged));
    }
}

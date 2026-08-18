//! Resolving a ticket's **verify command** — the shell command whose
//! exit code is the verdict on whether an agent's work is done.
//!
//! Prose acceptance criteria are for humans. An agent running AFK
//! needs a machine verdict, so every ticket an agent may claim
//! resolves to exactly one command, and exit zero is the only thing
//! that counts as done.
//!
//! Resolution order, first hit wins:
//!
//! 1. the ticket's own override,
//! 2. the owning project's `verifyCommand`,
//! 3. the nearest ancestor project that declares one.
//!
//! A ticket that resolves to nothing cannot be marked ready for an
//! agent — see `task_proto::agent_lane::check_agent_ready`.

use uuid::Uuid;

use crate::model::ProjectInfo;

/// How deep the parent chain is walked before giving up.
///
/// Project trees are documented as one level of nesting, with deeper
/// trees allowed. The cap exists so a cycle in `parent_id` — which
/// nothing in the schema prevents — degrades to "no default" instead
/// of hanging the resolver.
const MAX_DEPTH: usize = 16;

/// Resolve the verify command for one ticket.
///
/// `task_override` is the ticket's own setting, `project_id` the
/// project it belongs to, and `projects` any collection containing
/// that project and its ancestors (extra entries are ignored).
///
/// Returns `None` when nothing in the chain declares one.
#[must_use]
pub fn resolve(
    task_override: Option<&str>,
    project_id: Option<Uuid>,
    projects: &[ProjectInfo],
) -> Option<String> {
    if let Some(cmd) = task_override.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(cmd.to_string());
    }
    project_default(project_id, projects)
}

/// The default a project inherits, walking up the parent chain.
///
/// Split out from [`resolve`] so a project page can show
/// `"inherited from <parent>"` without inventing a ticket to ask about.
#[must_use]
pub fn project_default(project_id: Option<Uuid>, projects: &[ProjectInfo]) -> Option<String> {
    let mut current = project_id?;
    for _ in 0..MAX_DEPTH {
        let project = projects.iter().find(|p| p.id == current)?;
        let cmd = project.verify_command.trim();
        if !cmd.is_empty() {
            return Some(cmd.to_string());
        }
        current = project.parent_id?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(verify: &str, parent: Option<Uuid>) -> ProjectInfo {
        ProjectInfo {
            id: Uuid::new_v4(),
            title: "p".into(),
            verify_command: verify.into(),
            parent_id: parent,
            ..ProjectInfo::default()
        }
    }

    #[test]
    fn a_project_default_resolves_for_its_own_tickets() {
        let p = project("cargo check -p task", None);
        let id = p.id;
        assert_eq!(
            resolve(None, Some(id), &[p]),
            Some("cargo check -p task".into())
        );
    }

    #[test]
    fn a_subproject_with_no_override_inherits_its_parent() {
        let parent = project("cargo check --workspace", None);
        let child = project("", Some(parent.id));
        let child_id = child.id;
        assert_eq!(
            resolve(None, Some(child_id), &[parent, child]),
            Some("cargo check --workspace".into())
        );
    }

    #[test]
    fn a_subproject_with_its_own_default_wins_over_its_parent() {
        let parent = project("cargo check --workspace", None);
        let child = project("cargo test -p task-proto", Some(parent.id));
        let child_id = child.id;
        assert_eq!(
            resolve(None, Some(child_id), &[parent, child]),
            Some("cargo test -p task-proto".into())
        );
    }

    #[test]
    fn inheritance_skips_ancestors_that_declare_nothing() {
        let grandparent = project("just ci", None);
        let parent = project("", Some(grandparent.id));
        let child = project("", Some(parent.id));
        let child_id = child.id;
        assert_eq!(
            resolve(None, Some(child_id), &[grandparent, parent, child]),
            Some("just ci".into())
        );
    }

    #[test]
    fn a_ticket_override_beats_every_project_default() {
        let parent = project("cargo check --workspace", None);
        let child = project("cargo test -p task-proto", Some(parent.id));
        let child_id = child.id;
        assert_eq!(
            resolve(
                Some("cargo test -p task-proto verify"),
                Some(child_id),
                &[parent, child]
            ),
            Some("cargo test -p task-proto verify".into())
        );
    }

    #[test]
    fn nothing_anywhere_resolves_to_none() {
        let p = project("", None);
        let id = p.id;
        assert_eq!(resolve(None, Some(id), &[p]), None);
        assert_eq!(resolve(None, None, &[]), None);
    }

    #[test]
    fn a_blank_override_is_not_an_override() {
        // An empty --verify flag must fall through to the project
        // rather than silently leaving the ticket unverifiable.
        let p = project("cargo check", None);
        let id = p.id;
        assert_eq!(
            resolve(Some("   "), Some(id), &[p]),
            Some("cargo check".into())
        );
    }

    #[test]
    fn an_unknown_project_resolves_to_none_rather_than_panicking() {
        assert_eq!(resolve(None, Some(Uuid::new_v4()), &[]), None);
    }

    #[test]
    fn a_parent_cycle_terminates() {
        // Nothing in the schema forbids this; the resolver must
        // degrade to "no default" rather than spin.
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();
        let mut a = project("", Some(b_id));
        a.id = a_id;
        let mut b = project("", Some(a_id));
        b.id = b_id;
        assert_eq!(resolve(None, Some(a_id), &[a, b]), None);
    }
}

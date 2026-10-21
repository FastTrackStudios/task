//! Who created a record: the creator ref inbox items and tasks carry
//! (`created_by`), and the rules every surface reads it by.
//!
//! A ref is `"<kind>:<account uuid>"`: [`USER`] for a person,
//! [`BOT`] for a bot account (an assistant like Atlas or Commander with
//! a Task identity of its own). The server stamps it from the
//! authenticated principal behind a write and never from what a client
//! sends; the record keeps it for good after that. This module only
//! reads refs — it can't tell anyone who they are.

use crate::agent::AgentRef;

/// Kind prefix of a person's ref.
pub const USER: &str = "user";
/// Kind prefix of a bot account's ref.
pub const BOT: &str = "bot";

/// A person's ref: `user:<id>`.
#[must_use]
pub fn user_ref(id: &str) -> String {
    format!("{USER}:{id}")
}

/// A bot account's ref: `bot:<id>`.
#[must_use]
pub fn bot_ref(id: &str) -> String {
    format!("{BOT}:{id}")
}

/// The account uuid inside a ref: `"bot:<uuid>"` → `"<uuid>"`. A ref
/// with no kind prefix is its own id.
#[must_use]
pub fn creator_id(created_by: &str) -> &str {
    created_by.split_once(':').map_or(created_by, |(_, id)| id)
}

/// Whether a ref names a bot.
#[must_use]
pub fn is_bot(created_by: &str) -> bool {
    created_by
        .split_once(':')
        .is_some_and(|(kind, _)| kind == BOT)
}

/// The one creator-filter rule every list surface shares: `who`
/// matches a stored ref exactly, or by its bare account uuid, or — as
/// `"none"` / `"unknown"` — the rows with no recorded creator. Case
/// does not matter; an empty `who` matches everything.
// t[impl attribution.filter]
#[must_use]
pub fn creator_matches(created_by: Option<&str>, who: &str) -> bool {
    let who = who.trim();
    if who.is_empty() {
        return true;
    }
    match created_by {
        None => who.eq_ignore_ascii_case("none") || who.eq_ignore_ascii_case("unknown"),
        Some(r) => r.eq_ignore_ascii_case(who) || creator_id(r).eq_ignore_ascii_case(who),
    }
}

/// What a surface says about who made a record: `"from you"` when
/// the signed-in account (`me`, its uuid) made it, `"from <name>"`
/// otherwise. `None` — say nothing — for a record with no creator, and
/// for someone else's whose name was not recorded.
#[must_use]
pub fn creator_label(
    created_by: Option<&str>,
    name: Option<&str>,
    me: Option<&str>,
) -> Option<String> {
    let r = created_by?;
    if me.is_some_and(|me| creator_id(r).eq_ignore_ascii_case(me)) {
        return Some("from you".to_owned());
    }
    let name = name.map(str::trim).filter(|n| !n.is_empty())?;
    Some(format!("from {name}"))
}

/// [`creator_label`] with whom a personal bot acted for:
/// `"from Atlas (for Cody)"`, or `"from Atlas (for you)"` when that is
/// the signed-in account. With no `on_behalf_of`, exactly
/// [`creator_label`]; with one whose name is unknown (and not `me`),
/// the creator alone.
// t[impl attribution.on-behalf-of.shown]
#[must_use]
pub fn attribution_label(
    created_by: Option<&str>,
    name: Option<&str>,
    on_behalf_of: Option<&str>,
    on_behalf_of_name: Option<&str>,
    me: Option<&str>,
) -> Option<String> {
    let from = creator_label(created_by, name, me)?;
    let Some(obo) = on_behalf_of else {
        return Some(from);
    };
    let whom = if me.is_some_and(|me| creator_id(obo).eq_ignore_ascii_case(me)) {
        "you".to_owned()
    } else {
        match on_behalf_of_name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.to_owned(),
            None => return Some(from),
        }
    };
    Some(format!("{from} (for {whom})"))
}

/// The assignee-filter rule (`TaskListFilter::assignee`): `who` is a
/// person's ref or bare account uuid (`"user:<uuid>"`, `"<uuid>"`)
/// matching a [`AgentRef::Human`], an `"agent:<name>"` matching an
/// [`AgentRef::Agent`], or `"none"` for no assignee at all. Case does
/// not matter; an empty `who` matches everything.
// t[impl attribution.assignee.filter]
#[must_use]
pub fn assignee_matches(assignees: &[AgentRef], who: &str) -> bool {
    let who = who.trim();
    if who.is_empty() {
        return true;
    }
    if who.eq_ignore_ascii_case("none") || who.eq_ignore_ascii_case("unassigned") {
        return assignees.is_empty();
    }
    let (kind, id) = who.split_once(':').unwrap_or(("", who));
    assignees.iter().any(|a| match a {
        AgentRef::Human { user_id, .. } => {
            (kind.is_empty() || kind == USER || kind == BOT || kind == "human")
                && user_id.eq_ignore_ascii_case(id)
        }
        AgentRef::Agent { name, .. } => kind == "agent" && name.eq_ignore_ascii_case(id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_carry_kind_and_id() {
        assert_eq!(user_ref("u1"), "user:u1");
        assert_eq!(bot_ref("b1"), "bot:b1");
        assert_eq!(creator_id("bot:b1"), "b1");
        assert_eq!(creator_id("b1"), "b1");
        assert!(is_bot("bot:b1"));
        assert!(!is_bot("user:u1"));
    }

    #[test]
    fn the_filter_rule() {
        assert!(creator_matches(None, "none"));
        assert!(creator_matches(None, "unknown"));
        assert!(!creator_matches(None, "bot:b1"));
        assert!(creator_matches(Some("bot:b1"), "bot:b1"));
        assert!(
            creator_matches(Some("bot:b1"), "b1"),
            "a bare uuid matches its ref"
        );
        assert!(
            !creator_matches(Some("bot:b1"), "user:b1"),
            "the kind is part of a full ref"
        );
        assert!(!creator_matches(Some("bot:b1"), "none"));
        assert!(
            creator_matches(Some("bot:b1"), ""),
            "no filter keeps everything"
        );
        assert!(creator_matches(None, " "), "no filter keeps everything");
    }

    // t[verify attribution.shown]
    #[test]
    fn the_label_says_you_or_the_name() {
        assert_eq!(
            creator_label(Some("user:u1"), Some("Cody"), Some("u1")).as_deref(),
            Some("from you")
        );
        assert_eq!(
            creator_label(Some("bot:b1"), Some("Atlas"), Some("u1")).as_deref(),
            Some("from Atlas")
        );
        assert_eq!(creator_label(Some("bot:b1"), None, Some("u1")), None);
        assert_eq!(
            creator_label(None, Some("Atlas"), Some("u1")),
            None,
            "old records say nothing"
        );
    }

    // t[verify attribution.on-behalf-of.shown]
    #[test]
    fn a_personal_bot_says_whom_it_acted_for() {
        let label = |obo: Option<&str>, obo_name: Option<&str>, me: &str| {
            attribution_label(Some("bot:b1"), Some("Atlas"), obo, obo_name, Some(me))
        };
        assert_eq!(
            label(Some("user:c1"), Some("Cody"), "u2").as_deref(),
            Some("from Atlas (for Cody)")
        );
        assert_eq!(
            label(Some("user:c1"), Some("Cody"), "c1").as_deref(),
            Some("from Atlas (for you)")
        );
        assert_eq!(
            label(None, None, "u2").as_deref(),
            Some("from Atlas"),
            "a team bot acted for nobody"
        );
        assert_eq!(
            label(Some("user:c1"), None, "u2").as_deref(),
            Some("from Atlas"),
            "an unnamed person is not invented"
        );
        assert_eq!(
            attribution_label(Some("user:c1"), Some("Cody"), None, None, Some("c1")).as_deref(),
            Some("from you")
        );
    }

    // t[verify attribution.assignee.filter]
    #[test]
    fn the_assignee_rule() {
        let jo = AgentRef::Human {
            user_id: "j1".into(),
            display_name: Some("Jordan".into()),
        };
        let bot = AgentRef::agent("hermes");
        let both = vec![jo.clone(), bot];
        assert!(assignee_matches(&both, "user:j1"));
        assert!(assignee_matches(&both, "J1"), "a bare uuid, any case");
        assert!(assignee_matches(&both, "agent:hermes"));
        assert!(
            !assignee_matches(&both, "hermes"),
            "an agent is named as one"
        );
        assert!(!assignee_matches(&both, "none"));
        assert!(assignee_matches(&[], "none"));
        assert!(!assignee_matches(&[], "user:j1"));
        assert!(assignee_matches(&[jo], ""), "no filter keeps everything");
    }
}

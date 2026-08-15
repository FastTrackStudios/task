//! Triage — heuristic derivation engine (urgency 0–3 + the fixed
//! tag taxonomy) and the traits the server/agent plug into.
//!
//! HEURISTICS ONLY in v1: headers (`List-Unsubscribe` /
//! `List-Id` / `Precedence`), subject patterns, sender-in-
//! contacts, and the self-mail guard. The LLM hook
//! ([`DerivationEngine::derive_llm`]) is deliberately
//! unimplemented — the agent plugin supplies it later, and the
//! cache schema (kind + version) already accommodates its kinds.

use std::collections::BTreeSet;

use email_proto::{DerivationKind, EmailSyncError, Envelope};

/// Everything the engine may look at for one message.
pub struct DerivationInput<'a> {
    /// The account's own address (self-mail guard).
    pub account_address: &'a str,
    pub envelope: &'a Envelope,
    /// Raw header block (from `Message::headers_raw`).
    pub headers_raw: &'a str,
    pub body_text: Option<&'a str>,
    /// Sender resolves to a known contact (via [`ContactLookup`]).
    pub sender_known: bool,
}

/// Computes derivations for one message. Implementations must be
/// cheap enough for the bounded background pass.
pub trait DerivationEngine: Send + Sync {
    /// Heuristic pass — always available, no model calls.
    fn derive(&self, input: &DerivationInput<'_>) -> Vec<(DerivationKind, String)>;

    /// LLM pass — unimplemented in v1. The agent plugin supplies
    /// an engine that overrides this (summaries, draft replies,
    /// richer tags); the triage pass will call it only once an
    /// implementation exists.
    fn derive_llm(
        &self,
        _input: &DerivationInput<'_>,
    ) -> Result<Vec<(DerivationKind, String)>, EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "no LLM derivation engine wired (the agent plugin supplies one)".into(),
        ))
    }
}

/// Who counts as a known sender. The server implements this over
/// the org's contacts backend; [`NoContacts`] is the null default.
pub trait ContactLookup: Send + Sync {
    /// Lower-cased addresses of known contacts. Called once per
    /// triage pass (not per message) — implementations may hit
    /// their backing store each call.
    fn known_addresses(&self) -> BTreeSet<String>;
}

/// No contacts wired — every sender is unknown.
pub struct NoContacts;

impl ContactLookup for NoContacts {
    fn known_addresses(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
}

/// The v1 heuristic engine.
pub struct HeuristicEngine;

impl DerivationEngine for HeuristicEngine {
    fn derive(&self, input: &DerivationInput<'_>) -> Vec<(DerivationKind, String)> {
        let tags = derive_tags(input);
        let urgency = derive_urgency(input, &tags);
        vec![
            (DerivationKind::Tags, tags.join(",")),
            (DerivationKind::Urgency, urgency.to_string()),
        ]
    }
}

/// Case-insensitive "does this header exist" over the raw header
/// block. Good enough for presence checks — we never need the
/// values.
fn has_header(headers_raw: &str, name: &str) -> bool {
    let needle = format!("{}:", name.to_ascii_lowercase());
    headers_raw
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with(&needle))
}

fn header_value<'a>(headers_raw: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{}:", name.to_ascii_lowercase());
    headers_raw.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .starts_with(&needle)
            .then(|| l[needle.len()..].trim())
    })
}

fn sender_email(env: &Envelope) -> String {
    env.from
        .first()
        .map(|a| a.email.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_self_mail(input: &DerivationInput<'_>) -> bool {
    !input.account_address.is_empty()
        && sender_email(input.envelope) == input.account_address.to_ascii_lowercase()
}

const SOCIAL_DOMAINS: [&str; 12] = [
    "facebook.com",
    "facebookmail.com",
    "twitter.com",
    "x.com",
    "instagram.com",
    "linkedin.com",
    "tiktok.com",
    "youtube.com",
    "discord.com",
    "reddit.com",
    "redditmail.com",
    "pinterest.com",
];

fn subject_matches(subject: &str, needles: &[&str]) -> bool {
    let s = subject.to_ascii_lowercase();
    needles.iter().any(|n| s.contains(n))
}

/// The fixed taxonomy: action-needed, waiting, newsletter,
/// receipt, calendar, social, other.
fn derive_tags(input: &DerivationInput<'_>) -> Vec<String> {
    let env = input.envelope;
    let mut tags: Vec<&str> = Vec::new();

    // Self-mail: something you sent (to yourself / seen in the
    // pass) — you're waiting on the world, not being asked.
    if is_self_mail(input) {
        return vec!["waiting".to_string()];
    }

    // Bulk / list mail.
    let precedence_bulk = header_value(input.headers_raw, "Precedence")
        .is_some_and(|v| v.eq_ignore_ascii_case("bulk") || v.eq_ignore_ascii_case("list"));
    if has_header(input.headers_raw, "List-Unsubscribe")
        || has_header(input.headers_raw, "List-Id")
        || precedence_bulk
    {
        tags.push("newsletter");
    }

    // Receipts / billing.
    if subject_matches(
        &env.subject,
        &[
            "receipt",
            "invoice",
            "order confirmation",
            "payment",
            "your order",
            "billing statement",
        ],
    ) {
        tags.push("receipt");
    }

    // Calendar: a text/calendar part or invite-shaped subject.
    let calendarish = input
        .headers_raw
        .to_ascii_lowercase()
        .contains("text/calendar")
        || subject_matches(&env.subject, &["invitation", "invite:", "meeting request"]);
    if calendarish {
        tags.push("calendar");
    }

    // Social notifications by sender domain.
    let sender = sender_email(env);
    if SOCIAL_DOMAINS
        .iter()
        .any(|d| sender.ends_with(&format!("@{d}")) || sender.ends_with(&format!(".{d}")))
    {
        tags.push("social");
    }

    // Action needed.
    if subject_matches(
        &env.subject,
        &[
            "action required",
            "action needed",
            "approval",
            "please review",
            "please respond",
            "response required",
            "rsvp",
            "reminder",
            "urgent",
            "asap",
            "deadline",
            "confirm your",
            "verify your",
        ],
    ) {
        tags.push("action-needed");
    }

    if tags.is_empty() {
        tags.push("other");
    }
    tags.into_iter().map(str::to_string).collect()
}

/// 0–3. Bulk-ish mail is capped low; personal mail from known
/// contacts with urgent-shaped subjects climbs.
fn derive_urgency(input: &DerivationInput<'_>, tags: &[String]) -> u8 {
    if is_self_mail(input) {
        return 0;
    }
    let bulk = tags
        .iter()
        .any(|t| matches!(t.as_str(), "newsletter" | "social" | "receipt"));
    let action = tags.iter().any(|t| t == "action-needed");
    if bulk {
        // Bulk mail never alerts; an explicit action ask nudges
        // it to 1 so it still sorts above pure noise.
        return u8::from(action);
    }

    let env = input.envelope;
    let addressed_directly = !input.account_address.is_empty()
        && env
            .to
            .iter()
            .any(|a| a.email.eq_ignore_ascii_case(input.account_address));

    let mut score = 0u8;
    if addressed_directly {
        score += 1;
    }
    if input.sender_known {
        score += 1;
    }
    if action
        || subject_matches(&env.subject, &["urgent", "asap", "immediately", "today", "eod"])
    {
        score += 1;
    }
    score.min(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use email_proto::Addr;

    fn env(from: &str, to: &str, subject: &str) -> Envelope {
        Envelope {
            message_id: "<t@x>".into(),
            folder: "INBOX".into(),
            thread_id: None,
            subject: subject.into(),
            from: vec![Addr {
                name: None,
                email: from.into(),
            }],
            to: vec![Addr {
                name: None,
                email: to.into(),
            }],
            cc: vec![],
            date_ms: 0,
            flags: vec![],
            has_attachments: false,
            size: 0,
            snippet: None,
        }
    }

    fn derive(
        env: &Envelope,
        headers: &str,
        known: bool,
    ) -> (u8, Vec<String>) {
        let input = DerivationInput {
            account_address: "me@example.com",
            envelope: env,
            headers_raw: headers,
            body_text: None,
            sender_known: known,
        };
        let rows = HeuristicEngine.derive(&input);
        let mut urgency = 0;
        let mut tags = Vec::new();
        for (kind, payload) in rows {
            match kind {
                DerivationKind::Urgency => urgency = payload.parse().unwrap(),
                DerivationKind::Tags => {
                    tags = payload.split(',').map(str::to_string).collect();
                }
            }
        }
        (urgency, tags)
    }

    #[test]
    fn newsletter_is_tagged_and_calm() {
        let e = env("news@list.example.com", "me@example.com", "Weekly digest");
        let headers = "List-Unsubscribe: <mailto:u@list.example.com>\r\nSubject: Weekly digest\r\n";
        let (urgency, tags) = derive(&e, headers, false);
        assert!(tags.contains(&"newsletter".to_string()), "{tags:?}");
        assert_eq!(urgency, 0);
    }

    #[test]
    fn known_sender_direct_urgent_scores_three() {
        let e = env("alice@example.com", "me@example.com", "URGENT: server down");
        let (urgency, tags) = derive(&e, "", true);
        assert_eq!(urgency, 3);
        assert!(tags.contains(&"action-needed".to_string()), "{tags:?}");
    }

    #[test]
    fn self_mail_guard_yields_waiting_zero() {
        let e = env("me@example.com", "bob@example.com", "URGENT please reply");
        let (urgency, tags) = derive(&e, "", false);
        assert_eq!(urgency, 0);
        assert_eq!(tags, vec!["waiting".to_string()]);
    }

    #[test]
    fn receipt_and_calendar_and_social_tags() {
        let e = env("shop@store.com", "me@example.com", "Your order confirmation");
        let (u, tags) = derive(&e, "", false);
        assert!(tags.contains(&"receipt".to_string()));
        assert_eq!(u, 0);

        let e = env("cal@corp.com", "me@example.com", "Invitation: standup");
        let (_, tags) = derive(&e, "Content-Type: text/calendar\r\n", false);
        assert!(tags.contains(&"calendar".to_string()));

        let e = env("notify@facebookmail.com", "me@example.com", "New friend request");
        let (u, tags) = derive(&e, "", false);
        assert!(tags.contains(&"social".to_string()));
        assert_eq!(u, 0);
    }

    #[test]
    fn plain_mail_falls_back_to_other() {
        let e = env("bob@example.com", "me@example.com", "lunch?");
        let (u, tags) = derive(&e, "", false);
        assert_eq!(tags, vec!["other".to_string()]);
        assert_eq!(u, 1); // addressed directly, unknown sender
    }

    #[test]
    fn llm_hook_is_unimplemented() {
        let e = env("a@b.c", "me@example.com", "x");
        let input = DerivationInput {
            account_address: "me@example.com",
            envelope: &e,
            headers_raw: "",
            body_text: None,
            sender_known: false,
        };
        assert!(matches!(
            HeuristicEngine.derive_llm(&input),
            Err(EmailSyncError::Unsupported(_))
        ));
    }
}

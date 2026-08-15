//! Session history feed — the `workflows-proto` audit trail
//! (`Transition` state changes + `Activity` events) rendered as
//! one chronological list. Records carry UUIDv7 ids, so byte
//! order == creation order; we sort on the id (timestamp as the
//! tiebreak for any legacy v4 rows).

use architect_ui::lucide_dioxus::{
    ArrowRight, ExternalLink, GitCommitHorizontal, Handshake, MessageSquare, StickyNote, Wrench,
};
use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use uuid::Uuid;
use workflows_proto::{Activity, ActivityKind, Transition};

use crate::views::detail_full::SectionLabel;

/// One feed row — either kind of audit record.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    Transition(Transition),
    Activity(Activity),
}

impl SessionEvent {
    #[must_use]
    pub fn id(&self) -> Uuid {
        match self {
            Self::Transition(t) => t.id,
            Self::Activity(a) => a.id,
        }
    }

    #[must_use]
    pub fn at(&self) -> DateTime<Utc> {
        match self {
            Self::Transition(t) => t.at,
            Self::Activity(a) => a.at,
        }
    }
}

/// Merge both record streams into one chronological feed.
/// Primary key: the UUIDv7 id bytes (creation order); fallback:
/// the `at` timestamp.
#[must_use]
pub fn merge_session_events(
    transitions: Vec<Transition>,
    activities: Vec<Activity>,
) -> Vec<SessionEvent> {
    let mut events: Vec<SessionEvent> = transitions
        .into_iter()
        .map(SessionEvent::Transition)
        .chain(activities.into_iter().map(SessionEvent::Activity))
        .collect();
    events.sort_by(|a, b| {
        a.id()
            .as_bytes()
            .cmp(b.id().as_bytes())
            .then_with(|| a.at().cmp(&b.at()))
    });
    events
}

/// Short label for an activity kind.
#[must_use]
pub fn activity_label(kind: &ActivityKind) -> String {
    match kind {
        ActivityKind::Commit => "Commit".to_string(),
        ActivityKind::Comment => "Comment".to_string(),
        ActivityKind::External => "External".to_string(),
        ActivityKind::ToolCall => "Tool call".to_string(),
        ActivityKind::Handoff => "Handoff".to_string(),
        ActivityKind::Note => "Note".to_string(),
        ActivityKind::Custom { tag } => tag.clone(),
    }
}

/// Best-effort one-liner from an activity's opaque JSON payload.
/// Strings render as themselves; objects surface a `message` /
/// `text` / `sha` field when present; everything else is hidden.
#[must_use]
pub fn payload_preview(payload: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let s = match &v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            let msg = map
                .get("message")
                .or_else(|| map.get("text"))
                .or_else(|| map.get("note"))
                .and_then(|m| m.as_str());
            let sha = map.get("sha").and_then(|s| s.as_str());
            match (sha, msg) {
                (Some(sha), Some(m)) => format!("{} {m}", &sha[..sha.len().min(7)]),
                (Some(sha), None) => sha[..sha.len().min(7)].to_string(),
                (None, Some(m)) => m.to_string(),
                (None, None) => return None,
            }
        }
        _ => return None,
    };
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Keep the feed line-height stable.
    let mut out: String = s.chars().take(120).collect();
    if s.chars().count() > 120 {
        out.push('…');
    }
    Some(out)
}

#[derive(Props, Clone, PartialEq)]
pub struct SessionHistoryProps {
    pub transitions: Vec<Transition>,
    pub activities: Vec<Activity>,
}

#[component]
pub fn SessionHistory(props: SessionHistoryProps) -> Element {
    let events = merge_session_events(props.transitions.clone(), props.activities.clone());
    rsx! {
        section { class: "flex flex-col gap-2",
            SectionLabel { label: "Session history" }
            ol { class: "flex flex-col gap-0 border-l border-border pl-3 ml-1",
                for ev in events.into_iter() {
                    li { key: "{ev.id()}", class: "relative py-1.5",
                        span { class: "absolute -left-[17px] top-[13px] h-1.5 w-1.5 rounded-full bg-border" }
                        match ev {
                            SessionEvent::Transition(t) => rsx! {
                                TransitionRow { record: t }
                            },
                            SessionEvent::Activity(a) => rsx! {
                                ActivityRow { record: a }
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TransitionRow(record: Transition) -> Element {
    rsx! {
        div { class: "flex flex-col gap-0.5",
            div { class: "flex flex-wrap items-center gap-1.5 text-xs",
                span { class: "rounded bg-muted/50 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground",
                    "{record.from_state}"
                }
                ArrowRight { size: 10 }
                span { class: "rounded bg-primary/15 px-1.5 py-0.5 font-mono text-[10px] text-foreground",
                    "{record.to_state}"
                }
                EventMeta { actor: record.actor.short_label(), at: record.at }
            }
            if let Some(note) = record.note.as_deref() {
                span { class: "text-xs text-muted-foreground italic", "{note}" }
            }
        }
    }
}

#[component]
fn ActivityRow(record: Activity) -> Element {
    let icon = match &record.kind {
        ActivityKind::Commit => rsx! {
            GitCommitHorizontal { size: 12 }
        },
        ActivityKind::Comment => rsx! {
            MessageSquare { size: 12 }
        },
        ActivityKind::External => rsx! {
            ExternalLink { size: 12 }
        },
        ActivityKind::ToolCall => rsx! {
            Wrench { size: 12 }
        },
        ActivityKind::Handoff => rsx! {
            Handshake { size: 12 }
        },
        ActivityKind::Note | ActivityKind::Custom { .. } => rsx! {
            StickyNote { size: 12 }
        },
    };
    rsx! {
        div { class: "flex flex-col gap-0.5",
            div { class: "flex flex-wrap items-center gap-1.5 text-xs text-foreground",
                span { class: "text-muted-foreground", {icon} }
                span { class: "font-medium", "{activity_label(&record.kind)}" }
                EventMeta { actor: record.actor.short_label(), at: record.at }
            }
            if let Some(preview) = payload_preview(&record.payload) {
                span { class: "truncate font-mono text-[11px] text-muted-foreground",
                    "{preview}"
                }
            }
        }
    }
}

/// Trailing "who · when" cluster shared by both row kinds.
#[component]
fn EventMeta(actor: String, at: DateTime<Utc>) -> Element {
    let stamp = at.format("%b %-d, %H:%M");
    rsx! {
        span { class: "ml-auto flex items-center gap-1.5 text-[10px] text-muted-foreground",
            span { "{actor}" }
            span { "·" }
            span { "{stamp}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workflows_proto::AgentRef;

    #[test]
    fn merge_orders_by_uuidv7() {
        let session = Uuid::new_v4();
        let actor = AgentRef::agent("claude");
        // Created in sequence — now_v7 ids are monotonic.
        let t1 = Transition::record(session, "open", "claimed", actor.clone());
        let a1 = Activity::record(session, ActivityKind::Commit, actor.clone(), &"x");
        let t2 = Transition::record(session, "claimed", "branched", actor);

        // Feed them in shuffled, per-type batches.
        let merged = merge_session_events(vec![t2.clone(), t1.clone()], vec![a1.clone()]);
        let ids: Vec<Uuid> = merged.iter().map(SessionEvent::id).collect();
        assert_eq!(ids, vec![t1.id, a1.id, t2.id]);
    }

    #[test]
    fn payload_previews() {
        assert_eq!(payload_preview("null"), None);
        assert_eq!(payload_preview("{}"), None);
        assert_eq!(payload_preview("not json"), None);
        assert_eq!(payload_preview("\"plain note\""), Some("plain note".into()));
        assert_eq!(
            payload_preview(r#"{"sha":"deadbeefcafe","message":"fix the thing"}"#),
            Some("deadbee fix the thing".into())
        );
        assert_eq!(payload_preview(r#"{"text":"hello"}"#), Some("hello".into()));
    }

    #[test]
    fn activity_labels() {
        assert_eq!(activity_label(&ActivityKind::ToolCall), "Tool call");
        assert_eq!(
            activity_label(&ActivityKind::Custom { tag: "ci".into() }),
            "ci"
        );
    }
}

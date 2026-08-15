//! Shared loading / error / empty phase components for data pages.
//!
//! Before this, each page hand-rolled its own loading/error/empty
//! markup — and several rendered *nothing* on a failed fetch (the
//! ledger swallowed errors and showed a blank book; tasks had a
//! bespoke one-off banner), with no way to retry short of a full
//! reload. These give one consistent look across pages plus an
//! optional retry affordance. (codeberg #27, item 1.)

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{Inbox, RotateCw, TriangleAlert};
use architect_ui::prelude::*;

/// Classify a raw transport/store error string into human copy.
/// Returns `(title, message, technical_detail)` — `technical_detail`
/// is `Some(raw)` when the message was rewritten (the raw string still
/// matters for debugging, but belongs behind a disclosure, not in the
/// user's face).
///
/// The raw strings this matches come from `vox_clients` /
/// `use_connect_supervised` — plain `format!`ed transport errors like
/// ``ws connect `wss://…`: Io(…)``.
#[must_use]
pub fn friendly_error(raw: &str) -> (&'static str, String, Option<String>) {
    let lower = raw.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    if has(&["no vox url"]) {
        return (
            "No server configured",
            "This build doesn't know where your workspace lives — set TASK_VOX_URL and reload."
                .to_string(),
            Some(raw.to_string()),
        );
    }
    if has(&["awaiting org discovery"]) {
        return (
            "Connecting…",
            "Still finding your workspace. This usually resolves in a moment.".to_string(),
            Some(raw.to_string()),
        );
    }
    if has(&[
        "ws connect",
        "websocket",
        "establish",
        "connect failed",
        "connection",
        "unreachable",
        "timed out",
        "dial",
        "closed",
    ]) {
        return (
            "Can't reach the server",
            "Your workspace isn't answering. We'll keep retrying automatically — check that the server is running and that you're online."
                .to_string(),
            Some(raw.to_string()),
        );
    }
    ("Something went wrong", raw.to_string(), None)
}

/// Skeleton placeholder shown while a page's primary data loads.
/// `rows` controls how many shimmer cards render.
#[component]
pub fn LoadingState(#[props(default = 4)] rows: usize) -> Element {
    rsx! {
        div { class: "flex flex-col gap-3",
            for i in 0..rows {
                div { key: "{i}", class: "flex flex-col gap-2 rounded-xl border border-border/70 bg-card p-4",
                    Skeleton { class: "h-5 w-40" }
                    Skeleton { class: "h-4 w-full" }
                }
            }
        }
    }
}

/// Error card that keeps the failure visible instead of leaving a
/// blank page, with an optional **Try again** button. Pass `on_retry`
/// when the caller can re-run the fetch (e.g. a Dioxus `Resource`'s
/// `restart()`); omit it for read models that have no cheap refetch.
#[component]
pub fn ErrorState(
    /// The error detail to show under the title. Raw transport strings
    /// are rewritten into human copy ([`friendly_error`]); the raw
    /// text moves behind a "Technical details" disclosure.
    message: String,
    /// Headline above the message. Defaults to a generic line (which a
    /// recognized transport error replaces with its own).
    #[props(default = "Something went wrong".to_string())]
    title: String,
    /// Optional retry handler — renders the **Try again** button.
    #[props(default)]
    on_retry: Option<EventHandler<()>>,
) -> Element {
    let (ftitle, fmessage, detail) = friendly_error(&message);
    // A caller-supplied headline wins; the classified one only fills
    // the generic default.
    let title = if title == "Something went wrong" {
        ftitle.to_string()
    } else {
        title
    };
    rsx! {
        div { class: "flex flex-col items-center gap-3 rounded-2xl border border-destructive/40 bg-destructive/10 px-6 py-10 text-center",
            div { class: "flex size-11 items-center justify-center rounded-xl bg-destructive/15 text-destructive",
                TriangleAlert { size: 22 }
            }
            div { class: "flex flex-col gap-1",
                Heading { level: HeadingLevel::H3, "{title}" }
                Text { variant: TextVariant::Muted, class: "max-w-md break-words", "{fmessage}" }
            }
            if let Some(retry) = on_retry {
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Small,
                    on_click: move |_| retry.call(()),
                    RotateCw { size: 14 }
                    "Try again"
                }
            }
            if let Some(raw) = detail {
                details { class: "max-w-md text-left",
                    summary { class: "cursor-pointer text-xs text-muted-foreground hover:text-foreground",
                        "Technical details"
                    }
                    pre { class: "mt-1 whitespace-pre-wrap break-all rounded-md bg-background/60 p-2 font-mono text-[11px] text-muted-foreground",
                        "{raw}"
                    }
                }
            }
        }
    }
}

/// Compact error row for panels and sidebars — the friendly headline
/// with the raw error on hover, and an optional retry icon-button.
/// Use where [`ErrorState`]'s full card would swallow a small panel.
#[component]
pub fn InlineError(
    /// The raw error string (rewritten to human copy; raw kept as the
    /// hover title).
    message: String,
    /// Optional context prefix, e.g. "Backlinks" → "Backlinks — can't
    /// reach the server".
    #[props(default)]
    label: Option<String>,
    /// Optional retry handler — renders a small refresh button.
    #[props(default)]
    on_retry: Option<EventHandler<()>>,
) -> Element {
    let (ftitle, fmessage, detail) = friendly_error(&message);
    let text = match &label {
        Some(l) => {
            let mut t = ftitle.to_string();
            if let Some(first) = t.get_mut(0..1) {
                first.make_ascii_lowercase();
            }
            format!("{l} — {t}")
        }
        None => ftitle.to_string(),
    };
    let hover = detail.unwrap_or(fmessage);
    rsx! {
        div {
            class: "flex items-center gap-2 rounded-lg border border-destructive/25 bg-destructive/5 px-2.5 py-1.5 text-xs text-destructive/90",
            title: "{hover}",
            TriangleAlert { size: 13 }
            span { class: "min-w-0 flex-1 truncate", "{text}" }
            if let Some(retry) = on_retry {
                button {
                    r#type: "button",
                    class: "shrink-0 rounded p-0.5 text-destructive/70 transition-colors hover:bg-destructive/10 hover:text-destructive",
                    title: "Try again",
                    onclick: move |_| retry.call(()),
                    RotateCw { size: 12 }
                }
            }
        }
    }
}

/// Empty-state placeholder for a page with no rows yet: an icon, a
/// title, and an optional hint line.
#[component]
pub fn EmptyState(
    /// Headline (e.g. "No transactions yet").
    title: String,
    /// Optional secondary line explaining how to populate the view.
    #[props(default)]
    hint: Option<String>,
) -> Element {
    rsx! {
        div { class: "flex flex-col items-center gap-3 rounded-2xl border border-dashed border-border/70 bg-card/40 px-6 py-16 text-center",
            div { class: "flex size-12 items-center justify-center rounded-2xl bg-muted text-muted-foreground",
                Inbox { size: 24 }
            }
            Heading { level: HeadingLevel::H3, "{title}" }
            if let Some(h) = hint {
                Text { variant: TextVariant::Muted, "{h}" }
            }
        }
    }
}

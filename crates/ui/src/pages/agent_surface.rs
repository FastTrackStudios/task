//! The agent surface — everything blocking a human, in one place.
//!
//! Three panels, in this order for a reason: **questions** first,
//! because that is the only one where something has *stopped* and is
//! waiting on you specifically; then **running**, which you watch;
//! then **awaiting review**, which you pick up when you choose to.
//!
//! One component, two mountings. The fleet view at `/runners` passes
//! no project; the project page passes its own. Sharing the component
//! is what keeps them from drifting into two different answers to the
//! same question.

use dioxus::prelude::*;
use uuid::Uuid;

use crate::feeds::AgentSurface;

/// Poll interval. The stream carries per-run output; this is the
/// coarse "has the shape of things changed" refresh, so it can be
/// slow without feeling stale.
const REFRESH_SECS: u64 = 15;

#[component]
pub fn AgentSurfaceView(slug: String, project: Option<Uuid>, heading: bool) -> Element {
    let mut tick = use_signal(|| 0_u32);
    use_future(move || async move {
        loop {
            crate::pages::agent_surface::sleep(REFRESH_SECS).await;
            tick += 1;
        }
    });

    let fetch_slug = slug.clone();
    let data = use_resource(move || {
        let _ = tick.read();
        let slug = fetch_slug.clone();
        async move { crate::feeds::fetch_agent_surface(&slug, project).await }
    });

    let snapshot = data.read_unchecked().as_ref().cloned();

    rsx! {
        section { class: "flex flex-col gap-4",
            if heading {
                h2 { class: "text-lg font-semibold text-foreground", "Agents" }
            }
            match snapshot {
                // A failed fetch must never render as three empty
                // panels — that reads as "nothing is blocking you",
                // which is the opposite of the truth.
                Some(Err(why)) => rsx! {
                    div { class: "rounded-md border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive",
                        "Couldn't read the agent surface: {why}"
                    }
                },
                Some(Ok(s)) => rsx! { SurfacePanels { surface: s } },
                None => rsx! {
                    div { class: "text-sm text-muted-foreground", "Loading…" }
                },
            }
        }
    }
}

#[component]
fn SurfacePanels(surface: AgentSurface) -> Element {
    rsx! {
        div { class: "grid gap-4 md:grid-cols-3",
            Panel {
                title: "Questions awaiting you".to_string(),
                count: surface.questions.len(),
                emphasise: !surface.questions.is_empty(),
                {
                    surface.questions.iter().map(|(req, ticket)| {
                        let title = ticket
                            .as_ref()
                            .map_or_else(|| "(no ticket)".to_string(), |t| t.title.clone());
                        let text = req
                            .questions
                            .first()
                            .map_or_else(String::new, |q| q.text.clone());
                        rsx! {
                            li { key: "{req.id}", class: "flex flex-col gap-0.5 py-1",
                                span { class: "text-sm font-medium text-foreground", "{title}" }
                                span { class: "text-xs text-muted-foreground", "{text}" }
                            }
                        }
                    })
                }
            }
            Panel {
                title: "Running now".to_string(),
                count: surface.running.len(),
                emphasise: false,
                {
                    surface.running.iter().map(|(run, ticket)| {
                        let title = ticket
                            .as_ref()
                            .map_or_else(|| "(unknown ticket)".to_string(), |t| t.title.clone());
                        rsx! {
                            li { key: "{run.id}", class: "flex flex-col gap-0.5 py-1",
                                span { class: "text-sm font-medium text-foreground", "{title}" }
                                span { class: "text-xs text-muted-foreground", "on {run.runner}" }
                            }
                        }
                    })
                }
            }
            Panel {
                title: "Awaiting review".to_string(),
                count: surface.review.len(),
                emphasise: false,
                {
                    surface.review.iter().map(|t| {
                        let branch = agent_branch(t.id);
                        rsx! {
                            li { key: "{t.id}", class: "flex flex-col gap-0.5 py-1",
                                span { class: "text-sm font-medium text-foreground", "{t.title}" }
                                span { class: "font-mono text-xs text-muted-foreground", "{branch}" }
                            }
                        }
                    })
                }
            }
        }
    }
}

#[component]
fn Panel(title: String, count: usize, emphasise: bool, children: Element) -> Element {
    let ring = if emphasise {
        "border-amber-500/40"
    } else {
        "border-border"
    };
    rsx! {
        div { class: "rounded-lg border {ring} bg-card p-3",
            div { class: "mb-2 flex items-baseline justify-between",
                span { class: "text-sm font-medium text-foreground", "{title}" }
                span { class: "text-xs tabular-nums text-muted-foreground", "{count}" }
            }
            if count == 0 {
                span { class: "text-xs text-muted-foreground", "Nothing" }
            } else {
                ul { class: "divide-y divide-border", {children} }
            }
        }
    }
}

/// The branch a ticket's work lands on.
///
/// Mirrors `agent_worktree::branch_for`, which this crate cannot
/// depend on — it is native-only and this compiles to wasm.
fn agent_branch(id: Uuid) -> String {
    let short = id.to_string();
    format!("agent/{}", &short[..8.min(short.len())])
}

/// Sleep, on whichever runtime this is.
async fn sleep(secs: u64) {
    architect::platform::sleep(std::time::Duration::from_secs(secs)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_name_matches_the_runners() {
        // `agent_worktree::branch_for` takes the same 8-char short id
        // the CLI prints, so a branch shown here is one you can
        // actually check out.
        let id = Uuid::parse_str("77a0266e-2416-4d4e-af02-01470113683d").unwrap();
        assert_eq!(agent_branch(id), "agent/77a0266e");
    }
}

//! `/tasks/:id` — the deep-linkable task detail page.
//!
//! Wires [`task_ui::TaskDetailFull`] (dumb component: data in via
//! props, intents out via callbacks) to the org-tagged task set. The
//! whole selected-org row set is fetched once and everything —
//! subtasks, blocker/relates titles, the task itself — resolves from
//! it client-side; the store-migration issue replaces this fetch with
//! the shared task store + live events.

use std::collections::HashMap;

use dioxus::prelude::*;
use task_proto::TaskInfo;
use task_ui::views::resolve_links;
use task_ui::{ClaimState, LinkedTaskRef, SubtaskRow, TaskDetailFull};
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection, selected_slugs};
use crate::routes::Route;

/// Signed-out claim identity: a human ref distinguishable from agent
/// claims in the board and the CLI. Only the fallback — signed-in
/// claims carry the real account via [`claimant_json`].
// Serde shape must match workflows_proto::AgentRef::Human exactly —
// the backend from_str's it. (The old {"kind":"human","id":"web"}
// shape silently failed to parse.)
const WEB_CLAIMANT: &str = r#"{"kind":"human","user_id":"web","display_name":"Guest (web)"}"#;

/// JSON-encoded `workflows_proto::AgentRef` for `try_claim` — the
/// server `serde_json::from_str`s it (see `TaskBackend::try_claim`).
/// Signed in ⇒ `Human { user_id, display_name }` serialized with
/// workflows_proto's own serde (internally tagged, `kind`); signed
/// out ⇒ the legacy [`WEB_CLAIMANT`] const.
fn claimant_json(account: Option<&crate::auth::ActiveAccount>) -> String {
    account.map_or_else(
        || WEB_CLAIMANT.to_owned(),
        |a| {
            serde_json::to_string(&task_proto::workflows_proto::AgentRef::Human {
                user_id: a.user_id.to_string(),
                display_name: Some(a.name.clone()),
            })
            .unwrap_or_else(|_| WEB_CLAIMANT.to_owned())
        },
    )
}

#[component]
pub fn TaskDetailPage(id: Uuid) -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    // Claim identity — the signed-in account (Guest by default).
    // Hoisted: hooks can't live inside the match arms below.
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    let nav = use_navigator();

    let rows = use_resource(move || {
        let slugs = selected_slugs(&selection.read(), &org_list.read());
        async move { crate::feeds::fetch_tasks_tagged(&slugs).await }
    });
    let mut status_line = use_signal(String::new);

    match &*rows.read_unchecked() {
        None => rsx! {
            div { class: "p-6 text-sm text-muted-foreground", "Loading task…" }
        },
        Some(Err(e)) => rsx! {
            div { class: "p-6",
                crate::states::ErrorState {
                    title: "Couldn't load this task",
                    message: e.clone(),
                }
            }
        },
        Some(Ok(tagged)) => {
            let Some((slug, task)) = tagged
                .iter()
                .find(|(_, t)| t.id == id)
                .map(|(s, t)| (s.clone(), t.clone()))
            else {
                return rsx! {
                    div { class: "p-6 text-sm text-muted-foreground",
                        "Task {id} isn't in the selected organizations."
                    }
                };
            };

            // Everything below resolves from the already-fetched set —
            // no extra round-trips.
            let titles: Vec<LinkedTaskRef> = tagged
                .iter()
                .map(|(_, t)| LinkedTaskRef {
                    id: t.id,
                    title: Some(t.title.clone()),
                })
                .collect();
            let org_of: HashMap<Uuid, String> =
                tagged.iter().map(|(s, t)| (t.id, s.clone())).collect();

            let wf = task.workflow.clone().unwrap_or_default();
            let subtasks: Vec<SubtaskRow> = tagged
                .iter()
                .filter(|(_, t)| t.workflow.as_ref().and_then(|w| w.parent) == Some(id))
                .map(|(_, t)| SubtaskRow {
                    task: t.clone(),
                    claimant: t
                        .workflow
                        .as_ref()
                        .and_then(|w| w.assignees.0.first().cloned()),
                })
                .collect();
            let blockers = resolve_links(&wf.blockers.0, &titles);
            let relates = resolve_links(&wf.relates_to.0, &titles);
            let claim = if wf.assignees.0.is_empty() {
                ClaimState::Unclaimed
            } else {
                ClaimState::Held {
                    holder: agent_label(&wf.assignees.0[0]),
                }
            };

            let claim_slug = slug.clone();
            let on_claim = move |()| {
                let slug = claim_slug.clone();
                let claimant = claimant_json(account.peek().as_ref());
                let mut rows = rows;
                spawn(async move {
                    match claim_task(&slug, id, &claimant).await {
                        Ok(()) => {
                            status_line.set(String::new());
                            rows.restart();
                        }
                        Err(e) => status_line.set(format!("Claim failed: {e}")),
                    }
                });
            };

            let est_slug = slug.clone();
            let est_task = task.clone();
            let on_set_estimate = move |est| {
                let slug = est_slug.clone();
                let mut next = est_task.clone();
                let mut rows = rows;
                spawn(async move {
                    let mut wf = next.workflow.take().unwrap_or_default();
                    wf.estimate = est;
                    next.workflow = Some(wf);
                    match update_task(&slug, next).await {
                        Ok(()) => {
                            status_line.set(String::new());
                            rows.restart();
                        }
                        Err(e) => status_line.set(format!("Estimate update failed: {e}")),
                    }
                });
            };

            let on_open = move |target: Uuid| {
                // Navigate within the detail page; unknown ids (cross-org
                // links outside the selection) fall back to the board.
                if org_of.contains_key(&target) {
                    nav.push(Route::TaskDetailRoute { id: target });
                } else {
                    nav.push(Route::TasksRoute {});
                }
            };

            rsx! {
                div { class: "mx-auto w-full max-w-4xl p-4 lg:p-6",
                    if !status_line().is_empty() {
                        div { class: "mb-3 rounded border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive",
                            "{status_line}"
                        }
                    }
                    TaskDetailFull {
                        task,
                        cycle_label: None,
                        subtasks,
                        resolved_blockers: blockers,
                        resolved_relates_to: relates,
                        transitions: Vec::new(),
                        activities: Vec::new(),
                        claim,
                        on_set_estimate,
                        on_claim,
                        on_open,
                    }
                }
            }
        }
    }
}

/// Friendly one-line label for a claim holder.
fn agent_label(a: &task_proto::workflows_proto::AgentRef) -> String {
    match a {
        task_proto::workflows_proto::AgentRef::Human {
            user_id,
            display_name,
        } => display_name.clone().unwrap_or_else(|| user_id.clone()),
        task_proto::workflows_proto::AgentRef::Agent { name, .. } => name.clone(),
    }
}

async fn claim_task(slug: &str, id: Uuid, claimant: &str) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        let client = crate::vox_clients::task_client(slug).await?;
        client
            .try_claim(id, claimant.to_owned(), false)
            .await
            .map_err(|e| format!("try_claim: {e:?}"))?;
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (slug, id, claimant);
        Err("native client not wired yet".to_owned())
    }
}

async fn update_task(slug: &str, task: TaskInfo) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        let client = crate::vox_clients::task_client(slug).await?;
        client
            .update(task)
            .await
            .map_err(|e| format!("update: {e:?}"))?;
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (slug, task);
        Err("native client not wired yet".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::ActiveAccount;
    use task_proto::workflows_proto::AgentRef;

    fn cody() -> ActiveAccount {
        ActiveAccount {
            user_id: Uuid::parse_str("3f2c8a4e-1d5b-4c6f-9e7a-2b8d4f6c1a3e").unwrap(),
            email: "cody@fasttrackstudios.com".into(),
            name: "Cody Wright".into(),
            token: "session-token".into(),
        }
    }

    /// The signed-in claimant must byte-match workflows_proto's own
    /// serde for `AgentRef::Human` — the server `from_str`s exactly
    /// that shape in `TaskBackend::try_claim`.
    #[test]
    fn claimant_json_matches_workflows_proto_serde() {
        let account = cody();
        let expected = serde_json::to_string(&AgentRef::Human {
            user_id: account.user_id.to_string(),
            display_name: Some(account.name.clone()),
        })
        .expect("serialize AgentRef");
        assert_eq!(claimant_json(Some(&account)), expected);
    }

    /// And it must parse back through workflows_proto's serde — the
    /// real proof the server-side `serde_json::from_str` accepts it.
    #[test]
    fn claimant_json_round_trips_through_agent_ref() {
        let account = cody();
        let parsed: AgentRef =
            serde_json::from_str(&claimant_json(Some(&account))).expect("server-side parse");
        assert_eq!(
            parsed,
            AgentRef::Human {
                user_id: account.user_id.to_string(),
                display_name: Some("Cody Wright".into()),
            }
        );
    }

    /// Signed out falls back to the legacy const, unchanged.
    #[test]
    fn claimant_json_falls_back_to_web_claimant() {
        assert_eq!(claimant_json(None), WEB_CLAIMANT);
    }
}

#[cfg(test)]
mod claimant_tests {
    use super::WEB_CLAIMANT;

    #[test]
    fn legacy_fallback_parses_as_agent_ref() {
        let parsed: task_proto::workflows_proto::AgentRef =
            serde_json::from_str(WEB_CLAIMANT).expect("fallback claimant must parse");
        match parsed {
            task_proto::workflows_proto::AgentRef::Human { user_id, .. } => {
                assert_eq!(user_id, "web")
            }
            other @ task_proto::workflows_proto::AgentRef::Agent { .. } => {
                panic!("expected human ref, got {other:?}")
            }
        }
    }
}

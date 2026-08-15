//! Forge webhook receiver.
//!
//! Forgejo (and GitHub) POST issue / pull-request events here.
//! We map the event back to a local `TaskInfo` via the per-org
//! `IssueLink` store and apply the state change — so closing an
//! issue *on the forge* closes the linked task locally, with no
//! polling.
//!
//! Route: `POST /org/{slug}/webhooks/forge`
//!
//! Signature: Forgejo/Gitea sign the raw body with HMAC-SHA256
//! using the webhook's configured secret, hex-encoded in the
//! `X-Gitea-Signature` (or `X-Forgejo-Signature`) header. We
//! verify against the per-org secret at
//! `<data_root>/orgs/<slug>/webhook-secret` (single line). If
//! no secret file exists, verification is skipped with a warning
//! — fine for a trusted LAN, not for a public endpoint.

use std::path::PathBuf;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use git_config::FileStore;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use task::service::TaskService;

use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

/// `POST /org/{slug}/webhooks/forge` handler.
pub async fn forge_webhook_handler(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Verify the signature if a secret is configured.
    match verify_signature(&state, &slug, &headers, &body) {
        SignatureCheck::Ok | SignatureCheck::Skipped => {}
        SignatureCheck::Bad => {
            tracing::warn!(org = %slug, "webhook signature mismatch — rejecting");
            return StatusCode::UNAUTHORIZED;
        }
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(org = %slug, error = %e, "webhook body not JSON");
            return StatusCode::BAD_REQUEST;
        }
    };

    match apply_event(&state, &slug, &payload) {
        Ok(Some(action)) => {
            tracing::info!(org = %slug, %action, "webhook applied");
            StatusCode::OK
        }
        // Recognized event but nothing to do (e.g. an action we
        // don't map, or no linked task) — 200 so the forge
        // doesn't retry.
        Ok(None) => StatusCode::OK,
        Err(e) => {
            tracing::error!(org = %slug, error = %e, "webhook apply failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

enum SignatureCheck {
    Ok,
    Bad,
    Skipped,
}

fn webhook_secret_path(state: &AppState, slug: &str) -> PathBuf {
    state.data_root.org(slug).path().join("webhook-secret")
}

fn verify_signature(
    state: &AppState,
    slug: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> SignatureCheck {
    let path = webhook_secret_path(state, slug);
    let Ok(secret) = std::fs::read_to_string(&path) else {
        return SignatureCheck::Skipped;
    };
    let secret = secret.trim();
    if secret.is_empty() {
        return SignatureCheck::Skipped;
    }

    // Forgejo: X-Forgejo-Signature; Gitea-compat: X-Gitea-Signature.
    let sig = headers
        .get("x-forgejo-signature")
        .or_else(|| headers.get("x-gitea-signature"))
        .or_else(|| headers.get("x-hub-signature-256")); // GitHub form
    let Some(sig) = sig.and_then(|v| v.to_str().ok()) else {
        return SignatureCheck::Bad;
    };
    // GitHub prefixes the hex with "sha256="; strip if present.
    let sig_hex = sig.strip_prefix("sha256=").unwrap_or(sig);
    let Ok(sig_bytes) = hex::decode(sig_hex) else {
        return SignatureCheck::Bad;
    };

    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return SignatureCheck::Bad;
    };
    mac.update(body);
    match mac.verify_slice(&sig_bytes) {
        Ok(()) => SignatureCheck::Ok,
        Err(_) => SignatureCheck::Bad,
    }
}

/// Open the per-org issue-link store. Same path the CLI uses.
fn link_store(state: &AppState, slug: &str) -> Result<FileStore, String> {
    let path = state.data_root.org(slug).path().join("issue-links.json");
    FileStore::open(path).map_err(|e| format!("open link store: {e}"))
}

/// Apply a forge event to the local task. Returns the action
/// label on success (for logging), or `None` when the event is
/// recognized but has no local effect.
fn apply_event(
    state: &AppState,
    slug: &str,
    payload: &serde_json::Value,
) -> Result<Option<String>, String> {
    let action = payload.get("action").and_then(|v| v.as_str());

    // Issue events carry `issue`; PR events carry `pull_request`.
    // Both expose `.number`; the repo is `repository.full_name`.
    let (number, is_pr, merged) = if let Some(issue) = payload.get("issue") {
        (
            issue.get("number").and_then(serde_json::Value::as_u64),
            false,
            false,
        )
    } else if let Some(pr) = payload.get("pull_request") {
        let merged = pr
            .get("merged")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        (
            pr.get("number").and_then(serde_json::Value::as_u64),
            true,
            merged,
        )
    } else {
        return Ok(None); // not an issue/PR event (push, etc.)
    };

    let Some(number) = number else {
        return Ok(None);
    };
    let full_name = payload
        .get("repository")
        .and_then(|r| r.get("full_name"))
        .and_then(|v| v.as_str());
    let Some(full_name) = full_name else {
        return Ok(None);
    };
    let (owner, repo) = match full_name.split_once('/') {
        Some(t) => t,
        None => return Ok(None),
    };

    // Decide the local status transition from the forge action.
    //  - issues/PR closed  -> done   (PR merged or just closed)
    //  - issues/PR reopened -> open
    // Other actions (edited, labeled, assigned, …) are ignored
    // for now; field-level sync is a separate concern.
    let new_status = match action {
        Some("closed") => "done",
        Some("reopened" | "opened") => "open",
        _ => return Ok(None),
    };

    // Find the linked task. The link store keys on RepoId, but
    // matching the host is fiddly across forge base-url forms;
    // match on (owner, repo, number) which is unique enough for
    // our single-forge-per-repo reality.
    let store = link_store(state, slug)?;
    // We don't know the exact base_url the link was stored with,
    // so scan all links for this org and match owner/repo/number.
    let task_id = find_task_for_issue(&store, owner, repo, number)?;
    let Some(task_id) = task_id else {
        return Ok(None); // forge issue we don't track locally
    };

    // Mutate the TaskInfo through the org's task backend. Take a
    // *clone* of the org state (`AppState::org` drops the registry
    // guard before returning) — the task get/update below is disk
    // IO, and holding the global orgs lock across it would stall
    // every other org's boot-time writes for no reason.
    let org = state
        .org(slug)
        .ok_or_else(|| format!("org `{slug}` not hosted"))?;
    let mut t = org
        .tasks
        .get(task_id)
        .map_err(|e| format!("get task {task_id}: {e:?}"))?;

    // No-op if already in the target terminal state.
    let already = match new_status {
        "done" => t.status == "done",
        "open" => t.status == "open",
        _ => false,
    };
    if already {
        return Ok(None);
    }

    t.status = new_status.to_string();
    if new_status == "done" {
        t.completed_date = Some(chrono::Local::now().date_naive());
        if let Some(w) = t.workflow.as_mut() {
            w.session = None;
        }
    } else {
        t.completed_date = None;
    }
    org.tasks
        .update(t)
        .map_err(|e| format!("update task {task_id}: {e:?}"))?;

    let kind = if is_pr { "pr" } else { "issue" };
    let merged_s = if merged { " (merged)" } else { "" };
    Ok(Some(format!(
        "{kind} {owner}/{repo}#{number} {action:?}{merged_s} -> task {task_id} = {new_status}"
    )))
}

/// Scan the link store for a task linked to `(owner, repo, number)`,
/// ignoring the forge base-url (single forge per repo in practice).
fn find_task_for_issue(
    store: &FileStore,
    owner: &str,
    repo: &str,
    number: u64,
) -> Result<Option<uuid::Uuid>, String> {
    // `BindingStore` doesn't expose a scan-by-(owner,repo,number)
    // method, so reconstruct via the public surface: we don't
    // know the host, but `tasks_for_issue` needs a full RepoId.
    // Fall back to reading the JSON directly through a helper the
    // FileStore exposes for exactly this case.
    store
        .find_task_by_owner_repo_number(owner, repo, number)
        .map_err(|e| format!("link lookup: {e}"))
        .map(|opt| opt.and_then(|s| uuid::Uuid::parse_str(&s).ok()))
}

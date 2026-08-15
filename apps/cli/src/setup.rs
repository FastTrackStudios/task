//! `task setup …` — one-shot forge/webhook integration setup.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::forge::build_repo_id;
use crate::forge::forge_link_store;
use crate::forge::forgejo_base_url;
use crate::forge::forgejo_token;
use crate::forge::github_token;
use crate::forge::parse_repo_slug;
use crate::resolve_active_org;

/// `task setup *` — guided integration setup.
#[derive(Subcommand)]
pub(crate) enum SetupCmd {
    /// Connect a forge repo to this org: ensure a webhook
    /// secret exists, register the webhook on the forge, and
    /// record the repo binding. Idempotent — re-running updates
    /// the existing webhook rather than duplicating it.
    Forge {
        /// `owner/repo` on the forge.
        #[arg(long)]
        repo: String,
        /// Target GitHub instead of Forgejo.
        #[arg(long)]
        github: bool,
        /// Forgejo host base URL. Falls back to `TASK_FORGEJO_BASE_URL`.
        #[arg(long)]
        base_url: Option<String>,
        /// Public URL the forge should POST events to. Should end
        /// in `/org/<slug>/webhooks/forge`. If omitted, derived
        /// from `--public-base` + the active org slug.
        #[arg(long)]
        webhook_url: Option<String>,
        /// Public base of the task-server (e.g.
        /// `https://tasks.example.com`). Used to build
        /// `<base>/org/<slug>/webhooks/forge` when `--webhook-url`
        /// isn't given.
        #[arg(long)]
        public_base: Option<String>,
        /// Optional project UUID to associate this repo with.
        #[arg(long)]
        project: Option<uuid::Uuid>,
        #[arg(long)]
        org: Option<String>,
    },
}

pub(crate) async fn run_setup(cmd: SetupCmd) -> eyre::Result<()> {
    use git_config::BindingStore as _;
    match cmd {
        SetupCmd::Forge {
            repo,
            github,
            base_url,
            webhook_url,
            public_base,
            project,
            org,
        } => {
            let slug = resolve_active_org(org)?;
            let repo_id = build_repo_id(&repo, github, base_url.clone())?;
            let (owner, repo_name) = parse_repo_slug(&repo)?;

            // 1. Ensure a webhook secret exists for this org.
            let secret = ensure_webhook_secret(&slug)?;

            // 2. Resolve the webhook URL.
            let hook_url = if let Some(u) = webhook_url {
                u
            } else {
                let base = public_base.ok_or_else(|| {
                    eyre::eyre!(
                        "pass --webhook-url, or --public-base to derive <base>/org/{slug}/webhooks/forge"
                    )
                })?;
                format!("{}/org/{slug}/webhooks/forge", base.trim_end_matches('/'))
            };

            // 3. Register (or update) the webhook via the forge API.
            let token = if github {
                github_token()?
            } else {
                forgejo_token()?
            };
            let api_base = if github {
                "https://api.github.com".to_string()
            } else {
                forgejo_base_url(base_url)?
            };
            register_webhook(
                &api_base, github, &owner, &repo_name, &token, &hook_url, &secret,
            )
            .await?;

            // 4. Record the repo binding (project/org -> repo).
            let store = forge_link_store(&slug)?;
            let project_id = project.map_or_else(|| slug.clone(), |p| p.to_string());
            store
                .add_repo_binding(git_config::RepoBinding {
                    project_id,
                    repo: repo_id,
                })
                .map_err(|e| eyre::eyre!("repo binding: {e}"))?;

            let forge_label = if github { "github" } else { "forgejo" };
            println!("✓ {forge_label} integration ready for {repo}");
            println!("  webhook → {hook_url}");
            println!("  events: issues, pull_request (signed with the org webhook secret)");
            println!("  secret: ~/.task/orgs/{slug}/webhook-secret");
            if let Some(p) = project {
                println!("  bound to project {p}");
            }
            println!(
                "\nClosing an issue/PR on the forge will now close the linked task\n\
                 (once the task-server is reachable at the webhook URL)."
            );
        }
    }
    Ok(())
}

/// Read the per-org webhook secret, generating + persisting one
/// (64 hex chars) if it doesn't exist yet.
///
/// Vox-unification judgment: STAYS a direct file write, on
/// purpose. The secret is SERVER-side config — the task-server
/// validates incoming forge webhooks against this exact file
/// under its own data root — so `task setup forge` only makes
/// sense co-resident with the server (or its embedded stand-in).
/// Provisioning a REMOTE server's webhook secret needs an
/// org-management RPC; until then this is honest local setup, not
/// a bypass.
fn ensure_webhook_secret(org_slug: &str) -> eyre::Result<String> {
    let home = std::env::var_os("HOME").ok_or_else(|| eyre::eyre!("HOME not set"))?;
    let path = std::path::Path::new(&home)
        .join(".task")
        .join("orgs")
        .join(org_slug)
        .join("webhook-secret");
    if path.exists() {
        let s = std::fs::read_to_string(&path).map_err(|e| eyre::eyre!("read secret: {e}"))?;
        let t = s.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    // Generate 32 bytes of entropy as hex via two v4 UUIDs.
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, &secret).map_err(|e| eyre::eyre!("write secret: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(secret)
}

/// Register (or update if one already targets the same URL) a
/// webhook on a forge repo. Forgejo + GitHub hook APIs differ
/// only in the `type`/`name` field.
async fn register_webhook(
    api_base: &str,
    github: bool,
    owner: &str,
    repo: &str,
    token: &str,
    hook_url: &str,
    secret: &str,
) -> eyre::Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("task/setup")
        .build()
        .map_err(|e| eyre::eyre!("http client: {e}"))?;
    let hooks_url = format!("{api_base}/api/v1/repos/{owner}/{repo}/hooks");
    let hooks_url = if github {
        format!("{api_base}/repos/{owner}/{repo}/hooks")
    } else {
        hooks_url
    };

    // Common config block both forges accept.
    let config = serde_json::json!({
        "url": hook_url,
        "content_type": "json",
        "secret": secret,
    });
    let mut body = serde_json::json!({
        "active": true,
        "events": ["issues", "pull_request"],
        "config": config,
    });
    // Forgejo wants `type`; GitHub wants `name: "web"`.
    if github {
        body["name"] = serde_json::json!("web");
    } else {
        body["type"] = serde_json::json!("forgejo");
    }

    // Idempotency: if a hook already targets this URL, PATCH it.
    let existing: Vec<serde_json::Value> = client
        .get(&hooks_url)
        .header("Authorization", format!("token {token}"))
        .send()
        .await
        .map_err(|e| eyre::eyre!("list hooks: {e}"))?
        .json()
        .await
        .unwrap_or_default();
    let existing_id = existing.iter().find_map(|h| {
        let url = h
            .get("config")
            .and_then(|c| c.get("url"))
            .and_then(|v| v.as_str());
        if url == Some(hook_url) {
            h.get("id").and_then(serde_json::Value::as_u64)
        } else {
            None
        }
    });

    let resp = if let Some(id) = existing_id {
        client
            .patch(format!("{hooks_url}/{id}"))
            .header("Authorization", format!("token {token}"))
            .json(&body)
            .send()
            .await
    } else {
        client
            .post(&hooks_url)
            .header("Authorization", format!("token {token}"))
            .json(&body)
            .send()
            .await
    }
    .map_err(|e| eyre::eyre!("register hook: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(eyre::eyre!("forge rejected webhook ({status}): {text}"));
    }
    Ok(())
}

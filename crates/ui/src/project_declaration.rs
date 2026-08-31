//! Naming the account that owns a project made by making a folder.
//!
//! **This is enrichment, never the critical path.** A folder made at a
//! place *is* a project the moment it exists: the sync service creates
//! the root, places it, and writes its `project.md` there and then,
//! deriving everything it can — the title from the folder, the org from
//! the place, who asked from the kernel. None of that waits for a
//! window to be open.
//!
//! The one thing a service cannot know is the **account**, because it
//! has no session. So `lead` is left blank rather than guessed, and
//! this fills it in the next time somebody is signed in on a machine
//! that holds the project. A studio that never opens the app still has
//! its projects, correctly declared, minus one field.
//!
//! That ordering is the point. Anything that must happen for a project
//! to be a project belongs in the code that makes it; anything that
//! needs a person's identity can be late.

use dioxus::prelude::*;

/// Mount the one-shot declaration effect. Call from `App`, after
/// `provide_auth`.
pub fn use_project_declaration() {
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    // Cheap when there is nothing to do, and it walks a socket and a
    // filesystem — once per launch, not once per signal change.
    let mut attempted = use_signal(|| false);

    use_effect(move || {
        let signed_in = account
            .read()
            .as_ref()
            .filter(|a| a.email != crate::auth::GUEST_EMAIL)
            .cloned();
        let Some(account) = signed_in else { return };
        if *attempted.peek() {
            return;
        }
        attempted.set(true);
        spawn(async move {
            let said = native::declare(&account.email).await;
            tracing::info!("projects: {said}");
        });
    });
}

/// The browser build: no agent, no local trees, nothing to declare.
#[cfg(target_arch = "wasm32")]
mod native {
    pub async fn declare(_email: &str) -> String {
        "the web app declares no local projects".to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    /// Write the missing `project.md` files. Returns the log line.
    pub async fn declare(email: &str) -> String {
        let client = match agent().await {
            Ok(client) => client,
            Err(e) => return format!("no agent to ask ({e})"),
        };
        let roots = match client.placed_roots().await {
            Ok(roots) => roots,
            Err(e) => return format!("could not list the roots ({e})"),
        };

        let mut declared = 0;
        let mut failed = 0;
        for root in roots {
            // Only what this machine watched being made. A root that
            // arrived from a peer was declared wherever it was created,
            // and writing a second declaration here would invent an
            // owner for somebody else's project.
            let Some(_made_by) = &root.made_by else {
                continue;
            };
            if root.path.is_empty() {
                continue;
            }
            let page = std::path::Path::new(&root.path).join("project.md");
            let Ok(existing) = std::fs::read_to_string(&page) else {
                // No page at all means the service could not write one.
                // Not this module's job to paper over that — it would
                // hide a real failure behind a partial fix.
                continue;
            };
            // Only ever a blank `lead`. A project that already names an
            // owner keeps it, whoever happens to be signed in here.
            if !needs_lead(&existing) {
                continue;
            }

            match std::fs::write(&page, with_lead(&existing, email)) {
                Ok(()) => {
                    declared += 1;
                    tracing::info!(place = %root.place, "named the owner of a project made here");
                }
                Err(e) => {
                    failed += 1;
                    tracing::warn!(place = %root.place, error = %e, "could not name its owner");
                }
            }
        }

        match (declared, failed) {
            (0, 0) => "every project made here already names its owner".to_string(),
            (n, 0) => format!("named the owner of {n} project(s) made here"),
            (n, f) => format!("named {n}, could not name {f}"),
        }
    }

    /// Does this page still have no `lead`?
    ///
    /// Frontmatter only: a `lead:` in the body is prose about somebody,
    /// not a declaration.
    fn needs_lead(page: &str) -> bool {
        let mut lines = page.lines();
        if lines.next().map(str::trim) != Some("---") {
            return false;
        }
        !lines
            .take_while(|l| l.trim() != "---")
            .any(|l| l.trim_start().starts_with("lead:"))
    }

    /// The same page with `lead` filled in, as the last frontmatter key.
    fn with_lead(page: &str, email: &str) -> String {
        let mut out = String::with_capacity(page.len() + email.len() + 8);
        let mut seen_open = false;
        for line in page.lines() {
            if line.trim() == "---" {
                if seen_open {
                    out.push_str(&format!("lead: {email}\n"));
                } else {
                    seen_open = true;
                }
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    async fn agent()
    -> Result<files_daemon_proto::service::DaemonControlServiceClient, String> {
        let bind =
            std::env::var("FTS_FILES_DAEMON_BIND").unwrap_or_else(|_| "127.0.0.1:4055".into());
        vox::connect_lane(&format!("ws://{bind}/vox"))
            .establish()
            .await
            .map_err(|e| e.to_string())
    }
}

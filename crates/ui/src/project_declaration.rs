//! Declaring the projects somebody made by making a folder.
//!
//! The agent turns a new folder at a place into a File Root, and
//! records who made it — the OS user and the machine, which is all a
//! background service can honestly know. It deliberately stops there: a
//! service has no session, and a project's *owner* is an account.
//!
//! This is the other half. The app is signed in, so it can take "made
//! by `cody` on THEBATTLESHIP at 14:20" and write the declaration that
//! makes it a project: a `project.md` in the root's own tree, carrying
//! the title, the org it belongs to, and the account that owns it.
//!
//! # Why a file in the tree, and not a vault page
//!
//! `ProjectBackend::adopt` writes exactly this page, and resolves its
//! directory *relative to the vault root*. These projects are File
//! Roots on a NAS, outside any vault, so that path cannot reach them —
//! and their siblings, the projects a studio already had, carry their
//! `project.md` in the tree beside the sessions. Writing it where the
//! rest of them are keeps one convention rather than two.
//!
//! The write is an ordinary write into the root's live tree, so the
//! agent's watcher captures it like any other edit and it syncs to
//! every machine that holds the project.
//!
//! # Once, quietly, and never overwriting
//!
//! A project that already declares itself is left alone — this only
//! ever fills in a blank. Failure is a log line: a machine where it
//! does not run still has the project, it just has not said whose it
//! is yet, and the next launch tries again.

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
            let Some(made_by) = &root.made_by else {
                continue;
            };
            if root.path.is_empty() {
                continue;
            }
            let page = std::path::Path::new(&root.path).join("project.md");
            if page.exists() {
                continue;
            }

            match std::fs::write(&page, declaration(&root, email, made_by)) {
                Ok(()) => {
                    declared += 1;
                    tracing::info!(place = %root.place, "declared a project made here");
                }
                Err(e) => {
                    failed += 1;
                    tracing::warn!(place = %root.place, error = %e, "could not declare it");
                }
            }
        }

        match (declared, failed) {
            (0, 0) => "every project made here already declares itself".to_string(),
            (n, 0) => format!("declared {n} project(s) made on this machine"),
            (n, f) => format!("declared {n}, could not declare {f}"),
        }
    }

    /// The page itself.
    ///
    /// The keys are the ones the project parser already reads — `title`,
    /// `status`, `lead` — rather than a second vocabulary meaning the
    /// same things. `organization` matches what the studio's existing
    /// pages carry.
    fn declaration(
        root: &files_daemon_proto::service::PlacedRoot,
        email: &str,
        made_by: &files_daemon_proto::model::MadeBy,
    ) -> String {
        let org = root
            .place
            .split_once('/')
            .map(|(org, _)| org)
            .unwrap_or_default();
        format!(
            "---\n\
             title: {title}\n\
             status: active\n\
             organization: {org}\n\
             lead: {email}\n\
             created: {created}\n\
             ---\n\
             \n\
             # {title}\n\
             \n\
             Made on {machine} by {user}.\n",
            title = root.name,
            created = made_by.at.to_rfc3339(),
            machine = made_by
                .device
                .map_or_else(|| "this machine".into(), |d| d.to_string()),
            user = made_by.user,
        )
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

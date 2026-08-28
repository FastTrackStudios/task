//! Pairing this machine with the org, from inside the app.
//!
//! Sync between two machines needs each to know the other's endpoint id.
//! Both halves used to be a person copying a hex string between a
//! terminal and a text file: a fine operator procedure and a poor
//! product, since the machine already knows its own id, the server knows
//! its own, and somebody signed in on this laptop is exactly the
//! authority that should be allowed to introduce them.
//!
//! So the app does the introduction with the session it already holds:
//!
//! 1. ask the local agent for its endpoint id (`fts-files-daemon id`),
//! 2. `enroll_device` — the org admits it, on the strength of the
//!    caller's sign-in,
//! 3. `coordinator` — the org's own id comes back,
//! 4. re-install the agent's service with that coordinator, so it syncs
//!    now and keeps syncing across restarts.
//!
//! # Why it lives here and not in `task-ui-core`
//!
//! `ui-core` depends on no `*-proto` crate on purpose — adding one would
//! put it in every consumer's dependency graph. This needs the Files
//! sync lane, so it belongs on the app side of that line.
//!
//! # Native only, once, and quiet
//!
//! There is no agent in a browser and nothing to pair, so the whole
//! module is compiled out there. On desktop it runs once per launch,
//! after a real sign-in, prompts nobody, and reports what it did to the
//! log: every step can fail for ordinary reasons — no agent shipped, the
//! org has no endpoint, the network is down — and none of them is worth
//! interrupting somebody over. It retries on the next launch.

use dioxus::prelude::*;

use crate::orgs::active_slug;
use task_ui_core::orgs::{OrgMeta, OrgSelection};

/// Mount the one-shot pairing effect. Call from `App`, after
/// `provide_auth` — it waits for a signed-in account.
pub fn use_device_pairing() {
    let org_selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    // Pairing is idempotent, but it shells out to a subprocess and
    // rewrites a service unit; doing that on every signal change would
    // restart a healthy agent whenever the org list refreshed.
    let mut attempted = use_signal(|| false);

    use_effect(move || {
        let signed_in = account
            .read()
            .as_ref()
            .filter(|a| a.email != crate::auth::GUEST_EMAIL)
            .is_some();
        let slug = active_slug(&org_selection.read(), &org_list.read());
        if !signed_in || slug.is_empty() || *attempted.peek() {
            return;
        }
        attempted.set(true);
        spawn(async move {
            let outcome = native::pair(&slug).await;
            tracing::info!("{outcome}");
        });
    });
}

/// The browser build: nothing to pair, and no subprocess to pair it
/// with. Kept as a same-shaped stub so the call site needs no `cfg`.
#[cfg(target_arch = "wasm32")]
mod native {
    pub async fn pair(_slug: &str) -> String {
        "sync: the web app has no local agent to pair".to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::PathBuf;
    use std::process::Command;

    use files_proto::service::sync::SyncServiceClient;

    /// The agent binary shipped beside this app — `Contents/MacOS/` in a
    /// bundle, a sibling file on Linux, which is "next to the
    /// executable" either way.
    fn agent() -> Option<PathBuf> {
        let here = std::env::current_exe().ok()?;
        let candidate = here.parent()?.join("fts-files-daemon");
        candidate.exists().then_some(candidate)
    }

    /// Ask the agent a one-line question.
    fn ask(agent: &PathBuf, args: &[&str]) -> Result<String, String> {
        let out = Command::new(agent)
            .args(args)
            .output()
            .map_err(|e| format!("running the agent: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// This machine's name as the org's device list should show it —
    /// "cody-macbook", the thing a person recognises, not a uuid.
    fn machine_name() -> String {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
            .or_else(|| {
                Command::new("hostname")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "this machine".to_string())
    }

    /// The first eight characters of an endpoint id: enough to
    /// recognise, short enough to read.
    fn short(id: &str) -> &str {
        id.get(..8).unwrap_or(id)
    }

    /// The agent running on this machine, over its local control socket.
    ///
    /// The same surface the `fts-files-daemon` CLI drives — the app is
    /// not a special client, which is the property that keeps the two
    /// from drifting.
    async fn local_agent()
    -> Result<files_daemon_proto::DaemonControlServiceClient, String> {
        let bind = std::env::var("FTS_FILES_DAEMON_BIND")
            .unwrap_or_else(|_| "127.0.0.1:4055".into());
        vox::connect_lane(&format!("ws://{bind}/vox"))
            .establish()
            .await
            .map_err(|e| e.to_string())
    }

    /// Pair this machine with `slug`. Returns the log line.
    pub async fn pair(slug: &str) -> String {
        let Some(agent) = agent() else {
            return "sync: no agent beside this build — background sync is off".into();
        };
        let endpoint = match ask(&agent, &["id"]) {
            Ok(id) if !id.is_empty() => id,
            Ok(_) => return "sync: not paired — the agent reported no endpoint id".into(),
            Err(e) => return format!("sync: not paired — {e}"),
        };

        let client: SyncServiceClient = match task_ui_core::vox_clients::establish_for(slug).await {
            Ok(client) => client,
            Err(e) => return format!("sync: not paired — connecting to {slug}: {e}"),
        };
        if let Err(e) = client.enroll_device(endpoint.clone(), machine_name()).await {
            return format!("sync: not paired — enrolling this machine: {e}");
        }
        let coordinator = match client.coordinator().await {
            Ok(id) => id,
            Err(e) => return format!("sync: not paired — asking {slug} for its endpoint id: {e}"),
        };

        // Already pointed here? Then there is nothing to do, and doing
        // it anyway would interrupt whatever is in flight.
        if ask(&agent, &["status"]).is_ok_and(|s| s.contains(&coordinator)) {
            return format!("sync: already paired with {slug}");
        }

        // A running agent is *told* its coordinator. Re-running the
        // installer would rewrite the service unit and restart it, which
        // interrupts transfers to deliver a string it could simply have
        // been handed — and on a machine syncing with other machines,
        // that restart is felt by all of them.
        if let Ok(client) = local_agent().await {
            if client.set_coordinator(coordinator.clone()).await.is_ok() {
                return format!(
                    "sync: this machine ({}) is paired with {slug} ({})",
                    short(&endpoint),
                    short(&coordinator)
                );
            }
        }

        // No agent answering: this machine has the binary but no
        // service, so install one.
        match Command::new(&agent)
            .args(["install", "--coordinator", &coordinator])
            .output()
        {
            Ok(out) if out.status.success() => format!(
                "sync: this machine ({}) is paired with {slug} ({})",
                short(&endpoint),
                short(&coordinator)
            ),
            Ok(out) => format!(
                "sync: not paired — {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => format!("sync: not paired — installing the agent's service: {e}"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_machine_has_a_name_a_person_would_recognise() {
            let name = machine_name();
            assert!(!name.is_empty());
            assert!(!name.contains('\n'), "{name:?}");
        }

        #[test]
        fn shortening_an_id_never_panics_on_a_short_one() {
            assert_eq!(short("8badf00d8badf00d"), "8badf00d");
            assert_eq!(short("abc"), "abc");
            assert_eq!(short(""), "");
        }
    }
}

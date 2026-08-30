//! `/sync` — what this machine is syncing, and with whom.
//!
//! Everything the sync agent can do has, until now, been a terminal
//! command. That is the right surface for installing a service and the
//! wrong one for the questions people actually ask about sync: is my
//! work on the other machine yet, why is this folder not moving, and
//! what do I do about the file we both edited.
//!
//! So this page reads the local agent's control socket — the same
//! surface `fts-files-daemon status` drives, so the two cannot drift —
//! and shows:
//!
//! - whether an agent is even running, and this machine's endpoint id,
//! - every root, where it comes from, how far along, and when it last
//!   synced,
//! - the paths two machines changed, with the button that settles them.
//!
//! # Why it polls
//!
//! The control surface has a `status_events` stream and this could
//! subscribe. It polls at a few seconds instead, because the page is a
//! panel a person opens, looks at, and closes: a poll that stops when
//! the component unmounts is less machinery than a subscription with
//! the same lifetime, and the numbers it shows change on a 30-second
//! tick anyway.
//!
//! # Native only
//!
//! There is no agent behind a browser tab. The web build renders the
//! one honest thing it can say.

use architect_ui::prelude::*;
use dioxus::prelude::*;

#[component]
pub fn SyncView() -> Element {
    rsx! {
        div { class: "mx-auto flex max-w-5xl flex-col gap-6 p-4 sm:p-6 lg:p-10",
            Heading { level: HeadingLevel::H1, "Sync" }
            Text {
                variant: TextVariant::Muted,
                "Files this machine keeps in step with your other machines. The agent runs in the background — it keeps syncing when this window is closed."
            }
            AgentPanel {}
        }
    }
}

/// The browser: no agent, and no pretending otherwise.
#[cfg(target_arch = "wasm32")]
#[component]
fn AgentPanel() -> Element {
    rsx! {
        task_ui_core::states::EmptyState {
            title: "Sync runs in the desktop app".to_string(),
            hint: Some(
                "A background agent keeps files in step after the window is closed, which a browser tab cannot do."
                    .to_string(),
            ),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[component]
fn AgentPanel() -> Element {
    use files_daemon_proto::DaemonStatus;

    // `None` while the first read is in flight; `Err` once we know there
    // is nothing answering — which is a real answer and the one a person
    // needs when nothing is syncing.
    let mut status = use_signal(|| None::<Result<DaemonStatus, String>>);

    use_future(move || async move {
        loop {
            let read = match agent().await {
                // Version skew reads as a decoder complaint about
                // schemas, which tells a person nothing about why their
                // files are not moving — `agent_error` turns it into the
                // sentence they can act on.
                Ok(client) => client
                    .status()
                    .await
                    .map_err(|e| crate::device_pairing::native::agent_error(&e)),
                Err(e) => Err(e),
            };
            status.set(Some(read));
            architect::platform::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    let current = status.read().clone();
    match current {
        None => rsx! { Text { variant: TextVariant::Muted, "Looking for the sync agent…" } },
        Some(Err(why)) => rsx! {
            task_ui_core::states::EmptyState {
                title: "No sync agent is running".to_string(),
                hint: Some(format!("Nothing is being kept in step on this machine. {why}")),
            }
        },
        Some(Ok(status)) => rsx! { AgentStatus { status } },
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[component]
fn AgentStatus(status: files_daemon_proto::DaemonStatus) -> Element {
    let endpoint = status.endpoint_id.clone().unwrap_or_else(|| "—".into());
    let machines = match status.peers.len() {
        0 => "no other machines yet".to_string(),
        1 => "1 machine".to_string(),
        n => format!("{n} machines"),
    };

    rsx! {
        section { class: "flex flex-col gap-2",
            Heading { level: HeadingLevel::H3, "This machine" }
            div { class: "flex flex-col gap-1 text-sm",
                div { class: "flex gap-2",
                    Text { variant: TextVariant::Muted, "address" }
                    code { class: "font-mono text-xs", "{endpoint}" }
                }
                div { class: "flex gap-2",
                    Text { variant: TextVariant::Muted, "syncing with" }
                    Text { "{machines}" }
                }
            }
        }

        section { class: "flex flex-col gap-3",
            Heading { level: HeadingLevel::H3, "Folders" }
            if status.roots.is_empty() {
                Text {
                    variant: TextVariant::Muted,
                    "No folders are syncing yet. Share one with `fts-files-daemon share <folder>`, or name another machine to take what it shares."
                }
            }
            for root in status.roots.iter().cloned() {
                RootRow { root }
            }
        }
    }
}

/// One folder: how it is doing, and anything that needs a person.
#[cfg(not(target_arch = "wasm32"))]
#[component]
fn RootRow(root: files_daemon_proto::RootStatus) -> Element {
    use files_daemon_proto::RootSyncState;

    let percent = root.percent();
    let state = match root.state {
        RootSyncState::Idle => "up to date".to_string(),
        RootSyncState::Syncing => format!("syncing — {percent}%"),
        RootSyncState::Paused => "paused".to_string(),
        RootSyncState::Error => "not syncing".to_string(),
    };
    let when = root.last_synced_at.map_or_else(
        || "never".to_string(),
        |t| {
            let secs = (chrono::Utc::now() - t).num_seconds().max(0);
            match secs {
                0..=90 => format!("{secs}s ago"),
                91..=5400 => format!("{}m ago", secs / 60),
                _ => format!("{}h ago", secs / 3600),
            }
        },
    );

    rsx! {
        div { class: "rounded-lg border border-border p-3 flex flex-col gap-2",
            div { class: "flex items-baseline justify-between gap-3",
                Text { class: "font-medium", "{root.name}" }
                Text { variant: TextVariant::Muted, class: "text-xs", "{state} · {when}" }
            }
            if let Some(why) = root.last_error.clone() {
                Text { variant: TextVariant::Muted, class: "text-xs", "{why}" }
            }
            MountRow { root_id: root.root_id, name: root.name.clone(), mounted_at: root.mounted_at.clone() }
            // The one thing here that needs a decision. Named, with the
            // action beside it — a person told "2 conflicts" still has
            // to go and find out which files.
            for path in root.divergent.iter().cloned() {
                DivergentPath { root_id: root.root_id, path }
            }
        }
    }
}

/// Showing a folder, or the button that starts.
///
/// The cloud-folder half: a mounted root lists everything at its real
/// size and fetches what this machine does not hold when something
/// opens it. Where it mounts is not asked — `~/Task/<name>` is the
/// answer for almost everybody, and a path picker in front of a feature
/// whose whole point is "it is just a folder" is a question nobody
/// wants. The CLI takes an explicit directory for the case this does
/// not fit.
#[cfg(not(target_arch = "wasm32"))]
#[component]
fn MountRow(root_id: uuid::Uuid, name: String, mounted_at: Option<String>) -> Element {
    let mut working = use_signal(|| false);
    let mut failed = use_signal(|| None::<String>);
    let at = mounted_at.clone();
    let for_click = name.clone();

    rsx! {
        div { class: "flex items-center justify-between gap-3",
            div { class: "flex flex-col",
                match at.clone() {
                    Some(where_) => rsx! {
                        Text { variant: TextVariant::Muted, class: "text-xs", "showing at {where_}" }
                    },
                    None => rsx! {
                        Text {
                            variant: TextVariant::Muted,
                            class: "text-xs",
                            "not showing as a folder — mount it to browse the whole project and open what this machine does not hold"
                        }
                    },
                }
                if let Some(why) = failed.read().clone() {
                    Text { variant: TextVariant::Muted, class: "text-xs", "{why}" }
                }
            }
            Button {
                disabled: *working.read(),
                on_click: move |_| {
                    let name = for_click.clone();
                    let mounted = at.is_some();
                    working.set(true);
                    failed.set(None);
                    spawn(async move {
                        let outcome = match agent().await {
                            Ok(client) => {
                                if mounted {
                                    client.unmount(root_id).await.map_err(|e| e.to_string())
                                } else {
                                    let home = std::env::var("HOME").unwrap_or_default();
                                    let at = format!("{home}/Task/{name}");
                                    client.mount(root_id, at).await.map_err(|e| e.to_string())
                                }
                            }
                            Err(e) => Err(e),
                        };
                        if let Err(e) = outcome {
                            failed.set(Some(e));
                        }
                        working.set(false);
                    });
                },
                if mounted_at.is_some() { "Stop showing" } else { "Show as a folder" }
            }
        }
    }
}

/// A path two machines changed, and the button that keeps both.
#[cfg(not(target_arch = "wasm32"))]
#[component]
fn DivergentPath(root_id: uuid::Uuid, path: String) -> Element {

    let mut settling = use_signal(|| false);
    let mut failed = use_signal(|| None::<String>);
    let for_click = path.clone();

    rsx! {
        div { class: "flex items-center justify-between gap-3 rounded-md bg-muted/40 p-2",
            div { class: "flex flex-col",
                Text { class: "text-sm", "Two machines changed {path}" }
                Text {
                    variant: TextVariant::Muted,
                    class: "text-xs",
                    "Keeping both puts the other version beside it, so nothing is lost."
                }
                if let Some(why) = failed.read().clone() {
                    Text { variant: TextVariant::Muted, class: "text-xs", "{why}" }
                }
            }
            Button {
                disabled: *settling.read(),
                on_click: move |_| {
                    let path = for_click.clone();
                    settling.set(true);
                    spawn(async move {
                        let outcome = match agent().await {
                            Ok(client) => client.keep_both(root_id, path).await.map_err(|e| e.to_string()),
                            Err(e) => Err(e),
                        };
                        if let Err(e) = outcome {
                            failed.set(Some(e));
                        }
                        settling.set(false);
                    });
                },
                if *settling.read() { "Keeping both…" } else { "Keep both" }
            }
        }
    }
}

/// The agent on this machine, over its local control socket.
///
/// The same surface the CLI drives. The app being an ordinary client of
/// it is what stops the two from growing different ideas of what the
/// agent is doing.
#[cfg(not(target_arch = "wasm32"))]
async fn agent() -> Result<files_daemon_proto::DaemonControlServiceClient, String> {
    let bind =
        std::env::var("FTS_FILES_DAEMON_BIND").unwrap_or_else(|_| "127.0.0.1:4055".into());
    vox::connect_lane(&format!("ws://{bind}/vox"))
        .establish()
        .await
        .map_err(|e| format!("no agent answering on {bind} ({e})"))
}

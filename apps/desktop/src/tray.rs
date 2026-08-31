//! The taskbar presence: Task as a thing that is *running*, not a
//! window that is open.
//!
//! The sync agent already outlives the window — that is the point of it
//! being a separate process — so for most of a working day there is no
//! Task on screen at all while it is very much working. A tray icon is
//! where that gets said, and where the two decisions somebody actually
//! revisits during a day live: is my project showing as a folder, and
//! am I looking at it by client or all together.
//!
//! # It drives the same socket as everything else
//!
//! Every item here is a call on `DaemonControlService` — the surface the
//! CLI drives and the `/sync` page drives. Nothing is implemented twice,
//! so the menu cannot come to believe something the agent does not.

use dioxus::desktop::trayicon::{DioxusTrayIcon, DioxusTrayMenu, init_tray_icon};
use dioxus::desktop::use_tray_menu_event_handler;
use dioxus::prelude::*;
// Through dioxus rather than a direct `tray-icon` dependency: the two
// would resolve different `muda` versions, and the menu types would
// then not satisfy the trait dioxus's own menu asks for — an error
// about `IsMenuItem` that says nothing about versions.
use dioxus::desktop::muda::{MenuId, MenuItem, PredefinedMenuItem, Submenu};

/// Where the composed tree is mounted.
///
/// `~/Task` because that is what the agent's own default roots
/// directory is, so the tray and the CLI put things in the same place
/// without either being told.
fn tree_root() -> String {
    std::env::var("FTS_FILES_TREE")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/Task")
        })
}

/// The ids are matched on click, so they are named once here rather
/// than spelled twice and drifting.
mod item {
    pub const SHOW: &str = "task.show";
    pub const BY_ORG: &str = "task.mount.by-org";
    pub const FLAT: &str = "task.mount.flat";
    pub const UNMOUNT: &str = "task.unmount";
    pub const CAPTURE: &str = "task.capture";
    pub const QUIT: &str = "task.quit";
}

/// Build the menu.
///
/// Deliberately short. A tray menu is read standing up, and everything
/// here is either "show me the app" or one of the two questions a
/// person re-asks during a day. Everything else belongs in the window,
/// where there is room to explain it.
fn menu() -> DioxusTrayMenu {
    let tray = DioxusTrayMenu::new();
    let tree = tree_root();

    let _ = tray.append(&MenuItem::with_id(
        MenuId::new(item::SHOW),
        "Open Task",
        true,
        None,
    ));
    let _ = tray.append(&PredefinedMenuItem::separator());

    // The two views, as one either/or rather than two verbs — they are
    // the same roots and picking one is picking how to look at them.
    let folders = Submenu::new(format!("Show folders in {tree}"), true);
    let _ = folders.append(&MenuItem::with_id(
        MenuId::new(item::BY_ORG),
        "Grouped by organisation",
        true,
        None,
    ));
    let _ = folders.append(&MenuItem::with_id(
        MenuId::new(item::FLAT),
        "All together — one Projects, one Assets",
        true,
        None,
    ));
    let _ = folders.append(&PredefinedMenuItem::separator());
    let _ = folders.append(&MenuItem::with_id(
        MenuId::new(item::UNMOUNT),
        "Stop showing them",
        true,
        None,
    ));
    let _ = tray.append(&folders);

    let _ = tray.append(&MenuItem::with_id(
        MenuId::new(item::CAPTURE),
        "Read what has not been read yet",
        true,
        None,
    ));

    let _ = tray.append(&PredefinedMenuItem::separator());
    // Quits the *app*. The agent is a service and keeps syncing, which
    // is the whole reason it is a service — so this does not say "Quit"
    // on its own, which would read as "stop syncing".
    let _ = tray.append(&MenuItem::with_id(
        MenuId::new(item::QUIT),
        "Close window (sync keeps running)",
        true,
        None,
    ));

    tray
}

/// Put Task in the taskbar and wire the menu to the agent.
///
/// Call once, from a component mounted for the life of the app.
pub fn use_tray() {
    // A desktop with no system tray is a desktop with no system tray,
    // not a broken app. On Linux the icon needs libayatana-appindicator
    // loaded at *runtime*, and the loader panics from inside the FFI
    // shim when it is absent — which took the whole window down with it,
    // because a panic while rendering a component is a panic in the
    // render. So the tray is attempted, and its absence is a log line.
    let placed = use_hook(|| {
        std::panic::catch_unwind(|| {
            init_tray_icon(menu(), None::<DioxusTrayIcon>);
        })
        .inspect_err(|_| {
            tracing::warn!(
                "no system tray on this desktop — Task runs without one \
                 (Linux needs libayatana-appindicator at runtime)"
            );
        })
        .is_ok()
    });
    if !placed {
        return;
    }

    use_tray_menu_event_handler(move |event| {
        let id = event.id.as_ref().to_string();
        match id.as_str() {
            item::SHOW => {
                let window = dioxus::desktop::window();
                window.set_visible(true);
                window.set_focus();
            }
            item::QUIT => dioxus::desktop::window().close(),
            // Everything else is the agent's work, and the agent is
            // across a socket — so it happens off the event loop. A
            // menu click that mounted forty roots inline would freeze
            // the window it was clicked from.
            _ => {
                spawn(act(id));
            }
        }
    });
}

/// One menu action, against the running agent.
///
/// Failure is logged rather than surfaced: a tray menu has nowhere to
/// put an error, and the `/sync` page shows the same state with room to
/// explain what went wrong.
async fn act(id: String) {
    let client = match agent().await {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("tray: {e}");
            return;
        }
    };
    let tree = tree_root();

    let outcome = match id.as_str() {
        item::BY_ORG | item::FLAT => client
            .mount_all(tree.clone(), id == item::FLAT)
            .await
            .map(|done| {
                let failed = done.iter().filter(|(_, e)| e.is_some()).count();
                format!("{} shown, {failed} could not be", done.len() - failed)
            })
            .map_err(|e| e.to_string()),
        item::UNMOUNT => {
            let mounted = client.mounts().await.map_err(|e| e.to_string());
            match mounted {
                Ok(mounted) => {
                    let count = mounted.len();
                    for (root, _) in mounted {
                        if let Err(e) = client.unmount(root).await {
                            tracing::warn!("tray: unmounting {root}: {e}");
                        }
                    }
                    Ok(format!("stopped showing {count}"))
                }
                Err(e) => Err(e),
            }
        }
        item::CAPTURE => client
            .start_capture()
            .await
            .map(|waiting| match waiting {
                0 => "every root has been read".to_string(),
                n => format!("reading {n} roots — the sync page shows where it is"),
            })
            .map_err(|e| e.to_string()),
        other => Err(format!("no such menu item: {other}")),
    };

    match outcome {
        Ok(said) => tracing::info!("tray: {said}"),
        Err(e) => tracing::warn!("tray: {e}"),
    }
}

/// The agent on this machine, over its local control socket — the same
/// one the CLI and the app's sync page use.
async fn agent() -> Result<files_daemon_proto::service::DaemonControlServiceClient, String> {
    let bind = std::env::var("FTS_FILES_DAEMON_BIND").unwrap_or_else(|_| "127.0.0.1:4055".into());
    vox::connect_lane(&format!("ws://{bind}/vox"))
        .establish()
        .await
        .map_err(|e| format!("no agent answering on {bind} ({e})"))
}

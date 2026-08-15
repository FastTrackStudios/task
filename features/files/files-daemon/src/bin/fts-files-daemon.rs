//! `fts-files-daemon` — the standalone headless sync daemon (issue
//! #265): studio rigs and servers run this as a system/user service.
//!
//! It opens the daemon over a local replica store, mounts
//! [`files_daemon::DaemonControlService`] on a local WebSocket (the
//! desktop app and the `fts files daemon` CLI connect to it), and runs
//! the reconcile tick on a fixed interval. The device identity lives in
//! the data dir, so the daemon re-announces as the same device across
//! restarts and keeps syncing without an interactive login — the "keep
//! syncing / survive session expiry" acceptance criterion.
//!
//! Config is environment-driven so it drops into a service unit:
//! `FTS_FILES_DAEMON_DATA` (data dir, default `~/.local/share/fts-files`),
//! `FTS_FILES_DAEMON_BIND` (control socket, default `127.0.0.1:4055`),
//! `FTS_FILES_DAEMON_INTERVAL_SECS` (tick cadence, default 30).

use std::net::SocketAddr;
use std::time::Duration;

use architect::LayerRouter;
use axum::extract::ws::WebSocketUpgrade;
use axum::response::IntoResponse;
use axum::routing::any;
use files::FilesBackend;
use files_daemon::{DaemonControl, SyncDaemon};

const VOX_SUBPROTOCOL: &str = "vox";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let data_dir = std::env::var("FTS_FILES_DAEMON_DATA").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{home}/.local/share/fts-files")
    });
    let bind: SocketAddr = std::env::var("FTS_FILES_DAEMON_BIND")
        .unwrap_or_else(|_| "127.0.0.1:4055".into())
        .parse()?;
    let interval = Duration::from_secs(
        std::env::var("FTS_FILES_DAEMON_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30),
    );

    // The daemon's own replica store lives under the data dir; the vault
    // is a sibling (curated version entities ride the vault, not the
    // replica content).
    let store = format!("{data_dir}/store");
    let vault = format!("{data_dir}/vault");
    std::fs::create_dir_all(&store)?;
    let backend = FilesBackend::new(&store, &vault)?;
    let daemon = SyncDaemon::open(backend, format!("{data_dir}/daemon"))?;
    tracing::info!(device = %daemon.device_id(), %bind, "fts-files-daemon starting");

    // Point the daemon at its coordinator — the `SyncService` endpoint
    // it pulls from (task-server's `/…/vox`). Without this, every
    // `set_sync_choice` over the control socket would fail with "no
    // coordinator set" and nothing would ever sync (PR #292 review), so
    // an unset coordinator is a hard startup error rather than a silent
    // one discovered later.
    let coordinator_url = std::env::var("FTS_FILES_DAEMON_COORDINATOR").map_err(|_| {
        "FTS_FILES_DAEMON_COORDINATOR is required — the ws:// SyncService endpoint to sync from"
    })?;
    let coordinator: files_daemon::files_sync::SyncServiceClient =
        vox::connect_lane(&coordinator_url)
            .establish()
            .await
            .map_err(|e| format!("connecting to coordinator {coordinator_url}: {e}"))?;
    daemon.set_coordinator(coordinator);
    tracing::info!(coordinator = %coordinator_url, "coordinator connected");

    // The reconcile tick loop — keeps every chosen root syncing.
    let ticker = daemon.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        loop {
            timer.tick().await;
            ticker.tick().await;
        }
    });

    // The control surface on a local WebSocket.
    let control = DaemonControl::new(daemon);
    let app = axum::Router::new().route(
        "/vox",
        any(move |ws: WebSocketUpgrade| {
            let router = LayerRouter::new()
                .merge(files_daemon::service::layer(control.clone()))
                .merge(files_daemon::service::stream_layer(control.clone()));
            async move {
                ws.protocols([VOX_SUBPROTOCOL])
                    .on_upgrade(move |socket| architect::axum_ws::serve_router(socket, router))
                    .into_response()
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "fts-files-daemon control socket listening");
    axum::serve(listener, app).await?;
    Ok(())
}

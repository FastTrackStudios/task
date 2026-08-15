//! Single-turn chat demo. Spawns `codex app-server` for a
//! workspace, sends `thread/start` + `turn/start`, returns a
//! stream of translated `AgentEvent`s.
//!
//! Not the final `Agents` impl — this is the
//! validation surface CLI demos build on. Slice 2c wraps
//! these primitives behind the trait.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use std::pin::Pin;

use futures::Stream;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::vendor::types::WorkspaceSettings;

use agent_proto::event::AgentEvent;

use crate::CodexBackend;
use crate::translate::translate;
use crate::vendor::app_server::{WorkspaceSession, spawn_workspace_session};
use crate::vendor::types::{WorkspaceEntry, WorkspaceKind};

/// User-facing options for one chat turn.
#[derive(Debug, Clone, Default)]
pub struct ChatOpts {
    /// Codex binary to invoke. `None` ⇒ resolve `codex` on
    /// `$PATH`.
    pub codex_bin: Option<String>,
    /// Extra args passed to `codex app-server` on spawn.
    pub codex_args: Option<String>,
    /// `$CODEX_HOME` override. `None` ⇒ daemon picks up
    /// the user's default.
    pub codex_home: Option<PathBuf>,
    /// Model id (`"gpt-5.4-mini"`, `"o3"`, ...). `None`
    /// ⇒ daemon default.
    pub model: Option<String>,
    /// Reasoning-effort hint
    /// (`"none" | "minimal" | "low" | "medium" | "high"`).
    pub effort: Option<String>,
    /// `"read-only" | "current" | "full-access"`. Default
    /// is `"current"` (matches `CodexMonitor`).
    pub access_mode: Option<String>,
}

/// Result of one chat dispatch. The event stream completes
/// when the daemon sends `turn/completed` (or any
/// `TurnErrored`); the session handle stays alive so the
/// caller can decide whether to keep the workspace process
/// running or drop it.
pub struct ChatHandle {
    pub session_id: String,
    pub thread_id: String,
    pub workspace_path: PathBuf,
    /// Keep-alive handle to the underlying `codex app-server`
    /// process. Holding `ChatHandle` keeps the daemon
    /// alive until the events stream drops; explicit
    /// shutdown lands in 2c.
    #[allow(dead_code)]
    pub(crate) workspace_session: Arc<WorkspaceSession>,
    pub events: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
}

impl CodexBackend {
    /// Dispatch a single chat turn.
    ///
    /// - Spawns `codex app-server` rooted at
    ///   `workspace_path`.
    /// - Sends `thread/start` to get a `threadId`.
    /// - Sends `turn/start` with `text` + `opts`.
    /// - Returns a [`ChatHandle`] whose `events` stream
    ///   yields translated `AgentEvent`s until the turn
    ///   completes.
    pub async fn chat(
        &self,
        workspace_path: PathBuf,
        text: String,
        opts: ChatOpts,
    ) -> Result<ChatHandle, String> {
        let workspace_id = format!("ws-{}", Uuid::new_v4().simple());
        let session_id = format!("sess-{}", Uuid::new_v4().simple());

        let entry = WorkspaceEntry {
            id: workspace_id.clone(),
            name: workspace_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("workspace")
                .to_string(),
            path: workspace_path.to_string_lossy().to_string(),
            kind: WorkspaceKind::Main,
            parent_id: None,
            worktree: None,
            settings: WorkspaceSettings::default(),
        };

        let session = spawn_workspace_session(
            entry,
            opts.codex_bin.clone(),
            opts.codex_args.clone(),
            opts.codex_home.clone(),
            "agent-codex/0.1".to_string(),
            self.sink(),
        )
        .await?;

        // Subscribe to event firehose *before* sending the
        // first RPC so we don't miss anything.
        let raw_rx = self.subscribe_raw();

        // Create a thread within the workspace.
        let thread_resp = tokio::time::timeout(
            Duration::from_secs(30),
            session.send_request(
                "thread/start",
                json!({
                    "cwd": workspace_path.to_string_lossy(),
                    "approvalPolicy": match opts.access_mode.as_deref() {
                        Some("full-access") => "never",
                        _ => "on-request",
                    },
                }),
            ),
        )
        .await
        .map_err(|_| "thread/start timed out".to_string())??;

        // `send_request` returns the full JSON-RPC envelope.
        // Codex puts the id at `result.thread.id`; older
        // daemons used `result.threadId`. Try both.
        let result = thread_resp
            .get("result")
            .ok_or_else(|| format!("thread/start response missing result: {thread_resp}"))?;
        if let Some(err) = thread_resp.get("error") {
            return Err(format!("thread/start error: {err}"));
        }
        let thread_id = result
            .get("thread")
            .and_then(|t| t.get("id"))
            .and_then(|v| v.as_str())
            .or_else(|| result.get("threadId").and_then(|v| v.as_str()))
            .ok_or_else(|| format!("thread/start response missing thread.id: {thread_resp}"))?
            .to_string();

        // Build the turn input + sandbox policy.
        let sandbox_policy: Value = match opts.access_mode.as_deref() {
            Some("full-access") => json!({ "type": "dangerFullAccess" }),
            Some("read-only") => json!({ "type": "readOnly" }),
            _ => json!({
                "type": "workspaceWrite",
                "writableRoots": [workspace_path.to_string_lossy()],
                "networkAccess": true,
            }),
        };
        let approval_policy = if matches!(opts.access_mode.as_deref(), Some("full-access")) {
            "never"
        } else {
            "on-request"
        };

        let turn_params = json!({
            "threadId": thread_id,
            "input": [{ "type": "text", "text": text }],
            "cwd": workspace_path.to_string_lossy(),
            "approvalPolicy": approval_policy,
            "sandboxPolicy": sandbox_policy,
            "model": opts.model,
            "effort": opts.effort,
        });

        session
            .send_request_for_workspace(&workspace_id, "turn/start", turn_params)
            .await?;

        let session_id_for_filter = session_id.clone();
        let session_id_for_translate = session_id.clone();
        let workspace_id_for_filter = workspace_id.clone();

        let debug_raw = std::env::var("AGENT_CODEX_DEBUG").is_ok();
        let events = BroadcastStream::new(raw_rx).filter_map(move |ev| {
            let session_id = session_id_for_translate.clone();
            let workspace_id = workspace_id_for_filter.clone();
            async move {
                let ev = ev.ok()?;
                if ev.workspace_id != workspace_id {
                    return None;
                }
                if debug_raw {
                    eprintln!(
                        "[raw] {} {}",
                        ev.message
                            .get("method")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?"),
                        serde_json::to_string(&ev.message).unwrap_or_default()
                    );
                }
                translate(&ev, &session_id)
            }
        });

        let _ = session_id_for_filter;

        Ok(ChatHandle {
            session_id,
            thread_id,
            workspace_path,
            workspace_session: session,
            events: Box::pin(events),
        })
    }
}

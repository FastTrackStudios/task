//! Vendored from
//! `CodexMonitor/src-tauri/src/backend/events.rs`.
//!
//! Trimmed: we only need `AppServerEvent` + the
//! `emit_app_server_event` half of the `EventSink` trait;
//! terminal events are stripped (Task wires terminals
//! separately).

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Debug)]
pub struct AppServerEvent {
    pub workspace_id: String,
    pub message: Value,
}

pub trait EventSink: Clone + Send + Sync + 'static {
    fn emit_app_server_event(&self, event: AppServerEvent);
}

#![allow(unexpected_cfgs)]
#![cfg(not(target_arch = "wasm32"))]

//! `label-proto` — Label entity.
//!
//! Labels are colored tags scoped to a Workspace. A `TaskInfo`
//! can carry any number of labels; the join table lives in
//! `label-config` (next slice). When a TaskInfo links to a
//! forge Issue, labels with a matching name on the forge mirror
//! automatically — see `git-config::sync`.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One label.
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "labels", repo)]
pub struct Label {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Which workspace owns this label. Labels don't cross
    /// workspaces — same-name labels in different workspaces
    /// are independent entities.
    #[architect(filterable, sortable)]
    pub workspace_id: Uuid,

    /// Display name. Case-preserved; uniqueness checked
    /// case-insensitively per workspace.
    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Optional human-readable description shown on hover.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,

    /// 6-char hex without the leading `#`. `None` means the UI
    /// picks a default from a stable palette derived from the
    /// label name's hash.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<String>,

    /// If `Some`, the label belongs to a label group (e.g. all
    /// `priority/*` labels group under "Priority"). Groups
    /// constrain that a task carries at most one label from any
    /// given group — useful for things like priority and area.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub group: Option<String>,

    /// If `Some`, the label is scoped to one project and only
    /// surfaces there. `None` means org-scoped — available across
    /// every project in the workspace (the default).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(filterable)]
    pub project_id: Option<Uuid>,

    #[architect(filterable, sortable)]
    pub created_at: DateTime<Utc>,

    #[architect(filterable, sortable)]
    pub updated_at: DateTime<Utc>,
}

impl Label {
    /// Build a fresh label.
    #[must_use]
    pub fn new(workspace_id: Uuid, name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            workspace_id,
            name: name.into(),
            description: None,
            color: None,
            group: None,
            project_id: None,
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_initializes_workspace_and_name() {
        let ws = Uuid::new_v4();
        let l = Label::new(ws, "bug");
        assert_eq!(l.workspace_id, ws);
        assert_eq!(l.name, "bug");
        assert!(l.color.is_none());
    }

    #[test]
    fn new_label_is_org_scoped() {
        let l = Label::new(Uuid::nil(), "bug");
        assert!(l.project_id.is_none(), "fresh labels are org-scoped");
    }

    #[test]
    fn project_id_omitted_when_none() {
        // org-scoped labels don't serialize a `project_id` key, and
        // legacy `labels.json` written before the field existed still
        // deserializes (the missing key defaults to `None`).
        let l = Label::new(Uuid::nil(), "bug");
        let json = serde_json::to_string(&l).unwrap();
        assert!(!json.contains("project_id"));
        let back: Label = serde_json::from_str(&json).unwrap();
        assert!(back.project_id.is_none());
    }

    #[test]
    fn project_scoped_label_round_trips() {
        let pid = Uuid::new_v4();
        let mut l = Label::new(Uuid::nil(), "frontend");
        l.project_id = Some(pid);
        let json = serde_json::to_string(&l).unwrap();
        let back: Label = serde_json::from_str(&json).unwrap();
        assert_eq!(back.project_id, Some(pid));
    }
}

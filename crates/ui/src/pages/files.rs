//! `/files` — the Files explorer pane (issue #266).
//!
//! The whole surface lives in the `files-ui` feature crate, which owns
//! its own `FilesService` calls and its note-widget provider; the shell
//! only routes to it (the same split as `/goals`, `/email`, `/repos` —
//! see `crates/task/ui/ARCHITECTURE.md`).

pub use files_ui::FilesPane as FilesView;

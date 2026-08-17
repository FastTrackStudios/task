//! Umbrella crate. Downstream apps depend on `editor` and get
//! the state + view surface from one place.

pub use editor_state::*;
pub use editor_view::{self, Editor, TransactionEvent, dispatch_spec};
pub use editor_vim;

/// Built-in commands. Re-exported as `editor::commands::*`.
pub mod commands {
    pub use editor_state::commands::*;
}

/// Turnkey [`EditorApp`] Dioxus component — drop into any app
/// for a working markdown editor with no setup.
pub mod app;
pub use app::{EDITOR_STYLE, EditorApp, combined_decorations, standard_markdown_keymap};

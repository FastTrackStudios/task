//! Pure-logic core for the editor: document, transactions,
//! selections, decorations. No DOM, no Dioxus.
//!
//! Mirrors the `@codemirror/state` package in spirit — see the
//! [design notes](../../../docs/design.md) for the concept ↔
//! Rust-type mapping. Reference source at
//! `~/Development/research/codemirror/state/src/`.

pub mod bracket_match;
pub mod change;
pub mod command;
pub mod commands;
pub mod decoration;
pub mod doc;
pub mod history;
pub mod hover;
pub mod markdown;
pub mod selection;
pub mod state;
pub mod transaction;

pub use change::{Change, Changes};
pub use command::{Command, KeyBinding, KeySpec, Keymap};
pub use decoration::{
    DecoPhase, DecoRangeSet, DecoratedRange, Decoration, DecorationKind, DecorationSet, deco_phase,
    set_deco_phase,
};
pub use doc::Doc;
pub use history::History;
pub use hover::{HoverSource, HoverTooltip};
pub use selection::{Range, Selection};
pub use state::EditorState;
pub use transaction::{Transaction, TransactionSpec};

//! Dioxus view layer for the editor. Renders an
//! [`editor_state::EditorState`] into a contenteditable element
//! and translates user input into transactions.
//!
//! v1 scope: plain-text editing. Decorations come next.

pub use editor_state;

mod bridge;
mod editor;
mod event;
#[cfg(feature = "native")]
pub mod native;
pub mod hover;
pub mod slash;
pub mod tile;
pub mod trigger;

pub use editor::{DecorationSource, Editor, coarse_pointer};
pub use event::{TransactionEvent, dispatch_spec};
pub use hover::{HoverPopup, HoverTooltipView};
pub use trigger::{Candidate, CompletionKind, CompletionSource, CompletionState};

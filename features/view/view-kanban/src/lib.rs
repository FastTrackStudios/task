//! Kanban board — Dioxus port inspired by janhesters/shadcn-kanban-board.
//!
//! Dumb UI crate: feed `columns` + `cards` in via props, receive
//! mutations back via `EventHandler<KanbanEvent>`. No CRDT/DB
//! awareness. Use architect-ui primitives only.
//!
//! # Shape
//!
//! - `KanbanCard { id, title, description }` — atomic, draggable.
//! - `KanbanColumn { id, title, color, cards: Vec<CardId> }` — owns
//!   an ordered list of card ids. Cards live in a flat
//!   `HashMap<CardId, KanbanCard>` on the state so reorder + move
//!   are id-list edits, not card-content edits.
//!
//! # Drag-and-drop
//!
//! Native HTML5 (`draggable`, `ondragstart` / `ondragover` / `ondrop`
//! / `ondragend`). A single `DragContext` signal tracks which card
//! is being dragged so columns and cards can highlight valid targets
//! without prop-drilling. Cross-column moves and intra-column
//! reorders are both modelled as `KanbanEvent::MoveCard { to_column,
//! to_index }`.

pub mod components;
pub mod store;
pub mod types;

pub use components::{Kanban, KanbanProps};
pub use store::{KanbanEvent, KanbanState, apply};
pub use types::{CardId, ColorTag, ColumnId, KanbanCard, KanbanColumn};

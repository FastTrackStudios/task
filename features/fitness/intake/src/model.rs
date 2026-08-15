//! The daily food-intake models — `IntakeLog` / `IntakeEntry` /
//! `IntakeSource` (+ the `scale_nutrition` helper) — now live in
//! the wasm-clean [`fitness_proto`] crate so the web UI can bind
//! to the wire contract directly. `Nutrition` itself is shared
//! via `cookbook_proto` (re-exported by `cookbook`). Re-exported
//! here so the existing `intake::model::*` paths keep working.

pub use fitness_proto::intake::model::*;

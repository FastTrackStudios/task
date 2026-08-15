# Plan: icon-bearing Tags + dioxus-icons foundation

**Status:** proposed (awaiting review)
**Date:** 2026-06-14

## Goal

A general, icon-bearing **Tag** the user defines once (name + chosen icon +
color) and applies to time blocks, tasks, and projects. In the calendar, when a
block is too small for its title, it shows its first tag's icon (the way meal
blocks read as "food" at a glance); click / day-view shows full detail.

Two user decisions already taken:
- **Icon foundation now, mass-migration later** — adopt the icon libraries and
  build Tags on them; migrating existing `architect_ui::lucide_dioxus` call sites is a
  separate sweep.
- **General tags** — events + tasks + projects, not scheduling-only.

## Icon libraries (approved)

- **`dioxus-icons` v0.1** (DioxusLabs) — Lucide 1700+ as components
  (`dioxus_icons::lucide::Name { size }`). The default set. Same shape as the
  current `architect_ui::lucide_dioxus`, so the later migration is near-mechanical.
- **`dioxus-free-icons` v0.9** — icons as values (`Icon { icon, width, height }`),
  18+ packs. Source for anything Lucide lacks.

### The `TagIcon` type (the crux) — OPEN, not a closed set

The user must be able to **pick a common icon OR paste their own**, so the
representation is open:

```rust
// tag proto (wasm-clean): the persisted choice
pub enum TagIcon {
    /// A key into our supported icon set — curated ~40–60 common Lucide
    /// names, EXPANDABLE by adding a (key → component) arm to the render
    /// helper. Unknown key → neutral fallback dot.
    Named(String),
    /// Raw pasted SVG markup — the "copy & paste your own" path. Rendered
    /// inline (sanitized); needs no library entry, so any icon is possible
    /// without a 1700-arm match.
    Svg(String),
}
```

- Lives in the tag proto crate (wasm-clean) so both the entity and the UI use it.
- Render helper `TagIconView { icon: TagIcon, size }`: `Named` → match known keys →
  `dioxus-icons` lucide component (or `dioxus-free-icons` value for extras);
  `Svg` → inline markup.
- The **picker** lists the curated `Named` keys + a "paste SVG / custom" field.
  "Expand the set" = add a key + arm; "use anything now" = paste SVG.

## Tag entity + service

Reconcile the three existing concepts rather than add a fourth:
- `label-proto::Label` — barely built (entity only, native-only, no service,
  lightly used by agent-tasks + CLI).
- `TaskInfo.tags: StringList` — free-form string tags already on tasks.
- contexts — GTD strings; out of scope.

**Decided:** evolve `label` into the general **Tag** feature (it already has
name/color/workspace-scope/group/project-scope) — add `icon: TagIcon`, make it
wasm-clean, give it a real `TagService` (`#[architect::rpc]`, per-org CRUD +
list). Rename `Label` → `Tag` (the user's word); preserve the agent-tasks/CLI
call sites.

**Association = string tag NAMES** (decided — the user wants to tag in raw
markdown). Entities reference tags by name in a `tags: Vec<String>` frontmatter
list, exactly like `TaskInfo.tags` does today:
- `TaskInfo.tags` — **reuse the existing string list** (no new field).
- `CalEvent.tags: Vec<String>` (new).
- `ProjectInfo.tags: Vec<String>` (new, if not present).

The **Tag registry** maps a name → `{ icon, color }`. Resolution is by name
(case-insensitive). A tag used in markdown with no registry entry renders as a
plain chip (no icon); the UI can offer to create it. No UUID refs, no join table
— markdown stays the source of truth, the registry just decorates.

## Phases (each its own PR; protos ⇒ schema skew, rebuild server)

1. **Icon foundation** — add both crates; `TagIcon` type (`Named`/`Svg`) in the
   tag proto + `TagIconView` render helper + `IconPicker` (curated keys + paste
   SVG). No behavior change yet.
2. **Tag feature** — evolve `label` → `tag` (entity + `icon: TagIcon`),
   wasm-clean, `TagService` CRUD/list, crdt/db. Tag-management UI (list/create/
   edit with the picker). A by-name resolver `Vec<Tag> -> name → {icon,color}`.
3. **Apply to time blocks** — `CalEvent.tags: Vec<String>`; tag-assign UI in the
   event editor; calendar chip renders first tag's icon when compact (the
   headline request).
4. **Apply to tasks + projects** — reuse `TaskInfo.tags`; add `ProjectInfo.tags`;
   assign UI + decorated chips on rows.
5. *(later)* mass-migrate existing `architect_ui::lucide_dioxus` → `dioxus-icons`.

## Decisions (resolved)

1. Evolve `label` → `Tag`. ✓
2. Tags are **string names** in markdown frontmatter (reuse `TaskInfo.tags`); the
   Tag registry decorates them by name. ✓ (No UUID refs.)
3. `TagIcon` is **open** (`Named`/`Svg`): curated ~40–60 common icons in the
   picker + paste-your-own SVG; expandable. ✓

## Acceptance

- Tag defined once (name/icon/color) via service; CLI + UI share it.
- First-tag icon renders in compact calendar chips; full detail on click/day.
- `cargo check` native + wasm; tests for the Tag service + the resolver.

## Risk

- Proto changes (Tag entity, CalEvent/TaskInfo/ProjectInfo fields) ⇒ schema skew.
- Scope creep: keep `TaskInfo.tags` string↔entity unification OUT of this arc.
- `dioxus-free-icons` values vs `dioxus-icons` components are different shapes —
  the `TagIcon` render helper hides that behind one component.

# Plan: `#[derive(EntityCrdt)]` proc-macro

## The problem

Adding an entity today requires hand-writing ~80 lines of CRDT
codec boilerplate in `features/<feature>/<feature>-crdt/src/<entity>.rs`.
For each entity:

- `impl EntityCrdt for <Entity>Entity`
  - `const ROOT: &'static str` (the LoroMap key — usually the
    table_name from the `#[architect(...)]` attr)
  - `fn id(e) -> Uuid` (just `e.id`)
  - `fn from_create(c) -> Wire` — copy every Create field into a
    new Wire, generate a fresh `Uuid` for the primary key
  - `fn encode_into(m, e)` — one `write_<type>(m, "<field>", e.<field>)`
    per field
  - `fn decode_from(m) -> Wire` — symmetric `read_<type>` calls
  - `fn apply_update(m, u)` — one `if let Some(v) = u.<field>`
    per non-PK field
  - `fn build_list(items, total, page) -> List` (trivial struct
    initialization)
  - `fn sort_items(items, field, order)` — switch on field names
    marked `#[architect(sortable)]`, sort the slice, reverse on
    `Desc`

Plus the `pub struct <Entity>Entity;` + `pub struct <Entity>RepoLoro`
+ trait forward impl (~30 lines). The codec part dominates.

Today's count: **two entities × ~110 lines each = 220 lines**.
Add Cycle and Milestone back (or any new entity) and we're at
500+ lines of boilerplate that's mechanical from the struct
definition.

## The shape

```rust
// In features/project/project-crdt/src/task.rs

use project_proto::Task;  // has #[derive(Entity)]
use crdt::EntityCrdt;     // re-exports the derive

#[derive(EntityCrdt)]
#[entity(
    wire = Task,
    create = TaskCreate,
    update = TaskUpdate,
    list = TaskList,
    root = "tasks",
)]
pub struct TaskEntity;

// That's it. The derive emits:
//   impl EntityCrdt for TaskEntity { ... 80 lines ... }
//   pub struct TaskRepoLoro { inner: LoroRepo<TaskEntity> }
//   impl TaskRepoLoro { pub fn new(doc: &CrdtDoc) -> Self { ... } }
//   impl TaskRepo for TaskRepoLoro { ...forward to inner... }
```

The macro reads `Task`'s field list via either:

1. **Eager inspection** — the macro re-imports the wire struct
   in its expansion via `<Task as crudcrate::CrudModel>` or
   similar trait we add to `architect-derive`. The wire struct's
   fields would need to be exposed as a `const FIELDS: &[(&str,
   FieldKind)]` from the `Entity` derive.

2. **Lazy duplication** — declare fields again in the
   `#[entity(...)]` attribute:

   ```rust
   #[entity(
       wire = Task,
       fields(
           id: Uuid (pk),
           project_id: Uuid (filterable),
           title: String (filterable, sortable, fulltext),
           done: bool (filterable, sortable),
       )
   )]
   ```

   Ugly, redundant. Skip.

(1) is the right design but requires extending `architect-derive`
to emit a field manifest. About half the work.

## The codec table

For each known scalar type, the macro emits paired `read_X` / `write_X`
calls:

| Field type | Reader | Writer |
|---|---|---|
| `Uuid` | `read_uuid` | `write_uuid` |
| `Option<Uuid>` | `read_opt_uuid` | `write_opt_uuid` |
| `String` | `read_str` | `write_str` |
| `Option<String>` | `read_opt_str` | `write_opt_str` |
| `i64` | `read_i64` | `write_i64` |
| `Option<i64>` | `read_opt_i64` | `write_opt_i64` |
| `bool` | `read_bool` | `write_bool` |
| `DateTime<Utc>` | `read_dt` | `write_dt` |
| `Option<DateTime<Utc>>` | `read_opt_dt` | `write_opt_dt` |
| `Vec<String>` | `read_string_list` | `write_string_list` |
| `Option<Vec<String>>` | (none) | `write_opt_string_list` |

Unknown types → compile error pointing at the offending field
("EntityCrdt doesn't know how to codec `Foo`; add a Reborrow-impl
or use `Option<String>` if you need to wire it through").

## Implementation steps

1. **Extend `architect-derive`** to emit a `const FIELDS: &[FieldMeta]`
   per `#[derive(Entity)]` struct, where each `FieldMeta` carries
   name + type-name string + filterable/sortable/fulltext flags.
2. **New crate `crdt-derive`** in `architect/macros/crdt-derive/`
   (sibling to `architect-derive`). Single proc-macro
   `#[derive(EntityCrdt)]` that:
   - Parses the `#[entity(...)]` attribute on the marker struct.
   - Resolves the wire type's `FIELDS` const.
   - Switches each field type to its codec pair via a static
     table.
   - Emits the impl block + RepoLoro newtype + Repo forward impl.
3. **Re-export** from `crdt`: `pub use crdt_derive::EntityCrdt;`.
4. **Migrate** `features/project/project-crdt/src/{project,task}.rs`
   to use the derive. Net reduction: ~120 lines.

## Why defer

- Today's slice has 2 entities. The pain is real but bounded.
- The work splits into "architect-derive emits FIELDS" (real,
  upstream) and "new crdt-derive crate" (real, this repo's
  workspace). 2-3 hours of focused work.
- Higher-priority blockers right now: the realtime sync hardening
  is already paying off; the entity codec is mechanical typing,
  not a correctness gap.

Do this the moment we want to bring back any 2+ entity types
(Cycle, Milestone, Tag, …). The savings compound linearly: the
3rd entity is the break-even point on macro authorship cost.

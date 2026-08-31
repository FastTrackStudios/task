# `ui` — the Task app shell, and how to get code out of it

`crates/task/ui` (package `ui`) is the app shell: the router, the
chrome (top bar, rail, explorer, tabs, palette, shortcuts), auth and
org discovery, the presence/collab wiring, and the store table. It is
**not** meant to be where every feature's UI lives — it grew that way
because a page could not survive outside it.

This document is the recipe for moving a feature's UI out.

## The layering

```
                 ┌──────────────────────────────┐
                 │  ui  (this crate)            │  router · chrome · auth
                 │                              │  org discovery · stores
                 └───────┬──────────────┬───────┘
                         │              │
        ┌────────────────┘              └──────────────┐
        ▼                                              ▼
┌───────────────────────┐                  ┌───────────────────────────┐
│ features/task/<slice>/│                  │ crates/task/player-ui     │
│   <slice>-ui          │                  │ (the session player)      │
│  page + its RPCs      │                  └─────────────┬─────────────┘
└───────────┬───────────┘                                │
            │                                            │
            └──────────────┬─────────────────────────────┘
                           ▼
                 ┌──────────────────────┐
                 │  task-ui-core        │  vox · orgs · feeds! · format
                 │  (crates/task/ui-core)│  states · frontmatter · nav
                 └──────────────────────┘
```

`task-ui-core` is the seam. It deliberately depends on **no `*-proto`
crate** — adding one there would put it in every consumer's graph,
which is the coupling we are undoing. It gives a feature crate:

| module | what it replaces in the shell |
| --- | --- |
| `vox_clients` | `crate::vox_clients::establish_for` — the per-org cached connection root |
| `vox_session` | `crate::vox_session::vox_url` — endpoint resolution |
| `feeds` | `crate::feeds`'s `feeds!` macro + `fan_out` / `fan_out_tagged` / `collect` |
| `orgs` | `crate::orgs`'s `OrgMeta` / `OrgSelection` / `selected_slugs` (discovery stays in the shell) |
| `states` | `crate::states` — `LoadingState` / `ErrorState` / `EmptyState` |
| `format` | `crate::format` — money, playback clocks, status badges |
| `frontmatter` | the vault-note frontmatter readers |
| `nav` | `crate::routes::Route` — see "the one hard edge" below |

## The recipe

Worked examples: `features/task/links/links-ui` (smallest),
`features/task/scripture/scripture-ui` (largest, 750 lines + 12 RPCs).
Older props-only component crates — `task-ui`, `threads-ui`,
`scheduling-ui` — are the *dumb component* variant of the same idea;
these page crates additionally own their data access.

**1. Measure the boundary first.** For the page you want to move,
count what it reaches for and what reaches for it:

```bash
# outward: what does the page use from the shell?
rg -o 'crate::[a-z_]+' crates/task/ui/src/pages/<page>.rs | sort | uniq -c

# inward: does anything else in the shell use the page?
rg 'pages::<page>\b' crates/task/ui/src
```

A page is ready to move when the outward set is a subset of
{`orgs`, `feeds`, `states`, `format`, `vox_clients`} and the inward
count is zero. `chrome`, `stores`, `routes`, `document_session`,
`collab` and `pages::<other>` in that list mean more work — see
"what resists" below.

**2. Create the crate** at `features/task/<slice>/<slice>-ui`, named
`<slice>-ui`, and register the path dep in the root
`[workspace.dependencies]`. `features/task/*/*` is a glob member, so it
becomes a workspace member automatically. Depend on `dioxus`,
`architect-ui`, `task-ui-core`, and the slice's own `*-proto`.

**3. `git mv` the page** to `src/lib.rs` (or `src/<page>.rs` for a
multi-page slice) so git records a rename and the diff stays reviewable
as code motion.

**4. Take the slice's RPCs with it.** Move the feature's entries out of
the shell's `feeds.rs` into the new crate and declare them with
`task_ui_core::feeds!`. This is the step that actually removes weight:
the shell drops the slice's `*-proto` dependency. If the shell still
calls one of them (`ui::vault_lookup` needs
`scripture_ui::fetch_comparison`), keep **one** definition in the
feature crate and have the shell call through it — never copy.

**5. Rewrite the paths**: `crate::orgs::…` → `task_ui_core::orgs::…`,
`crate::states::…` → `task_ui_core::states::…`, `crate::feeds::<fn>` →
the local `<fn>`, and so on.

**6. Give it a way in.** Two options, and the second is preferred for
anything that is an *app* rather than part of Task itself:

*a. A route in the shell* — `routes.rs` keeps the `Route` variant and
its wrapper component; only the body changes. Right for core surfaces
(vault, tasks, calendar) that every build has.

*b. A plugin* — the feature crate gets a thin `apps/plugins/<name>`
wrapper exporting a `PluginApp`, added to the `task-plugins` bundle.
The shell then has no route, no nav entry and no dependency: it reaches
the screen through `/app/<id>`, and the app claims its own wikilinks
and URL schemes. Scripture is the worked migration — see
`apps/plugins/scripture`.

Either way, drop the module from `pages/mod.rs` and the slice's
now-unused deps from `crates/ui/Cargo.toml`.

**7. Verify** — all three, every time:

```bash
cargo check -p ui
cargo check -p task-app-web --target wasm32-unknown-unknown
cargo test  -p ui --lib
```

## The one hard edge: routing

A feature crate must not name `ui::routes::Route` — a route variant
drags in every other feature's route parameters, and the dependency
would be circular anyway. But pages do need to link out.

So the shell hands href builders down as contexts
(`task_ui_core::nav`). The shell renders the typed route to a URL; the
feature crate only renders the anchor:

```rust
// shell (chrome.rs, with the other contexts):
use_context_provider(|| {
    NoteHref(Callback::new(|path: String| {
        Route::VaultRoute { path, org: String::new() }.to_string()
    }))
});

// feature crate:
let note_href = use_note_href();
rsx! { Link { to: note_href(path.clone()), "{title}" } }
```

Add a builder to `task_ui_core::nav` when the next slice needs a
different destination. Each one has a fallback for isolated previews,
so a feature crate never has to handle a missing context.

## What resists extraction, and why

These are measured, not guessed — the counts are `crate::…` references
out of each page.

- **`pages/vault.rs`** (1612 lines, 20 inbound references) — the vault
  page is the shell's centre of gravity. `palette`, `vault_lookup`,
  `shortcuts`, `explorer`, `note_view` and `note_header` all reach into
  it for `SongFront`, `slugify` and the note-loading helpers. The
  frontmatter half already moved to `task-ui-core`; the rest needs the
  explorer/tab/document-session wiring untangled first.
- **`pages/note_view.rs`** (44 outward references across 13 modules) —
  it composes vault, note_properties, experience, chrome contexts,
  collab and the player. It is the shell's compositor, not a feature.
- **`pages/project_detail.rs`, `pages/tasks.rs`, `pages/projects.rs`,
  `pages/timer.rs`, `pages/inbox.rs`** — all bound to `crate::stores`.
  The `stores!` table is app-root state, provided once in `app.rs` and
  shared across pages (the project store resolves `[[Project]]`
  wikilinks typed into the *task* quick-add). Extracting these means
  deciding whether a store belongs to a slice or to the shell, one
  entity at a time.
- **`pages/timer.rs`** (7 `crate::chrome` references) — it drives the
  status bar and the paused-timer hint. That is chrome state, so the
  page and the chrome would have to swap a context type first, the way
  `NowPlaying` moved out with the player.
- **`pages/watch.rs`, `pages/wiki*.rs`, `pages/repos.rs`** — clean
  enough to move next (5–7 outward references, 0–1 inbound); they were
  left because `routes::Route` appears in each and each wants a
  different `nav` builder. That is a small, mechanical follow-up.

## Note widgets: the other way out

Pages leave the shell through the recipe above; **note embeds** leave it
through the widget registry. `crates/task/widgets` (`task-widgets`) is
the contract: provider crates expose `widgets() -> Vec<WidgetSpec>`
(keyed by note type / `experience:` / frontmatter flag / embed target
type), the app root registers them (`app.rs` — the one place providers
are named), and `note_view` mounts/dispatches through the registry
without knowing who provided what. The player's song/setlist embeds and
the section tabs (`crates/task/note-tabs`) already live behind it — see
`crates/task/widgets/README.md` for the authoring contract and match
precedence. Widgets are one contribution type of the plugin system
(`task-plugin`); each spec names its owning plugin id.

## Also here

- `crates/task/player-ui` — the browser session player (Web Audio, the
  engraved chart pane, the Now Playing engine, the in-tab
  daw-standalone session engine). It lives under `crates/task/` rather
  than `crates/session/` because it parses `type: song` vault
  frontmatter and dials `/org/<slug>/vox`; putting it under
  `crates/session/` would make the session domain depend on the Task
  app. See that crate's module docs.
- `feeds.rs` and `stores.rs` are declaration tables, not lists of
  functions. Adding a feed is a `feeds!` entry; adding a store is a
  `stores!` row. Both have a "The shape" note above the macro.

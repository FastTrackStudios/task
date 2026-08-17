# task-widgets — how a plugin contributes note widgets

The Task shell renders vault notes; **plugins** teach it what a
`type: setlist` note *is* without the shell ever naming them. This
crate is the contract between the two: a provider crate builds
`WidgetSpec`s, the app root registers them, and the shell's note view
mounts whatever the `WidgetRegistry` says claims the open note.

Widgets are one contribution type of the Task plugin system (see
`task-plugin` for the catalog
for the design). A spec names its owning plugin id
(`.plugin("fasttrackstudio")`) so the registry can later drop
contributions from plugins an org has disabled — today that field is a
hook only.

## Writing a provider

A provider is an ordinary crate that exposes **one** function:

```rust
use task_widgets::{WidgetMatch, WidgetSpec};

pub fn widgets() -> Vec<WidgetSpec> {
    vec![
        WidgetSpec::new("myplugin.recipe", vec![WidgetMatch::NoteType("recipe")])
            .render(|ctx| rsx! { RecipeWidget { ctx } })
            .on_href(recipe_href)          // optional
            .decorations(recipe_pass)      // optional
            .hide_note_header()            // optional flag
            .fullscreen_owns_body()        // optional flag
            .plugin("myplugin"),
    ]
}
```

and gets registered at the app root (`ui/src/app.rs`) — the **only**
place the shell names a provider:

```rust
registry.register(task_player_ui::widgets());
registry.register(task_note_tabs::widgets());
```

That's the whole integration. No linker magic, no `inventory`: explicit
registration is debuggable, wasm-safe, and the app-root list *is* the
widget roster.

**What a provider may depend on:** `task-widgets` (this contract),
`task-ui-core` (org/vox/format/frontmatter seam), its own `*-proto`
crates, `editor` (for decoration passes), `architect-ui`, `dioxus`. **Never
the shell (`ui`)** — a provider cannot name `ui::routes::Route` or any
shell module; navigation goes through the callbacks in `WidgetCtx`
(`open_note`, `note_href` — the `task_ui_core::nav` pattern), and RPC
goes through `task_ui_core::vox_clients::establish_for` with
`ctx.org`. Widgets must render on all three platforms: no
`document::Stylesheet { href }`, styles via Tailwind classes already in
the app's `@source` globs (the globs cover `crates/task/*/src`) or
inline.

## What a spec can contribute

| capability | when it runs |
| --- | --- |
| `render(fn(WidgetCtx) -> Element)` | mounted by the note view above the note body when this spec is the first claimant with a render fn |
| `on_href(fn(&str, &WidgetCtx) -> bool)` | editor link clicks (`data-href` from decorations), dispatched at click time |
| `decorations(fn(&EditorState) -> Vec<DecoratedRange>)` | inside the note editor's decoration source, every editor render |
| `hide_note_header` / `fullscreen_owns_body` | flags the shell reads (OR-ed across all claimants) |

`WidgetCtx` is the whole world a widget sees: org slug, note path +
title, what matched (`WidgetTarget`), the shared `fullscreen` signal,
the nav callbacks, a lazy `doc()` reader (the live editor buffer), and
a `resolve(target)` closure into the host's vault index (a content
miss queues the lazy fetch; ask again when the host re-renders).

## Match semantics and precedence

`WidgetMatch` covers, most specific first:

1. `NoteType("setlist")` — the open note's frontmatter `type:` as the
   folder index resolved it (host-normalized: lowercased).
2. `NoteExperience("setlist")` — the note's own `experience:` key, an
   explicit opt-in on any note.
3. `NoteFlag("tabs")` — a truthy frontmatter key (`tabs: true`).
4. `EmbedType("song")` — a standalone `[[wikilink]]`/`![[embed]]` line
   whose resolved target has that `type:` (exact, as written in the
   target's frontmatter).
5. `FencedLanguage("chart")` — reserved: the variant exists so the
   contract doesn't churn when fenced-block widgets land; the shell
   does not consult it yet.

A spec may carry several matches; its rank is its best claim.

**Two claims on the same target:** most specific wins; ties break by
registration order, which the app root controls. Only ONE spec renders
(the first claimant with a render fn), so providers can't stack
conflicting views; the boolean flags OR across *all* claimants, so a
note that is both an event and a setlist keeps both behaviors. A
duplicate spec **id** is a wiring bug: the registry keeps the first and
warns.

**Href dispatch** (`handle_href`) is click-time, ordered: note
claimants first, then embed claimants in body order, then every
remaining handler as a fallback. The fallback exists because href
schemes travel with *editor decorations*, which gate on the live
document — that can outrun the folder index the note-type match reads,
and a rendered widget's click must never die in that window. Handlers
therefore MUST self-gate on their own scheme prefix (`song-play:`,
`event-tab:`, …) and return `false` otherwise; the scheme namespace is
effectively global and matching only orders the candidates. Pick
prefixes that carry your plugin's vocabulary.

**Decoration passes** are registry-wide, not match-gated: they are
plain `fn`s composed once into the note's `DecorationSource` (whose
equality contract — Rc identity, create once in `use_hook` — forbids
rebuilding per match change), so each pass self-gates on the state it
is handed and reads signals for anything that should re-render the
editor. Interactive content inside a replace-decoration widget must set
`dataset.widgetFocused` handling exactly as the editor's built-in
widgets do — focus inside a widget suspends editor key handling.

## The proof providers

- `task-player-ui::widgets()` (`plugin: fasttrackstudio`) —
  `player.song` (`type: song` → auto-fullscreen player / compact song
  card), `player.setlist` (`type: setlist` / `experience: setlist` →
  fullscreen `SetlistPlayer`; the embedded view is the editor's own
  setlist-title + song-strip decorations), `player.embed` (song/setlist
  wikilink embeds → the `song-play:`/`setlist-play:`/`setlist-open:`/
  `song-more:` hrefs, queue resolved at click time).
- `task-note-tabs::widgets()` (`plugin: core`) — section tabs for
  `type: event` / `tabs: true` notes: a decoration pass + the
  `event-tab:` href, plus an event-only header-suppression spec.

## Roadmap

- **Fenced-block languages**: the shell scans fences and mounts
  `FencedLanguage` claimants as inline replace-widgets (the
  `WidgetTarget::Fence` arm is already in the contract).
- **Frontmatter-driven params**: pass the matched note's parsed
  frontmatter map through `WidgetCtx` so widgets stop re-parsing.
- **Plugin gating**: `WidgetRegistry` learns the org's `PluginSet` and
  drops specs whose `plugin` is disabled (the id hook is in place).

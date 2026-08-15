//! `task-widgets` — the note-widget extension contract.
//!
//! External crates provide widgets for vault notes — keyed by **note
//! type** (`type: setlist` frontmatter), **embed target** (a standalone
//! `[[wikilink]]` / `![[embed]]` whose resolved target has a given
//! type), or **fenced-block language** — and the shell renders them
//! without naming the provider. The shell's note view:
//!
//! 1. builds a [`WidgetCtx`] for the open note (resolved path, org,
//!    lazy doc/content access, the nav callbacks),
//! 2. asks the [`WidgetRegistry`] which specs claim the note
//!    ([`WidgetRegistry::note_matches`]) and mounts the first claimant's
//!    [`WidgetSpec::render`],
//! 3. composes every spec's [`WidgetSpec::decorations`] pass into its
//!    editor decoration source, and
//! 4. forwards editor link clicks through
//!    [`WidgetRegistry::handle_href`] before its own wikilink fallback.
//!
//! Registration is **explicit**: each provider crate exposes one
//! `widgets() -> Vec<WidgetSpec>` fn, and the app root builds the
//! registry and calls [`WidgetRegistry::register`] with each provider's
//! specs — no linker magic, no `inventory`, wasm-safe, greppable. That
//! app-root call is the only place the shell may name a provider crate.
//! Widgets are one contribution type of the Task **plugin** system: a
//! spec can carry its owning plugin's catalog id
//! ([`WidgetSpec::plugin`]) so the registry can later gate providers on
//! an org's enabled plugin set.
//!
//! ## Match semantics and precedence
//!
//! For the note-facing capabilities (render + flags), the claim ranks
//! are, most specific first:
//!
//! 1. [`WidgetMatch::NoteType`] — the folder index's frontmatter
//!    `type:` (host-normalized: lowercased).
//! 2. [`WidgetMatch::NoteExperience`] — the note's own `experience:`
//!    frontmatter key (an explicit opt-in on any note).
//! 3. [`WidgetMatch::NoteFlag`] — a truthy frontmatter key
//!    (`tabs: true`).
//!
//! Ties break by registration order (the app root controls it). One
//! spec **renders** — the first claimant with a render fn; boolean
//! flags ([`WidgetSpec::hide_note_header`],
//! [`WidgetSpec::fullscreen_owns_body`]) OR across *all* claimants, so
//! a note that is both an event and a setlist keeps both behaviors.
//!
//! Href dispatch ([`WidgetRegistry::handle_href`]) runs at click time:
//! note-claimants first, then embed-claimants in document order of the
//! note's standalone wikilinks, then every remaining handler as a
//! fallback. The fallback exists because href schemes travel with
//! *editor decorations*, which gate on the live document — that can
//! outrun the folder index the note-type match reads, and a widget's
//! click must never die in that window. Handlers therefore MUST
//! self-gate on their own href scheme (`song-play:`, `event-tab:`, …)
//! and return `false` for anything else; the scheme namespace is
//! effectively global, matching only orders the candidates.
//!
//! ## Equality / re-render contract
//!
//! [`WidgetCtx`] is rebuilt by the note view every render; its `doc` /
//! `resolve` closures read live signals, so a click-time call through a
//! render-stale ctx still sees current state. `PartialEq` compares the
//! identity fields plus closure identity — a rebuilt ctx compares
//! unequal, so widget components re-render with their host note (the
//! same cadence the hand-wired embeds had). Decoration passes are plain
//! `fn` pointers composed ONCE into the host's decoration source
//! (created in `use_hook`) — that source's equality is `Rc` identity,
//! so passes must be stateless-per-call and self-gating; per-note state
//! belongs in signals the pass reads (see `editor-view`'s
//! `DecorationSource` docs for why rebuilding the source forces editor
//! re-renders).

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use editor::DecoratedRange;
use editor::state::EditorState;

mod fullscreen;
pub use fullscreen::{EnterExperienceButton, FullscreenExperience};

/// A decoration pass a widget contributes to the note editor. Plain
/// `fn` so identity is comparable and the pass can be deduped; it runs
/// inside the host's decoration source on every editor render and must
/// self-gate (return `[]` for notes it doesn't apply to). Reading a
/// signal (e.g. an active-tab `GlobalSignal`) inside the pass is what
/// re-renders the editor when that state changes.
pub type DecorationPass = fn(&EditorState) -> Vec<DecoratedRange>;

/// What a [`WidgetSpec`] claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetMatch {
    /// The open note's frontmatter `type:` as resolved by the host's
    /// folder index (lowercased). `NoteType("setlist")` claims a
    /// `type: setlist` page.
    NoteType(&'static str),
    /// The open note's own `experience:` frontmatter key (trimmed,
    /// quotes stripped) — an explicit per-note opt-in that works on any
    /// type (`experience: setlist` on an event note).
    NoteExperience(&'static str),
    /// A truthy frontmatter key on the open note: `<key>: true`
    /// (`NoteFlag("tabs")` claims `tabs: true` notes).
    NoteFlag(&'static str),
    /// A standalone `[[wikilink]]` / `![[embed]]` line in the note body
    /// whose resolved target's frontmatter `type:` equals this value
    /// (exact, as written in the target's frontmatter).
    EmbedType(&'static str),
    /// A fenced code block with this language tag (```` ```chart ````).
    /// Reserved: designed now, not yet consulted by the shell.
    FencedLanguage(&'static str),
}

/// What actually matched, from the widget's point of view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WidgetTarget {
    /// The open note itself (its resolved `type:`, if the index knows it).
    Note { note_type: Option<String> },
    /// An embedded target: the raw wikilink target text, the resolved
    /// vault path, and the target's frontmatter `type:`.
    Embed {
        target: String,
        path: String,
        target_type: String,
    },
    /// A fenced block (reserved with [`WidgetMatch::FencedLanguage`]).
    Fence { language: String },
}

/// A wikilink target resolved through the host's vault index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// Vault-relative path of the target note.
    pub path: String,
    /// The target's frontmatter `type:` (trimmed, quotes stripped) —
    /// `None` until the content is cached, or when it has no type.
    pub note_type: Option<String>,
    /// The target's raw content. `None` while the host's lazy fetch is
    /// in flight — asking for it *queues* the fetch, and the host
    /// re-renders when it lands, so treat `None` as "soon".
    pub content: Option<String>,
}

/// Everything a widget gets from the shell — deliberately no shell
/// internals. Widgets needing RPC establish their own clients via
/// `task_ui_core::vox_clients::establish_for` (they know the org from
/// [`WidgetCtx::org`]).
#[derive(Clone)]
pub struct WidgetCtx {
    /// Org slug the vault lives under.
    pub org: String,
    /// Vault-relative path of the open (host) note.
    pub path: String,
    /// Display title of the host note (path basename).
    pub title: String,
    /// What matched — the note itself, or an embedded target.
    pub target: WidgetTarget,
    /// The note view's fullscreen-experience state. A widget whose spec
    /// sets [`WidgetSpec::fullscreen_owns_body`] renders its immersive
    /// view while this is `true`; the host hides the note-body editor
    /// underneath (its autosave sink must not catch the experience's
    /// keystrokes).
    pub fullscreen: Signal<bool>,
    /// Navigate the shell to a vault note by path (route push).
    pub open_note: Callback<String>,
    /// Render a vault-note path to an href (for anchors) — the
    /// `task_ui_core::nav` builder.
    pub note_href: Callback<String, String>,
    /// The host note's live text (the open editor buffer, not the last
    /// saved file). Non-reactive read — call it when you need it.
    pub doc: Rc<dyn Fn() -> String>,
    /// Resolve a wikilink target name through the host's vault index.
    /// A content miss queues the lazy fetch (see [`ResolvedTarget`]).
    pub resolve: Rc<dyn Fn(&str) -> Option<ResolvedTarget>>,
}

impl WidgetCtx {
    fn with_target(&self, target: WidgetTarget) -> Self {
        Self {
            target,
            ..self.clone()
        }
    }
}

impl PartialEq for WidgetCtx {
    /// Identity fields + closure identity. The host rebuilds the ctx
    /// (fresh closures) every render, so widget components re-render
    /// with their host note — the same cadence inline rsx had.
    fn eq(&self, other: &Self) -> bool {
        self.org == other.org
            && self.path == other.path
            && self.title == other.title
            && self.target == other.target
            && std::ptr::addr_eq(Rc::as_ptr(&self.doc), Rc::as_ptr(&other.doc))
            && std::ptr::addr_eq(Rc::as_ptr(&self.resolve), Rc::as_ptr(&other.resolve))
    }
}

/// One widget: what it claims and what it contributes. Build with
/// [`WidgetSpec::new`] + the builder methods; every capability is
/// optional, so a spec can be decorations-only (section tabs), href-only
/// (an embed's play buttons), or a full note-type view (the player).
#[derive(Clone)]
pub struct WidgetSpec {
    /// Stable, unique id (`"player.setlist"`). Duplicate registrations
    /// are dropped with a warning; the id also dedups a spec that
    /// matches a note both directly and via an embed.
    pub id: &'static str,
    /// The claims. A spec may carry several (e.g. `NoteType("setlist")`
    /// + `NoteExperience("setlist")`); its rank is its best claim.
    pub matches: Vec<WidgetMatch>,
    /// Block UI the note view mounts (above the note body) when this
    /// spec is the first claimant with a render fn.
    pub render: Option<Rc<dyn Fn(WidgetCtx) -> Element>>,
    /// Editor link-click handler for the spec's own href schemes.
    /// MUST self-gate on the scheme prefix and return `false` for
    /// anything else — see the crate docs on dispatch order.
    pub on_href: Option<Rc<dyn Fn(&str, &WidgetCtx) -> bool>>,
    /// A decoration pass composed into the host's editor decoration
    /// source (see [`DecorationPass`]).
    pub decorations: Option<DecorationPass>,
    /// Suppress the shell's note header for claimed notes (the widget
    /// renders its own title, e.g. the editor's setlist-title widget).
    pub hide_note_header: bool,
    /// While [`WidgetCtx::fullscreen`] is set, this widget's experience
    /// owns the screen and the host must unmount the note-body editor
    /// (autosave-sink protection).
    pub fullscreen_owns_body: bool,
    /// The owning plugin's id from the `task-plugin` catalog
    /// (`"fasttrackstudio"`, `"scripture"`, …). The registry drops
    /// specs whose plugin is disabled for the active org (see
    /// [`WidgetRegistry::set_plugin_set`]). `None` (or `"core"`) means
    /// always available.
    pub plugin: Option<&'static str>,
}

impl WidgetSpec {
    /// A spec with the given claims and no capabilities; chain the
    /// builder methods for what the widget actually contributes.
    #[must_use]
    pub fn new(id: &'static str, matches: Vec<WidgetMatch>) -> Self {
        Self {
            id,
            matches,
            render: None,
            on_href: None,
            decorations: None,
            hide_note_header: false,
            fullscreen_owns_body: false,
            plugin: None,
        }
    }

    /// Set the block render fn.
    #[must_use]
    pub fn render(mut self, f: impl Fn(WidgetCtx) -> Element + 'static) -> Self {
        self.render = Some(Rc::new(f));
        self
    }

    /// Set the href handler.
    #[must_use]
    pub fn on_href(mut self, f: impl Fn(&str, &WidgetCtx) -> bool + 'static) -> Self {
        self.on_href = Some(Rc::new(f));
        self
    }

    /// Set the decoration pass.
    #[must_use]
    pub fn decorations(mut self, f: DecorationPass) -> Self {
        self.decorations = Some(f);
        self
    }

    /// Claimed notes render without the shell's note header.
    #[must_use]
    pub fn hide_note_header(mut self) -> Self {
        self.hide_note_header = true;
        self
    }

    /// The widget's fullscreen experience owns the screen (the host
    /// unmounts the note body while fullscreen).
    #[must_use]
    pub fn fullscreen_owns_body(mut self) -> Self {
        self.fullscreen_owns_body = true;
        self
    }

    /// Name the owning plugin (a `task-plugin` catalog id) so the
    /// registry can later gate this spec on the org's enabled set.
    #[must_use]
    pub fn plugin(mut self, id: &'static str) -> Self {
        self.plugin = Some(id);
        self
    }

    fn try_href(&self, href: &str, ctx: &WidgetCtx) -> bool {
        self.on_href.as_ref().is_some_and(|h| h(href, ctx))
    }
}

/// The registry the app root fills and provides as Dioxus context.
/// Cheap to clone (shared inner). Registration happens once, at the
/// root, before any note renders; the *active* set additionally
/// filters on the org's enabled plugins (see [`Self::set_plugin_set`])
/// at read time, so an org switch retunes the same registry.
#[derive(Clone)]
pub struct WidgetRegistry {
    specs: Rc<RefCell<Vec<WidgetSpec>>>,
    /// The active org's enabled plugins. Defaults to "everything on";
    /// the shell mirrors the discovered org state into it.
    plugins: Rc<RefCell<task_plugin::PluginSet>>,
}

impl Default for WidgetRegistry {
    fn default() -> Self {
        Self {
            specs: Rc::default(),
            plugins: Rc::new(RefCell::new(task_plugin::PluginSet::resolve(None))),
        }
    }
}

impl WidgetRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a provider's specs. A duplicate id keeps the first
    /// registration and warns — explicit registration means a dup is a
    /// wiring bug at the app root, not something to resolve silently.
    pub fn register(&self, specs: Vec<WidgetSpec>) {
        let mut inner = self.specs.borrow_mut();
        for spec in specs {
            if inner.iter().any(|s| s.id == spec.id) {
                tracing::warn!(
                    "widget spec `{}` registered twice; keeping the first",
                    spec.id
                );
                continue;
            }
            inner.push(spec);
        }
    }

    /// Point the registry at the ACTIVE org's enabled plugin set. Specs
    /// whose [`WidgetSpec::plugin`] is not in the set stop matching,
    /// decorating, and handling hrefs until the set changes back —
    /// registration is untouched, so this is cheap to flip on every org
    /// switch.
    pub fn set_plugin_set(&self, set: task_plugin::PluginSet) {
        *self.plugins.borrow_mut() = set;
    }

    /// The registered specs whose owning plugin is enabled for the
    /// active org (`plugin: None` is always available).
    fn active_specs(&self) -> Vec<WidgetSpec> {
        let plugins = self.plugins.borrow();
        self.specs
            .borrow()
            .iter()
            .filter(|s| s.plugin.is_none_or(|p| plugins.contains(p)))
            .cloned()
            .collect()
    }

    /// The specs claiming the open note, most specific claim first
    /// (`NoteType` > `NoteExperience` > `NoteFlag`, then registration
    /// order). The first entry with a render fn is the note's widget
    /// view; boolean flags OR across all entries.
    #[must_use]
    pub fn note_matches(&self, ctx: &WidgetCtx) -> Vec<WidgetSpec> {
        let note_type = match &ctx.target {
            WidgetTarget::Note { note_type } => note_type.clone(),
            _ => None,
        };
        let doc = (ctx.doc)();
        let specs = self.active_specs();
        note_matches_inner(&specs, note_type.as_deref(), &doc)
    }

    /// Every enabled decoration pass, deduped by fn identity (a
    /// provider may attach the same pass to several specs).
    #[must_use]
    pub fn decoration_passes(&self) -> Vec<DecorationPass> {
        let mut out: Vec<DecorationPass> = Vec::new();
        for spec in &self.active_specs() {
            if let Some(p) = spec.decorations {
                if !out.iter().any(|q| std::ptr::fn_addr_eq(*q, p)) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Click-time href dispatch: note-claimants first, then
    /// embed-claimants in body order, then every remaining handler (see
    /// the crate docs for why the fallback exists). Returns `true` when
    /// a widget consumed the href; the host then skips its own link
    /// handling.
    #[must_use]
    pub fn handle_href(&self, href: &str, ctx: &WidgetCtx) -> bool {
        let mut tried: Vec<&'static str> = Vec::new();
        // 1. Specs claiming the note itself.
        for spec in self.note_matches(ctx) {
            tried.push(spec.id);
            if spec.try_href(href, ctx) {
                return true;
            }
        }
        // 2. Specs claiming an embedded target, in document order.
        let doc = (ctx.doc)();
        let specs = self.active_specs();
        for target in standalone_wikilink_targets(&doc) {
            let Some(resolved) = (ctx.resolve)(&target) else {
                continue;
            };
            let Some(target_type) = resolved.note_type.clone() else {
                continue;
            };
            for spec in embed_matches_inner(&specs, &target_type) {
                if tried.contains(&spec.id) {
                    continue;
                }
                tried.push(spec.id);
                let ectx = ctx.with_target(WidgetTarget::Embed {
                    target: target.clone(),
                    path: resolved.path.clone(),
                    target_type: target_type.clone(),
                });
                if spec.try_href(href, &ectx) {
                    return true;
                }
            }
        }
        // 3. Fallback: any remaining handler (self-gating by scheme).
        for spec in &specs {
            if tried.contains(&spec.id) {
                continue;
            }
            if spec.try_href(href, ctx) {
                return true;
            }
        }
        false
    }
}

/// Note-claim ranking (see [`WidgetRegistry::note_matches`]).
fn note_matches_inner(specs: &[WidgetSpec], note_type: Option<&str>, doc: &str) -> Vec<WidgetSpec> {
    let experience = frontmatter_scalar(doc, "experience");
    let mut ranked: Vec<(u8, usize)> = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let mut best: Option<u8> = None;
        for m in &spec.matches {
            let rank = match m {
                WidgetMatch::NoteType(t) => (note_type == Some(*t)).then_some(0u8),
                WidgetMatch::NoteExperience(e) => (experience.as_deref() == Some(*e)).then_some(1),
                WidgetMatch::NoteFlag(k) => {
                    (frontmatter_scalar(doc, k).as_deref() == Some("true")).then_some(2)
                }
                WidgetMatch::EmbedType(_) | WidgetMatch::FencedLanguage(_) => None,
            };
            if let Some(r) = rank {
                best = Some(best.map_or(r, |b| b.min(r)));
            }
        }
        if let Some(r) = best {
            ranked.push((r, i));
        }
    }
    ranked.sort_by_key(|&(r, i)| (r, i));
    ranked.into_iter().map(|(_, i)| specs[i].clone()).collect()
}

/// Specs with an [`WidgetMatch::EmbedType`] claim on `target_type`, in
/// registration order.
fn embed_matches_inner(specs: &[WidgetSpec], target_type: &str) -> Vec<WidgetSpec> {
    specs
        .iter()
        .filter(|s| {
            s.matches
                .iter()
                .any(|m| matches!(m, WidgetMatch::EmbedType(t) if *t == target_type))
        })
        .cloned()
        .collect()
}

/// A frontmatter scalar, trimmed with quotes stripped — the
/// normalization every `type:`-adjacent comparison in the vault uses.
fn frontmatter_scalar(doc: &str, key: &str) -> Option<String> {
    task_ui_core::frontmatter::frontmatter_value(doc, key)
        .map(|v| v.trim().trim_matches(['"', '\'']).trim().to_owned())
}

/// Standalone-wikilink embed targets of a note body, in document order:
/// one `[[Target]]` / `![[Target]]` per line (plain or as a list item),
/// alias/section suffixes stripped, folder qualifiers dropped (vault
/// resolution is by basename), deduped.
fn standalone_wikilink_targets(text: &str) -> Vec<String> {
    let body = text
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---").map(|(_, b)| b))
        .unwrap_or(text);
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        let t = t
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['-', '*', '.', ')'])
            .trim_start();
        let t = t.strip_prefix('!').unwrap_or(t);
        let Some(inner) = t.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")) else {
            continue;
        };
        let target = inner.split(['|', '#']).next().unwrap_or(inner).trim();
        let target = target.rsplit('/').next().unwrap_or(target).trim();
        if !target.is_empty() && !out.iter().any(|s: &String| s == target) {
            out.push(target.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &'static str, matches: Vec<WidgetMatch>) -> WidgetSpec {
        WidgetSpec::new(id, matches)
    }

    #[test]
    fn note_match_precedence_type_over_experience_over_flag() {
        let specs = vec![
            spec("flag", vec![WidgetMatch::NoteFlag("tabs")]),
            spec("exp", vec![WidgetMatch::NoteExperience("setlist")]),
            spec("typed", vec![WidgetMatch::NoteType("event")]),
        ];
        let doc = "---\ntype: event\nexperience: setlist\ntabs: true\n---\nbody\n";
        let hits = note_matches_inner(&specs, Some("event"), doc);
        let ids: Vec<_> = hits.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["typed", "exp", "flag"]);
    }

    #[test]
    fn note_match_ties_break_by_registration_order() {
        let specs = vec![
            spec("first", vec![WidgetMatch::NoteType("song")]),
            spec("second", vec![WidgetMatch::NoteType("song")]),
        ];
        let hits = note_matches_inner(&specs, Some("song"), "no frontmatter");
        let ids: Vec<_> = hits.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["first", "second"]);
    }

    #[test]
    fn spec_rank_is_its_best_claim() {
        // A spec matching via both NoteFlag and NoteType ranks as NoteType.
        let specs = vec![
            spec("exp-only", vec![WidgetMatch::NoteExperience("setlist")]),
            spec(
                "both",
                vec![
                    WidgetMatch::NoteFlag("tabs"),
                    WidgetMatch::NoteType("event"),
                ],
            ),
        ];
        let doc = "---\ntype: event\nexperience: setlist\ntabs: true\n---\n";
        let hits = note_matches_inner(&specs, Some("event"), doc);
        assert_eq!(hits[0].id, "both");
    }

    #[test]
    fn embed_type_claims_are_exact_and_ordered() {
        let specs = vec![
            spec("player", vec![WidgetMatch::EmbedType("setlist")]),
            spec("other", vec![WidgetMatch::EmbedType("setlist")]),
            spec("song", vec![WidgetMatch::EmbedType("song")]),
        ];
        let ids: Vec<_> = embed_matches_inner(&specs, "setlist")
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["player", "other"]);
        // Case-sensitive: the target's frontmatter value as written.
        assert!(embed_matches_inner(&specs, "Setlist").is_empty());
    }

    #[test]
    fn wikilink_targets_standalone_lines_only() {
        let doc = "---\ntype: event\n---\n# Title\n- [[Songs/Praise|P]]\n1. ![[Sunday Setlist#top]]\nprose with [[Inline]] stays out\n[[Praise]]\n";
        assert_eq!(
            standalone_wikilink_targets(doc),
            vec!["Praise".to_owned(), "Sunday Setlist".to_owned()]
        );
    }

    #[test]
    fn duplicate_registration_keeps_the_first() {
        let reg = WidgetRegistry::new();
        reg.register(vec![spec("a", vec![]).hide_note_header()]);
        reg.register(vec![spec("a", vec![])]);
        let inner = reg.specs.borrow();
        assert_eq!(inner.len(), 1);
        assert!(inner[0].hide_note_header, "first registration wins");
    }

    #[test]
    fn disabled_plugin_specs_stop_matching_and_decorating() {
        fn pass(_: &EditorState) -> Vec<DecoratedRange> {
            Vec::new()
        }
        let reg = WidgetRegistry::new();
        reg.register(vec![
            spec("meal", vec![WidgetMatch::NoteType("recipe")])
                .decorations(pass)
                .plugin("mealplan"),
            spec("plain", vec![WidgetMatch::NoteType("recipe")]),
        ]);
        let specs = reg.active_specs();
        assert_eq!(specs.len(), 2, "everything on by default");
        assert_eq!(reg.decoration_passes().len(), 1);

        reg.set_plugin_set(task_plugin::PluginSet::resolve(Some(
            &task_plugin::PluginChoice::Disabled(vec!["mealplan".into()]),
        )));
        let specs = reg.active_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "plain", "un-owned spec survives");
        assert!(reg.decoration_passes().is_empty(), "gated pass dropped");

        // Flipping back re-enables without re-registration.
        reg.set_plugin_set(task_plugin::PluginSet::resolve(None));
        assert_eq!(reg.active_specs().len(), 2);
    }

    #[test]
    fn decoration_passes_dedup_by_fn_identity() {
        fn pass(_: &EditorState) -> Vec<DecoratedRange> {
            Vec::new()
        }
        let reg = WidgetRegistry::new();
        reg.register(vec![
            spec("a", vec![]).decorations(pass),
            spec("b", vec![]).decorations(pass),
        ]);
        assert_eq!(reg.decoration_passes().len(), 1);
    }
}

//! Client-side [`VaultLookup`] — cross-file wikilink / embed
//! resolution for the vault page's editor, plus the autocomplete
//! candidate providers.
//!
//! The server-side editor playground resolves `[[Page]]` /
//! `![[Page#Heading]]` / `((uuid))` against an in-memory
//! `vault_live::Vault`; in the browser there is no filesystem, so
//! this module builds the same capability from the wire:
//!
//! - **Page existence + identity** come from `folder_index`
//!   ([`vault_proto::PageMeta`]) — already fetched for the
//!   sidebar tree, so resolution of "does `[[X]]` point at a real
//!   page" is instant and offline.
//! - **Content** (embed previews, section bodies, block lines) is
//!   pulled lazily via `get_file` into a small LRU
//!   ([`CONTENT_CACHE_CAP`] entries). A cache miss renders a
//!   `Loading…` placeholder and schedules the fetch; completion
//!   pokes the editor state signal so the decoration pass re-runs
//!   with the content in cache.
//!
//! ## Editor hookup
//!
//! The Editor's stateful seams (branch `feat/architect-seams`)
//! carry the state through props:
//!
//! - [`vault_decoration_source`] builds a capturing
//!   `editor_view::DecorationSource` over the index signal — the
//!   live-preview pass runs with the vault lookup, plus
//!   bracket-pair highlighting (the vault-aware
//!   `editor::combined_decorations`).
//! - [`vault_completion_source`] builds the
//!   `editor_view::trigger::CompletionSource` answering the `[[`
//!   (wikilink) and `#` (tag) triggers from
//!   [`wikilink_candidates`] / the `VaultGraph` tags RPC.
//!
//! Both follow the sources' equality contract: create **once**
//! (`use_hook`) and capture `Signal`s, so the editor only
//! re-renders when the underlying data changes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use dioxus::prelude::*;
use editor::EditorState;
use editor::editor_view;
use editor::editor_view::trigger::{Candidate, CompletionKind};
use editor::markdown::{VaultBlockHit, VaultLookup, VaultPageHit};
use vault_proto::{PageMeta, TagCount};

/// Max lazily-fetched file bodies kept in memory. Small on
/// purpose — embeds on one open page rarely touch more than a
/// handful of other notes.
const CONTENT_CACHE_CAP: usize = 32;

/// Placeholder preview while a referenced page's content is in
/// flight.
const LOADING: &str = "Loading…";

/// Cache-key scheme for lazily-fetched scripture passage text —
/// `scripture://<TX>/<osis-range>`. Shares the content cache + fetch
/// worker with page content; the worker routes on the prefix.
const SCRIPTURE_SCHEME: &str = "scripture://";

/// Edition used for verse cards / chip tooltips when the link carries
/// no `@TX` qualifier. WEB is always bundled (public domain).
const DEFAULT_TRANSLATION: &str = "WEB";

/// Client-side vault index: page metadata from `folder_index` +
/// a lazy content LRU. Implements [`VaultLookup`] for the
/// editor's live-preview pass.
pub struct ClientVaultIndex {
    /// The page's editor state signal. Poked (no-op write) when a
    /// lazy fetch lands so the editor re-renders and the
    /// decoration pass re-resolves against the warm cache.
    state: Signal<EditorState>,
    /// Lowercased basename → page meta.
    by_basename: HashMap<String, PageMeta>,
    cache: RefCell<ContentCache>,
    /// Page-owned lazy-fetch worker ([`use_vault_fetch_worker`]).
    fetcher: Coroutine<String>,
}

struct ContentCache {
    map: HashMap<String, String>,
    /// Recency queue, front = least recent. Touched on hit.
    order: VecDeque<String>,
    /// Paths with a fetch in flight (dedup).
    pending: HashSet<String>,
}

impl ContentCache {
    fn touch(&mut self, path: &str) {
        if let Some(pos) = self.order.iter().position(|p| p == path) {
            self.order.remove(pos);
        }
        self.order.push_back(path.to_owned());
    }

    fn insert(&mut self, path: String, text: String) {
        self.map.insert(path.clone(), text);
        self.touch(&path);
        while self.map.len() > CONTENT_CACHE_CAP {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.map.remove(&oldest);
        }
    }
}

impl ClientVaultIndex {
    /// Build from the folder index. Rebuilt whenever the index
    /// refreshes (cheap — content cache starts cold). `fetcher` is
    /// the page-owned lazy-fetch worker (see
    /// [`use_vault_fetch_worker`]) — the index only *sends* paths
    /// to it; all awaiting and signal writes happen in the worker,
    /// inside the page's scope.
    #[must_use]
    pub fn new(
        pages: &[PageMeta],
        state: Signal<EditorState>,
        fetcher: Coroutine<String>,
    ) -> Rc<Self> {
        let by_basename = pages
            .iter()
            .map(|p| (p.basename.to_lowercase(), p.clone()))
            .collect();
        Rc::new(Self {
            state,
            by_basename,
            cache: RefCell::new(ContentCache {
                map: HashMap::new(),
                order: VecDeque::new(),
                pending: HashSet::new(),
            }),
            fetcher,
        })
    }

    /// Cached content for `path`, scheduling a background fetch
    /// on miss. Misses return `None` *now*; the completed fetch
    /// pokes the editor state so the next decoration pass hits.
    ///
    /// Runs inside the editor's render (the decoration pass), so
    /// it must not spawn or write signals here: it only enqueues
    /// the path on the page-owned worker. (The old shape used
    /// `spawn_forever` + wrote `state` from the root scope — a
    /// signal-ownership violation the multiplayer suite's console
    /// gate flags as fatal.)
    pub fn content(&self, path: &str) -> Option<String> {
        let mut cache = self.cache.borrow_mut();
        if let Some(text) = cache.map.get(path).cloned() {
            cache.touch(path);
            return Some(text);
        }
        if cache.pending.insert(path.to_owned()) {
            self.fetcher.send(path.to_owned());
        }
        None
    }

    /// Land a completed lazy fetch: clear the pending mark, warm
    /// the cache, and poke the editor state (no-op write) so the
    /// next decoration pass re-resolves. Called by the page-owned
    /// fetch worker — never from a render path.
    pub fn complete_fetch(&self, path: &str, fetched: Result<String, String>) {
        let mut cache = self.cache.borrow_mut();
        cache.pending.remove(path);
        if let Ok(text) = fetched {
            cache.insert(path.to_owned(), text);
            drop(cache);
            let mut state = self.state;
            state.write();
        }
    }

    pub fn meta(&self, name: &str) -> Option<&PageMeta> {
        self.by_basename.get(&name.to_lowercase())
    }
}

/// Page-owned worker draining the lazy-fetch requests that
/// [`ClientVaultIndex::content`] enqueues from the decoration
/// pass. Create once per page, BEFORE the index (the index
/// captures the returned handle). Owning the awaits + the editor
/// poke here keeps every signal touch inside the page's scope —
/// `content` runs inside the editor's render, where neither
/// spawning at the root nor writing page signals is allowed.
pub fn use_vault_fetch_worker(
    org: Memo<String>,
    vault_id: String,
    lookup: Signal<Option<Rc<ClientVaultIndex>>>,
) -> Coroutine<String> {
    use futures_util::StreamExt;
    use_coroutine(move |mut rx: dioxus::prelude::UnboundedReceiver<String>| {
        let vault_id = vault_id.clone();
        async move {
            while let Some(path) = rx.next().await {
                let slug = org.peek().clone();
                let vault_id = vault_id.clone();
                let fetched = if let Some(rest) = path.strip_prefix(SCRIPTURE_SCHEME) {
                    // `scripture://<TX>/<osis>` → passage text via the
                    // compare verb (handles single verses and ranges).
                    // Errors are cached as text (not Err) so a missing
                    // translation doesn't refetch on every decoration
                    // pass.
                    let (tx, osis) = rest.split_once('/').unwrap_or((DEFAULT_TRANSLATION, rest));
                    Ok(
                        match scripture_ui::fetch_comparison(&slug, osis, vec![tx.to_owned()]).await
                        {
                            Ok(cv) => {
                                let joined = cv
                                    .rows
                                    .iter()
                                    .filter_map(|r| r.cells.first())
                                    .map(String::as_str)
                                    .filter(|c| !c.is_empty())
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                if joined.is_empty() {
                                    format!("({tx}: passage not installed)")
                                } else {
                                    joined
                                }
                            }
                            Err(e) => format!("⚠ {e}"),
                        },
                    )
                } else {
                    crate::document_session::fetch_file(slug, vault_id, path.clone()).await
                };
                // The index may have been swapped (folder-index refresh)
                // mid-fetch; landing on the current one still warms the
                // right cache (path-keyed, same org family).
                let Some(idx) = lookup.peek().clone() else {
                    continue;
                };
                idx.complete_fetch(&path, fetched);
            }
        }
    })
}

impl VaultLookup for ClientVaultIndex {
    /// Full-uuid block lookup. The client never holds the whole
    /// vault, so this resolves only against already-cached
    /// content — a `((uuid))` pointing into a never-fetched page
    /// stays unresolved (degraded, documented limitation until a
    /// server-side block-lookup verb exists).
    fn lookup_block(&self, uuid: &str) -> Option<VaultBlockHit> {
        let cache = self.cache.borrow();
        for (path, text) in &cache.map {
            if let Some(pos) = text.find(uuid) {
                let line_start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
                return Some(VaultBlockHit {
                    page: basename_of(path).to_owned(),
                    preview: first_line(text, line_start),
                });
            }
        }
        None
    }

    fn lookup_page(&self, name: &str) -> Option<VaultPageHit> {
        let meta = self.meta(name)?;
        let preview = match self.content(&meta.path) {
            Some(raw) => preview_of_page(&raw, 200),
            None => LOADING.to_owned(),
        };
        Some(VaultPageHit { preview })
    }

    fn lookup_section(&self, page: &str, heading: &str) -> Option<String> {
        let meta = self.meta(page)?;
        match self.content(&meta.path) {
            Some(raw) => section_body(&raw, heading),
            None => Some(LOADING.to_owned()),
        }
    }

    fn lookup_song(&self, name: &str) -> Option<editor::markdown::VaultSongHit> {
        let meta = self.meta(name)?;
        // A miss queues the fetch; the strip appears when content lands.
        let raw = self.content(&meta.path)?;
        // Only `type: song` notes get the strip.
        let is_song = crate::pages::vault::frontmatter_value(&raw, "type")
            .map(|v| v.trim().trim_matches(['"', '\'']).trim() == "song")
            .unwrap_or(false);
        if !is_song {
            return None;
        }
        let front = crate::pages::vault::song_front_from(&raw);
        Some(editor::markdown::VaultSongHit {
            title: meta.basename.clone(),
            artist: front.artist.clone(),
            duration_sec: front
                .duration_sec
                .or_else(|| front.sections.last().map(|s| s.end_sec))
                .unwrap_or(0.0),
            stem_count: front.stems.len(),
        })
    }

    fn lookup_note_kind(&self, name: &str) -> Option<String> {
        let meta = self.meta(name)?;
        let raw = self.content(&meta.path)?;
        crate::pages::vault::frontmatter_value(&raw, "type")
            .map(|v| v.trim().trim_matches(['"', '\'']).trim().to_owned())
    }

    fn lookup_setlist(&self, name: &str) -> Option<editor::markdown::VaultSetlistHit> {
        let meta = self.meta(name)?;
        let raw = self.content(&meta.path)?;
        let is_setlist = crate::pages::vault::frontmatter_value(&raw, "type")
            .map(|v| v.trim().trim_matches(['"', '\'']).trim() == "setlist")
            .unwrap_or(false);
        if !is_setlist {
            return None;
        }
        let links = crate::pages::vault::setlist_song_links_from_body(&raw);
        let songs: Vec<editor::markdown::VaultSetlistSongRow> = links
            .iter()
            .map(|link| {
                let hit = self.lookup_song(link);
                editor::markdown::VaultSetlistSongRow {
                    link: link.clone(),
                    artist: hit.as_ref().and_then(|h| h.artist.clone()),
                    duration_sec: hit.as_ref().map(|h| h.duration_sec).unwrap_or(0.0),
                    stem_count: hit.as_ref().map(|h| h.stem_count).unwrap_or(0),
                }
            })
            .collect();
        let total_seconds = songs.iter().map(|s| s.duration_sec).sum();
        Some(editor::markdown::VaultSetlistHit {
            title: meta.basename.clone(),
            song_count: songs.len(),
            total_seconds,
            songs,
        })
    }

    /// Scripture references: `[[John 3:16]]` / `[[John 3:16-20@ESV]]`
    /// targets that parse as a verse reference resolve to a chip /
    /// verse card. Only consulted when no page matches the target
    /// (the decoration pass checks pages first), so a real
    /// `John 3:16.md` note still wins. The passage text is fetched
    /// lazily through the same worker as page content, keyed
    /// `scripture://<TX>/<osis>`.
    fn lookup_scripture(&self, target: &str) -> Option<editor::markdown::VaultScriptureHit> {
        let scref = scripture_proto::ScriptureRef::parse(target).ok()?;
        let translation = scref
            .translation
            .clone()
            .unwrap_or_else(|| DEFAULT_TRANSLATION.to_owned());
        let osis = scref.range.osis();
        let text = self.content(&format!("{SCRIPTURE_SCHEME}{translation}/{osis}"));
        Some(editor::markdown::VaultScriptureHit {
            display: scref.range.to_string(),
            osis,
            text,
            translation,
        })
    }

    fn lookup_block_short(&self, page: &str, short_id: &str) -> Option<String> {
        let meta = self.meta(page)?;
        let raw = match self.content(&meta.path) {
            Some(raw) => raw,
            None => return Some(LOADING.to_owned()),
        };
        let needle = format!("^{short_id}");
        let pos = raw.find(&needle)?;
        let line_start = raw[..pos].rfind('\n').map_or(0, |n| n + 1);
        let line_end = raw[pos..].find('\n').map_or(raw.len(), |n| pos + n);
        let line = &raw[line_start..line_end];
        Some(line[..line.len() - needle.len()].trim_end().to_string())
    }
}

// ── Editor source bridges ─────────────────────────────────────────

/// `editor::combined_decorations` with vault resolution: the
/// live-preview pass runs with the current [`ClientVaultIndex`]
/// (so `[[wikilinks]]` resolve and embeds render), plus
/// bracket-pair highlighting. Captures the index *signal* — the
/// page swaps the index on folder-index refresh without
/// rebuilding the source. Create once per page (`use_hook`).
pub fn vault_decoration_source(
    lookup: Signal<Option<Rc<ClientVaultIndex>>>,
) -> editor_view::DecorationSource {
    editor_view::DecorationSource::new(move |state| {
        let guard = lookup.read();
        let resolver = guard.as_ref().map(|rc| rc.as_ref() as &dyn VaultLookup);
        let mut out = editor::markdown::live_preview_with(state, resolver);
        out.extend(editor::bracket_match::bracket_match(state));
        out
    })
}

// ── Autocomplete candidates ───────────────────────────────────────

/// Max rows handed to the completion menu per query.
const COMPLETION_LIMIT: usize = 20;

/// The editor's `[[` / `#` trigger source over the vault:
/// wikilink candidates from the folder index (basenames +
/// frontmatter aliases — see [`wikilink_candidates`]) and tag
/// candidates from the `VaultGraph` tags RPC (loaded into `tags`
/// by the page). Filtering is case-insensitive substring against
/// the match text (alias/basename/title for links, the tag path
/// for tags). Create once per page (`use_hook`).
pub fn vault_completion_source(
    links: Memo<Vec<LinkCandidate>>,
    tags: Signal<Vec<TagCount>>,
) -> editor_view::trigger::CompletionSource {
    editor_view::trigger::CompletionSource::new(move |query, kind| {
        let q = query.to_lowercase();
        match kind {
            CompletionKind::Wikilink => links
                .read()
                .iter()
                .filter(|c| {
                    c.insert.to_lowercase().contains(&q) || c.title.to_lowercase().contains(&q)
                })
                .take(COMPLETION_LIMIT)
                .map(|c| Candidate {
                    label: c.insert.clone(),
                    // Accepting wraps as `[[…]]`; alias rows emit
                    // the Obsidian `[[Target|alias]]` form.
                    insert_text: if c.is_alias {
                        format!("{}|{}", c.target, c.insert)
                    } else {
                        c.target.clone()
                    },
                    detail: if c.is_alias {
                        format!("{} → {}", c.path, c.target)
                    } else {
                        c.path.clone()
                    },
                })
                .collect(),
            CompletionKind::Tag => tags
                .read()
                .iter()
                .filter(|t| t.tag.to_lowercase().contains(&q))
                .take(COMPLETION_LIMIT)
                .map(|t| Candidate {
                    label: t.tag.clone(),
                    insert_text: t.tag.clone(),
                    detail: if t.count == 1 {
                        "1 page".to_owned()
                    } else {
                        format!("{} pages", t.count)
                    },
                })
                .collect(),
        }
    })
}

/// One `[[…]]` wikilink completion candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkCandidate {
    /// The wikilink target proper (page basename). Completing an
    /// alias should emit `[[target|insert]]`.
    pub target: String,
    /// The text the user's prefix is matched against — the
    /// basename itself, or one of the page's frontmatter aliases.
    pub insert: String,
    /// Page title, for the menu row's label.
    pub title: String,
    /// Vault-relative path, for disambiguation display.
    pub path: String,
    /// `insert` is an alias rather than the basename.
    pub is_alias: bool,
}

/// Wikilink candidates from the folder index: every page's
/// basename plus its frontmatter aliases, sorted by match text.
/// Pure — call it on the already-fetched `folder_index` pages.
#[must_use]
pub fn wikilink_candidates(pages: &[PageMeta]) -> Vec<LinkCandidate> {
    let mut out = Vec::new();
    for p in pages {
        out.push(LinkCandidate {
            target: p.basename.clone(),
            insert: p.basename.clone(),
            title: p.title.clone(),
            path: p.path.clone(),
            is_alias: false,
        });
        for alias in &p.aliases {
            out.push(LinkCandidate {
                target: p.basename.clone(),
                insert: alias.clone(),
                title: p.title.clone(),
                path: p.path.clone(),
                is_alias: true,
            });
        }
    }
    out.sort_by(|a, b| {
        a.insert
            .to_lowercase()
            .cmp(&b.insert.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// Tag completion candidates (`#…`): every tag in the vault —
/// frontmatter and inline — with page counts, by descending
/// count, via the `VaultGraph` RPC.
pub async fn tag_candidates(org: String, vault_id: String) -> Result<Vec<TagCount>, String> {
    let client = crate::vox_clients::vault_graph_client(&org).await?;
    #[cfg(target_arch = "wasm32")]
    {
        client
            .tags(vault_id)
            .await
            .map_err(|e| format!("tags: {e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client, vault_id);
        Err("native client not wired yet".to_owned())
    }
}

// ── Content helpers (ported from `vault_live::lookup`, which is
//    native-only) ──────────────────────────────────────────────────

fn basename_of(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".md").unwrap_or(file)
}

/// First line starting at `from`, capped at 80 chars for chip
/// display.
fn first_line(raw: &str, from: usize) -> String {
    let end = raw[from..].find('\n').map_or(raw.len(), |n| from + n);
    let line = &raw[from..end];
    if line.chars().count() > 80 {
        let truncated: String = line.chars().take(80).collect();
        format!("{truncated}…")
    } else {
        line.to_string()
    }
}

/// Page preview: body with frontmatter stripped, capped at `max`
/// chars.
fn preview_of_page(raw: &str, max: usize) -> String {
    let body_start = if let Some(rest) = raw.strip_prefix("---\n") {
        rest.find("\n---\n").map_or(0, |i| 4 + i + 5)
    } else {
        0
    };
    let body = &raw[body_start..];
    let trimmed = body.trim_start();
    let truncated: String = trimmed.chars().take(max).collect();
    if trimmed.chars().count() > max {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Body of the heading matching `name` (case-insensitive trim) up
/// to the next same-or-higher heading.
fn section_body(raw: &str, name: &str) -> Option<String> {
    let needle = name.trim().to_lowercase();
    let lines: Vec<(usize, usize)> = line_ranges(raw);
    let mut start_idx: Option<(usize, usize)> = None;
    for (i, (lf, lt)) in lines.iter().enumerate() {
        let line = &raw[*lf..*lt];
        if let Some((level, marker_end)) = parse_heading(line) {
            let title = line[marker_end..].trim();
            if title.to_lowercase() == needle {
                start_idx = Some((i, level));
                break;
            }
        }
    }
    let (start_i, start_level) = start_idx?;
    let mut body = String::new();
    for (lf, lt) in lines.iter().skip(start_i + 1) {
        let line = &raw[*lf..*lt];
        if let Some((level, _)) = parse_heading(line) {
            if level <= start_level {
                break;
            }
        }
        body.push_str(line);
        body.push('\n');
    }
    Some(body.trim().to_string())
}

fn line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    if start <= bytes.len() {
        out.push((start, bytes.len()));
    }
    out
}

fn parse_heading(line: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let mut level = 0;
    while level < 6 && b.get(level) == Some(&b'#') {
        level += 1;
    }
    if level == 0 {
        return None;
    }
    if b.get(level) != Some(&b' ') {
        return None;
    }
    Some((level, level + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(path: &str, basename: &str, title: &str, aliases: &[&str]) -> PageMeta {
        PageMeta {
            path: path.to_owned(),
            basename: basename.to_owned(),
            title: title.to_owned(),
            page_type: String::new(),
            folder: String::new(),
            sha256: String::new(),
            tags: Vec::new(),
            icon: String::new(),
            aliases: aliases.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn candidates_include_basenames_and_aliases_sorted() {
        let pages = vec![
            meta("Zeta.md", "Zeta", "Zeta", &[]),
            meta(
                "Wisdom/Plans.md",
                "Plans",
                "Plans rot",
                &["agenda", "Roadmap"],
            ),
        ];
        let c = wikilink_candidates(&pages);
        let inserts: Vec<&str> = c.iter().map(|x| x.insert.as_str()).collect();
        assert_eq!(inserts, vec!["agenda", "Plans", "Roadmap", "Zeta"]);
        let agenda = &c[0];
        assert!(agenda.is_alias);
        assert_eq!(agenda.target, "Plans");
        assert_eq!(agenda.title, "Plans rot");
    }

    #[test]
    fn preview_strips_frontmatter_and_caps() {
        let raw = "---\ntitle: X\n---\nBody starts here.\nMore.\n";
        assert_eq!(preview_of_page(raw, 200), "Body starts here.\nMore.\n");
        assert_eq!(preview_of_page(raw, 4), "Body…");
    }

    #[test]
    fn section_body_stops_at_same_level() {
        let raw = "# A\n\n## Target\nline one\nline two\n\n## Next\nnope\n";
        assert_eq!(
            section_body(raw, "target").as_deref(),
            Some("line one\nline two")
        );
        assert_eq!(section_body(raw, "missing"), None);
    }

    #[test]
    fn basename_strips_dirs_and_extension() {
        assert_eq!(basename_of("Wisdom/Plans.md"), "Plans");
        assert_eq!(basename_of("Loose.md"), "Loose");
    }
}

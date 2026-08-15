//! `/wiki/sources` + `/wiki/source/:name` — the SourceViewer.
//!
//! Renders an archived raw source (`Wiki/raw/sources/*.md`,
//! written by `task wiki archive`): provenance frontmatter as
//! a header strip, then the markdown body through a small
//! purpose-built block renderer (the archive emits a known
//! shape — headings, ledes, quotes, bullets, `^t<sec>`
//! transcript paragraphs — so no general renderer is needed).
//!
//! `content_type: video` sources embed the YouTube player
//! (`enablejsapi=1`); clicking a transcript block's `[mm:ss]`
//! chip seeks the player to that anchor via the IFrame
//! postMessage API. `content_type: podcast` sources embed a
//! native `<audio>` element over the provenance `media:`
//! enclosure URL — the same `[mm:ss]` chips seek it via a
//! trivial `currentTime` assignment, so podcast notes are
//! exactly as seekable as video notes. PDF sources render
//! their `^p<page>` anchors as `p. N` chips (link targets,
//! nothing to seek). v1 is the clean read-only viewer —
//! "insert note at current time" (getCurrentTime → `## Notes`
//! bullet) is the planned follow-up, see plans/wiki-archive.md.

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::orgs::{OrgMeta, OrgSelection, selected_slugs};

const WIKI_ID: &str = "default";
const EMBED_ID: &str = "source-viewer-embed";

// ── data ────────────────────────────────────────────────────

async fn fetch_sources(slug: &str) -> Result<Vec<wiki_proto::raw::RawSourceRef>, String> {
    let c =
        crate::vox_clients::establish_for::<wiki_proto::service::raw_layer::RawLayerClient>(slug)
            .await?;
    c.list_raw_sources(WIKI_ID.to_owned())
        .await
        .map_err(|e| format!("list_raw_sources: {e:?}"))
}

async fn fetch_source_text(slug: &str, name: &str) -> Result<String, String> {
    // `name` is the bare filename; sources are flat under
    // raw/sources/ by convention.
    let path = format!("raw/sources/{name}");
    let c =
        crate::vox_clients::establish_for::<wiki_proto::service::raw_layer::RawLayerClient>(slug)
            .await?;
    let bytes = c
        .read_raw_source(WIKI_ID.to_owned(), path)
        .await
        .map_err(|e| format!("read_raw_source: {e:?}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn first_slug(selection: &Signal<OrgSelection>, orgs: &Signal<Vec<OrgMeta>>) -> Option<String> {
    selected_slugs(&selection.read(), &orgs.read())
        .first()
        .cloned()
}

// ── parsed source shape ─────────────────────────────────────

/// Provenance fields lifted from the archive frontmatter.
#[derive(Debug, Clone, Default, PartialEq)]
struct Provenance {
    title: String,
    source_url: String,
    content_type: String,
    archived_at: String,
    extractor: String,
    media: String,
    duration: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Anchor {
    /// `^t<sec>` — seekable timestamp (video / podcast).
    Time(u64),
    /// `^p<page>` — PDF page marker (link target only).
    Page(u64),
}

/// Which player the viewer embedded — decides what a
/// timestamp chip's click does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Player {
    YouTube,
    Audio,
    None,
}

#[derive(Debug, Clone, PartialEq)]
enum Block {
    Heading(u8, String),
    /// Italic one-liner (`_channel · date · 3:33_`).
    Lede(String),
    Quote(Vec<String>),
    Bullet(String),
    /// Paragraph; `anchor` = the `^t<sec>` / `^p<page>` id
    /// when present.
    Para {
        text: String,
        anchor: Option<Anchor>,
    },
}

fn parse_source(raw: &str) -> (Provenance, Vec<Block>) {
    let mut prov = Provenance::default();
    let body = if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                let Some((k, v)) = line.split_once(':') else {
                    continue;
                };
                let v = v.trim().trim_matches('"').to_string();
                match k.trim() {
                    "title" => prov.title = v,
                    "source_url" => prov.source_url = v,
                    "content_type" => prov.content_type = v,
                    "archived_at" => prov.archived_at = v,
                    "extractor" => prov.extractor = v,
                    "media" => prov.media = v,
                    "duration" => prov.duration = v,
                    _ => {}
                }
            }
            rest[end..].trim_start_matches("\n---").trim_start()
        } else {
            raw
        }
    } else {
        raw
    };
    (prov, parse_blocks(body))
}

fn parse_blocks(body: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    // Group into paragraphs on blank lines; quotes/bullets
    // are line-level.
    let mut para: Vec<String> = Vec::new();
    let mut quote: Vec<String> = Vec::new();
    let flush_para = |para: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if para.is_empty() {
            return;
        }
        let text = para.join(" ");
        para.clear();
        let trimmed = text.trim().to_string();
        if trimmed.len() > 2 && trimmed.starts_with('_') && trimmed.ends_with('_') {
            blocks.push(Block::Lede(trimmed.trim_matches('_').to_string()));
            return;
        }
        let (text, anchor) = split_anchor(&trimmed);
        blocks.push(Block::Para { text, anchor });
    };
    let flush_quote = |quote: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !quote.is_empty() {
            blocks.push(Block::Quote(std::mem::take(quote)));
        }
    };
    for line in body.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            flush_para(&mut para, &mut blocks);
            flush_quote(&mut quote, &mut blocks);
        } else if let Some(h) = t.strip_prefix("### ") {
            flush_para(&mut para, &mut blocks);
            flush_quote(&mut quote, &mut blocks);
            blocks.push(Block::Heading(3, h.to_string()));
        } else if let Some(h) = t.strip_prefix("## ") {
            flush_para(&mut para, &mut blocks);
            flush_quote(&mut quote, &mut blocks);
            blocks.push(Block::Heading(2, h.to_string()));
        } else if let Some(h) = t.strip_prefix("# ") {
            flush_para(&mut para, &mut blocks);
            flush_quote(&mut quote, &mut blocks);
            blocks.push(Block::Heading(1, h.to_string()));
        } else if let Some(q) = t.strip_prefix("> ") {
            flush_para(&mut para, &mut blocks);
            quote.push(q.to_string());
        } else if t.trim() == ">" {
            quote.push(String::new());
        } else if let Some(b) = t.strip_prefix("- ") {
            flush_para(&mut para, &mut blocks);
            flush_quote(&mut quote, &mut blocks);
            blocks.push(Block::Bullet(b.to_string()));
        } else {
            flush_quote(&mut quote, &mut blocks);
            para.push(t.trim().to_string());
        }
    }
    flush_para(&mut para, &mut blocks);
    flush_quote(&mut quote, &mut blocks);
    blocks
}

/// `"… text ^t870"` → `("… text", Some(Time(870)))`;
/// `"… text ^p12"` → `("… text", Some(Page(12)))`.
fn split_anchor(text: &str) -> (String, Option<Anchor>) {
    for (marker, make) in [
        (" ^t", Anchor::Time as fn(u64) -> Anchor),
        (" ^p", Anchor::Page as fn(u64) -> Anchor),
    ] {
        if let Some((head, id)) = text.rsplit_once(marker) {
            if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                if let Ok(n) = id.parse() {
                    return (head.to_string(), Some(make(n)));
                }
            }
        }
    }
    (text.to_string(), None)
}

fn mmss(total: u64) -> String {
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// YouTube video id out of the provenance `media:` URL.
fn youtube_id(media: &str) -> Option<String> {
    let (marker, rest) = if let Some(i) = media.find("v=") {
        ("v=", &media[i + 2..])
    } else if let Some(i) = media.find("youtu.be/") {
        ("youtu.be/", &media[i + 9..])
    } else {
        return None;
    };
    let _ = marker;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    (id.len() >= 6).then_some(id)
}

/// Seek the embedded `<audio>` player — podcast `media:` is a
/// plain enclosure URL, so this is just a `currentTime`
/// assignment.
fn seek_audio(sec: u64) {
    let js = format!(
        "const a = document.getElementById('{EMBED_ID}-audio');
if (a) {{ a.currentTime = {sec}; a.play(); }}"
    );
    let _ = dioxus::document::eval(&js);
}

/// Seek the embedded player via the IFrame postMessage API.
/// The `listening` handshake activates command handling on
/// embeds created without the full JS SDK.
fn seek_embed(sec: u64) {
    let js = format!(
        "const f = document.getElementById('{EMBED_ID}');
if (f && f.contentWindow) {{
  f.contentWindow.postMessage(JSON.stringify({{event:'listening', id:'{EMBED_ID}'}}), '*');
  f.contentWindow.postMessage(JSON.stringify({{event:'command', func:'seekTo', args:[{sec}, true]}}), '*');
  f.contentWindow.postMessage(JSON.stringify({{event:'command', func:'playVideo', args:[]}}), '*');
}}"
    );
    let _ = dioxus::document::eval(&js);
}

// ── pages ───────────────────────────────────────────────────

/// `/wiki/sources` — every archived raw source, linked to its
/// viewer.
#[component]
pub fn WikiSourcesView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    let sources = use_resource(move || async move {
        let slug = first_slug(&selection, &org_list)
            .ok_or_else(|| "no organization selected".to_string())?;
        let mut rows = fetch_sources(&slug).await?;
        rows.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok::<_, String>(rows)
    });

    let body = match &*sources.read() {
        Some(Ok(rows)) if rows.is_empty() => rsx! {
            div { class: "rounded-2xl border border-dashed border-border/70 bg-card/40 py-16 text-center",
                Heading { level: HeadingLevel::H3, "No archived sources yet" }
                Text { variant: TextVariant::Muted, "Run `task wiki archive <url>` to capture one." }
            }
        },
        Some(Ok(rows)) => rsx! {
            div { class: "flex flex-col divide-y divide-border/60 rounded-xl border border-border/70 bg-card/30",
                for r in rows.clone() {
                    Link {
                        key: "{r.filename}",
                        to: crate::routes::Route::WikiSourceRoute { name: r.filename.clone() },
                        class: "flex items-baseline justify-between gap-3 px-4 py-2.5 text-sm hover:bg-accent/40",
                        span { class: "truncate font-medium", "{r.filename}" }
                        span { class: "shrink-0 text-xs text-muted-foreground", "{r.size} bytes" }
                    }
                }
            }
        },
        Some(Err(e)) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't list sources",
                message: e.clone(),
            }
        },
        None => rsx! {
            div { class: "flex items-center justify-center rounded-xl border border-border/70 bg-card/30 py-16",
                Text { variant: TextVariant::Muted, "Loading sources…" }
            }
        },
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-4xl flex-col gap-4 overflow-y-auto p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Wiki"
                }
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Archived sources" }
                Text { variant: TextVariant::Muted,
                    "Everything `task wiki archive` captured into raw/sources/."
                }
            }
            {body}
        }
    }
}

/// `/wiki/source/:name` — the SourceViewer proper.
#[component]
pub fn WikiSourceView(name: String) -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    let fetch_name = name.clone();
    let source = use_resource(use_reactive!(|(fetch_name,)| async move {
        let slug = first_slug(&selection, &org_list)
            .ok_or_else(|| "no organization selected".to_string())?;
        fetch_source_text(&slug, &fetch_name).await
    }));

    let body = match &*source.read() {
        Some(Ok(raw)) => {
            let (prov, blocks) = parse_source(raw);
            render_source(&prov, &blocks)
        }
        Some(Err(e)) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't load this source",
                message: e.clone(),
            }
        },
        None => rsx! {
            div { class: "flex items-center justify-center rounded-xl border border-border/70 bg-card/30 py-16",
                Text { variant: TextVariant::Muted, "Loading source…" }
            }
        },
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-3xl flex-col gap-4 overflow-y-auto p-4 sm:p-6 lg:p-8",
            Link {
                to: crate::routes::Route::WikiSourcesRoute {},
                class: "text-xs text-muted-foreground hover:text-foreground",
                "← All archived sources"
            }
            {body}
        }
    }
}

fn render_source(prov: &Provenance, blocks: &[Block]) -> Element {
    let video_id = (prov.content_type == "video")
        .then(|| youtube_id(&prov.media))
        .flatten();
    // Podcasts: `media:` is the playable enclosure URL.
    let audio_src = (prov.content_type == "podcast"
        && prov.media.starts_with("http")
        && !prov.media.contains("open.spotify.com"))
    .then(|| prov.media.clone());
    let player = if video_id.is_some() {
        Player::YouTube
    } else if audio_src.is_some() {
        Player::Audio
    } else {
        Player::None
    };
    let archived_day = prov
        .archived_at
        .split('T')
        .next()
        .unwrap_or_default()
        .to_string();
    let duration = prov
        .duration
        .parse::<u64>()
        .ok()
        .map(mmss)
        .unwrap_or_default();

    rsx! {
        header { class: "flex flex-col gap-2",
            Heading { level: HeadingLevel::H2, class: "tracking-tight", "{prov.title}" }
            div { class: "flex flex-wrap items-center gap-2 text-xs text-muted-foreground",
                if !prov.content_type.is_empty() {
                    span { class: "rounded-full border border-border/70 bg-card/60 px-2 py-0.5 font-medium uppercase tracking-wide",
                        "{prov.content_type}"
                    }
                }
                if !duration.is_empty() {
                    span { "{duration}" }
                }
                if !archived_day.is_empty() {
                    span { "archived {archived_day}" }
                }
                if !prov.extractor.is_empty() {
                    span { "via {prov.extractor}" }
                }
                if !prov.source_url.is_empty() {
                    a {
                        href: "{prov.source_url}",
                        target: "_blank",
                        class: "underline decoration-border underline-offset-2 hover:text-foreground",
                        "original ↗"
                    }
                }
            }
        }
        if let Some(vid) = video_id {
            iframe {
                id: EMBED_ID,
                class: "aspect-video w-full rounded-xl border border-border/70 bg-black",
                src: "https://www.youtube.com/embed/{vid}?enablejsapi=1",
                allow: "autoplay; encrypted-media; picture-in-picture",
                allowfullscreen: true,
            }
        }
        if let Some(src) = audio_src {
            // Sticky so the player stays reachable while
            // scrolling a long transcript.
            div { class: "sticky top-0 z-10 -mx-2 rounded-xl border border-border/70 bg-card/95 px-3 py-2 backdrop-blur",
                audio {
                    id: "{EMBED_ID}-audio",
                    class: "w-full",
                    controls: true,
                    preload: "metadata",
                    src: "{src}",
                }
            }
        }
        article { class: "flex flex-col gap-3 pb-12",
            for (i, block) in blocks.iter().enumerate() {
                {render_block(i, block, player)}
            }
        }
    }
}

fn render_block(key: usize, block: &Block, player: Player) -> Element {
    match block {
        // The H1 duplicates the provenance title — skip it.
        Block::Heading(1, _) => rsx! {},
        Block::Heading(2, text) => rsx! {
            Heading { key: "{key}", level: HeadingLevel::H3, class: "mt-4 tracking-tight", "{text}" }
        },
        Block::Heading(_, text) => rsx! {
            Heading { key: "{key}", level: HeadingLevel::H4, class: "mt-2 tracking-tight", "{text}" }
        },
        Block::Lede(text) => rsx! {
            Text { key: "{key}", variant: TextVariant::Muted, class: "italic", "{text}" }
        },
        Block::Quote(lines) => rsx! {
            blockquote {
                key: "{key}",
                class: "border-l-2 border-border/80 pl-3 text-sm text-muted-foreground",
                for (j, l) in lines.iter().enumerate() {
                    p { key: "{j}", class: "min-h-4", "{l}" }
                }
            }
        },
        Block::Bullet(text) => rsx! {
            div { key: "{key}", class: "flex gap-2 text-sm",
                span { class: "text-muted-foreground", "•" }
                span { "{text}" }
            }
        },
        Block::Para {
            text,
            anchor: Some(Anchor::Time(sec)),
        } => {
            let sec = *sec;
            let stamp = mmss(sec);
            // Strip the leading `[mm:ss] ` the archive writes —
            // the chip carries the timestamp.
            let text = text
                .strip_prefix(&format!("[{stamp}] "))
                .unwrap_or(text)
                .to_string();
            rsx! {
                div { key: "{key}", id: "t{sec}",
                    class: "group flex items-start gap-2 rounded-lg px-2 py-1 -mx-2 hover:bg-accent/30",
                    button {
                        class: "mt-0.5 shrink-0 rounded-md border border-border/70 bg-card/60 px-1.5 py-0.5 font-mono text-[0.7rem] text-muted-foreground hover:border-primary/60 hover:text-foreground",
                        title: "Seek player to {stamp}",
                        onclick: move |_| match player {
                            Player::Audio => seek_audio(sec),
                            Player::YouTube | Player::None => seek_embed(sec),
                        },
                        "{stamp}"
                    }
                    p { class: "text-sm leading-relaxed", "{text}" }
                }
            }
        }
        Block::Para {
            text,
            anchor: Some(Anchor::Page(page)),
        } => {
            let page = *page;
            // Strip the `[p. N] ` prefix the archive writes —
            // the chip carries the page number. Nothing to
            // seek for a PDF; the chip is the `#^pN` target.
            let text = text
                .strip_prefix(&format!("[p. {page}] "))
                .unwrap_or(text)
                .to_string();
            rsx! {
                div { key: "{key}", id: "p{page}",
                    class: "group flex items-start gap-2 rounded-lg px-2 py-1 -mx-2 hover:bg-accent/30",
                    span {
                        class: "mt-0.5 shrink-0 rounded-md border border-border/70 bg-card/60 px-1.5 py-0.5 font-mono text-[0.7rem] text-muted-foreground",
                        title: "Page {page}",
                        "p. {page}"
                    }
                    p { class: "text-sm leading-relaxed", "{text}" }
                }
            }
        }
        Block::Para { text, anchor: None } => rsx! {
            p { key: "{key}", class: "text-sm leading-relaxed", "{text}" }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "---\ntitle: \"A Video\"\nsource_url: \"https://youtu.be/abc123xyz\"\ncanonical_url: \"https://www.youtube.com/watch?v=abc123xyz\"\ncontent_type: video\narchived_at: 2026-06-12T05:00:00Z\nextractor: \"yt-dlp\"\nmedia: \"https://www.youtube.com/watch?v=abc123xyz\"\nduration: 213\n---\n\n# A Video\n\n_Channel · 2009-10-25 · 3:33_\n\n## Transcript\n\n[0:01] First block text ^t1\n\n[0:47] Second block ^t47\n";

    #[test]
    fn parses_provenance_and_anchored_blocks() {
        let (prov, blocks) = parse_source(FIXTURE);
        assert_eq!(prov.title, "A Video");
        assert_eq!(prov.content_type, "video");
        assert_eq!(prov.duration, "213");
        assert_eq!(youtube_id(&prov.media).as_deref(), Some("abc123xyz"));
        assert!(blocks.contains(&Block::Heading(2, "Transcript".into())));
        assert!(blocks.contains(&Block::Lede("Channel · 2009-10-25 · 3:33".into())));
        let anchored: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para {
                    anchor: Some(Anchor::Time(s)),
                    ..
                } => Some(*s),
                _ => None,
            })
            .collect();
        assert_eq!(anchored, vec![1, 47]);
    }

    #[test]
    fn page_anchors_parse_as_page_kind() {
        let blocks = parse_blocks(
            "## Pages\n\n[p. 1] Intro text. ^p1\n\nUnanchored continuation.\n\n[p. 2] Second page. ^p2\n",
        );
        let pages: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para {
                    anchor: Some(Anchor::Page(p)),
                    ..
                } => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(pages, vec![1, 2]);
        assert!(blocks.contains(&Block::Para {
            text: "Unanchored continuation.".into(),
            anchor: None
        }));
    }

    #[test]
    fn podcast_media_is_audio_not_youtube() {
        let raw = "---\ntitle: \"An Episode\"\nsource_url: \"https://podcasts.apple.com/us/podcast/x/id1?i=2\"\ncontent_type: podcast\nmedia: \"https://cdn.example.com/ep.mp3?tracker=1\"\nduration: 3725\n---\n\n# An Episode\n\n[0:00] Welcome. ^t0\n";
        let (prov, blocks) = parse_source(raw);
        assert_eq!(prov.content_type, "podcast");
        assert!(prov.media.starts_with("http"));
        assert_eq!(youtube_id(&prov.media), None);
        assert!(matches!(
            blocks.last(),
            Some(Block::Para {
                anchor: Some(Anchor::Time(0)),
                ..
            })
        ));
    }

    #[test]
    fn quotes_and_bullets_group() {
        let blocks = parse_blocks("## Highlights\n\n> line one\n> line two\n\n- a bullet\n");
        assert_eq!(
            blocks,
            vec![
                Block::Heading(2, "Highlights".into()),
                Block::Quote(vec!["line one".into(), "line two".into()]),
                Block::Bullet("a bullet".into()),
            ]
        );
    }

    #[test]
    fn plain_markdown_without_frontmatter_still_renders() {
        let (prov, blocks) = parse_source("Just a paragraph.\n");
        assert!(prov.title.is_empty());
        assert_eq!(
            blocks,
            vec![Block::Para {
                text: "Just a paragraph.".into(),
                anchor: None
            }]
        );
    }
}

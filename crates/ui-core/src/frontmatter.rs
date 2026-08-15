//! Vault-note frontmatter reads.
//!
//! Notes carry a leading `---` YAML-ish block. These are the
//! hand-rolled readers the vault page, the note views and the session
//! player all share — deliberately not `serde_yaml`, because a note's
//! frontmatter is user-edited and may be mid-keystroke invalid, and a
//! partial read beats an error.
//!
//! [`SongFront`] is the `type: song` shape: the metadata + stem list the
//! player needs to build a one-song setlist without fetching a manifest.

// ── generic reads ───────────────────────────────────────────────────────────

/// Read a scalar `key: value` from the note's leading `---` frontmatter block.
#[must_use]
pub fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    let rest = text.strip_prefix("---")?;
    let (front, _) = rest.split_once("\n---")?;
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                return Some(v.to_owned());
            }
        }
    }
    None
}

/// Parse a frontmatter block list of maps under `key:`:
///
/// ```yaml
/// key:
///   - name: Click
///     group: Guide
/// ```
///
/// Each `-` starts a new entry; indented `k: v` lines extend the current
/// one. Values are trimmed of quotes/whitespace. Stops at the next
/// top-level (unindented) key.
#[must_use]
pub fn front_block_maps(text: &str, key: &str) -> Vec<Vec<(String, String)>> {
    let Some(rest) = text.strip_prefix("---") else {
        return Vec::new();
    };
    let Some((front, _)) = rest.split_once("\n---") else {
        return Vec::new();
    };
    let clean = |s: &str| s.trim().trim_matches(['"', '\'']).trim().to_owned();
    let lines: Vec<&str> = front.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        // The opening `key:` line, top-level (unindented) with no value.
        let is_open = line
            .strip_prefix(key)
            .and_then(|r| r.strip_prefix(':'))
            .is_some_and(|r| r.trim().is_empty());
        if !is_open {
            continue;
        }
        let mut out = Vec::new();
        let mut cur: Vec<(String, String)> = Vec::new();
        while i < lines.len() {
            let raw = lines[i];
            let t = raw.trim_start();
            if t.is_empty() {
                i += 1;
                continue;
            }
            if !raw.starts_with(' ') && !raw.starts_with('\t') {
                break; // next top-level key
            }
            if let Some(after_dash) = t.strip_prefix('-') {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                if let Some((k, v)) = after_dash.trim_start().split_once(':') {
                    cur.push((k.trim().to_owned(), clean(v)));
                }
            } else if let Some((k, v)) = t.split_once(':') {
                cur.push((k.trim().to_owned(), clean(v)));
            }
            i += 1;
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        return out;
    }
    Vec::new()
}

/// Lowercase, spaces/underscores → hyphens, drop other punctuation.
#[must_use]
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if c == ' ' || c == '_' || c == '-' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

// ── song-note frontmatter (stems as attachments) ────────────────────────────

/// One stem parsed from a song note's frontmatter `stems:` block. The
/// audio lives in the org's content-addressed blob store; `content_hash`
/// is resolved to a signed `/blobs/download` URL at play time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrontStem {
    pub name: String,
    pub group: Option<String>,
    pub default_muted: bool,
    pub content_hash: String,
}

/// One section parsed from the frontmatter `sections:` block (song-local
/// seconds, 0-based).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrontSection {
    pub name: String,
    pub start_sec: f64,
    pub end_sec: f64,
}

/// Song metadata + stems parsed from a `type: song` note's frontmatter.
/// When `stems` is non-empty the player streams from the attachment
/// blob store instead of `/media/songs/{slug}/…`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SongFront {
    pub artist: Option<String>,
    pub key: Option<String>,
    pub bpm: Option<f64>,
    pub time_signature: Option<String>,
    pub duration_sec: Option<f64>,
    pub sections: Vec<FrontSection>,
    pub stems: Vec<FrontStem>,
}

/// Parse the song frontmatter (scalars + the `stems:` / `sections:`
/// block lists) from a note's text.
#[must_use]
pub fn song_front_from(text: &str) -> SongFront {
    let fv = |k: &str| {
        frontmatter_value(text, k)
            .map(|v| v.trim().trim_matches(['"', '\'']).trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let num = |k: &str| fv(k).and_then(|v| v.parse::<f64>().ok());

    let stems = front_block_maps(text, "stems")
        .into_iter()
        .filter_map(|pairs| {
            let get = |k: &str| {
                pairs
                    .iter()
                    .find(|(pk, _)| pk == k)
                    .map(|(_, v)| v.clone())
                    .filter(|v| !v.is_empty())
            };
            let hash = get("content_hash")?;
            Some(FrontStem {
                name: get("name")?,
                group: get("group"),
                default_muted: get("default_muted").is_some_and(|v| v == "true"),
                content_hash: hash,
            })
        })
        .collect();

    let sections = front_block_maps(text, "sections")
        .into_iter()
        .filter_map(|pairs| {
            let get = |k: &str| pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.clone());
            Some(FrontSection {
                name: get("name")?,
                start_sec: get("start_sec")?.parse().ok()?,
                end_sec: get("end_sec")?.parse().ok()?,
            })
        })
        .collect();

    SongFront {
        artist: fv("artist"),
        key: fv("key"),
        bpm: num("bpm"),
        time_signature: fv("time_signature"),
        duration_sec: num("duration_sec"),
        sections,
        stems,
    }
}

// ── setlist / song-queue frontmatter ────────────────────────────────────────

/// Media slug for a `type: song` note. Prefers a `song_slug:` (or `slug:`)
/// key in the leading YAML frontmatter block; otherwise slugifies the note
/// basename. The slug selects `/media/songs/{slug}/…` (served same-origin).
#[must_use]
pub fn song_slug_from(text: &str, basename: &str) -> String {
    if let Some(v) =
        frontmatter_value(text, "song_slug").or_else(|| frontmatter_value(text, "slug"))
    {
        let v = v.trim().trim_matches(['"', '\'']).trim();
        if !v.is_empty() {
            return v.to_owned();
        }
    }
    slugify(basename)
}

/// Ordered media slugs for a `type: setlist` note, parsed from the `songs:`
/// YAML list in the leading frontmatter. Accepts both the block form
///
/// ```yaml
/// songs:
///   - song-a
///   - song-b
/// ```
///
/// and the inline flow form `songs: [song-a, song-b]`. Each entry is trimmed
/// of quotes/whitespace; blanks are dropped.
/// Songs referenced as standalone `[[SongTitle]]` wikilinks in the note
/// BODY, in document order — the composable authoring form: one wikilink
/// per line = one setlist entry, slugified from the link target. Lines may
/// be plain (`[[Praise]]`) or list items (`- [[Praise]]`, `1. [[Praise]]`).
#[must_use]
pub fn setlist_songs_from_body(text: &str) -> Vec<String> {
    // Skip the frontmatter block if present.
    let body = text
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---").map(|(_, b)| b))
        .unwrap_or(text);
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        // Allow list markers / numbering before the link.
        let t = t
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['-', '*', '.', ')'])
            .trim_start();
        let Some(inner) = t.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")) else {
            continue;
        };
        // `[[Target|alias]]` → target; `[[Target#section]]` → target.
        let target = inner
            .split('|')
            .next()
            .unwrap_or(inner)
            .split('#')
            .next()
            .unwrap_or(inner)
            .trim();
        // Strip a `Songs/` folder prefix if the link is fully qualified.
        let target = target.strip_prefix("Songs/").unwrap_or(target);
        if !target.is_empty() {
            let slug = slugify(target);
            if !out.contains(&slug) {
                out.push(slug);
            }
        }
    }
    out
}

/// The RAW wikilink target names of a setlist body, in document order
/// (the un-slugified companion of [`setlist_songs_from_body`]).
#[must_use]
pub fn setlist_song_links_from_body(text: &str) -> Vec<String> {
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
        let Some(inner) = t.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")) else {
            continue;
        };
        let target = inner
            .split(['|', '#'])
            .next()
            .unwrap_or(inner)
            .trim()
            .trim_start_matches("Songs/");
        if !target.is_empty() && !out.iter().any(|s: &String| s == target) {
            out.push(target.to_owned());
        }
    }
    out
}

/// A setlist note's ordered song slugs: the composable form wins —
/// standalone `[[SongTitle]]` wikilinks in the body, in document order —
/// with the frontmatter `songs:` list as the fallback for notes that
/// predate wikilink authoring.
#[must_use]
pub fn setlist_songs_from(text: &str) -> Vec<String> {
    let from_body = setlist_songs_from_body(text);
    if !from_body.is_empty() {
        return from_body;
    }
    setlist_songs_from_frontmatter(text)
}

fn setlist_songs_from_frontmatter(text: &str) -> Vec<String> {
    let Some(rest) = text.strip_prefix("---") else {
        return Vec::new();
    };
    let Some((front, _)) = rest.split_once("\n---") else {
        return Vec::new();
    };
    let clean = |s: &str| s.trim().trim_matches(['"', '\'']).trim().to_owned();

    let mut lines = front.lines();
    let mut out = Vec::new();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(after) = trimmed.strip_prefix("songs:") else {
            continue;
        };
        let after = after.trim();
        // Inline flow list: songs: [a, b, c]
        if let Some(inner) = after.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            out.extend(inner.split(',').map(clean).filter(|s| !s.is_empty()));
            return out;
        }
        // Block list: subsequent `- item` lines.
        for l in lines.by_ref() {
            let t = l.trim_start();
            if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix('-')) {
                let v = clean(item);
                if !v.is_empty() {
                    out.push(v);
                }
            } else if t.is_empty() {
                continue;
            } else {
                break; // next frontmatter key ends the list
            }
        }
        return out;
    }
    out
}

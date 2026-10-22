//! What a session file references — the dependency half of an atomic
//! facet (`files.sync.selective`).
//!
//! "A session arrives with the media it references" needs somebody to
//! know what a session references. For REAPER that is written down: an
//! `.rpp` is text, and every source item names its file on a `FILE`
//! line, relative to the project's folder unless it was recorded
//! elsewhere. This module reads those lines and resolves them against
//! the root, so the media can sit anywhere — `Audio Files/`, `Media/`,
//! a shared samples folder no tool layout names — and still come with
//! the session.
//!
//! Formats this module cannot read (Pro Tools' binary `.ptx`) fall back
//! to the folder: the atomic tool directories beside the session
//! (`Audio Files`, `Clip Groups`, …) are its media, which is what Pro
//! Tools itself assumes. The result says which reading it is, so a
//! caller can tell "these are the references" from "this is the
//! folder".
//!
//! Nothing here touches a disk. The lane reads the session's bytes and
//! hands them in.

use std::collections::BTreeSet;

use files_proto::path::RootPath;

use crate::facet::{FacetMap, Source};

/// How a session file's references can be learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// REAPER's `.rpp`: text, one `FILE` line per source.
    Reaper,
    /// Anything else a capability calls a session — read by its folder.
    Opaque,
}

/// The format of a session file, by extension.
#[must_use]
pub fn format_of(session: &RootPath) -> Format {
    let name = session.name().unwrap_or_default();
    match name.rsplit_once('.') {
        Some((_, ext)) if ext.eq_ignore_ascii_case("rpp") => Format::Reaper,
        _ => Format::Opaque,
    }
}

/// What a session references.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Referenced {
    /// Root-relative paths, deduplicated. Whether each exists in the
    /// root is the caller's question — a reference to a file that was
    /// moved is still a reference, and is reported as missing rather
    /// than silently dropped.
    pub inside: BTreeSet<RootPath>,
    /// References that cannot be followed inside this root, as written:
    /// absolute paths from the machine that recorded them, or relative
    /// ones that climb out of the root.
    pub outside: Vec<String>,
}

/// Read a REAPER project's media references.
///
/// `session` is the `.rpp`'s own root path: a relative `FILE` resolves
/// against its folder, as REAPER resolves it. Every `FILE` line counts,
/// whichever source block it sits in (audio, video, MIDI, a section of
/// a take) — a session that opened with one missing is a session that
/// glitches or refuses to play, which is the thing this exists to stop.
#[must_use]
// t[impl files.sync.selective] — the media a session references, read
// off the session itself
pub fn reaper_references(session: &RootPath, text: &str) -> Referenced {
    let folder = session.parent().unwrap_or_else(RootPath::root);
    let mut out = Referenced::default();
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line
            .strip_prefix("FILE ")
            .or_else(|| line.strip_prefix("FILE\t"))
        else {
            continue;
        };
        let Some(raw) = first_token(rest) else {
            continue;
        };
        match resolve(&folder, &raw) {
            Some(path) => {
                out.inside.insert(path);
            }
            None => {
                if !out.outside.contains(&raw) {
                    out.outside.push(raw);
                }
            }
        }
    }
    out
}

/// The media of a session whose file cannot be read: every file in the
/// session's folder under an atomic tool directory.
///
/// `files` is the root's listing. A file directly beside the session,
/// or under a directory no tool made, is not its media — a Pro Tools
/// session folder's `Bounced Files` are mixes, not sources.
pub fn folder_references<'a, I>(session: &RootPath, files: I, facets: &FacetMap) -> Referenced
where
    I: IntoIterator<Item = &'a RootPath>,
{
    let folder = session.parent().unwrap_or_else(RootPath::root);
    let mut out = Referenced::default();
    for file in files {
        if file == session || !file.is_within(&folder) {
            continue;
        }
        let mut probe = file.parent();
        while let Some(dir) = probe {
            if dir == folder || !dir.is_within(&folder) {
                break;
            }
            let binding = facets.resolve(dir.as_str());
            if binding.source != Source::Unmapped {
                if binding.atomic {
                    out.inside.insert(file.clone());
                }
                break;
            }
            probe = dir.parent();
        }
    }
    out
}

/// The first argument of a REAPER line: quoted with `"`, `'` or `` ` ``
/// (REAPER picks whichever the path does not contain), or bare up to
/// whitespace.
fn first_token(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let mut chars = rest.chars();
    let first = chars.next()?;
    if matches!(first, '"' | '\'' | '`') {
        let body = &rest[first.len_utf8()..];
        let end = body.find(first)?;
        let token = &body[..end];
        return (!token.is_empty()).then(|| token.to_owned());
    }
    rest.split_whitespace().next().map(str::to_owned)
}

/// Resolve a reference against the session's folder, or `None` when it
/// cannot be followed inside the root.
fn resolve(folder: &RootPath, raw: &str) -> Option<RootPath> {
    let unified = raw.replace('\\', "/");
    if unified.starts_with('/') || has_drive(&unified) {
        return None;
    }
    let mut parts: Vec<&str> = folder.components().collect();
    for component in unified.split('/') {
        match component {
            "" | "." => {}
            // Climbing out of the root is not a path into it.
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return None;
    }
    RootPath::parse(parts.join("/")).ok()
}

/// `C:/…` — a Windows absolute path.
fn has_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facet::Capability;

    fn p(s: &str) -> RootPath {
        RootPath::parse(s).unwrap()
    }

    /// The shape REAPER writes, trimmed: items whose sources name their
    /// files relative to the project folder, one in a sibling folder,
    /// one absolute from the machine that recorded it.
    const RPP: &str = r#"<REAPER_PROJECT 0.1 "7.09" 1728662400
  RECORD_PATH "Audio Files" ""
  RENDER_FILE "Renders/Track One.wav"
  <TRACK
    NAME Vox
    <ITEM
      <SOURCE WAVE
        FILE "Audio Files/Vox_1.wav"
      >
    >
    <ITEM
      <SOURCE WAVE
        FILE 'Audio Files/Vox "comp".wav'
      >
    >
  >
  <TRACK
    NAME Drums
    <ITEM
      <SOURCE WAVE
        FILE Audio\Drums_1.wav 1
      >
    >
    <ITEM
      <SOURCE WAVE
        FILE "../Shared Samples/clap.wav"
      >
    >
    <ITEM
      <SOURCE WAVE
        FILE "/Users/cody/Desktop/stray.wav"
      >
    >
    <ITEM
      <SOURCE WAVE
        FILE "C:\Samples\kick.wav"
      >
    >
    <ITEM
      <SOURCE WAVE
        FILE "../../../escape.wav"
      >
    >
    <ITEM
      <SOURCE WAVE
        FILE "Audio Files/Vox_1.wav"
      >
    >
  >
>
"#;

    #[test]
    // t[verify files.sync.selective]
    fn a_reaper_session_names_its_media_relative_to_its_folder() {
        let refs = reaper_references(&p("Album/Track One/Track One.RPP"), RPP);
        let inside: Vec<&str> = refs.inside.iter().map(RootPath::as_str).collect();
        assert_eq!(
            inside,
            vec![
                "Album/Shared Samples/clap.wav",
                "Album/Track One/Audio Files/Vox \"comp\".wav",
                "Album/Track One/Audio Files/Vox_1.wav",
                "Album/Track One/Audio/Drums_1.wav",
            ],
            "every FILE line once, resolved against the session's folder"
        );
        assert_eq!(
            refs.outside,
            vec![
                "/Users/cody/Desktop/stray.wav".to_string(),
                "C:\\Samples\\kick.wav".to_string(),
                "../../../escape.wav".to_string(),
            ],
            "what cannot be followed into the root is reported as written"
        );
    }

    #[test]
    fn render_and_record_paths_are_not_media() {
        let refs = reaper_references(&p("Song.rpp"), RPP);
        assert!(
            !refs
                .inside
                .iter()
                .any(|r| r.as_str().starts_with("Renders")),
            "a render target is output, not a source: {refs:?}"
        );
    }

    #[test]
    fn a_session_at_the_top_of_the_root_resolves_from_the_root() {
        let refs = reaper_references(&p("Song.rpp"), "FILE \"Media/a.wav\"\nFILE ../b.wav\n");
        assert_eq!(
            refs.inside.into_iter().collect::<Vec<_>>(),
            vec![p("Media/a.wav")]
        );
        assert_eq!(refs.outside, vec!["../b.wav".to_string()]);
    }

    #[test]
    fn only_rpp_is_read_as_text() {
        assert_eq!(format_of(&p("a/Song.RPP")), Format::Reaper);
        assert_eq!(format_of(&p("a/Song.ptx")), Format::Opaque);
    }

    #[test]
    // t[verify files.sync.selective]
    fn an_unreadable_session_takes_the_atomic_directories_beside_it() {
        let m = FacetMap::new([Capability::MusicProduction]);
        let files: Vec<RootPath> = [
            "01 Song/01 Song.ptx",
            "01 Song/Audio Files/kick.wav",
            "01 Song/Clip Groups/verse.cg",
            "01 Song/Bounced Files/rough.wav",
            "01 Song/notes.txt",
            "02 Song/Audio Files/kick.wav",
        ]
        .iter()
        .map(|s| p(s))
        .collect();
        let refs = folder_references(&p("01 Song/01 Song.ptx"), files.iter(), &m);
        assert_eq!(
            refs.inside.into_iter().collect::<Vec<_>>(),
            vec![
                p("01 Song/Audio Files/kick.wav"),
                p("01 Song/Clip Groups/verse.cg")
            ],
            "its own folder's sources — not its mixes, not the next song's audio"
        );
    }
}

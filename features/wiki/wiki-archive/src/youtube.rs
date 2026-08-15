//! yt-dlp wrapper — video metadata + timestamped transcript.
//!
//! Subprocess boundary: `yt-dlp -J --no-download <url>` (one
//! JSON blob, no media bytes). Subtitle tracks come back as
//! URLs inside that blob; we fetch the best `json3` track
//! (manual captions preferred over ASR) with reqwest — no
//! second subprocess.
//!
//! Everything after the two I/O edges is pure and
//! fixture-tested: `parse_probe_json`, `parse_json3_cues`,
//! `coalesce_cues`, `render_video_markdown`.
//!
//! ## The `^t<seconds>` convention
//!
//! The transcript is written as ~45 s coalesced paragraphs,
//! each ending with a `^t<start-second>` block anchor. That's
//! legal under the EXISTING obsidian block-anchor grammar
//! (`vault-obsidian::BLOCK_ID_REGEX` — `[a-zA-Z0-9-]+` ids),
//! so `[[Sources/x#^t870]]` deep links work today with zero
//! parser changes. Curator notes later land on the generated
//! source *page* (not the immutable raw file) as
//! `- [mm:ss] … ^t<sec>-noteN` bullets under `## Notes`.
//!
//! ## Operational honesty
//!
//! YouTube rotates bot checks; yt-dlp rotates workarounds.
//! Nothing is pinned here — "update yt-dlp" is the standing
//! fix, and failures surface as retryable errors, never
//! partial archives. (A bgutil POT-provider sidecar exists
//! for hard cases; see `plans/wiki-archive.md`.)

use std::time::Duration;

use serde_json::Value;

use crate::ArchiveError;
use crate::provenance::mmss;

/// Default coalescing window for transcript blocks. PRD says
/// "~30–60 s"; 45 splits the difference.
pub const DEFAULT_BLOCK_SECS: u64 = 45;

/// How long one `yt-dlp -J` probe may run before we kill it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// Probe attempts (YouTube's bot checks are flaky-transient;
/// one retry catches most of them).
const PROBE_ATTEMPTS: u32 = 2;

/// Handle to the yt-dlp binary (path comes from `--yt-dlp` /
/// `TASK_YTDLP`; nothing is bundled or pinned).
#[derive(Debug, Clone)]
pub struct YtDlp {
    pub binary: String,
}

/// What we keep from `yt-dlp -J`.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoMeta {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration_secs: Option<u64>,
    /// `YYYYMMDD` as yt-dlp reports it.
    pub upload_date: Option<String>,
    pub description: String,
    pub chapters: Vec<Chapter>,
    /// Best timestamped-subtitle track: `(lang, url)` for a
    /// `json3` format, manual captions preferred over ASR.
    pub json3_track: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub title: String,
    pub start_secs: u64,
}

/// One subtitle cue (pre-coalescing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_secs: u64,
    pub text: String,
}

/// One rendered transcript paragraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptBlock {
    pub start_secs: u64,
    pub text: String,
}

impl YtDlp {
    #[must_use]
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// `yt-dlp -J --no-download <url>` with timeout + retry.
    pub async fn probe(&self, url: &str) -> Result<VideoMeta, ArchiveError> {
        let mut last = String::new();
        for attempt in 1..=PROBE_ATTEMPTS {
            match self.probe_once(url).await {
                Ok(meta) => return Ok(meta),
                Err(ArchiveError::Subprocess { message, .. }) if attempt < PROBE_ATTEMPTS => {
                    tracing::warn!(attempt, %message, "yt-dlp probe failed; retrying");
                    last = message;
                }
                Err(e) => return Err(e),
            }
        }
        Err(ArchiveError::Subprocess {
            program: self.binary.clone(),
            message: format!("{last} (after {PROBE_ATTEMPTS} attempts — try updating yt-dlp)"),
        })
    }

    async fn probe_once(&self, url: &str) -> Result<VideoMeta, ArchiveError> {
        let child = tokio::process::Command::new(&self.binary)
            .args(["-J", "--no-download", "--no-warnings", url])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .spawn()
            .map_err(|e| ArchiveError::Subprocess {
                program: self.binary.clone(),
                message: if e.kind() == std::io::ErrorKind::NotFound {
                    "binary not found — install yt-dlp or pass --yt-dlp/TASK_YTDLP".to_string()
                } else {
                    format!("spawn: {e}")
                },
            })?;
        let out = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| ArchiveError::Subprocess {
                program: self.binary.clone(),
                message: format!("probe timed out after {}s", PROBE_TIMEOUT.as_secs()),
            })?
            .map_err(|e| ArchiveError::Subprocess {
                program: self.binary.clone(),
                message: format!("wait: {e}"),
            })?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(ArchiveError::Subprocess {
                program: self.binary.clone(),
                message: format!(
                    "exit {}: {}",
                    out.status,
                    stderr.lines().last().unwrap_or("(no stderr)")
                ),
            });
        }
        parse_probe_json(&String::from_utf8_lossy(&out.stdout))
    }
}

/// Parse `yt-dlp -J` output. Pure — fixture-tested.
pub fn parse_probe_json(json: &str) -> Result<VideoMeta, ArchiveError> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| ArchiveError::ImportParse(format!("yt-dlp -J: {e}")))?;
    // Playlist probes nest entries; take the first (archiving
    // whole playlists is out of scope for phase 1).
    let v = if v.get("entries").is_some() {
        v.get("entries")
            .and_then(|e| e.get(0))
            .cloned()
            .ok_or_else(|| ArchiveError::ImportParse("playlist with no entries".into()))?
    } else {
        v
    };

    let str_of = |k: &str| v.get(k).and_then(Value::as_str).map(ToString::to_string);
    let title = str_of("title").unwrap_or_default();
    if title.is_empty() {
        return Err(ArchiveError::ImportParse("yt-dlp -J: missing title".into()));
    }

    let chapters = v
        .get("chapters")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    Some(Chapter {
                        title: c.get("title")?.as_str()?.to_string(),
                        start_secs: c.get("start_time")?.as_f64()? as u64,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(VideoMeta {
        id: str_of("id").unwrap_or_default(),
        title,
        channel: str_of("channel").or_else(|| str_of("uploader")),
        duration_secs: v.get("duration").and_then(Value::as_f64).map(|d| d as u64),
        upload_date: str_of("upload_date"),
        description: str_of("description").unwrap_or_default(),
        chapters,
        json3_track: pick_json3_track(&v),
    })
}

/// Prefer manual `subtitles` over `automatic_captions`;
/// within each, prefer English-family tracks, then anything.
fn pick_json3_track(v: &Value) -> Option<(String, String)> {
    for key in ["subtitles", "automatic_captions"] {
        let Some(map) = v.get(key).and_then(Value::as_object) else {
            continue;
        };
        let mut langs: Vec<&String> = map.keys().collect();
        // `en`, `en-US`, `en-orig`, … sort before the rest.
        langs.sort_by_key(|l| (!l.starts_with("en"), l.as_str().to_string()));
        for lang in langs {
            let Some(tracks) = map.get(lang).and_then(Value::as_array) else {
                continue;
            };
            for t in tracks {
                if t.get("ext").and_then(Value::as_str) == Some("json3") {
                    if let Some(url) = t.get("url").and_then(Value::as_str) {
                        return Some((lang.clone(), url.to_string()));
                    }
                }
            }
        }
    }
    None
}

/// Parse a json3 subtitle track into cues. json3 carries
/// word-level offsets (`segs[].tOffsetMs`); we keep event
/// granularity — the coalescer below is what sets block size.
pub fn parse_json3_cues(json: &str) -> Result<Vec<Cue>, ArchiveError> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| ArchiveError::ImportParse(format!("json3: {e}")))?;
    let events = v
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| ArchiveError::ImportParse("json3: no events array".into()))?;
    let mut cues = Vec::new();
    for ev in events {
        let Some(segs) = ev.get("segs").and_then(Value::as_array) else {
            continue; // style/window events carry no text
        };
        let start_ms = ev.get("tStartMs").and_then(Value::as_u64).unwrap_or(0);
        let text: String = segs
            .iter()
            .filter_map(|s| s.get("utf8").and_then(Value::as_str))
            .collect::<String>()
            .replace('\n', " ")
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        cues.push(Cue {
            start_secs: start_ms / 1000,
            text,
        });
    }
    Ok(cues)
}

/// Coalesce cues into ~`window_secs` paragraphs. Each block
/// keeps the start second of its first cue — that second IS
/// the anchor id.
#[must_use]
pub fn coalesce_cues(cues: &[Cue], window_secs: u64) -> Vec<TranscriptBlock> {
    let mut blocks: Vec<TranscriptBlock> = Vec::new();
    let mut current: Option<TranscriptBlock> = None;
    for cue in cues {
        match &mut current {
            Some(block) if cue.start_secs.saturating_sub(block.start_secs) < window_secs => {
                block.text.push(' ');
                block.text.push_str(&cue.text);
            }
            _ => {
                if let Some(done) = current.take() {
                    blocks.push(done);
                }
                current = Some(TranscriptBlock {
                    start_secs: cue.start_secs,
                    text: cue.text.clone(),
                });
            }
        }
    }
    if let Some(done) = current.take() {
        blocks.push(done);
    }
    blocks
}

/// Render the raw-source markdown body for a video: metadata
/// lede, chapters (when present), then the anchored
/// transcript. (Title/heading + frontmatter come from
/// [`crate::provenance::compose_source_markdown`].)
#[must_use]
pub fn render_video_markdown(meta: &VideoMeta, blocks: &[TranscriptBlock]) -> String {
    let mut out = String::new();

    let mut lede: Vec<String> = Vec::new();
    if let Some(c) = &meta.channel {
        lede.push(c.clone());
    }
    if let Some(d) = &meta.upload_date {
        if d.len() == 8 {
            lede.push(format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]));
        }
    }
    if let Some(secs) = meta.duration_secs {
        lede.push(mmss(secs));
    }
    if !lede.is_empty() {
        out.push_str(&format!("_{}_\n\n", lede.join(" · ")));
    }

    if !meta.description.trim().is_empty() {
        out.push_str("## Description\n\n");
        out.push_str(meta.description.trim());
        out.push_str("\n\n");
    }

    if !meta.chapters.is_empty() {
        out.push_str("## Chapters\n\n");
        for ch in &meta.chapters {
            out.push_str(&format!("- [{}] {}\n", mmss(ch.start_secs), ch.title));
        }
        out.push('\n');
    }

    out.push_str("## Transcript\n\n");
    if blocks.is_empty() {
        out.push_str("_No captions available for this video._\n");
    } else {
        for b in blocks {
            out.push_str(&format!(
                "[{}] {} ^t{}\n\n",
                mmss(b.start_secs),
                b.text,
                b.start_secs
            ));
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed `yt-dlp -J` shape (fields we read + decoys).
    const PROBE_FIXTURE: &str = r#"{
      "id": "dQw4w9WgXcQ",
      "title": "Never Gonna Give You Up",
      "channel": "Rick Astley",
      "uploader": "rickastleyofficial",
      "duration": 213.1,
      "upload_date": "20091025",
      "description": "The official video.",
      "chapters": [
        {"title": "Intro", "start_time": 0.0, "end_time": 19.0},
        {"title": "Chorus", "start_time": 43.5, "end_time": 60.0}
      ],
      "subtitles": {
        "en": [
          {"ext": "vtt", "url": "https://example.com/en.vtt"},
          {"ext": "json3", "url": "https://example.com/en.json3"}
        ]
      },
      "automatic_captions": {
        "en-orig": [{"ext": "json3", "url": "https://example.com/asr.json3"}]
      }
    }"#;

    const JSON3_FIXTURE: &str = r#"{
      "wireMagic": "pb3",
      "events": [
        {"tStartMs": 0, "dDurationMs": 2000, "wWinId": 1},
        {"tStartMs": 480, "dDurationMs": 3000, "segs": [
          {"utf8": "We're no strangers", "tOffsetMs": 0},
          {"utf8": " to love", "tOffsetMs": 900}
        ]},
        {"tStartMs": 4000, "segs": [{"utf8": "\n"}]},
        {"tStartMs": 5200, "segs": [{"utf8": "You know the rules"}]},
        {"tStartMs": 50100, "segs": [{"utf8": "and so do I"}]},
        {"tStartMs": 99000, "segs": [{"utf8": "A full commitment's"}]}
      ]
    }"#;

    #[test]
    fn probe_parses_meta_and_prefers_manual_json3() {
        let m = parse_probe_json(PROBE_FIXTURE).unwrap();
        assert_eq!(m.id, "dQw4w9WgXcQ");
        assert_eq!(m.channel.as_deref(), Some("Rick Astley"));
        assert_eq!(m.duration_secs, Some(213));
        assert_eq!(m.chapters.len(), 2);
        assert_eq!(m.chapters[1].start_secs, 43);
        // Manual `en` json3 wins over ASR `en-orig`.
        assert_eq!(
            m.json3_track,
            Some(("en".to_string(), "https://example.com/en.json3".to_string()))
        );
    }

    #[test]
    fn probe_falls_back_to_asr_track() {
        let v: Value = serde_json::from_str(PROBE_FIXTURE).unwrap();
        let mut v = v;
        v.as_object_mut().unwrap().remove("subtitles");
        let m = parse_probe_json(&v.to_string()).unwrap();
        assert_eq!(
            m.json3_track,
            Some((
                "en-orig".to_string(),
                "https://example.com/asr.json3".to_string()
            ))
        );
    }

    #[test]
    fn json3_cues_skip_styling_and_blank_events() {
        let cues = parse_json3_cues(JSON3_FIXTURE).unwrap();
        assert_eq!(cues.len(), 4);
        assert_eq!(cues[0].start_secs, 0);
        assert_eq!(cues[0].text, "We're no strangers to love");
        assert_eq!(cues[1].text, "You know the rules");
    }

    #[test]
    fn coalesce_windows_by_start_second() {
        let cues = parse_json3_cues(JSON3_FIXTURE).unwrap();
        let blocks = coalesce_cues(&cues, DEFAULT_BLOCK_SECS);
        // 0 + 5 in one block; 50 in the second; 99 in third.
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].start_secs, 0);
        assert!(blocks[0].text.contains("You know the rules"));
        assert_eq!(blocks[1].start_secs, 50);
        assert_eq!(blocks[2].start_secs, 99);
    }

    #[test]
    fn rendered_transcript_anchors_are_block_id_legal() {
        let m = parse_probe_json(PROBE_FIXTURE).unwrap();
        let cues = parse_json3_cues(JSON3_FIXTURE).unwrap();
        let blocks = coalesce_cues(&cues, DEFAULT_BLOCK_SECS);
        let md = render_video_markdown(&m, &blocks);
        assert!(md.contains("_Rick Astley · 2009-10-25 · 3:33_"));
        assert!(md.contains("## Chapters"));
        assert!(md.contains("- [0:43] Chorus"));
        // Anchor sits at end-of-line: `… ^t50` — exactly the
        // shape vault-obsidian's BLOCK_ID_REGEX recognizes.
        assert!(md.contains(" ^t0\n"), "{md}");
        assert!(md.contains("[0:50] and so do I ^t50"), "{md}");
        let re = regex_lite_check(&md);
        assert!(re, "every transcript line must end with ^t<sec>");
    }

    /// Cheap stand-in for vault-obsidian's BLOCK_ID_REGEX
    /// (`(?:^|\s)\^([a-zA-Z0-9\-]+)\s*$`) without a regex dep:
    /// every transcript paragraph ends in ` ^t<digits>`.
    fn regex_lite_check(md: &str) -> bool {
        md.lines()
            .filter(|l| l.starts_with('[') && l.contains("^t"))
            .all(|l| {
                l.rsplit_once(" ^t")
                    .is_some_and(|(_, id)| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
            })
    }

    #[test]
    fn no_captions_renders_honest_placeholder() {
        let m = parse_probe_json(PROBE_FIXTURE).unwrap();
        let md = render_video_markdown(&m, &[]);
        assert!(md.contains("_No captions available for this video._"));
    }

    #[test]
    fn playlist_probe_takes_first_entry() {
        let wrapped = format!(r#"{{"entries": [{PROBE_FIXTURE}]}}"#);
        let m = parse_probe_json(&wrapped).unwrap();
        assert_eq!(m.id, "dQw4w9WgXcQ");
    }
}

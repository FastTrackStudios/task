//! Timestamped transcript for a video/audio resource (a sermon, a talk,
//! a podcast). Stored as a second sidecar next to the resource —
//! `<slug>.transcript.json` — alongside the annotation sidecar. The
//! transcript is the *content* of the resource (read-only, like the
//! Bible text); annotations point *into* it by timestamp anchor (`#t:secs`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ResourceError;

/// One transcript cue: spoken text and when it occurs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Start time in seconds.
    pub start: f32,
    /// Duration in seconds.
    #[serde(default)]
    pub dur: f32,
    pub text: String,
}

impl TranscriptSegment {
    /// End time in seconds.
    #[must_use]
    pub fn end(&self) -> f32 {
        self.start + self.dur
    }
}

/// A resource's full transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Transcript {
    pub slug: String,
    /// Where it came from (`youtube-auto`, `whisper`, `manual`…).
    #[serde(default)]
    pub source: String,
    pub segments: Vec<TranscriptSegment>,
}

impl Transcript {
    #[must_use]
    pub fn new(slug: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            source: source.into(),
            segments: Vec::new(),
        }
    }

    /// Total length in seconds (end of the last cue).
    #[must_use]
    pub fn duration(&self) -> f32 {
        self.segments.last().map_or(0.0, TranscriptSegment::end)
    }

    /// The cue covering `secs` (or the last one before it) — what is
    /// being said at a moment, for an annotation's captured text.
    #[must_use]
    pub fn at(&self, secs: f32) -> Option<&TranscriptSegment> {
        // Segments are time-ordered; take the last one that has started.
        self.segments.iter().take_while(|s| s.start <= secs).last()
    }

    /// Joined text spoken in `[from, to)` seconds — the passage an
    /// annotation covers when it spans more than one cue.
    #[must_use]
    pub fn text_between(&self, from: f32, to: f32) -> String {
        let mut out = String::new();
        for s in self
            .segments
            .iter()
            .filter(|s| s.start >= from && s.start < to)
        {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(s.text.trim());
        }
        out
    }
}

/// Parse YouTube's `json3` caption format (`yt-dlp --sub-format json3`)
/// into cues. Each `events[]` entry carries `tStartMs` + `dDurationMs`
/// and a list of text `segs`; events with no visible text (formatting /
/// blank) are skipped.
pub fn parse_json3(json: &str) -> Result<Vec<TranscriptSegment>, ResourceError> {
    #[derive(serde::Deserialize)]
    struct Json3 {
        #[serde(default)]
        events: Vec<Event>,
    }
    #[derive(serde::Deserialize)]
    struct Event {
        #[serde(rename = "tStartMs", default)]
        t_start_ms: i64,
        #[serde(rename = "dDurationMs", default)]
        d_dur_ms: i64,
        #[serde(default)]
        segs: Vec<Seg>,
    }
    #[derive(serde::Deserialize)]
    struct Seg {
        #[serde(default)]
        utf8: String,
    }

    let doc: Json3 = serde_json::from_str(json).map_err(|e| ResourceError::Json(e.to_string()))?;
    let mut out = Vec::new();
    for e in doc.events {
        let text: String = e.segs.iter().map(|s| s.utf8.as_str()).collect();
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        out.push(TranscriptSegment {
            start: e.t_start_ms as f32 / 1000.0,
            dur: e.d_dur_ms as f32 / 1000.0,
            text: text.to_string(),
        });
    }
    Ok(out)
}

/// Sidecar path for a resource's transcript:
/// `…/<slug>.transcript.json`.
#[must_use]
pub fn transcript_path(resource_path: impl AsRef<Path>) -> PathBuf {
    resource_path.as_ref().with_extension("transcript.json")
}

/// Load a transcript; a missing file is an empty (not error) transcript.
pub fn load(path: impl AsRef<Path>) -> Result<Transcript, ResourceError> {
    match std::fs::read_to_string(path.as_ref()) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| ResourceError::Json(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Transcript::default()),
        Err(e) => Err(ResourceError::Io(e.to_string())),
    }
}

/// Write a transcript sidecar (compact JSON — transcripts are large).
pub fn save(path: impl AsRef<Path>, transcript: &Transcript) -> Result<(), ResourceError> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent).map_err(|e| ResourceError::Io(e.to_string()))?;
    }
    let json = serde_json::to_string(transcript).map_err(|e| ResourceError::Json(e.to_string()))?;
    std::fs::write(path, json).map_err(|e| ResourceError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f32, dur: f32, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start,
            dur,
            text: text.into(),
        }
    }

    #[test]
    fn at_finds_current_cue() {
        let mut t = Transcript::new("s", "test");
        t.segments = vec![
            seg(0.0, 2.0, "hello"),
            seg(2.0, 3.0, "there"),
            seg(5.0, 2.0, "world"),
        ];
        assert_eq!(t.at(3.5).unwrap().text, "there");
        assert_eq!(t.at(6.0).unwrap().text, "world");
        assert!(t.at(-1.0).is_none());
        assert!((t.duration() - 7.0).abs() < 1e-6);
    }

    #[test]
    fn text_between_joins_cues() {
        let mut t = Transcript::new("s", "test");
        t.segments = vec![
            seg(0.0, 2.0, "I have"),
            seg(2.0, 2.0, "sinned"),
            seg(10.0, 2.0, "later"),
        ];
        assert_eq!(t.text_between(0.0, 5.0), "I have sinned");
    }

    #[test]
    fn path_swaps_extension() {
        let p = transcript_path("/x/sermons/god-restores.md");
        assert!(p.ends_with("god-restores.transcript.json"));
    }

    #[test]
    fn parse_json3_extracts_cues_and_skips_blanks() {
        let json = r#"{"events":[
          {"tStartMs":0,"dDurationMs":1500,"segs":[{"utf8":"Hello "},{"utf8":"there"}]},
          {"tStartMs":1500,"dDurationMs":500,"segs":[{"utf8":"\n"}]},
          {"tStartMs":2000,"dDurationMs":2000,"segs":[{"utf8":"world"}]}
        ]}"#;
        let segs = parse_json3(json).unwrap();
        assert_eq!(segs.len(), 2); // the newline-only event is dropped
        assert_eq!(segs[0].text, "Hello there");
        assert!((segs[0].start - 0.0).abs() < 1e-6);
        assert!((segs[1].start - 2.0).abs() < 1e-6);
        assert_eq!(segs[1].text, "world");
    }
}

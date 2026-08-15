//! Transcript format parsers — SRT, WebVTT, and the
//! podcasting-2.0 JSON shape — all normalizing into the same
//! [`Cue`] stream the YouTube path uses, so coalescing and
//! `^t<sec>` rendering are shared verbatim.
//!
//! The `<podcast:transcript>` tag is the FAST PATH: ~5.1M
//! episodes carry one, and using it means no audio fetch and
//! no transcription model at all.

use crate::ArchiveError;
use crate::feed::TranscriptRef;
use crate::youtube::Cue;

/// Order transcript references by how well we parse them:
/// VTT and SRT carry clean timestamps; JSON (podcasting-2.0)
/// is fine too; HTML/plain-text lack timestamps and are
/// skipped entirely (a transcript without timestamps can't
/// anchor).
#[must_use]
pub fn pick_transcripts(refs: &[TranscriptRef]) -> Vec<&TranscriptRef> {
    let rank = |mime: &str| -> Option<u8> {
        let mime = mime.to_ascii_lowercase();
        match mime.as_str() {
            "text/vtt" | "application/x-subrip+vtt" => Some(0),
            "application/srt" | "application/x-subrip" | "text/srt" => Some(1),
            "application/json" => Some(2),
            // No declared type: sniffable at parse time.
            "" => Some(3),
            _ => None, // text/html, text/plain — no timestamps
        }
    };
    let mut ranked: Vec<(u8, &TranscriptRef)> = refs
        .iter()
        .filter_map(|r| rank(&r.mime).map(|k| (k, r)))
        .collect();
    ranked.sort_by_key(|(k, _)| *k);
    ranked.into_iter().map(|(_, r)| r).collect()
}

/// Parse transcript `content` according to its declared MIME
/// type (sniffing when the feed omitted it).
pub fn parse_transcript(content: &str, mime: &str) -> Result<Vec<Cue>, ArchiveError> {
    let mime = mime.to_ascii_lowercase();
    let cues = match mime.as_str() {
        "text/vtt" => parse_vtt(content),
        "application/srt" | "application/x-subrip" | "text/srt" => parse_srt(content),
        "application/json" => parse_json_transcript(content)?,
        "" => sniff_parse(content)?,
        other => {
            return Err(ArchiveError::ImportParse(format!(
                "transcript type `{other}` carries no usable timestamps"
            )));
        }
    };
    if cues.is_empty() {
        return Err(ArchiveError::ImportParse(
            "transcript parsed to zero cues".into(),
        ));
    }
    Ok(cues)
}

fn sniff_parse(content: &str) -> Result<Vec<Cue>, ArchiveError> {
    let head = content.trim_start();
    if head.starts_with("WEBVTT") {
        Ok(parse_vtt(content))
    } else if head.starts_with('{') || head.starts_with('[') {
        parse_json_transcript(content)
    } else {
        Ok(parse_srt(content))
    }
}

/// SRT: blank-line-separated blocks of
/// `index? / HH:MM:SS,mmm --> HH:MM:SS,mmm / text…`.
#[must_use]
pub fn parse_srt(s: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    for block in s.replace("\r\n", "\n").split("\n\n") {
        let mut start: Option<u64> = None;
        let mut text_lines: Vec<&str> = Vec::new();
        for line in block.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if start.is_none() {
                if let Some((from, _)) = line.split_once("-->") {
                    start = parse_timestamp(from.trim());
                    continue;
                }
                // Numeric index line (or junk) before the
                // timestamp — skip.
                if line.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
            } else {
                text_lines.push(line);
            }
        }
        if let Some(start_secs) = start {
            let text = strip_tags(&text_lines.join(" "));
            if !text.is_empty() {
                cues.push(Cue { start_secs, text });
            }
        }
    }
    cues
}

/// WebVTT: like SRT but with a `WEBVTT` header, optional
/// `NOTE`/`STYLE`/`REGION` blocks, dot-millis timestamps that
/// may omit hours, and `<v Speaker>`-style inline tags.
#[must_use]
pub fn parse_vtt(s: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    for block in s.replace("\r\n", "\n").split("\n\n") {
        let head = block.trim_start();
        if head.starts_with("WEBVTT")
            || head.starts_with("NOTE")
            || head.starts_with("STYLE")
            || head.starts_with("REGION")
        {
            continue;
        }
        let mut start: Option<u64> = None;
        let mut text_lines: Vec<&str> = Vec::new();
        for line in block.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if start.is_none() {
                if let Some((from, _)) = line.split_once("-->") {
                    start = parse_timestamp(from.trim());
                }
                // else: cue identifier line — skip.
            } else {
                text_lines.push(line);
            }
        }
        if let Some(start_secs) = start {
            let text = strip_tags(&text_lines.join(" "));
            if !text.is_empty() {
                cues.push(Cue { start_secs, text });
            }
        }
    }
    cues
}

/// Podcasting-2.0 JSON transcript:
/// `{"segments": [{"startTime": 0.8, "body": "…", "speaker": "Alice"}, …]}`.
/// Speaker labels are kept inline when they change — that's
/// real information in interview shows.
pub fn parse_json_transcript(s: &str) -> Result<Vec<Cue>, ArchiveError> {
    let v: serde_json::Value = serde_json::from_str(s)
        .map_err(|e| ArchiveError::ImportParse(format!("json transcript: {e}")))?;
    let segments = v
        .get("segments")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ArchiveError::ImportParse("json transcript: no segments array".into()))?;
    let mut cues = Vec::new();
    let mut last_speaker = String::new();
    for seg in segments {
        let start = seg
            .get("startTime")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
            .max(0.0) as u64;
        let body = seg
            .get("body")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        if body.is_empty() {
            continue;
        }
        let speaker = seg
            .get("speaker")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        let text = if !speaker.is_empty() && speaker != last_speaker {
            last_speaker = speaker.to_string();
            format!("**{speaker}:** {body}")
        } else {
            body.to_string()
        };
        cues.push(Cue {
            start_secs: start,
            text,
        });
    }
    Ok(cues)
}

/// Whisper segments → cues. whisper.cpp reports `t0`/`t1` in
/// 10 ms units; this is the pure half of the transcription
/// path, testable without the `whisper` feature or a model.
#[must_use]
pub fn segments_to_cues<I, S>(segments: I) -> Vec<Cue>
where
    I: IntoIterator<Item = (i64, S)>,
    S: AsRef<str>,
{
    segments
        .into_iter()
        .filter_map(|(t0_centi_x10, text)| {
            let text = text.as_ref().trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(Cue {
                // 10 ms units → whole seconds.
                start_secs: (t0_centi_x10.max(0) as u64) / 100,
                text,
            })
        })
        .collect()
}

/// `HH:MM:SS,mmm`, `HH:MM:SS.mmm`, or `MM:SS.mmm` → whole
/// seconds.
fn parse_timestamp(s: &str) -> Option<u64> {
    let s = s.split_whitespace().next()?; // drop cue settings
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<u64>().ok()?, m.parse::<u64>().ok()?, *s),
        [m, s] => (0, m.parse::<u64>().ok()?, *s),
        _ => return None,
    };
    let secs = sec.split('.').next()?.parse::<u64>().ok()?;
    Some(h * 3600 + m * 60 + secs)
}

/// Drop `<v Name>` / `<i>` / `{…}`-style inline markup.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_angle = false;
    let mut in_brace = false;
    for c in s.chars() {
        match c {
            '<' => in_angle = true,
            '>' => in_angle = false,
            '{' => in_brace = true,
            '}' => in_brace = false,
            c if !in_angle && !in_brace => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:04,000\nFirst line\ncontinues here.\n\n2\n00:01:02,500 --> 00:01:05,000\n<i>Second</i> cue.\n\n";

    const VTT: &str = "WEBVTT\n\nNOTE this is a comment\n\n00:01.000 --> 00:04.000\n<v Alice>Hello there.\n\nchapter-2\n01:00:02.000 --> 01:00:05.000 align:start\nDeep into hour one.\n";

    const JSON: &str = r#"{"version":"1.0.0","segments":[
        {"speaker":"Alice","startTime":0.8,"endTime":4.2,"body":"Hi."},
        {"speaker":"Alice","startTime":4.5,"endTime":7.0,"body":"Still me."},
        {"speaker":"Bob","startTime":7.5,"endTime":9.9,"body":"Hello!"}
    ]}"#;

    #[test]
    fn srt_parses_and_strips_tags() {
        let cues = parse_srt(SRT);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start_secs, 1);
        assert_eq!(cues[0].text, "First line continues here.");
        assert_eq!(cues[1].start_secs, 62);
        assert_eq!(cues[1].text, "Second cue.");
    }

    #[test]
    fn vtt_skips_notes_handles_short_and_long_stamps() {
        let cues = parse_vtt(VTT);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start_secs, 1);
        assert_eq!(cues[0].text, "Hello there.");
        assert_eq!(cues[1].start_secs, 3602);
    }

    #[test]
    fn json_keeps_speaker_changes_only() {
        let cues = parse_json_transcript(JSON).unwrap();
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].text, "**Alice:** Hi.");
        assert_eq!(cues[1].text, "Still me.");
        assert_eq!(cues[2].text, "**Bob:** Hello!");
        assert_eq!(cues[2].start_secs, 7);
    }

    #[test]
    fn ranking_prefers_vtt_then_srt_then_json() {
        let refs = vec![
            TranscriptRef {
                url: "j".into(),
                mime: "application/json".into(),
            },
            TranscriptRef {
                url: "h".into(),
                mime: "text/html".into(),
            },
            TranscriptRef {
                url: "s".into(),
                mime: "application/srt".into(),
            },
            TranscriptRef {
                url: "v".into(),
                mime: "text/vtt".into(),
            },
        ];
        let picked: Vec<&str> = pick_transcripts(&refs)
            .iter()
            .map(|r| r.url.as_str())
            .collect();
        assert_eq!(picked, vec!["v", "s", "j"]); // html dropped
    }

    #[test]
    fn sniffing_dispatches_without_mime() {
        assert!(parse_transcript(VTT, "").unwrap().len() == 2);
        assert!(parse_transcript(SRT, "").unwrap().len() == 2);
        assert!(parse_transcript(JSON, "").unwrap().len() == 3);
        assert!(parse_transcript("plain words, no stamps", "text/plain").is_err());
    }

    #[test]
    fn whisper_segments_convert_10ms_units() {
        let cues = segments_to_cues(vec![
            (0_i64, " Hello"),
            (4_550, "world"),
            (12_000, ""),
            (700_000, "much later"),
        ]);
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].start_secs, 0);
        assert_eq!(cues[0].text, "Hello");
        assert_eq!(cues[1].start_secs, 45);
        assert_eq!(cues[2].start_secs, 7000);
    }
}

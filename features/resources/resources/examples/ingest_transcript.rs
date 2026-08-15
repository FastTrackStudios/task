//! Fetch a YouTube video's auto-captions with `yt-dlp` and write them as
//! a transcript sidecar the watch view reads. Captions only — no whisper,
//! no audio download, so no ffmpeg needed. `yt-dlp` must be on PATH (it's
//! in the flake's dev shell + the server image).
//!
//! Run:
//!   cargo run -p resources --example ingest_transcript -- <youtube-url-or-id> [KIND] [ORG_ROOT]
//! KIND defaults to `video` (→ `resources/videos/<id>.transcript.json`);
//! pass `sermon`/`song` to target those dirs. The slug is the video id.

use std::path::PathBuf;
use std::process::Command;

use resources::{Transcript, parse_json3, transcript};

/// Bare 11-char id, or pull it out of a URL.
fn video_id(input: &str) -> Option<String> {
    let s = input.trim();
    let rest = if let Some(i) = s.find("v=") {
        &s[i + 2..]
    } else if let Some(i) = s.find("youtu.be/") {
        &s[i + 9..]
    } else if let Some(i) = s.find("/embed/") {
        &s[i + 7..]
    } else {
        s
    };
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    (id.len() >= 6).then_some(id)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .expect("usage: ingest_transcript <url-or-id> [kind] [org_root]");
    let kind = args.next().unwrap_or_else(|| "video".to_string());
    let org = args.next().map_or_else(
        || PathBuf::from(std::env::var("HOME").unwrap()).join(".task/orgs/codywright"),
        PathBuf::from,
    );

    let id = video_id(&input).expect("could not parse a YouTube id");
    let tmp = std::env::temp_dir().join(format!("yt-transcript-{id}"));
    std::fs::create_dir_all(&tmp).expect("mkdir tmp");

    // Captions only: skip the video, write English auto-subs as json3.
    let out_tmpl = tmp.join("%(id)s").to_string_lossy().into_owned();
    let status = Command::new("yt-dlp")
        .args([
            "--skip-download",
            "--write-auto-subs",
            "--write-subs",
            "--sub-langs",
            "en.*",
            "--sub-format",
            "json3",
            "-o",
            &out_tmpl,
            &format!("https://youtu.be/{id}"),
        ])
        .status()
        .expect("failed to run yt-dlp (is it on PATH? `nix develop`)");
    assert!(status.success(), "yt-dlp exited with {status}");

    // yt-dlp writes `<id>.<lang>.json3`; take the first one.
    let json3 = std::fs::read_dir(&tmp)
        .expect("read tmp")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "json3"))
        .expect("no .json3 captions produced (video may have none)");
    let raw = std::fs::read_to_string(&json3).expect("read captions");
    let segments = parse_json3(&raw).expect("parse json3");

    let mut t = Transcript::new(&id, "youtube-auto");
    t.segments = segments;
    let dur_min = t.duration() / 60.0;

    let sidecar = org.join(format!("resources/{kind}s/{id}.transcript.json"));
    transcript::save(&sidecar, &t).expect("write sidecar");
    let _ = std::fs::remove_dir_all(&tmp);

    println!(
        "{id}: {} cues ({dur_min:.1} min) → {}",
        t.segments.len(),
        sidecar.display()
    );
}

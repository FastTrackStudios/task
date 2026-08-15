//! Podcast resolution + rendering.
//!
//! The directory dance (live-verified shapes, 2026-06):
//!
//! - **Apple**: `podcasts.apple.com/...id<podcast>` →
//!   `itunes.apple.com/lookup?id=<podcast>` → `feedUrl`. An
//!   episode link (`?i=<episode>`) needs a second hop: a
//!   DIRECT `lookup?id=<episode>` returns 0 results — you
//!   must list the show's episodes via
//!   `entity=podcastEpisode` and match `trackId`.
//! - **Spotify**: no public audio, no public transcript. We
//!   take the oEmbed title (metadata-only) and optionally try
//!   Podcast Index to find the show's public RSS feed; when
//!   that fails the archive is an honest stub.
//! - **Podcast Index**: title-search resolver. Auth =
//!   `sha1(key + secret + unix_time)` in three headers.
//!
//! Enclosure URLs are wrapped in stacked redirect trackers
//! (podtrac → chartable → cdn …) — the dedicated client
//! follows a deeper redirect chain than the article client.

use sha1::{Digest, Sha1};

use crate::ArchiveError;
use crate::article::USER_AGENT;
use crate::feed::FeedItem;
use crate::provenance::mmss;
use crate::youtube::TranscriptBlock;

/// Show-level resolution from the iTunes lookup API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplePodcast {
    pub feed_url: String,
    pub show_title: Option<String>,
}

/// GET the iTunes lookup for a podcast id → its RSS feed URL.
pub async fn apple_lookup_feed(
    client: &reqwest::Client,
    podcast_id: &str,
) -> Result<ApplePodcast, ArchiveError> {
    let url = format!("https://itunes.apple.com/lookup?id={podcast_id}");
    let v = fetch_json(client, &url).await?;
    let result = v
        .get("results")
        .and_then(serde_json::Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| ArchiveError::BadResponse {
            url: url.clone(),
            message: "itunes lookup returned no results for this podcast id".into(),
        })?;
    let feed_url = result
        .get("feedUrl")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ArchiveError::BadResponse {
            url,
            message: "itunes lookup result has no feedUrl (Apple-hosted exclusive?)".into(),
        })?
        .to_string();
    Ok(ApplePodcast {
        feed_url,
        show_title: result
            .get("collectionName")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
    })
}

/// Resolve an episode id (`?i=…`) to its title by LISTING the
/// show's episodes — a direct `lookup?id=<episode>` returns 0
/// results (live-verified), so this is the only working path.
pub async fn apple_lookup_episode_title(
    client: &reqwest::Client,
    podcast_id: &str,
    episode_id: &str,
) -> Result<Option<String>, ArchiveError> {
    let url =
        format!("https://itunes.apple.com/lookup?id={podcast_id}&entity=podcastEpisode&limit=200");
    let v = fetch_json(client, &url).await?;
    let Some(results) = v.get("results").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    Ok(find_episode_title(results, episode_id))
}

/// Pure matcher over the lookup payload — `trackId` arrives
/// as a JSON number; the URL param is a string.
#[must_use]
pub fn find_episode_title(results: &[serde_json::Value], episode_id: &str) -> Option<String> {
    results.iter().find_map(|r| {
        let is_episode = r
            .get("wrapperType")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|w| w == "podcastEpisode");
        if !is_episode {
            return None;
        }
        let track_id = r.get("trackId")?;
        let matches = match track_id {
            serde_json::Value::Number(n) => n.to_string() == episode_id,
            serde_json::Value::String(s) => s == episode_id,
            _ => false,
        };
        if !matches {
            return None;
        }
        r.get("trackName")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
    })
}

/// Spotify oEmbed — the only public metadata Spotify exposes.
/// Returns the (episode or show) title.
pub async fn spotify_oembed_title(
    client: &reqwest::Client,
    spotify_url: &str,
) -> Result<String, ArchiveError> {
    let url = format!("https://open.spotify.com/oembed?url={spotify_url}");
    let v = fetch_json(client, &url).await?;
    v.get("title")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ArchiveError::BadResponse {
            url,
            message: "spotify oembed returned no title".into(),
        })
}

/// Podcast Index auth headers for one request at `now_unix`:
/// `(X-Auth-Key, X-Auth-Date, Authorization)`. Pure —
/// fixture-tested against the documented sha1 scheme.
#[must_use]
pub fn podcastindex_auth(
    api_key: &str,
    api_secret: &str,
    now_unix: u64,
) -> (String, String, String) {
    let date = now_unix.to_string();
    let mut h = Sha1::new();
    h.update(api_key.as_bytes());
    h.update(api_secret.as_bytes());
    h.update(date.as_bytes());
    let auth = format!("{:x}", h.finalize());
    (api_key.to_string(), date, auth)
}

/// Title-search resolver: find a show's public RSS feed by
/// name. Returns the best match's feed URL.
pub async fn podcastindex_feed_by_title(
    client: &reqwest::Client,
    api_key: &str,
    api_secret: &str,
    query: &str,
) -> Result<Option<String>, ArchiveError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (key, date, auth) = podcastindex_auth(api_key, api_secret, now);
    let url = format!(
        "https://api.podcastindex.org/api/1.0/search/bytitle?q={}",
        urlencode(query)
    );
    let resp = client
        .get(&url)
        .header("X-Auth-Key", key)
        .header("X-Auth-Date", date)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| ArchiveError::Fetch {
            url: url.clone(),
            message: e.to_string(),
        })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ArchiveError::BadResponse {
            url,
            message: format!("HTTP {status} (check PODCASTINDEX_API_KEY/SECRET)"),
        });
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| ArchiveError::Fetch {
        url,
        message: format!("json: {e}"),
    })?;
    Ok(v.get("feeds")
        .and_then(serde_json::Value::as_array)
        .and_then(|a| a.first())
        .and_then(|f| f.get("url"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string))
}

/// Client for enclosure (audio) fetches: podcast enclosures
/// route through stacked redirect trackers, and the files are
/// big — deeper redirect cap, longer timeout.
pub fn enclosure_client() -> Result<reqwest::Client, ArchiveError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(15))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| ArchiveError::Fetch {
            url: String::new(),
            message: format!("client build: {e}"),
        })
}

/// Groq transcription backfill (`whisper-large-v3-turbo`,
/// OpenAI-compatible endpoint, ~$0.04/audio-hour). Plain
/// HTTP — available without the `whisper` build feature.
pub async fn groq_transcribe(
    client: &reqwest::Client,
    api_key: &str,
    audio: Vec<u8>,
    filename: &str,
) -> Result<Vec<crate::youtube::Cue>, ArchiveError> {
    let url = "https://api.groq.com/openai/v1/audio/transcriptions";
    let part = reqwest::multipart::Part::bytes(audio).file_name(filename.to_string());
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", "whisper-large-v3-turbo")
        .text("response_format", "verbose_json");
    let resp = client
        .post(url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| ArchiveError::Fetch {
            url: url.to_string(),
            message: e.to_string(),
        })?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ArchiveError::BadResponse {
            url: url.to_string(),
            message: format!(
                "HTTP {status}: {}",
                body.chars().take(300).collect::<String>()
            ),
        });
    }
    parse_groq_verbose_json(&body)
}

/// Parse Groq/OpenAI `verbose_json` segments. Pure —
/// fixture-tested.
pub fn parse_groq_verbose_json(body: &str) -> Result<Vec<crate::youtube::Cue>, ArchiveError> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| ArchiveError::ImportParse(format!("groq verbose_json: {e}")))?;
    let segments = v
        .get("segments")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ArchiveError::ImportParse("groq verbose_json: no segments".into()))?;
    let mut cues = Vec::new();
    for seg in segments {
        let text = seg
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        let start = seg
            .get("start")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
            .max(0.0) as u64;
        cues.push(crate::youtube::Cue {
            start_secs: start,
            text,
        });
    }
    Ok(cues)
}

/// Render the raw-source markdown body for a podcast episode:
/// metadata lede, show notes, then the anchored transcript —
/// the same shape as videos so the SourceViewer needs no new
/// block kinds. `transcript_note` explains an absent
/// transcript honestly (Spotify-exclusive, no tag + no model,
/// …).
#[must_use]
pub fn render_podcast_markdown(
    show_title: Option<&str>,
    item: &FeedItem,
    blocks: &[TranscriptBlock],
    transcript_note: Option<&str>,
) -> String {
    let mut out = String::new();

    let mut lede: Vec<String> = Vec::new();
    if let Some(show) = show_title {
        lede.push(show.to_string());
    }
    if let Some(date) = &item.pub_date {
        lede.push(date.clone());
    }
    if let Some(secs) = item.duration_secs {
        lede.push(mmss(secs));
    }
    if !lede.is_empty() {
        out.push_str(&format!("_{}_\n\n", lede.join(" · ")));
    }

    if let Some(desc) = &item.description {
        // Feed descriptions are routinely HTML.
        let text = crate::article::clean_html_to_markdown(desc).unwrap_or_else(|_| desc.clone());
        if !text.trim().is_empty() {
            out.push_str("## Show notes\n\n");
            out.push_str(text.trim());
            out.push_str("\n\n");
        }
    }

    out.push_str("## Transcript\n\n");
    if blocks.is_empty() {
        out.push_str(&format!(
            "_{}_\n",
            transcript_note.unwrap_or("No transcript available for this episode.")
        ));
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

async fn fetch_json(
    client: &reqwest::Client,
    url: &str,
) -> Result<serde_json::Value, ArchiveError> {
    let text = crate::article::fetch_text(client, url, "application/json").await?;
    serde_json::from_str(&text).map_err(|e| ArchiveError::BadResponse {
        url: url.to_string(),
        message: format!("non-JSON response: {e}"),
    })
}

/// Minimal query-component percent-encoding (we only encode
/// search queries; reqwest handles the rest of the URL).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::TranscriptRef;

    #[test]
    fn podcastindex_auth_is_sha1_of_key_secret_date() {
        // sha1("XYZ" + "abc" + "1700000000") — precomputed.
        let (key, date, auth) = podcastindex_auth("XYZ", "abc", 1_700_000_000);
        assert_eq!(key, "XYZ");
        assert_eq!(date, "1700000000");
        let mut h = Sha1::new();
        h.update(b"XYZabc1700000000");
        assert_eq!(auth, format!("{:x}", h.finalize()));
        assert_eq!(auth.len(), 40);
    }

    #[test]
    fn episode_matching_handles_numeric_track_ids() {
        let results: Vec<serde_json::Value> = serde_json::from_str(
            r#"[
            {"wrapperType":"track","collectionName":"The Show","trackId":528458508},
            {"wrapperType":"podcastEpisode","trackId":1000123456789,"trackName":"Ep 42: Answers"},
            {"wrapperType":"podcastEpisode","trackId":1000999999999,"trackName":"Ep 43"}
        ]"#,
        )
        .unwrap();
        assert_eq!(
            find_episode_title(&results, "1000123456789").as_deref(),
            Some("Ep 42: Answers")
        );
        // The show-level "track" wrapper never matches even if
        // ids collide.
        assert_eq!(find_episode_title(&results, "528458508"), None);
        assert_eq!(find_episode_title(&results, "1"), None);
    }

    #[test]
    fn groq_segments_parse() {
        let body = r#"{"text":"all","segments":[
            {"start":0.0,"end":4.2,"text":" Hello there."},
            {"start":4.8,"end":9.0,"text":"Second segment."},
            {"start":9.5,"end":10.0,"text":"  "}
        ]}"#;
        let cues = parse_groq_verbose_json(body).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Hello there.");
        assert_eq!(cues[1].start_secs, 4);
    }

    #[test]
    fn podcast_markdown_mirrors_video_shape() {
        let item = FeedItem {
            title: "Ep 42".into(),
            pub_date: Some("Tue, 10 Jun 2026 09:00:00 +0000".into()),
            duration_secs: Some(3725),
            description: Some("<p>Notes with <b>bold</b>.</p>".into()),
            enclosure_url: Some("https://cdn.example.com/ep.mp3".into()),
            enclosure_type: Some("audio/mpeg".into()),
            transcripts: vec![TranscriptRef {
                url: "x".into(),
                mime: "text/vtt".into(),
            }],
            ..Default::default()
        };
        let blocks = vec![
            TranscriptBlock {
                start_secs: 0,
                text: "Welcome to the show.".into(),
            },
            TranscriptBlock {
                start_secs: 47,
                text: "Deep dive time.".into(),
            },
        ];
        let md = render_podcast_markdown(Some("Test Show"), &item, &blocks, None);
        assert!(
            md.contains("_Test Show · Tue, 10 Jun 2026 09:00:00 +0000 · 1:02:05_"),
            "{md}"
        );
        assert!(md.contains("## Show notes"), "{md}");
        assert!(md.contains("**bold**"), "{md}");
        assert!(md.contains("[0:00] Welcome to the show. ^t0"), "{md}");
        assert!(md.contains("[0:47] Deep dive time. ^t47"), "{md}");

        let empty = render_podcast_markdown(
            None,
            &item,
            &[],
            Some("Spotify-exclusive: no public audio."),
        );
        assert!(
            empty.contains("_Spotify-exclusive: no public audio._"),
            "{empty}"
        );
    }
}

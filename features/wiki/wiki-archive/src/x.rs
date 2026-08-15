//! X/Twitter post extraction — the degradation ladder
//! (accept-fragility tier; every shape here drifts every few
//! months, so EVERY field is optional and every rung is
//! allowed to fail forward):
//!
//! 1. **Syndication** `cdn.syndication.twimg.com/tweet-result`
//!    — the token is computed the way react-tweet does it:
//!    `((id / 1e15) * π).toString(36).replace(/(0+|\.)/g, '')`.
//!    Long posts (note tweets) come back TRUNCATED here —
//!    detected and escalated to the next rung.
//! 2. **FxEmbed** `api.fxtwitter.com` — carries full
//!    note-tweet text.
//! 3. **vxtwitter** `api.vxtwitter.com` — same idea,
//!    different operator.
//! 4. **Official oEmbed** `publish.twitter.com/oembed` —
//!    text-only (HTML blockquote, stripped).
//! 5. Nothing worked → the CLI stores the URL as an
//!    unarchived stub for `task wiki archive retry`.
//!
//! Nitter is deliberately NOT a dependency (instances churn
//! too fast to promise anything).

use serde_json::Value;

use crate::ArchiveError;

/// One extracted post — every field optional except `text`,
/// because the upstream shapes drift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tweet {
    pub text: String,
    pub author_name: Option<String>,
    pub author_handle: Option<String>,
    /// As reported by whichever rung answered (format varies).
    pub created_at: Option<String>,
    /// Photo/video URLs when the rung exposes them.
    pub media_urls: Vec<String>,
    /// Honest caveat (e.g. "truncated note tweet").
    pub note: Option<String>,
}

/// Result of running the ladder: the tweet + which rung
/// produced it (recorded as the provenance `extractor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderResult {
    pub tweet: Tweet,
    pub rung: &'static str,
}

/// Syndication token, computed the way react-tweet does:
/// `((Number(id) / 1e15) * Math.PI).toString(36)` with zeros
/// and the dot stripped. The radix conversion is a port of
/// V8's `DoubleToRadixCString` fraction loop (delta = half an
/// ulp, round-to-even, carry back-propagation), so the digits
/// match what a JS engine prints — verified against node
/// vectors in the tests. A drift here just 404s rung 1 and
/// the ladder moves on.
#[must_use]
pub fn syndication_token(id: u64) -> String {
    let v = (id as f64 / 1e15) * std::f64::consts::PI;
    js_f64_to_radix36(v).replace(['.', '0'], "")
}

/// JS `Number.prototype.toString(36)` for finite positive
/// doubles — V8's algorithm.
// The exact `== 0.5` comparison is V8's round-to-even tie
// check, ported verbatim — a margin would change the digits.
#[allow(clippy::float_cmp)]
fn js_f64_to_radix36(value: f64) -> String {
    const RADIX: u32 = 36;
    const CHARS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut integer = value.trunc();
    let mut fraction = value.fract();
    // Half an ulp of the value, clamped to the smallest
    // denormal — V8's stopping threshold for fraction digits.
    let mut delta = (0.5 * (value.next_up() - value)).max(f64::from_bits(1));

    let mut frac_buf: Vec<u8> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= f64::from(RADIX);
            delta *= f64::from(RADIX);
            let digit = fraction as u32; // trunc, < 36
            frac_buf.push(CHARS[digit as usize]);
            fraction -= f64::from(digit);
            // Round to even.
            if (fraction > 0.5 || (fraction == 0.5 && digit & 1 == 1)) && fraction + delta > 1.0 {
                // Round up with carry back-propagation.
                loop {
                    match frac_buf.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(c) => {
                            let d = u32::from(if c > b'9' { c - b'a' + 10 } else { c - b'0' });
                            if d + 1 < RADIX {
                                frac_buf.push(CHARS[(d + 1) as usize]);
                                break;
                            }
                        }
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }

    let mut ip = integer as u64;
    let mut int_buf: Vec<u8> = Vec::new();
    if ip == 0 {
        int_buf.push(b'0');
    }
    while ip > 0 {
        int_buf.push(CHARS[(ip % u64::from(RADIX)) as usize]);
        ip /= u64::from(RADIX);
    }
    int_buf.reverse();
    let mut s = String::from_utf8(int_buf).unwrap_or_default();
    if !frac_buf.is_empty() {
        s.push('.');
        s.push_str(std::str::from_utf8(&frac_buf).unwrap_or_default());
    }
    s
}

/// Run the full ladder for one status id.
pub async fn fetch_tweet(
    client: &reqwest::Client,
    status_id: &str,
) -> Result<LadderResult, ArchiveError> {
    let mut failures: Vec<String> = Vec::new();

    match fetch_syndication(client, status_id).await {
        Ok(tweet) => {
            if tweet.note.is_none() {
                return Ok(LadderResult {
                    tweet,
                    rung: "x-syndication",
                });
            }
            // Truncated note tweet: prefer a rung with full
            // text, but keep this as a fallback.
            failures.push("syndication: note tweet truncated".into());
            match fetch_fxtwitter(client, status_id).await {
                Ok(full) => {
                    return Ok(LadderResult {
                        tweet: full,
                        rung: "fxembed",
                    });
                }
                Err(e) => {
                    failures.push(format!("fxembed: {e}"));
                    return Ok(LadderResult {
                        tweet,
                        rung: "x-syndication",
                    });
                }
            }
        }
        Err(e) => failures.push(format!("syndication: {e}")),
    }
    match fetch_fxtwitter(client, status_id).await {
        Ok(tweet) => {
            return Ok(LadderResult {
                tweet,
                rung: "fxembed",
            });
        }
        Err(e) => failures.push(format!("fxembed: {e}")),
    }
    match fetch_vxtwitter(client, status_id).await {
        Ok(tweet) => {
            return Ok(LadderResult {
                tweet,
                rung: "vxtwitter",
            });
        }
        Err(e) => failures.push(format!("vxtwitter: {e}")),
    }
    match fetch_oembed(client, status_id).await {
        Ok(tweet) => {
            return Ok(LadderResult {
                tweet,
                rung: "x-oembed",
            });
        }
        Err(e) => failures.push(format!("oembed: {e}")),
    }

    Err(ArchiveError::Extract {
        url: format!("https://x.com/i/status/{status_id}"),
        message: format!("every ladder rung failed — {}", failures.join("; ")),
    })
}

async fn fetch_syndication(
    client: &reqwest::Client,
    status_id: &str,
) -> Result<Tweet, ArchiveError> {
    let id: u64 = status_id.parse().map_err(|_| {
        ArchiveError::ImportParse(format!("status id `{status_id}` is not numeric"))
    })?;
    let url = format!(
        "https://cdn.syndication.twimg.com/tweet-result?id={status_id}&token={}&lang=en",
        syndication_token(id)
    );
    let body = crate::article::fetch_text(client, &url, "application/json").await?;
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| ArchiveError::ImportParse(format!("syndication json: {e}")))?;
    parse_syndication(&v).ok_or_else(|| ArchiveError::BadResponse {
        url,
        message: "syndication payload had no text (tombstone/not-found?)".into(),
    })
}

/// Pure — fixture-tested. `None` when there's no tweet text.
#[must_use]
pub fn parse_syndication(v: &Value) -> Option<Tweet> {
    let text = v.get("text").and_then(Value::as_str)?.to_string();
    if text.is_empty() {
        return None;
    }
    // Note-tweet truncation: the syndication API caps long
    // posts. Either an explicit hint, or the trailing
    // ellipsis-with-trailing-range tell.
    let truncated = v.get("note_tweet").is_some()
        || v.get("__typename").and_then(Value::as_str) == Some("TweetTombstone")
        || text.ends_with('…');
    let media_urls = v
        .get("mediaDetails")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("media_url_https").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(Tweet {
        text,
        author_name: v
            .pointer("/user/name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        author_handle: v
            .pointer("/user/screen_name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        created_at: v
            .get("created_at")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        media_urls,
        note: truncated.then(|| "syndication API truncated this note tweet".to_string()),
    })
}

async fn fetch_fxtwitter(client: &reqwest::Client, status_id: &str) -> Result<Tweet, ArchiveError> {
    let url = format!("https://api.fxtwitter.com/i/status/{status_id}");
    let body = crate::article::fetch_text(client, &url, "application/json").await?;
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| ArchiveError::ImportParse(format!("fxtwitter json: {e}")))?;
    parse_fxtwitter(&v).ok_or_else(|| ArchiveError::BadResponse {
        url,
        message: "fxtwitter payload had no tweet text".into(),
    })
}

/// Pure — fixture-tested.
#[must_use]
pub fn parse_fxtwitter(v: &Value) -> Option<Tweet> {
    let tweet = v.get("tweet")?;
    let text = tweet.get("text").and_then(Value::as_str)?.to_string();
    if text.is_empty() {
        return None;
    }
    let media_urls = tweet
        .pointer("/media/all")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("url").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(Tweet {
        text,
        author_name: tweet
            .pointer("/author/name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        author_handle: tweet
            .pointer("/author/screen_name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        created_at: tweet
            .get("created_at")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        media_urls,
        note: None,
    })
}

async fn fetch_vxtwitter(client: &reqwest::Client, status_id: &str) -> Result<Tweet, ArchiveError> {
    let url = format!("https://api.vxtwitter.com/i/status/{status_id}");
    let body = crate::article::fetch_text(client, &url, "application/json").await?;
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| ArchiveError::ImportParse(format!("vxtwitter json: {e}")))?;
    parse_vxtwitter(&v).ok_or_else(|| ArchiveError::BadResponse {
        url,
        message: "vxtwitter payload had no tweet text".into(),
    })
}

/// Pure — fixture-tested. vxtwitter is a flat object.
#[must_use]
pub fn parse_vxtwitter(v: &Value) -> Option<Tweet> {
    let text = v.get("text").and_then(Value::as_str)?.to_string();
    if text.is_empty() {
        return None;
    }
    let media_urls = v
        .get("mediaURLs")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(Tweet {
        text,
        author_name: v
            .get("user_name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        author_handle: v
            .get("user_screen_name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        created_at: v
            .get("date")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        media_urls,
        note: None,
    })
}

async fn fetch_oembed(client: &reqwest::Client, status_id: &str) -> Result<Tweet, ArchiveError> {
    let url = format!(
        "https://publish.twitter.com/oembed?url=https://twitter.com/i/status/{status_id}&omit_script=1"
    );
    let body = crate::article::fetch_text(client, &url, "application/json").await?;
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| ArchiveError::ImportParse(format!("oembed json: {e}")))?;
    parse_oembed(&v).ok_or_else(|| ArchiveError::BadResponse {
        url,
        message: "oembed payload had no html".into(),
    })
}

/// Pure — fixture-tested. oEmbed only gives a blockquote of
/// HTML; strip it to text. Honest about being text-only.
#[must_use]
pub fn parse_oembed(v: &Value) -> Option<Tweet> {
    let html = v.get("html").and_then(Value::as_str)?;
    let text = crate::article::clean_html_to_markdown(html).ok()?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(Tweet {
        text,
        author_name: v
            .get("author_name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        author_handle: None,
        created_at: None,
        media_urls: Vec::new(),
        note: Some("recovered via official oEmbed — text only, no media metadata".into()),
    })
}

/// Render the raw-source body for a post.
#[must_use]
pub fn render_tweet_markdown(tweet: &Tweet, source_url: &str) -> String {
    let mut out = String::new();
    let mut lede: Vec<String> = Vec::new();
    match (&tweet.author_name, &tweet.author_handle) {
        (Some(n), Some(h)) => lede.push(format!("{n} (@{h})")),
        (Some(n), None) => lede.push(n.clone()),
        (None, Some(h)) => lede.push(format!("@{h}")),
        (None, None) => {}
    }
    if let Some(d) = &tweet.created_at {
        lede.push(d.clone());
    }
    if !lede.is_empty() {
        out.push_str(&format!("_{}_\n\n", lede.join(" · ")));
    }
    for line in tweet.text.lines() {
        out.push_str("> ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    if !tweet.media_urls.is_empty() {
        out.push_str("## Media\n\n");
        for m in &tweet.media_urls {
            out.push_str(&format!("- <{m}>\n"));
        }
        out.push('\n');
    }
    if let Some(note) = &tweet.note {
        out.push_str(&format!("_Note: {note}._\n\n"));
    }
    out.push_str(&format!("Original: <{source_url}>\n"));
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_matches_node_reference_vectors() {
        // Reference values computed in node with the
        // react-tweet formula:
        // ((Number(id) / 1e15) * Math.PI).toString(36)
        //   .replace(/(0+|\.)/g, '')
        for (id, expected) in [
            (1_629_307_668_568_633_344_u64, "3y6mctgwzxo"),
            (20, "6dq1a2xwd93"),
            (1_700_000_000_000_000_000, "44cpgxmyurn"),
            (1_234_567_890_123_456_789, "2zqic77uqyk"),
            (1_820_160_924_512_919_999, "4eu7cmmrz7r"),
        ] {
            assert_eq!(syndication_token(id), expected, "id {id}");
        }
    }

    #[test]
    fn syndication_parse_and_truncation_tell() {
        let v: Value = serde_json::from_str(
            r#"{"text":"Short post","user":{"name":"Jane","screen_name":"jane"},
                "created_at":"2026-06-01T10:00:00.000Z",
                "mediaDetails":[{"media_url_https":"https://pbs.twimg.com/media/x.jpg"}]}"#,
        )
        .unwrap();
        let t = parse_syndication(&v).unwrap();
        assert_eq!(t.author_handle.as_deref(), Some("jane"));
        assert_eq!(t.media_urls.len(), 1);
        assert!(t.note.is_none());

        let truncated: Value =
            serde_json::from_str(r#"{"text":"Long note tweet that got cut…"}"#).unwrap();
        assert!(parse_syndication(&truncated).unwrap().note.is_some());
        // All-fields-missing shapes don't panic.
        assert!(parse_syndication(&serde_json::json!({})).is_none());
    }

    #[test]
    fn fx_and_vx_parse_with_everything_optional() {
        let fx: Value = serde_json::from_str(
            r#"{"code":200,"tweet":{"text":"Full note tweet text.","author":{"name":"Jane","screen_name":"jane"},"media":{"all":[{"url":"https://video.example/v.mp4"}]}}}"#,
        )
        .unwrap();
        let t = parse_fxtwitter(&fx).unwrap();
        assert_eq!(t.text, "Full note tweet text.");
        assert_eq!(t.media_urls, vec!["https://video.example/v.mp4"]);
        // Bare-minimum shape still parses.
        let fx_min: Value = serde_json::from_str(r#"{"tweet":{"text":"hi"}}"#).unwrap();
        assert!(parse_fxtwitter(&fx_min).is_some());

        let vx: Value = serde_json::from_str(
            r#"{"text":"vx text","user_name":"Jane","user_screen_name":"jane","date":"x","mediaURLs":["https://m"]}"#,
        )
        .unwrap();
        let t = parse_vxtwitter(&vx).unwrap();
        assert_eq!(t.media_urls, vec!["https://m"]);
        assert!(parse_vxtwitter(&serde_json::json!({"no":"text"})).is_none());
    }

    #[test]
    fn oembed_strips_html_and_flags_text_only() {
        let v: Value = serde_json::from_str(
            r#"{"author_name":"Jane","html":"<blockquote><p>Just text here.</p>&mdash; Jane (@jane)</blockquote>"}"#,
        )
        .unwrap();
        let t = parse_oembed(&v).unwrap();
        assert!(t.text.contains("Just text here."));
        assert!(t.note.as_deref().unwrap().contains("text only"));
    }

    #[test]
    fn rendered_post_quotes_text_and_links_original() {
        let tweet = Tweet {
            text: "Line one\nLine two".into(),
            author_name: Some("Jane".into()),
            author_handle: Some("jane".into()),
            created_at: Some("2026-06-01".into()),
            media_urls: vec!["https://pbs.example/m.jpg".into()],
            note: Some("recovered via official oEmbed — text only".into()),
        };
        let md = render_tweet_markdown(&tweet, "https://x.com/jane/status/123");
        assert!(md.contains("_Jane (@jane) · 2026-06-01_"), "{md}");
        assert!(md.contains("> Line one\n> Line two"), "{md}");
        assert!(md.contains("- <https://pbs.example/m.jpg>"), "{md}");
        assert!(
            md.contains("Original: <https://x.com/jane/status/123>"),
            "{md}"
        );
    }
}

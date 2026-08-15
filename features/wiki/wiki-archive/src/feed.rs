//! Podcast RSS feed parsing (quick-xml).
//!
//! Deliberately small: we read the handful of channel/item
//! fields the archive needs — title, guid, pubDate, itunes
//! duration, the enclosure, and the podcasting-2.0
//! `<podcast:transcript>` references (the transcript FAST
//! PATH — when a feed carries one, no audio ever needs to be
//! fetched or transcribed).
//!
//! Namespace handling is prefix-tolerant: tags are matched on
//! their local name (`itunes:duration` and `duration` both
//! count), because real-world feeds disagree on prefixes far
//! more than on local names.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::ArchiveError;

/// What we keep from a podcast RSS channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feed {
    pub title: String,
    pub author: Option<String>,
    /// Items in document order (podcast feeds are
    /// newest-first by convention).
    pub items: Vec<FeedItem>,
}

/// One `<item>` (episode).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedItem {
    pub title: String,
    pub guid: Option<String>,
    /// Raw RFC-2822 `pubDate` string as the feed carries it.
    pub pub_date: Option<String>,
    /// Parsed `itunes:duration` in whole seconds.
    pub duration_secs: Option<u64>,
    pub enclosure_url: Option<String>,
    pub enclosure_type: Option<String>,
    /// `<podcast:transcript url=… type=…>` references.
    pub transcripts: Vec<TranscriptRef>,
    /// `<description>` (often HTML — convert before use).
    pub description: Option<String>,
    pub link: Option<String>,
}

/// One transcript reference from the podcasting-2.0 tag.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptRef {
    pub url: String,
    /// MIME type as declared (`text/vtt`, `application/srt`,
    /// `application/json`, …). Empty when the feed omits it.
    pub mime: String,
}

/// Parse a podcast RSS document. Pure — fixture-tested.
pub fn parse_feed(xml: &str) -> Result<Feed, ArchiveError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut feed = Feed::default();
    let mut item: Option<FeedItem> = None;
    // Local name of the text-bearing tag we're inside.
    let mut capture: Option<String> = None;
    let mut depth_in_channel = false;

    loop {
        match reader.read_event() {
            Err(e) => {
                return Err(ArchiveError::ImportParse(format!("rss: {e}")));
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                let local = local_name(&start);
                match local.as_str() {
                    "channel" => depth_in_channel = true,
                    "item" => item = Some(FeedItem::default()),
                    "enclosure" => {
                        if let Some(it) = item.as_mut() {
                            let (url, mime) = url_type_attrs(&reader, &start);
                            if let Some(url) = url {
                                it.enclosure_url = Some(url);
                                it.enclosure_type = mime;
                            }
                        }
                    }
                    "transcript" => {
                        if let Some(it) = item.as_mut() {
                            let (url, mime) = url_type_attrs(&reader, &start);
                            if let Some(url) = url {
                                it.transcripts.push(TranscriptRef {
                                    url,
                                    mime: mime.unwrap_or_default(),
                                });
                            }
                        }
                    }
                    "title" | "guid" | "pubdate" | "duration" | "description" | "link"
                    | "author" => capture = Some(local),
                    _ => {}
                }
            }
            Ok(Event::Empty(start)) => {
                let local = local_name(&start);
                if let Some(it) = item.as_mut() {
                    match local.as_str() {
                        "enclosure" => {
                            let (url, mime) = url_type_attrs(&reader, &start);
                            if let Some(url) = url {
                                it.enclosure_url = Some(url);
                                it.enclosure_type = mime;
                            }
                        }
                        "transcript" => {
                            let (url, mime) = url_type_attrs(&reader, &start);
                            if let Some(url) = url {
                                it.transcripts.push(TranscriptRef {
                                    url,
                                    mime: mime.unwrap_or_default(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(tag) = capture.as_deref() {
                    let text = t.xml_content().unwrap_or_default().trim().to_string();
                    store_text(&mut feed, &mut item, depth_in_channel, tag, text);
                }
            }
            Ok(Event::CData(c)) => {
                if let Some(tag) = capture.as_deref() {
                    let text = String::from_utf8_lossy(&c).trim().to_string();
                    store_text(&mut feed, &mut item, depth_in_channel, tag, text);
                }
            }
            Ok(Event::End(end)) => {
                let local =
                    String::from_utf8_lossy(end.name().local_name().as_ref()).to_ascii_lowercase();
                if local == "item" {
                    if let Some(it) = item.take() {
                        feed.items.push(it);
                    }
                }
                capture = None;
            }
            Ok(_) => {}
        }
    }

    if feed.title.is_empty() && feed.items.is_empty() {
        return Err(ArchiveError::ImportParse(
            "rss: no channel title and no items — not a podcast feed?".into(),
        ));
    }
    Ok(feed)
}

fn store_text(
    feed: &mut Feed,
    item: &mut Option<FeedItem>,
    in_channel: bool,
    tag: &str,
    text: String,
) {
    if text.is_empty() {
        return;
    }
    match item.as_mut() {
        Some(it) => match tag {
            "title" if it.title.is_empty() => it.title = text,
            "guid" => it.guid = Some(text),
            "pubdate" => it.pub_date = Some(text),
            "duration" => it.duration_secs = parse_itunes_duration(&text),
            "description" if it.description.is_none() => it.description = Some(text),
            "link" => it.link = Some(text),
            _ => {}
        },
        None if in_channel => match tag {
            "title" if feed.title.is_empty() => feed.title = text,
            "author" if feed.author.is_none() => feed.author = Some(text),
            _ => {}
        },
        None => {}
    }
}

fn local_name(start: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(start.name().local_name().as_ref()).to_ascii_lowercase()
}

/// Pull `url` + `type` attributes off `<enclosure>` /
/// `<podcast:transcript>`.
fn url_type_attrs(
    reader: &Reader<&[u8]>,
    start: &BytesStart<'_>,
) -> (Option<String>, Option<String>) {
    let mut url = None;
    let mut mime = None;
    for attr in start.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.local_name().as_ref()).to_ascii_lowercase();
        let value = attr
            .decode_and_unescape_value(reader.decoder())
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "url" => url = Some(value),
            "type" => mime = Some(value),
            _ => {}
        }
    }
    (url, mime)
}

/// `itunes:duration` is a mess: `3725`, `62:05`, `1:02:05`
/// all occur in the wild.
#[must_use]
pub fn parse_itunes_duration(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    let nums: Option<Vec<u64>> = parts.iter().map(|p| p.trim().parse().ok()).collect();
    let nums = nums?;
    match nums.as_slice() {
        [secs] => Some(*secs),
        [m, s] => Some(m * 60 + s),
        [h, m, s] => Some(h * 3600 + m * 60 + s),
        _ => None,
    }
}

/// Pick the episode to archive: exact normalized title match
/// first, then case-insensitive substring, then (no hint) the
/// first item — feeds are newest-first.
#[must_use]
pub fn pick_episode<'a>(feed: &'a Feed, title_hint: Option<&str>) -> Option<&'a FeedItem> {
    let Some(hint) = title_hint else {
        return feed.items.first();
    };
    let norm = normalize_title(hint);
    feed.items
        .iter()
        .find(|it| normalize_title(&it.title) == norm)
        .or_else(|| {
            let lower = hint.to_lowercase();
            feed.items
                .iter()
                .find(|it| it.title.to_lowercase().contains(&lower))
        })
}

/// Lowercased alphanumerics only — Apple's episode titles and
/// the feed's drift on punctuation/whitespace.
fn normalize_title(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"
     xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"
     xmlns:podcast="https://podcastindex.org/namespace/1.0">
  <channel>
    <title>Test Show</title>
    <itunes:author>Jane Host</itunes:author>
    <item>
      <title><![CDATA[Episode 2: The Sequel]]></title>
      <guid isPermaLink="false">ep-2-guid</guid>
      <pubDate>Tue, 10 Jun 2026 09:00:00 +0000</pubDate>
      <itunes:duration>1:02:05</itunes:duration>
      <enclosure url="https://cdn.example.com/ep2.mp3" length="12345" type="audio/mpeg"/>
      <podcast:transcript url="https://cdn.example.com/ep2.vtt" type="text/vtt"/>
      <podcast:transcript url="https://cdn.example.com/ep2.srt" type="application/srt"/>
      <description><![CDATA[<p>We discuss <b>things</b>.</p>]]></description>
      <link>https://example.com/ep2</link>
    </item>
    <item>
      <title>Episode 1</title>
      <itunes:duration>3725</itunes:duration>
      <enclosure url="https://cdn.example.com/ep1.mp3" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_channel_and_items() {
        let feed = parse_feed(FEED_FIXTURE).unwrap();
        assert_eq!(feed.title, "Test Show");
        assert_eq!(feed.author.as_deref(), Some("Jane Host"));
        assert_eq!(feed.items.len(), 2);
        let ep2 = &feed.items[0];
        assert_eq!(ep2.title, "Episode 2: The Sequel");
        assert_eq!(ep2.guid.as_deref(), Some("ep-2-guid"));
        assert_eq!(ep2.duration_secs, Some(3725));
        assert_eq!(
            ep2.enclosure_url.as_deref(),
            Some("https://cdn.example.com/ep2.mp3")
        );
        assert_eq!(ep2.transcripts.len(), 2);
        assert_eq!(ep2.transcripts[0].mime, "text/vtt");
        assert!(
            ep2.description
                .as_deref()
                .unwrap()
                .contains("<b>things</b>")
        );
        assert_eq!(feed.items[1].duration_secs, Some(3725));
    }

    #[test]
    fn duration_formats() {
        assert_eq!(parse_itunes_duration("90"), Some(90));
        assert_eq!(parse_itunes_duration("62:05"), Some(3725));
        assert_eq!(parse_itunes_duration("1:02:05"), Some(3725));
        assert_eq!(parse_itunes_duration("not-a-time"), None);
    }

    #[test]
    fn episode_picking() {
        let feed = parse_feed(FEED_FIXTURE).unwrap();
        // No hint ⇒ newest (first).
        assert_eq!(
            pick_episode(&feed, None).unwrap().title,
            "Episode 2: The Sequel"
        );
        // Exact-normalized beats substring.
        assert_eq!(
            pick_episode(&feed, Some("episode 2 the sequel"))
                .unwrap()
                .title,
            "Episode 2: The Sequel"
        );
        // Substring works.
        assert_eq!(
            pick_episode(&feed, Some("sequel")).unwrap().title,
            "Episode 2: The Sequel"
        );
        assert!(pick_episode(&feed, Some("episode 9")).is_none());
    }

    #[test]
    fn not_a_feed_errors() {
        assert!(parse_feed("<html><body>hi</body></html>").is_err());
    }
}

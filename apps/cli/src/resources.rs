//! `task resources …` — the Resource Library from the CLI.
//!
//! Today that is the **sermon sync**: every message video on a YouTube
//! channel becomes a lightweight sermon resource on the server — the
//! video link plus its captions — so messages can be categorised,
//! quoted, referenced from wiki pages (`sermon:<slug>#t:<secs>`), and
//! jumped back into at a timestamp. No model is involved anywhere: the
//! captions come from yt-dlp, the scripture references from a grammar
//! over the cues (server-side, in `resources::scripture_refs`), and
//! the files from `ResourcesService::upsert_sermon`.
//!
//! Built to run from cron: `sync` always exits 0 once the channel
//! listing worked and prints one summary line; per-video failures are
//! counted, not fatal. yt-dlp rate-limits, so videos are fetched one
//! at a time with a pause between them.
//!
//! The yt-dlp glue (`probe` + json3 track pick) is `wiki_archive::
//! youtube`, shared with `task wiki archive`; only the channel listing
//! is new here.

use std::time::Duration;

use clap::{Args, Subcommand};
use resources_proto::{ResourcesServiceClient, SermonResource, TranscriptSegment};
use serde_json::Value;

use crate::{establish_for_url, resolve_active_org, resolve_org_vox_url};

#[derive(Subcommand, Debug)]
pub enum ResourcesCmd {
    /// Sermon resources — YouTube message videos with their captions.
    #[command(subcommand)]
    Sermons(SermonsCmd),
}

#[derive(Subcommand, Debug)]
pub enum SermonsCmd {
    /// Sync every video of a channel that is not yet a sermon resource.
    /// Lists the channel's videos tab, skips known ids, fetches
    /// captions per new video (manual `en` preferred, auto as the
    /// fallback), and upserts each. Exits 0 once the listing worked.
    Sync {
        /// Channel URL (`https://www.youtube.com/@CrossroadsCA`, with or
        /// without `/videos`), a bare `@handle`, or a channel id
        /// (`UCA82jGZtU5BAIwkw5bsuBhg`).
        channel: String,
        #[command(flatten)]
        common: SyncArgs,
        /// Only videos uploaded on or after this date (`YYYY-MM-DD`).
        /// The videos tab is newest-first, so the sync stops at the
        /// first older video.
        #[arg(long)]
        since: Option<String>,
        /// Sync at most this many new videos this run.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Sync a single video by URL or id.
    SyncOne {
        /// `https://youtu.be/<id>`, a watch URL, or the bare id.
        video: String,
        #[command(flatten)]
        common: SyncArgs,
    },
    /// Move a channel folder from the org-wide `resources/sermons/`
    /// tier into a named wiki (`Resources/Sermons/<folder>/`), so the
    /// sermons become that wiki's pages. Slugs, links, transcripts and
    /// annotations travel with them.
    Move {
        /// The channel folder (`crossroads`).
        #[arg(long)]
        folder: String,
        /// The wiki to move into (`bible`).
        #[arg(long)]
        wiki: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// The synced sermons the server holds.
    List {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args, Debug, Clone)]
pub struct SyncArgs {
    #[arg(long)]
    pub org: Option<String>,
    /// Server base URL (`https://task.example.com`); defaults to the
    /// session's server. Always pass it against a deployment.
    #[arg(long)]
    pub server: Option<String>,
    /// Subfolder under the sermons root the channel's sermons go in
    /// (`crossroads`). Also the default second tag.
    #[arg(long)]
    pub folder: String,
    /// The named wiki the sermons belong to (`bible`): they become
    /// pages under that wiki's `Resources/Sermons/<folder>/`, with a
    /// `Sermons.base` over them. Without it they land in the org-wide
    /// `resources/sermons/` tier. Also `TASK_WIKI`.
    #[arg(long, env = "TASK_WIKI")]
    pub wiki: Option<String>,
    /// `tags:` for a new sermon (repeatable). Default: the hierarchical
    /// `sermons/<folder>` (`sermons/crossroads`), so the explorer's
    /// Tags view nests the channel under Sermons.
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// Attribute the sermons to this name instead of the channel's
    /// own (`writers: [<channel>]`).
    #[arg(long)]
    pub channel_name: Option<String>,
    /// List what would be synced; fetch nothing, write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Path to the yt-dlp binary (also `TASK_YTDLP`).
    #[arg(long, env = "TASK_YTDLP", default_value = "yt-dlp")]
    pub yt_dlp: String,
    /// Seconds to wait between videos (yt-dlp rate limits).
    #[arg(long, default_value_t = 3)]
    pub pause: u64,
}

impl SyncArgs {
    fn tags(&self) -> Vec<String> {
        if self.tags.is_empty() {
            // One hierarchical tag: the wiki explorer nests on `/`, so
            // the channel sits under Sermons instead of beside it.
            vec![format!("sermons/{}", self.folder)]
        } else {
            self.tags.clone()
        }
    }
}

pub async fn run_resources(cmd: ResourcesCmd, global_org: Option<&str>) -> eyre::Result<()> {
    match cmd {
        ResourcesCmd::Sermons(SermonsCmd::Move {
            folder,
            wiki,
            org,
            server,
        }) => {
            let client = client(org.or_else(|| global_org.map(str::to_owned)), server).await?;
            let moved = client
                .relocate_sermons(folder.clone(), wiki.clone())
                .await
                .map_err(|e| eyre::eyre!("relocate_sermons: {e:?}"))?;
            println!(
                "moved {moved} sermon(s): resources/sermons/{folder}/ → wikis/{wiki}/Resources/Sermons/{folder}/"
            );
            Ok(())
        }
        ResourcesCmd::Sermons(SermonsCmd::List { org, server, json }) => {
            let client = client(org.or_else(|| global_org.map(str::to_owned)), server).await?;
            let mut list = client
                .list_sermons()
                .await
                .map_err(|e| eyre::eyre!("list_sermons: {e:?}"))?;
            list.sort_by(|a, b| b.published.cmp(&a.published).then(a.slug.cmp(&b.slug)));
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            if list.is_empty() {
                println!("no sermons synced yet");
                return Ok(());
            }
            for s in &list {
                println!(
                    "{:<10} {:<40} {:>7}  {:<3} refs  {}",
                    s.published,
                    truncate(&s.slug, 40),
                    mmss(s.duration_secs),
                    s.scripture.len(),
                    s.title
                );
            }
            Ok(())
        }
        ResourcesCmd::Sermons(SermonsCmd::SyncOne { video, common }) => {
            let id = video_id(&video)
                .ok_or_else(|| eyre::eyre!("`{video}` is not a YouTube URL or id"))?;
            let org = common.org.clone().or_else(|| global_org.map(str::to_owned));
            let mut client = client(org.clone(), common.server.clone()).await?;
            let known = known_ids(&client).await?;
            if known.contains_key(&id) {
                println!("{id}: already synced as `{}` — re-syncing", known[&id]);
            }
            let mut tally = Tally::default();
            sync_video(&mut client, &org, &common, &id, None, &mut tally).await;
            println!("{}", tally.summary());
            Ok(())
        }
        ResourcesCmd::Sermons(SermonsCmd::Sync {
            channel,
            common,
            since,
            limit,
        }) => {
            let org = common.org.clone().or_else(|| global_org.map(str::to_owned));
            let mut client = client(org.clone(), common.server.clone()).await?;
            let url = channel_videos_url(&channel);
            println!("listing {url}");
            // A listing failure is the one fatal error: nothing else can
            // be decided without it.
            let videos = list_channel(&common.yt_dlp, &url).await?;
            let known = known_ids(&client).await?;
            println!(
                "{} video(s) on the channel, {} already synced",
                videos.len(),
                videos.iter().filter(|v| known.contains_key(&v.id)).count()
            );

            let mut tally = Tally::default();
            let limit = limit.unwrap_or(usize::MAX);
            let mut first = true;
            for v in &videos {
                if known.contains_key(&v.id) {
                    tally.skipped += 1;
                    continue;
                }
                if tally.synced + tally.no_captions.len() + tally.errors >= limit {
                    println!("limit reached ({limit})");
                    break;
                }
                if common.dry_run {
                    println!("  would sync {} — {}", v.id, v.title);
                    tally.synced += 1;
                    continue;
                }
                if !first {
                    tokio::time::sleep(Duration::from_secs(common.pause)).await;
                }
                first = false;
                let stop = sync_video(
                    &mut client,
                    &org,
                    &common,
                    &v.id,
                    since.as_deref(),
                    &mut tally,
                )
                .await;
                if stop {
                    println!(
                        "reached a video older than {} — stopping",
                        since.as_deref().unwrap_or("")
                    );
                    break;
                }
            }
            println!("{}", tally.summary());
            Ok(())
        }
    }
}

async fn client(
    org: Option<String>,
    server: Option<String>,
) -> eyre::Result<ResourcesServiceClient> {
    let slug = resolve_active_org(org)?;
    establish_for_url(&resolve_org_vox_url(server, &slug)).await
}

/// video id → slug for every sermon the server already holds.
async fn known_ids(
    client: &ResourcesServiceClient,
) -> eyre::Result<std::collections::HashMap<String, String>> {
    Ok(client
        .list_sermons()
        .await
        .map_err(|e| eyre::eyre!("list_sermons: {e:?}"))?
        .into_iter()
        .filter(|s| !s.video_id.is_empty())
        .map(|s| (s.video_id, s.slug))
        .collect())
}

#[derive(Default)]
struct Tally {
    synced: usize,
    skipped: usize,
    no_captions: Vec<String>,
    errors: usize,
}

impl Tally {
    fn summary(&self) -> String {
        let mut s = format!(
            "synced {}, skipped {} already present, {} without captions, {} errors",
            self.synced,
            self.skipped,
            self.no_captions.len(),
            self.errors
        );
        if !self.no_captions.is_empty() {
            s.push_str(&format!(
                "\n  without captions: {}",
                self.no_captions.join(", ")
            ));
        }
        s
    }
}

/// Probe one video, fetch its captions, upsert. Returns `true` when the
/// video is older than `since` (the channel walk should stop).
///
/// `org` is the resolved org slug the client was built for: the vox
/// connection can drop while a slow caption download runs between two
/// upserts, and once it has every later upsert on the same client fails,
/// so a failed upsert reconnects once and retries before counting an
/// error.
async fn sync_video(
    client: &mut ResourcesServiceClient,
    org: &Option<String>,
    args: &SyncArgs,
    id: &str,
    since: Option<&str>,
    tally: &mut Tally,
) -> bool {
    let url = format!("https://youtu.be/{id}");
    let yt = wiki_archive::youtube::YtDlp::new(&args.yt_dlp);
    let meta = match yt.probe(&url).await {
        Ok(m) => m,
        Err(e) => {
            println!("  {id}: probe failed: {e}");
            tally.errors += 1;
            return false;
        }
    };
    let published = meta
        .upload_date
        .as_deref()
        .filter(|d| d.len() == 8)
        .map(|d| format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]))
        .unwrap_or_default();
    if let Some(since) = since {
        if !published.is_empty() && published.as_str() < since {
            return true;
        }
    }
    let Some((lang, track_url)) = meta.json3_track.clone() else {
        println!("  {id}: no captions — skipped ({})", meta.title);
        tally.no_captions.push(id.to_string());
        return false;
    };
    let segments = match fetch_segments(&track_url).await {
        Ok(s) => s,
        Err(e) => {
            println!("  {id}: captions ({lang}) failed: {e}");
            tally.errors += 1;
            return false;
        }
    };
    if segments.is_empty() {
        println!("  {id}: caption track is empty — skipped");
        tally.no_captions.push(id.to_string());
        return false;
    }
    let sermon = SermonResource {
        folder: args.folder.clone(),
        wiki: args.wiki.clone().unwrap_or_default(),
        video_id: id.to_string(),
        video_url: url,
        title: meta.title.clone(),
        channel: args
            .channel_name
            .clone()
            .or_else(|| meta.channel.clone())
            .unwrap_or_default(),
        tags: args.tags(),
        published,
        duration_secs: meta.duration_secs.unwrap_or(0),
        caption_kind: if meta.json3_manual { "manual" } else { "auto" }.to_string(),
        language: lang,
        segments,
    };
    if args.dry_run {
        println!(
            "  would upsert {id} — {} ({} cues, {} captions)",
            sermon.title,
            sermon.segments.len(),
            sermon.caption_kind
        );
        tally.synced += 1;
        return false;
    }
    let cues = sermon.segments.len();
    let mut result = client.upsert_sermon(sermon.clone()).await;
    if let Err(e) = &result {
        println!("  {id}: upsert_sermon: {e:?} — reconnecting and retrying once");
        match self::client(org.clone(), args.server.clone()).await {
            Ok(fresh) => {
                *client = fresh;
                result = client.upsert_sermon(sermon).await;
            }
            Err(e) => println!("  {id}: reconnect failed: {e}"),
        }
    }
    match result {
        Ok(out) => {
            println!(
                "  {id}: {} {} ({cues} cues, {} scripture refs, {} links{})",
                if out.created { "created" } else { "refreshed" },
                out.rel_path,
                out.scripture.len(),
                out.links,
                if out.body_kept { ", body kept" } else { "" }
            );
            tally.synced += 1;
        }
        Err(e) => {
            println!("  {id}: upsert_sermon: {e:?}");
            tally.errors += 1;
        }
    }
    false
}

/// Download a json3 track and parse it into cues (duration kept — the
/// transcript sidecar wants `dur`, which `wiki_archive`'s coalescer
/// drops).
async fn fetch_segments(track_url: &str) -> eyre::Result<Vec<TranscriptSegment>> {
    let http = wiki_archive::article::http_client().map_err(|e| eyre::eyre!("{e}"))?;
    let json3 = wiki_archive::article::fetch_text(&http, track_url, "application/json")
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
    let segs = resources::parse_json3(&json3).map_err(|e| eyre::eyre!("{e}"))?;
    Ok(segs
        .into_iter()
        .map(|s| TranscriptSegment {
            start: s.start,
            dur: s.dur,
            text: s.text,
        })
        .collect())
}

/// One entry of a channel's videos tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelVideo {
    pub id: String,
    pub title: String,
}

/// `yt-dlp --flat-playlist -J <videos-tab-url>`: ids + titles, newest
/// first, no per-video probe.
async fn list_channel(yt_dlp: &str, url: &str) -> eyre::Result<Vec<ChannelVideo>> {
    let child = tokio::process::Command::new(yt_dlp)
        .args(["--flat-playlist", "-J", "--no-warnings", url])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                eyre::eyre!("yt-dlp not found at `{yt_dlp}` — pass --yt-dlp or set TASK_YTDLP")
            } else {
                eyre::eyre!("spawn {yt_dlp}: {e}")
            }
        })?;
    let out = tokio::time::timeout(Duration::from_secs(600), child.wait_with_output())
        .await
        .map_err(|_| eyre::eyre!("channel listing timed out after 600s"))??;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(eyre::eyre!(
            "yt-dlp exit {}: {}",
            out.status,
            stderr.lines().last().unwrap_or("(no stderr)")
        ));
    }
    parse_channel_listing(&String::from_utf8_lossy(&out.stdout))
}

/// Pure: the videos out of a `--flat-playlist -J` blob. A channel root
/// (no `/videos`) nests one playlist per tab; those are flattened.
pub fn parse_channel_listing(json: &str) -> eyre::Result<Vec<ChannelVideo>> {
    let v: Value = serde_json::from_str(json).map_err(|e| eyre::eyre!("yt-dlp -J: {e}"))?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    collect_entries(&v, &mut out, &mut seen);
    Ok(out)
}

fn collect_entries(
    v: &Value,
    out: &mut Vec<ChannelVideo>,
    seen: &mut std::collections::HashSet<String>,
) {
    if let Some(entries) = v.get("entries").and_then(Value::as_array) {
        for e in entries {
            collect_entries(e, out, seen);
        }
        return;
    }
    let Some(id) = v.get("id").and_then(Value::as_str) else {
        return;
    };
    // Playlists / tabs carry non-video ids (`UC…`, `PL…`) and no
    // `entries` when the listing is truncated; only 11-char video ids
    // are videos.
    if id.len() != 11 || !seen.insert(id.to_string()) {
        return;
    }
    out.push(ChannelVideo {
        id: id.to_string(),
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    });
}

/// The channel's videos-tab URL for any of the accepted spellings.
#[must_use]
pub fn channel_videos_url(input: &str) -> String {
    let s = input.trim().trim_end_matches('/');
    if let Some(handle) = s.strip_prefix('@') {
        return format!("https://www.youtube.com/@{handle}/videos");
    }
    if s.starts_with("UC") && s.len() == 24 && !s.contains('/') {
        return format!("https://www.youtube.com/channel/{s}/videos");
    }
    if s.ends_with("/videos") {
        return s.to_string();
    }
    format!("{s}/videos")
}

/// Bare 11-char id, or pull it out of a URL.
#[must_use]
pub fn video_id(input: &str) -> Option<String> {
    let s = input.trim();
    let rest = if let Some(i) = s.find("v=") {
        &s[i + 2..]
    } else if let Some(i) = s.find("youtu.be/") {
        &s[i + 9..]
    } else if let Some(i) = s.find("/embed/") {
        &s[i + 7..]
    } else if let Some(i) = s.find("/shorts/") {
        &s[i + 8..]
    } else {
        s
    };
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    (id.len() == 11).then_some(id)
}

fn mmss(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n - 1).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_url_forms() {
        assert_eq!(
            channel_videos_url("https://www.youtube.com/@CrossroadsCA"),
            "https://www.youtube.com/@CrossroadsCA/videos"
        );
        assert_eq!(
            channel_videos_url("https://www.youtube.com/@CrossroadsCA/videos"),
            "https://www.youtube.com/@CrossroadsCA/videos"
        );
        assert_eq!(
            channel_videos_url("@CrossroadsCA"),
            "https://www.youtube.com/@CrossroadsCA/videos"
        );
        assert_eq!(
            channel_videos_url("UCA82jGZtU5BAIwkw5bsuBhg"),
            "https://www.youtube.com/channel/UCA82jGZtU5BAIwkw5bsuBhg/videos"
        );
    }

    #[test]
    fn video_id_forms() {
        for s in [
            "https://youtu.be/YMypVgZXFIU",
            "https://www.youtube.com/watch?v=YMypVgZXFIU&t=10",
            "YMypVgZXFIU",
        ] {
            assert_eq!(video_id(s).as_deref(), Some("YMypVgZXFIU"), "{s}");
        }
        assert_eq!(video_id("https://www.youtube.com/@CrossroadsCA"), None);
    }

    #[test]
    fn flat_listing_flattens_tabs_and_keeps_only_videos() {
        let json = r#"{"id":"UCA82jGZtU5BAIwkw5bsuBhg","title":"Crossroads - Videos","entries":[
            {"id":"YMypVgZXFIU","title":"God Restores Broken People","url":"https://www.youtube.com/watch?v=YMypVgZXFIU"},
            {"id":"PLxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx","title":"a playlist","entries":[
                {"id":"dQw4w9WgXcQ","title":"nested"},
                {"id":"YMypVgZXFIU","title":"dupe"}
            ]}
        ]}"#;
        let v = parse_channel_listing(json).unwrap();
        assert_eq!(
            v,
            vec![
                ChannelVideo {
                    id: "YMypVgZXFIU".into(),
                    title: "God Restores Broken People".into()
                },
                ChannelVideo {
                    id: "dQw4w9WgXcQ".into(),
                    title: "nested".into()
                },
            ]
        );
    }

    #[test]
    fn tags_default_to_sermon_plus_folder() {
        let a = SyncArgs {
            org: None,
            server: None,
            folder: "crossroads".into(),
            wiki: None,
            tags: vec![],
            channel_name: None,
            dry_run: false,
            yt_dlp: "yt-dlp".into(),
            pause: 0,
        };
        assert_eq!(a.tags(), ["sermons/crossroads"]);
        let b = SyncArgs {
            tags: vec!["message".into()],
            ..a
        };
        assert_eq!(b.tags(), ["message"]);
    }
}

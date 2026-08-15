//! Content-type router + canonical-URL normalization.
//!
//! `classify` decides which extractor handles a URL;
//! `canonicalize` produces the dedup key. Both are pure
//! string→value functions so they unit-test without network.
//!
//! ## Canonicalization rules
//!
//! Provider-specific first (these collapse the many spellings
//! of the same resource):
//!
//! - YouTube (`watch?v=`, `youtu.be/`, `/shorts/`, `/embed/`,
//!   `/live/`) → `https://www.youtube.com/watch?v=<id>`
//! - Google Docs → `https://docs.google.com/document/d/<id>`
//!
//! Generic fallback: lowercase host, strip a leading `www.`,
//! drop default ports + fragments + tracking params
//! (`utm_*`, click ids, …), sort surviving query params,
//! trim the trailing slash. Same article shared from Pocket
//! (`?utm_source=pocket`) and Readwise (clean URL) → one key.

use url::Url;

use crate::ArchiveError;

/// Which extractor a URL routes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Default: readability article extraction.
    Article,
    /// Google Docs — fetched via the `/export?format=md`
    /// endpoint instead of scraping the JS app shell.
    GoogleDoc { doc_id: String },
    /// YouTube (incl. Shorts / youtu.be / embeds). Carries
    /// the video id so the canonical URL and the
    /// SourceViewer iframe need no re-parse.
    YouTube { video_id: String },
    /// Other yt-dlp-able hosts (Vimeo, TikTok, X video) —
    /// same transcript path as YouTube, no id extraction.
    Video,
    /// PDF by URL extension (`…/paper.pdf`). URLs that serve
    /// `application/pdf` without the extension divert to the
    /// same extractor at fetch time via content-type sniff.
    Pdf,
    /// Apple Podcasts page. `podcast_id` is the `id<digits>`
    /// path segment; `episode_id` the `?i=` query param when
    /// the link points at one episode.
    ApplePodcast {
        podcast_id: String,
        episode_id: Option<String>,
    },
    /// Spotify podcast page (`/episode/` or `/show/`).
    /// Spotify exposes no public audio or transcript —
    /// extraction is metadata-only unless Podcast Index
    /// resolves the show to a public RSS feed.
    SpotifyPodcast { kind: SpotifyKind, id: String },
    /// Reddit thread (`/r/…/comments/…`). `permalink` is the
    /// normalized path — host variants (old/np/new/m) all
    /// collapse onto it.
    Reddit { permalink: String },
    /// X/Twitter post (`/status/<id>`). Phase 1 sent these to
    /// yt-dlp as `Video`; the text-extraction ladder owns
    /// them now (media URLs ride along in the payloads).
    Tweet { status_id: String },
}

/// Which Spotify podcast resource a URL names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotifyKind {
    Episode,
    Show,
}

/// Parse + classify in one step. The common entry point for
/// the CLI verb and the importers.
pub fn classify(input: &str) -> Result<(Url, Route), ArchiveError> {
    let url = Url::parse(input)
        .map_err(|e| ArchiveError::InvalidUrl(input.to_string(), e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ArchiveError::InvalidUrl(
            input.to_string(),
            format!("unsupported scheme `{}`", url.scheme()),
        ));
    }
    let route = route_for(&url);
    Ok((url, route))
}

/// `content_type:` frontmatter value for a route.
#[must_use]
pub fn content_type_for(route: &Route) -> &'static str {
    match route {
        Route::Article => "article",
        Route::GoogleDoc { .. } => "document",
        Route::YouTube { .. } | Route::Video => "video",
        Route::Pdf => "pdf",
        Route::ApplePodcast { .. } | Route::SpotifyPodcast { .. } => "podcast",
        Route::Reddit { .. } | Route::Tweet { .. } => "post",
    }
}

fn route_for(url: &Url) -> Route {
    let host = host_no_www(url);
    let path = url.path();

    // ── YouTube family ──────────────────────────────────
    if host == "youtu.be" {
        let id = path.trim_start_matches('/');
        if let Some(id) = clean_video_id(id) {
            return Route::YouTube { video_id: id };
        }
    }
    if host == "youtube.com" || host.ends_with(".youtube.com") {
        if path == "/watch" {
            if let Some(id) = url
                .query_pairs()
                .find(|(k, _)| k == "v")
                .and_then(|(_, v)| clean_video_id(&v))
            {
                return Route::YouTube { video_id: id };
            }
        }
        for prefix in ["/shorts/", "/embed/", "/live/", "/v/"] {
            if let Some(rest) = path.strip_prefix(prefix) {
                if let Some(id) = clean_video_id(rest.split('/').next().unwrap_or("")) {
                    return Route::YouTube { video_id: id };
                }
            }
        }
    }

    // ── Google Docs ─────────────────────────────────────
    if host == "docs.google.com" {
        if let Some(rest) = path.strip_prefix("/document/d/") {
            let id = rest.split('/').next().unwrap_or("");
            if !id.is_empty() {
                return Route::GoogleDoc {
                    doc_id: id.to_string(),
                };
            }
        }
    }

    // ── Apple Podcasts ──────────────────────────────────
    // podcasts.apple.com/<cc>/podcast/<slug>/id<digits>[?i=<episode>]
    if host == "podcasts.apple.com" {
        if let Some(podcast_id) = path
            .split('/')
            .filter_map(|seg| seg.strip_prefix("id"))
            .find(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        {
            let episode_id = url
                .query_pairs()
                .find(|(k, _)| k == "i")
                .map(|(_, v)| v.into_owned())
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()));
            return Route::ApplePodcast {
                podcast_id: podcast_id.to_string(),
                episode_id,
            };
        }
    }

    // ── Spotify podcasts ────────────────────────────────
    if host == "open.spotify.com" {
        for (prefix, kind) in [
            ("/episode/", SpotifyKind::Episode),
            ("/show/", SpotifyKind::Show),
        ] {
            // Locale-prefixed paths (`/intl-de/episode/…`) too.
            if let Some(i) = path.find(prefix) {
                let id: String = path[i + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                if !id.is_empty() {
                    return Route::SpotifyPodcast { kind, id };
                }
            }
        }
    }

    // ── Reddit threads ──────────────────────────────────
    // reddit.com / old. / new. / np. / m. — collapse to one
    // permalink. (redd.it short links need a fetch to
    // resolve; they stay on the article path.)
    if (host == "reddit.com" || host.ends_with(".reddit.com")) && path.contains("/comments/") {
        return Route::Reddit {
            permalink: path.trim_end_matches('/').to_string(),
        };
    }

    // ── X/Twitter posts ─────────────────────────────────
    if host == "x.com" || host == "twitter.com" || host == "mobile.twitter.com" {
        if let Some(i) = path.find("/status/") {
            let id: String = path[i + "/status/".len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if !id.is_empty() {
                return Route::Tweet { status_id: id };
            }
        }
    }

    // ── PDF by extension ────────────────────────────────
    if path.to_ascii_lowercase().ends_with(".pdf") {
        return Route::Pdf;
    }

    // ── Other yt-dlp hosts (same transcript path) ───────
    if host == "vimeo.com" || host.ends_with(".vimeo.com") {
        return Route::Video;
    }
    if host == "tiktok.com" || host.ends_with(".tiktok.com") {
        return Route::Video;
    }
    // X/Twitter `/status/` permalinks route to the Tweet
    // ladder above; profile/search pages stay articles.

    Route::Article
}

/// Canonical dedup key for a URL. Provider-specific
/// rewrites first; generic normalization otherwise.
#[must_use]
pub fn canonicalize(url: &Url, route: &Route) -> String {
    match route {
        Route::YouTube { video_id } => {
            format!("https://www.youtube.com/watch?v={video_id}")
        }
        Route::GoogleDoc { doc_id } => {
            format!("https://docs.google.com/document/d/{doc_id}")
        }
        Route::ApplePodcast {
            podcast_id,
            episode_id,
        } => match episode_id {
            // Episode links carry the episode in the key — the
            // same show page archived twice IS the same page,
            // but two episodes are two resources.
            Some(ep) => format!("https://podcasts.apple.com/podcast/id{podcast_id}?i={ep}"),
            None => format!("https://podcasts.apple.com/podcast/id{podcast_id}"),
        },
        Route::SpotifyPodcast { kind, id } => {
            let seg = match kind {
                SpotifyKind::Episode => "episode",
                SpotifyKind::Show => "show",
            };
            format!("https://open.spotify.com/{seg}/{id}")
        }
        Route::Reddit { permalink } => format!("https://www.reddit.com{permalink}"),
        // `/i/status/` is the user-agnostic spelling — the
        // same post shared from different handles (or after a
        // rename) collapses onto one key.
        Route::Tweet { status_id } => format!("https://x.com/i/status/{status_id}"),
        Route::Article | Route::Video | Route::Pdf => generic_canonical(url),
    }
}

fn generic_canonical(url: &Url) -> String {
    let host = host_no_www(url);
    let scheme = url.scheme();
    let port = match (url.port(), scheme) {
        (Some(443), "https") | (Some(80), "http") | (None, _) => String::new(),
        (Some(p), _) => format!(":{p}"),
    };
    let path = if url.path() == "/" {
        String::new()
    } else {
        url.path().trim_end_matches('/').to_string()
    };
    let mut params: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    params.sort();
    let query = if params.is_empty() {
        String::new()
    } else {
        let joined = params
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.clone()
                } else {
                    format!("{k}={v}")
                }
            })
            .collect::<Vec<_>>()
            .join("&");
        format!("?{joined}")
    };
    format!("{scheme}://{host}{port}{path}{query}")
}

/// Tracking / share-attribution params that never change the
/// resource: stripping them is what lets the same article
/// from two importers collapse to one canonical key.
fn is_tracking_param(key: &str) -> bool {
    if key.starts_with("utm_") || key.starts_with("_hs") || key.starts_with("hsa_") {
        return true;
    }
    matches!(
        key,
        "fbclid"
            | "gclid"
            | "gbraid"
            | "wbraid"
            | "msclkid"
            | "twclid"
            | "ttclid"
            | "yclid"
            | "dclid"
            | "mc_cid"
            | "mc_eid"
            | "igshid"
            | "igsh"
            | "si" // youtube/spotify share token
            | "ref"
            | "ref_src"
            | "ref_url"
            | "cmpid"
            | "s_kwcid"
            | "sscid"
            | "vero_id"
            | "oly_anon_id"
            | "oly_enc_id"
            | "guccounter"
            | "share_id"
    )
}

fn host_no_www(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// YouTube ids are `[A-Za-z0-9_-]{6,}`; share links append
/// junk (`?si=`, trailing slashes) that `Url` already split
/// off — this guards against empty/odd leftovers.
fn clean_video_id(raw: &str) -> Option<String> {
    let id: String = raw
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    if id.len() >= 6 { Some(id) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(s: &str) -> Route {
        classify(s).unwrap().1
    }

    fn canon(s: &str) -> String {
        let (url, r) = classify(s).unwrap();
        canonicalize(&url, &r)
    }

    #[test]
    fn youtube_spellings_collapse() {
        let forms = [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
            "https://youtu.be/dQw4w9WgXcQ?si=AbCdEfGh123",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ",
        ];
        for f in forms {
            assert_eq!(
                route(f),
                Route::YouTube {
                    video_id: "dQw4w9WgXcQ".into()
                },
                "route for {f}"
            );
            assert_eq!(
                canon(f),
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "canon for {f}"
            );
        }
    }

    #[test]
    fn google_doc_routes_to_export() {
        let u = "https://docs.google.com/document/d/1AbC_dEf-123/edit?tab=t.0#heading=h.x";
        assert_eq!(
            route(u),
            Route::GoogleDoc {
                doc_id: "1AbC_dEf-123".into()
            }
        );
        assert_eq!(canon(u), "https://docs.google.com/document/d/1AbC_dEf-123");
    }

    #[test]
    fn pocket_vs_readwise_spelling_dedups() {
        let pocket = "https://Example.com/blog/post/?utm_source=pocket&utm_medium=email&b=2&a=1";
        let readwise = "https://www.example.com/blog/post?a=1&b=2#section";
        assert_eq!(canon(pocket), canon(readwise));
        assert_eq!(canon(pocket), "https://example.com/blog/post?a=1&b=2");
    }

    #[test]
    fn meaningful_query_params_survive() {
        assert_eq!(
            canon("https://example.com/search?q=rust&page=2"),
            "https://example.com/search?page=2&q=rust"
        );
    }

    #[test]
    fn root_and_ports_normalize() {
        assert_eq!(canon("https://example.com:443/"), "https://example.com");
        assert_eq!(
            canon("http://example.com:8080/x/"),
            "http://example.com:8080/x"
        );
    }

    #[test]
    fn video_hosts_route_to_video() {
        assert_eq!(route("https://vimeo.com/12345"), Route::Video);
        assert_eq!(
            route("https://www.tiktok.com/@user/video/7123"),
            Route::Video
        );
        assert_eq!(route("https://x.com/user"), Route::Article);
    }

    #[test]
    fn reddit_spellings_collapse_to_one_permalink() {
        let forms = [
            "https://www.reddit.com/r/rust/comments/abc123/some_title/",
            "https://old.reddit.com/r/rust/comments/abc123/some_title",
            "https://np.reddit.com/r/rust/comments/abc123/some_title/?share_id=xyz",
        ];
        for f in forms {
            assert_eq!(
                route(f),
                Route::Reddit {
                    permalink: "/r/rust/comments/abc123/some_title".into()
                },
                "route for {f}"
            );
            assert_eq!(
                canon(f),
                "https://www.reddit.com/r/rust/comments/abc123/some_title",
                "canon for {f}"
            );
        }
        // Subreddit listings are not threads.
        assert_eq!(route("https://www.reddit.com/r/rust/"), Route::Article);
    }

    #[test]
    fn tweet_status_routes_user_agnostically() {
        for f in [
            "https://x.com/jane/status/1629307668568633344",
            "https://twitter.com/jane/status/1629307668568633344?s=20",
            "https://mobile.twitter.com/other/status/1629307668568633344/photo/1",
        ] {
            assert_eq!(
                route(f),
                Route::Tweet {
                    status_id: "1629307668568633344".into()
                },
                "route for {f}"
            );
            assert_eq!(canon(f), "https://x.com/i/status/1629307668568633344");
        }
    }

    #[test]
    fn pdf_extension_routes_to_pdf() {
        assert_eq!(route("https://arxiv.org/pdf/1706.03762v7.pdf"), Route::Pdf);
        assert_eq!(route("https://example.com/whitepaper.PDF"), Route::Pdf);
        // No extension ⇒ article (content-type sniff diverts
        // at fetch time, not here).
        assert_eq!(route("https://arxiv.org/pdf/1706.03762"), Route::Article);
    }

    #[test]
    fn apple_podcast_routes_with_optional_episode() {
        let show = "https://podcasts.apple.com/us/podcast/the-talk-show/id528458508";
        assert_eq!(
            route(show),
            Route::ApplePodcast {
                podcast_id: "528458508".into(),
                episode_id: None
            }
        );
        assert_eq!(
            canon(show),
            "https://podcasts.apple.com/podcast/id528458508"
        );
        let ep = "https://podcasts.apple.com/us/podcast/ep-1/id528458508?i=1000123456789&l=en";
        assert_eq!(
            route(ep),
            Route::ApplePodcast {
                podcast_id: "528458508".into(),
                episode_id: Some("1000123456789".into())
            }
        );
        assert_eq!(
            canon(ep),
            "https://podcasts.apple.com/podcast/id528458508?i=1000123456789"
        );
    }

    #[test]
    fn spotify_podcast_routes() {
        let ep = "https://open.spotify.com/episode/4rOoJ6Egrf8K2IrywzwOMk?si=share123";
        assert_eq!(
            route(ep),
            Route::SpotifyPodcast {
                kind: SpotifyKind::Episode,
                id: "4rOoJ6Egrf8K2IrywzwOMk".into()
            }
        );
        assert_eq!(
            canon(ep),
            "https://open.spotify.com/episode/4rOoJ6Egrf8K2IrywzwOMk"
        );
        assert_eq!(
            route("https://open.spotify.com/intl-de/show/abcDEF123ghi"),
            Route::SpotifyPodcast {
                kind: SpotifyKind::Show,
                id: "abcDEF123ghi".into()
            }
        );
        // Music URLs are not podcasts.
        assert_eq!(
            route("https://open.spotify.com/track/abcdef123"),
            Route::Article
        );
    }

    #[test]
    fn non_http_rejected() {
        assert!(classify("ftp://example.com/f").is_err());
        assert!(classify("not a url").is_err());
    }
}

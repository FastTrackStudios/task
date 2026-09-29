//! Where the global Now Playing player gets a song's audio.
//!
//! A song is a folder under the org's media (`/org/{org}/media/songs/{slug}`):
//! the colocated `song` schema (`song.md` → the default arrangement's
//! `arrangement.md` → its audio `attachmentRefs`), or — for songs not yet
//! migrated — a legacy `manifest.json`. Now Playing streams ONE stem per
//! song, the reference track, through a plain `<audio>` element.
//!
//! The multitrack rehearsal rig (stems, mixer, engraved chart, section
//! timeline) is Session's, not Task's: Task links out to it
//! ([`crate::session_link`]). So nothing here parses a chart — the
//! element reports the real duration once it has loaded.

#[cfg(target_arch = "wasm32")]
pub(crate) mod imp {
    use serde::Deserialize;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{HtmlAudioElement, Response};

    /// One playable row of a queue.
    #[derive(Clone, PartialEq)]
    pub(crate) struct Track {
        pub slug: String,
        pub title: String,
        /// `0.0` until the element loads, when the song has no manifest
        /// that states it.
        pub duration_sec: f64,
        /// Reference stem's file, relative to the song folder — `None`
        /// when the song has no audio yet (row renders, can't play).
        pub reference: Option<String>,
    }

    /// The fields of a song's manifest the player reads.
    #[derive(Deserialize)]
    struct Manifest {
        title: Option<String>,
        #[serde(default)]
        duration_sec: f64,
        #[serde(default)]
        stems: Vec<Stem>,
    }

    #[derive(Deserialize)]
    struct Stem {
        name: String,
        #[serde(default)]
        group: Option<String>,
        file: String,
    }

    /// Fields of `song.md` the player needs (the `song` folder index).
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SongIndex {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        default_arrangement: Option<String>,
        #[serde(default)]
        arrangements: Vec<ArrangementIndex>,
    }

    #[derive(Deserialize)]
    struct ArrangementIndex {
        id: String,
        dir: String,
    }

    /// Fields of `arrangement.md` the player needs.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Arrangement {
        #[serde(default)]
        attachment_refs: Vec<AttachmentRef>,
    }

    #[derive(Deserialize)]
    struct AttachmentRef {
        #[serde(default)]
        path: Option<String>,
    }

    async fn fetch_text(url: &str) -> Result<String, String> {
        let win = web_sys::window().ok_or_else(|| "no window".to_string())?;
        let resp: Response = JsFuture::from(win.fetch_with_str(url))
            .await
            .map_err(|e| format!("fetch {url}: {e:?}"))?
            .dyn_into()
            .map_err(|_| "fetch did not return a Response".to_string())?;
        if !resp.ok() {
            return Err(format!("{url}: HTTP {}", resp.status()));
        }
        let promise = resp.text().map_err(|e| format!("{url}: text: {e:?}"))?;
        JsFuture::from(promise)
            .await
            .map_err(|e| format!("{url}: text await: {e:?}"))?
            .as_string()
            .ok_or_else(|| format!("{url}: response was not text"))
    }

    fn parse_frontmatter<T: serde::de::DeserializeOwned>(
        src: &str,
        what: &str,
    ) -> Result<T, String> {
        let body = src
            .strip_prefix("---\n")
            .and_then(|rest| rest.find("\n---").map(|end| &rest[..=end]))
            .ok_or_else(|| format!("{what}: no frontmatter"))?;
        serde_yaml::from_str(body).map_err(|e| format!("{what}: {e}"))
    }

    fn is_audio_file(name: &str) -> bool {
        let l = name.to_lowercase();
        [".ogg", ".mp3", ".wav", ".webm", ".m4a", ".opus"]
            .iter()
            .any(|e| l.ends_with(e))
    }

    /// The stem a listener hears: the original/reference track, else the
    /// first non-guide stem, else the first stem.
    fn reference_of(stems: &[Stem]) -> Option<String> {
        let is = |s: &Stem, words: &[&str]| {
            let n = s.name.to_lowercase();
            let g = s.group.as_deref().unwrap_or_default().to_lowercase();
            let f = s.file.to_lowercase();
            words
                .iter()
                .any(|w| n.contains(w) || g.contains(w) || f.contains(w))
        };
        stems
            .iter()
            .find(|s| is(s, &["original", "reference"]))
            .or_else(|| {
                stems
                    .iter()
                    .find(|s| !is(s, &["click", "cue", "count", "guide"]))
            })
            .or_else(|| stems.first())
            .map(|s| s.file.clone())
    }

    /// A song from its colocated `song` folder, with no `manifest.json`.
    async fn from_song_folder(org: &str, slug: &str, tok: &str) -> Result<Manifest, String> {
        let base = format!("/org/{org}/media/songs/{slug}");
        let song_md = fetch_text(&format!("{base}/song.md{tok}")).await?;
        let idx: SongIndex = parse_frontmatter(&song_md, "song.md")?;
        let dir = idx
            .arrangements
            .iter()
            .find(|a| Some(&a.id) == idx.default_arrangement.as_ref())
            .or_else(|| idx.arrangements.first())
            .map(|a| a.dir.clone())
            .ok_or_else(|| format!("`{slug}`: no arrangements"))?;
        let arr_md = fetch_text(&format!("{base}/arrangements/{dir}/arrangement.md{tok}")).await?;
        let arr: Arrangement = parse_frontmatter(&arr_md, "arrangement.md")?;
        let mut stems: Vec<Stem> = arr
            .attachment_refs
            .iter()
            .filter_map(|a| a.path.as_deref())
            .filter(|p| is_audio_file(p))
            .map(|p| Stem {
                name: p.rsplit('/').next().unwrap_or(p).to_owned(),
                group: None,
                file: p.to_owned(),
            })
            .collect();
        stems.sort_by(|a, b| a.file.cmp(&b.file));
        Ok(Manifest {
            title: idx.title,
            duration_sec: 0.0,
            stems,
        })
    }

    /// Load a queue's tracks. An unresolved slug still gets a silent row,
    /// so the queue renders.
    pub(crate) async fn load_tracks(org: &str, slugs: &[String]) -> Result<Vec<Track>, String> {
        let mut out = Vec::with_capacity(slugs.len());
        for slug in slugs {
            // One signed grant covers the song's whole folder, and the
            // `<audio>` src below reads it back from the cache.
            let tok = crate::media_grant::suffix(org, slug).await;
            let manifest = match from_song_folder(org, slug, &tok).await {
                Ok(m) => Ok(m),
                Err(_) => {
                    let url = format!("/org/{org}/media/songs/{slug}/manifest.json{tok}");
                    fetch_text(&url).await.and_then(|txt| {
                        serde_json::from_str::<Manifest>(&txt)
                            .map_err(|e| format!("{url}: bad manifest json: {e}"))
                    })
                }
            };
            out.push(match manifest {
                Ok(m) => Track {
                    slug: slug.clone(),
                    title: m.title.unwrap_or_else(|| slug.replace('-', " ")),
                    duration_sec: m.duration_sec,
                    reference: reference_of(&m.stems),
                },
                Err(_) => Track {
                    slug: slug.clone(),
                    title: slug.replace('-', " "),
                    duration_sec: 0.0,
                    reference: None,
                },
            });
        }
        Ok(out)
    }

    /// The `<audio>` element for a song's stem, served off disk at
    /// `/org/{org}/media/songs/{slug}/{file}` — same-origin and
    /// Range-capable, so the browser streams it.
    pub(crate) fn element_for(
        org: &str,
        slug: &str,
        file: &str,
    ) -> Result<HtmlAudioElement, String> {
        let el = HtmlAudioElement::new().map_err(|e| format!("audio element: {e:?}"))?;
        el.set_preload("auto");
        // `set_src` is synchronous, so this reads the grant `load_tracks`
        // already minted for this song.
        let tok = crate::media_grant::cached_suffix(org, slug);
        el.set_src(&format!("/org/{org}/media/songs/{slug}/{file}{tok}"));
        Ok(el)
    }
}

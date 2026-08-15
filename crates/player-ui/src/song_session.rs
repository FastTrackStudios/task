//! `type: song` vault-note view — a browser multitrack **session player**
//! driven by the REAL fasttrackstudio session-ui components, rendered ABOVE
//! the note editor in the vault page.
//!
//! Ported from `apps/site/src/components/song_session.rs`. Self-contained,
//! client-side playback. The Task dev/prod server serves the song's media at
//! `/media/songs/{slug}/…` (same-origin):
//!
//! - `manifest.json` — title/key/bpm, the section map, and the stem list.
//! - `stems/*.ogg`   — one Opus file per stem.
//! - `chart.kf`      — the keyflow chart (optional).
//!
//! Each stem is **streamed** through an `HTMLAudioElement` (progressive
//! Opus/ogg — the browser range-requests and decodes on the fly, so memory
//! stays flat and playback starts fast). Every element is routed into a Web
//! Audio graph via `AudioContext.createMediaElementSource` → its own
//! `GainNode` → destination, so the per-stem mixer (mute/solo/volume) drives
//! the gains. The elements all share one wall clock; element 0 is the master
//! and a ~10 Hz poll loop resyncs any stem that drifts past 50 ms.
//!
//! ## Presentation
//!
//! Instead of bespoke divs we populate session-ui's global signals from the
//! player each tick and render the real session-ui components
//! (`SessionChartPane`, `SongTitle`, `SongProgressBar`, `SectionProgressBar`,
//! `TransportControlBar`, `MixerView`). Chart and Mixer are presented as
//! **tabs**; the transport + progress bars stay visible above the tabs.
//!
//! Signals populated (see `session_ui::signals`):
//! - `SETLIST_STRUCTURE` — a one-song `session_proto::Setlist` from the manifest.
//! - `SONG_CHARTS[project_guid]` — `SongChartHydration { chart_text }`.
//! - `ACTIVE_INDICES` — song/section index + progress + is_playing, each tick.
//! - `SONG_TRANSPORT[0]` — `TransportState { position, bpm, ts, is_playing }`.
//! - `PLAYBACK_STATE` — Playing/Stopped (via `apply_active_indices`).
//!
//! The heavy lifting is `wasm32`-only (Web Audio + media elements). Off-wasm
//! the crate still has to compile, so there's a tiny stub below.

// ─────────────────────────────────────────────────────────────────────────────
// wasm32: the real player.
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(target_arch = "wasm32")]
pub(crate) mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::prelude::*;
    use serde::Deserialize;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{AudioContext, GainNode, HtmlAudioElement, Response};

    use daw_proto::{MusicalPosition, Position, PositionInSeconds, TimeSignature, Track};
    use session_proto::{
        ActiveIndices, Section as SessionSection, SectionId, SectionType, Setlist,
        Song as SessionSong, SongChartHydration, SongId,
    };
    use session_ui::components::{
        MixerView, ProgressSection, SectionProgressBar, SongProgressBar,
        TransportControlBar,
    };
    use session_ui::{
        PLAYBACK_STATE, SETLIST_STRUCTURE, SONG_CHARTS, SONG_TRANSPORT,
        TransportState, apply_active_indices,
    };

    use crate::session_chart_pane::SessionChartPane;

    /// Drift tolerance (seconds) before a stem is snapped back to the master.
    const DRIFT_TOLERANCE: f64 = 0.05;
    /// `HTMLMediaElement.readyState` for HAVE_CURRENT_DATA.
    const HAVE_CURRENT_DATA: u16 = 2;
    /// Stable synthetic project guid for the single-song setlist.
    const PROJECT_GUID_PREFIX: &str = "web-session:";
    /// Transport poll interval (ms). ~10 Hz: smooth playhead + drift/buffering.
    pub(crate) const TICK_MS: u32 = 100;

    // ── manifest model ──────────────────────────────────────────────────────

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub(crate) struct Manifest {
        #[allow(dead_code)]
        slug: Option<String>,
        pub(crate) title: Option<String>,
        pub(crate) artist: Option<String>,
        pub(crate) key: Option<String>,
        pub(crate) bpm: Option<f64>,
        pub(crate) time_signature: Option<String>,
        pub(crate) duration_sec: f64,
        #[serde(default)]
        pub(crate) sections: Vec<Section>,
        #[serde(default)]
        pub(crate) stems: Vec<StemSpec>,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub(crate) struct Section {
        pub(crate) name: String,
        pub(crate) start_sec: f64,
        pub(crate) end_sec: f64,
    }

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    pub(crate) struct StemSpec {
        pub(crate) name: String,
        #[serde(default)]
        pub(crate) group: Option<String>,
        pub(crate) file: String,
        #[serde(default)]
        pub(crate) default_muted: bool,
    }

    /// Per-stem UI/mix state, indexed parallel to `Manifest::stems`.
    #[derive(Clone, Copy)]
    pub(crate) struct StemUi {
        pub(crate) muted: bool,
        pub(crate) soloed: bool,
        pub(crate) volume: f32,
    }

    // ── the streaming Web Audio engine ──────────────────────────────────────

    /// One streamed stem: its media element (the actual audio source, streamed
    /// progressively) plus the gain node the mixer drives. The
    /// `MediaElementAudioSourceNode` is kept alive so the routing survives.
    pub(crate) struct StemNode {
        el: HtmlAudioElement,
        gain: GainNode,
        // Held for its side effect only: dropping the source node would
        // tear the routing down.
        #[allow(dead_code)]
        node: web_sys::MediaElementAudioSourceNode,
        /// Post-gain tap for VU metering (reads the level the listener hears).
        /// A side branch off `gain` — it does not connect onward, so it never
        /// affects the audio path. Read by `peak_levels`, which the session
        /// view doesn't poll yet — kept for that wiring.
        #[allow(dead_code)]
        analyser: web_sys::AnalyserNode,
    }

    /// The shared playback graph. Held in an `Rc<RefCell<…>>` so the resource
    /// future, the poll loop, and every event handler can drive it. Element 0
    /// is the master clock; there is no separate anchor arithmetic.
    pub(crate) struct EngineInner {
        pub(crate) ctx: AudioContext,
        pub(crate) stems: Vec<StemNode>,
        pub(crate) duration: f64,
        pub(crate) playing: bool,
    }

    pub(crate) type Engine = Rc<RefCell<EngineInner>>;

    impl EngineInner {
        /// Current song position — the master element's playback time.
        pub(crate) fn position(&self) -> f64 {
            self.stems
                .first()
                .map(|s| s.el.current_time())
                .unwrap_or(0.0)
                .clamp(0.0, self.duration)
        }

        /// Per-stem peak level (0.0..=1.0), in stem order, from the metering
        /// analysers — the post-gain signal the listener hears (muted stems read
        /// ~0). Cheap; poll at UI rate (~20 fps), not per audio frame.
        /// Not polled by the session view yet — kept with `analyser` for
        /// the VU wiring.
        #[allow(dead_code)]
        pub(crate) fn peak_levels(&self) -> Vec<f32> {
            let mut buf = [0u8; 256];
            self.stems
                .iter()
                .map(|s| {
                    s.analyser.get_byte_time_domain_data(&mut buf);
                    // 128 = silence; the frame's max deviation → 0..1.
                    let peak = buf
                        .iter()
                        .map(|&b| (b as i16 - 128).unsigned_abs())
                        .max()
                        .unwrap_or(0) as f32
                        / 128.0;
                    peak.min(1.0)
                })
                .collect()
        }

        /// Resume the context, park every element at `offset`, and play them.
        pub(crate) fn play(&mut self, offset: f64) {
            let _ = self.ctx.resume();
            for s in &self.stems {
                s.el.set_current_time(offset);
                let _ = s.el.play();
            }
            self.playing = true;
        }

        pub(crate) fn pause(&mut self) {
            for s in &self.stems {
                let _ = s.el.pause();
            }
            self.playing = false;
        }

        /// Stop playback and release the audio graph. Intended for setlist
        /// song swaps so the old song's media elements stop streaming; the
        /// session view currently relies on Drop instead — kept for the
        /// explicit-swap wiring.
        #[allow(dead_code)]
        pub(crate) fn teardown(&mut self) {
            for s in &self.stems {
                let _ = s.el.pause();
                s.el.set_src("");
                let _ = s.node.disconnect();
                let _ = s.gain.disconnect();
            }
            self.playing = false;
            let _ = self.ctx.close();
        }

        /// Jump every element to `offset` (works whether or not playing —
        /// elements keep streaming from the new position).
        pub(crate) fn seek(&mut self, offset: f64) {
            for s in &self.stems {
                s.el.set_current_time(offset);
            }
        }

        pub(crate) fn set_stem_gain(&self, idx: usize, value: f32) {
            if let Some(stem) = self.stems.get(idx) {
                stem.gain.gain().set_value(value);
            }
        }

        /// Resync stems to the master (element 0). Never touches the master.
        pub(crate) fn correct_drift(&self) {
            let Some(master) = self.stems.first() else {
                return;
            };
            let m = master.el.current_time();
            for s in self.stems.iter().skip(1) {
                if (s.el.current_time() - m).abs() > DRIFT_TOLERANCE {
                    s.el.set_current_time(m);
                }
            }
        }

        /// How many stems have at least HAVE_CURRENT_DATA buffered.
        pub(crate) fn ready_count(&self) -> usize {
            self.stems
                .iter()
                .filter(|s| s.el.ready_state() >= HAVE_CURRENT_DATA)
                .count()
        }
    }

    /// Push the mixer state (mute/solo/volume) into the gain nodes. Solo wins:
    /// if anything is soloed, only soloed-and-unmuted stems are audible.
    pub(crate) fn apply_mix(eng: &Engine, ui: &[StemUi]) {
        let any_solo = ui.iter().any(|s| s.soloed);
        let e = eng.borrow();
        for (i, s) in ui.iter().enumerate() {
            let audible = if any_solo {
                s.soloed && !s.muted
            } else {
                !s.muted
            };
            e.set_stem_gain(i, if audible { s.volume } else { 0.0 });
        }
    }

    // ── fetch helpers (same-origin) ─────────────────────────────────────────

    pub(crate) async fn fetch_text(url: &str) -> Result<String, String> {
        let win = web_sys::window().ok_or_else(|| "no window".to_string())?;
        let resp_val = JsFuture::from(win.fetch_with_str(url))
            .await
            .map_err(|e| format!("fetch {url}: {e:?}"))?;
        let resp: Response = resp_val
            .dyn_into()
            .map_err(|_| "fetch did not return a Response".to_string())?;
        if !resp.ok() {
            return Err(format!("{url}: HTTP {}", resp.status()));
        }
        let promise = resp.text().map_err(|e| format!("{url}: text: {e:?}"))?;
        let val = JsFuture::from(promise)
            .await
            .map_err(|e| format!("{url}: text await: {e:?}"))?;
        val.as_string()
            .ok_or_else(|| format!("{url}: response was not text"))
    }

    pub(crate) async fn fetch_manifest(url: &str) -> Result<Manifest, String> {
        let txt = fetch_text(url).await?;
        serde_json::from_str(&txt).map_err(|e| format!("{url}: bad manifest json: {e}"))
    }

    // ── manifest-optional: derive a song model from the colocated `song`
    // folder schema — song.md + arrangements/<dir>/arrangement.md (features/
    // song) — so a folder plays without a hand-authored manifest.json ───────

    /// Fields of `song.md` the player needs (the `song` folder index).
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SongIndexLite {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        default_arrangement: Option<String>,
        #[serde(default)]
        arrangements: Vec<ArrIndexLite>,
    }

    #[derive(Deserialize)]
    struct ArrIndexLite {
        id: String,
        dir: String,
    }

    /// Fields of `arrangement.md` the player needs.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ArrangementLite {
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        chart_ref: Option<ChartRefLite>,
        #[serde(default)]
        attachment_refs: Vec<AttachmentRefLite>,
    }

    #[derive(Deserialize)]
    struct ChartRefLite {
        #[serde(default)]
        path: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AttachmentRefLite {
        #[serde(default)]
        path: Option<String>,
        // Mirrors the wire shape; not consulted yet (path alone drives
        // stem resolution).
        #[allow(dead_code)]
        #[serde(default)]
        kind: Option<String>,
    }

    /// The YAML body of a `---\n…\n---` frontmatter block.
    fn frontmatter_yaml(src: &str) -> Option<&str> {
        let rest = src.strip_prefix("---\n")?;
        let end = rest.find("\n---")?;
        Some(&rest[..=end])
    }

    fn parse_fm<T: serde::de::DeserializeOwned>(src: &str, what: &str) -> Result<T, String> {
        let body = frontmatter_yaml(src).ok_or_else(|| format!("{what}: no frontmatter"))?;
        serde_yaml::from_str(body).map_err(|e| format!("{what}: {e}"))
    }

    /// Human-facing section name for a laid-out chart section: a quoted label
    /// wins (`Interlude "Breakdown"` → "Breakdown"), else the kind plus its
    /// occurrence number ("Verse 1", "Intro").
    fn laid_section_name(s: &session::setlist::chart_import::LaidSection) -> String {
        use session::keyflow::actions::SectionKind::*;
        if let Some(label) = &s.label {
            if !label.trim().is_empty() {
                return label.clone();
            }
        }
        let base = match s.kind {
            Intro => "Intro",
            Verse => "Verse",
            PreChorus => "Pre-Chorus",
            Chorus => "Chorus",
            Bridge => "Bridge",
            Outro => "Outro",
            Instrumental => "Instrumental",
            Solo => "Solo",
            Hits => "Hits",
            Interlude => "Interlude",
            Breakdown => "Breakdown",
            Vamp => "Vamp",
            Refrain => "Refrain",
            Turnaround => "Turnaround",
            CountIn => "Count-In",
            End => "End",
        };
        match s.number {
            Some(n) => format!("{base} {n}"),
            None => base.to_string(),
        }
    }

    fn is_audio_file(name: &str) -> bool {
        let l = name.to_lowercase();
        [".ogg", ".mp3", ".wav", ".webm", ".m4a", ".opus"]
            .iter()
            .any(|e| l.ends_with(e))
    }

    /// Infer a stem's display name, mixer group, and default-mute from its
    /// in-folder path (`stems/03-original-track.ogg` → "Original Track").
    /// Guide stems (click/cue/count) default muted; the original/reference
    /// track is the audible baseline.
    pub(crate) fn stem_spec_from_path(path: &str) -> StemSpec {
        let filename = path.rsplit('/').next().unwrap_or(path);
        let stem = filename.rsplit_once('.').map(|(s, _)| s).unwrap_or(filename);
        // Drop a leading `NN-` ordering prefix.
        let core = stem
            .split_once('-')
            .filter(|(p, _)| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            .map(|(_, rest)| rest)
            .unwrap_or(stem);
        let name = core
            .split(['-', '_'])
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut cs = w.chars();
                match cs.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let lower = core.to_lowercase();
        let words: Vec<&str> = core.split(['-', '_']).filter(|w| !w.is_empty()).collect();
        let has = |kw: &[&str]| kw.iter().any(|k| lower.contains(k));
        let has_word = |w: &[&str]| words.iter().any(|x| w.contains(&x.to_lowercase().as_str()));
        let is_guide = has(&["click", "cue", "count", "guide"]);
        // Group inference from the filename convention — reproduces the mixer
        // groups the legacy manifest authored (Guide / Reference / Bass /
        // Guitars / Keys / Drums / Vocals / Tracks). Order matters: `bass`
        // before `guitar`/`synth` so `synth-bass` lands in Bass.
        let group = if is_guide {
            "Guide"
        } else if has(&["original", "reference"]) {
            "Reference"
        } else if has(&["bass"]) {
            "Bass"
        } else if has(&["guitar", "gtr"]) || has_word(&["ag", "eg"]) {
            "Guitars"
        } else if has(&["organ", "piano", "keys", "rhodes", "wurli", "synth", "pad"])
            || has_word(&["key"])
        {
            "Keys"
        } else if has(&["drum", "perc", "kick", "snare", "hat", "cymbal", "tom", "shaker"]) {
            "Drums"
        } else if has(&["vocal", "vox", "bgv", "choir", "harm"]) || has_word(&["lead", "bv"]) {
            "Vocals"
        } else if has(&["loop", "track", "stem", "arp", "fx"]) {
            "Tracks"
        } else {
            "Stems"
        };
        StemSpec {
            name: if name.is_empty() { core.to_string() } else { name },
            group: Some(group.to_string()),
            file: path.to_string(),
            default_muted: is_guide,
        }
    }

    /// Build a [`Manifest`] with NO `manifest.json`, from the colocated
    /// `song` folder schema (issues #57/#59): read `song.md` → the default
    /// arrangement's `arrangement.md` → its `chartRef` (chart → sections via
    /// [`session::setlist::chart_import::chart_to_layout`]) and `attachmentRefs` (the
    /// audio stems). The "drop a song folder and it plays" path.
    pub(crate) async fn fetch_kf_manifest(org: &str, slug: &str) -> Result<Manifest, String> {
        let base = format!("/org/{org}/media/songs/{slug}");
        // One signed grant covers this song's whole folder; it is a query
        // suffix, so it appends to the FULL url, never to `base`.
        let tok = crate::media_grant::suffix(org, slug).await;
        // song.md → the default arrangement's folder.
        let song_md = fetch_text(&format!("{base}/song.md{tok}"))
            .await
            .map_err(|e| format!("no song.md for `{slug}`: {e}"))?;
        let idx: SongIndexLite = parse_fm(&song_md, "song.md")?;
        let dir = idx
            .arrangements
            .iter()
            .find(|a| Some(&a.id) == idx.default_arrangement.as_ref())
            .or_else(|| idx.arrangements.first())
            .map(|a| a.dir.clone())
            .ok_or_else(|| format!("`{slug}`: no arrangements"))?;
        // arrangement.md → chartRef + attachmentRefs.
        let arr_md = fetch_text(&format!("{base}/arrangements/{dir}/arrangement.md{tok}"))
            .await
            .map_err(|e| format!("`{slug}` arrangement `{dir}`: {e}"))?;
        let arr: ArrangementLite = parse_fm(&arr_md, "arrangement.md")?;
        // Stems = the audio attachment refs (paths relative to the song root).
        let mut stems: Vec<StemSpec> = arr
            .attachment_refs
            .iter()
            .filter_map(|a| a.path.as_deref())
            .filter(|p| is_audio_file(p))
            .map(stem_spec_from_path)
            .collect();
        stems.sort_by(|a, b| a.file.cmp(&b.file));
        if stems.is_empty() {
            return Err(format!("`{slug}`: no audio stems"));
        }
        // Structure / tempo from the referenced chart (optional — a song can
        // play stems with no chart, just without a section timeline).
        let chart_path = arr.chart_ref.as_ref().and_then(|c| c.path.clone());
        let mut sections = Vec::new();
        let mut bpm = None;
        let mut time_signature = None;
        let mut chart_key = None;
        let mut chart_title = None;
        let mut chart_end = 0.0f64;
        if let Some(cp) = chart_path {
            if let Ok(chart_text) = fetch_text(&format!("{base}/{cp}{tok}")).await {
                if let Ok(layout) = session::setlist::chart_import::chart_to_layout(&chart_text) {
                    sections = layout
                        .sections
                        .iter()
                        .filter(|s| s.kind != session::keyflow::actions::SectionKind::CountIn)
                        .map(|s| Section {
                            name: laid_section_name(s),
                            start_sec: s.start_seconds,
                            end_sec: s.end_seconds,
                        })
                        .collect();
                    bpm = Some(layout.tempo_bpm);
                    time_signature =
                        Some(format!("{}/{}", layout.time_sig_num, layout.time_sig_den));
                    chart_key = layout.key;
                    chart_title = layout.title;
                    chart_end = layout.song_end_seconds;
                }
            }
        }
        let duration_sec = chart_end.max(sections.last().map(|s| s.end_sec).unwrap_or(0.0));
        Ok(Manifest {
            slug: Some(slug.to_owned()),
            title: idx.title.or(chart_title),
            artist: None,
            key: arr.key.or(chart_key),
            bpm,
            time_signature,
            duration_sec,
            sections,
            stems,
        })
    }

    /// Resolve a song's [`Manifest`] the colocated-schema-first way: the
    /// `song` folder (`song.md` → `arrangement.md`) first, then a legacy
    /// `manifest.json`. Shared by the single-song and setlist players so both
    /// read migrated (manifest-less) songs identically.
    pub(crate) async fn load_song_manifest(org: &str, slug: &str) -> Result<Manifest, String> {
        match fetch_kf_manifest(org, slug).await {
            Ok(m) => Ok(m),
            Err(_) => {
                let tok = crate::media_grant::suffix(org, slug).await;
                fetch_manifest(&format!(
                    "/org/{org}/media/songs/{slug}/manifest.json{tok}"
                ))
                .await
            }
        }
    }

    /// Where one stem's audio comes from.
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) enum StemSource {
        /// Plain URL for an `HTMLAudioElement` (legacy `/media` files,
        /// signed blob URLs — the browser range-requests over HTTP).
        Url(String),
        /// Streamed over the org's vox lane into a MediaSource-backed
        /// element (`vox_media_source`). webm/opus stems only.
        Vox { org: String, hash: String },
    }

    /// Resolve a frontmatter-stems song. webm/opus stems stream over
    /// vox (MediaSource fed by `MediaService` chunks); anything else
    /// falls back to a short-lived signed `/blobs/download` URL. The
    /// manifest is synthesized from the frontmatter scalars +
    /// `sections:` block.
    pub(crate) async fn resolve_front(
        org: &str,
        title: &str,
        front: &task_ui_core::frontmatter::SongFront,
    ) -> Result<(Manifest, Vec<StemSource>), String> {
        use attachments_proto::ContentHashArg;
        use crate::vox_media_source::mse_supported;

        let media = crate::media_client(org).await.ok();
        let mut sources = Vec::with_capacity(front.stems.len());
        for stem in &front.stems {
            // Vox-MSE path: browser speaks webm/opus MSE AND the blob
            // was ingested as webm (mime from the media stat).
            if mse_supported() {
                if let Some(m) = &media {
                    if let Ok(info) = m.stat(stem.content_hash.clone()).await {
                        if info.mime_type.starts_with("audio/webm") {
                            sources.push(StemSource::Vox {
                                org: org.to_owned(),
                                hash: stem.content_hash.clone(),
                            });
                            continue;
                        }
                    }
                }
            }
            // Fallback: signed HTTP URL (ogg-opus + older ingests).
            let client = crate::attachments_client(org).await?;
            let signed = client
                .get_download_url(ContentHashArg {
                    content_hash: stem.content_hash.clone(),
                })
                .await
                .map_err(|e| format!("stem `{}`: {e:?}", stem.name))?;
            sources.push(StemSource::Url(signed.url));
        }
        let duration_sec = front
            .duration_sec
            .or_else(|| front.sections.last().map(|s| s.end_sec))
            .unwrap_or(0.0);
        let manifest = Manifest {
            slug: None,
            title: Some(title.to_owned()),
            artist: front.artist.clone(),
            key: front.key.clone(),
            bpm: front.bpm,
            time_signature: front.time_signature.clone(),
            duration_sec,
            sections: front
                .sections
                .iter()
                .map(|s| Section {
                    name: s.name.clone(),
                    start_sec: s.start_sec,
                    end_sec: s.end_sec,
                })
                .collect(),
            stems: front
                .stems
                .iter()
                .map(|s| StemSpec {
                    name: s.name.clone(),
                    group: s.group.clone(),
                    file: String::new(),
                    default_muted: s.default_muted,
                })
                .collect(),
        };
        Ok((manifest, sources))
    }

    /// Build the streaming graph: create the context, and for each stem create
    /// an `<audio>` element — plain URL (progressive HTTP) or
    /// MediaSource-fed-from-vox, per its [`StemSource`] — routed through
    /// media-element-source → gain → destination. `sources` is parallel to
    /// `manifest.stems`. Synchronous and side-effect free apart from element
    /// creation (vox feeds run in spawned tasks) — nothing to re-fire on
    /// transport ticks.
    pub(crate) fn build_engine(
        manifest: &Manifest,
        sources: &[StemSource],
    ) -> Result<Engine, String> {
        let ctx = AudioContext::new().map_err(|e| format!("AudioContext: {e:?}"))?;
        let dest = ctx.destination();
        let mut stems = Vec::with_capacity(manifest.stems.len());
        for (spec, source) in manifest.stems.iter().zip(sources) {
            let el = match source {
                StemSource::Url(url) => HtmlAudioElement::new_with_src(url)
                    .map_err(|e| format!("audio element: {e:?}"))?,
                StemSource::Vox { org, hash } => {
                    crate::vox_media_source::audio_element_over_vox(
                        org.clone(),
                        hash.clone(),
                    )?
                }
            };
            el.set_preload("auto");
            el.set_loop(false);

            let node = ctx
                .create_media_element_source(&el)
                .map_err(|e| format!("media element source: {e:?}"))?;
            let gain = ctx.create_gain().map_err(|e| format!("create_gain: {e:?}"))?;
            // Deref coercion: &MediaElementAudioSourceNode / &GainNode → &AudioNode.
            let _ = node.connect_with_audio_node(&gain);
            let _ = gain.connect_with_audio_node(&dest);
            // Metering tap: gain → analyser (a side branch that isn't connected
            // onward, so it only observes and never colors the signal). Small
            // FFT → cheap time-domain reads for a peak meter.
            let analyser = ctx
                .create_analyser()
                .map_err(|e| format!("create_analyser: {e:?}"))?;
            analyser.set_fft_size(256);
            let _ = gain.connect_with_audio_node(&analyser);
            gain.gain()
                .set_value(if spec.default_muted { 0.0 } else { 1.0 });
            // Kick off buffering.
            el.load();

            stems.push(StemNode {
                el,
                gain,
                node,
                analyser,
            });
        }

        Ok(Rc::new(RefCell::new(EngineInner {
            duration: manifest.duration_sec,
            ctx,
            stems,
            playing: false,
        })))
    }

    // ── manifest → session-proto mapping ────────────────────────────────────

    /// Parse a `"num/denom"` time-signature string, defaulting to 4/4.
    fn parse_time_sig(s: Option<&String>) -> (u32, u32) {
        s.and_then(|t| {
            let (n, d) = t.split_once('/')?;
            Some((n.trim().parse().ok()?, d.trim().parse().ok()?))
        })
        .filter(|(n, d)| *n > 0 && *d > 0)
        .unwrap_or((4, 4))
    }

    /// Map a section name to a keyflow `SectionType` (for colors / abbrev),
    /// falling back to a custom type so unknown names still render.
    fn section_type_of(name: &str) -> SectionType {
        SectionType::parse(name).unwrap_or_else(|_| SectionType::Custom(name.to_string()))
    }

    /// Build a `session_proto::Section` from a manifest section — reused for
    /// both the `SETLIST_STRUCTURE` song and the progress-bar segments so
    /// colors/short-names come from one place.
    fn to_session_section(sec: &Section) -> SessionSection {
        SessionSection {
            section_id: SectionId::new(),
            id: None,
            name: sec.name.clone(),
            comment: None,
            section_type: section_type_of(&sec.name),
            start_seconds: sec.start_sec,
            end_seconds: sec.end_sec,
            number: None,
            color: None,
        }
    }

    /// Sections derived from the KEYFLOW chart, labelled exactly like the
    /// engraved chart (`VS 1 A`, `CH 2 A`, `PRE-CH`, …) via
    /// `chart_section_timeline`, with timing from the chart's measures (real
    /// music starts after the count-in). `None` if there's no parseable chart
    /// or it has no real sections — the caller then falls back to the manifest's
    /// audio-region sections.
    fn chart_sections(chart_text: &str, bpm: f64, ts_num: u32) -> Option<Vec<SessionSection>> {
        use keyflow::engraver::layout::chart::section_layout::chart_section_timeline;
        let chart = keyflow::parse(chart_text).ok()?;
        let spans = chart_section_timeline(&chart);
        let spm = (60.0 / bpm.max(1.0)) * (ts_num.max(1) as f64);
        // Real music begins after the count-in; align section 0 to that offset
        // so the chart-measure timeline sits on the audio's SONGSTART.
        let content_start = spans
            .iter()
            .find(|s| s.is_count_in)
            .map(|s| s.measure_count as f64 * spm)
            .unwrap_or(0.0);
        let sections: Vec<SessionSection> = spans
            .iter()
            .filter(|s| !s.is_count_in)
            .map(|s| SessionSection {
                section_id: SectionId::new(),
                id: None,
                name: s.label.clone(),
                comment: None,
                section_type: s.section_type.clone(),
                start_seconds: content_start + s.start_measure as f64 * spm,
                end_seconds: content_start + (s.start_measure + s.measure_count) as f64 * spm,
                number: None,
                color: None,
            })
            .collect();
        (!sections.is_empty()).then_some(sections)
    }

    /// Build one `session_proto::Song` from a manifest — the per-song core
    /// shared by the single-song player and the setlist player. Each song's
    /// sections are in its own local seconds (0-based), and its `project_guid`
    /// is `web-session:{slug}` so `SONG_CHARTS` and the chart pane can key off
    /// it. Sections prefer the KEYFLOW chart's own labelling/timing (so the
    /// navigator + progress bars read `VS 1 A` / `CH 2 A` like the chart), and
    /// fall back to the manifest's audio-region sections when there's no chart.
    pub(crate) fn build_song(
        slug: &str,
        manifest: &Manifest,
        chart_text: Option<String>,
    ) -> SessionSong {
        let (ts_num, ts_denom) = parse_time_sig(manifest.time_signature.as_ref());
        let sections: Vec<SessionSection> = chart_text
            .as_deref()
            .and_then(|t| chart_sections(t, manifest.bpm.unwrap_or(120.0), ts_num))
            .unwrap_or_else(|| manifest.sections.iter().map(to_session_section).collect());
        SessionSong {
            id: SongId::new(),
            name: manifest.title.clone().unwrap_or_default(),
            project_guid: format!("{PROJECT_GUID_PREFIX}{slug}"),
            start_seconds: 0.0,
            end_seconds: manifest.duration_sec,
            count_in_seconds: None,
            sections,
            comments: Vec::new(),
            tempo: manifest.bpm,
            time_signature: Some(TimeSignature::new(ts_num, ts_denom)),
            measure_positions: Vec::new(),
            chart_text,
            parsed_chart: None,
            detected_chords: Vec::new(),
            chart_fingerprint: None,
            advance_mode: None,
            color: None,
        }
    }

    /// Build the one-song `Setlist` that drives session-ui from the manifest.
    fn build_setlist(slug: &str, manifest: &Manifest, chart_text: Option<String>) -> Setlist {
        let song = build_song(slug, manifest, chart_text);
        Setlist {
            id: Some(slug.to_string()),
            name: manifest.title.clone().unwrap_or_default(),
            advance_mode: session_proto::AdvanceMode::Wait,
            songs: vec![song],
        }
    }

    /// Progress-bar segments (percent of song duration) from the manifest.
    pub(crate) fn progress_sections(manifest: &Manifest) -> Vec<ProgressSection> {
        let dur = manifest.duration_sec.max(0.001);
        manifest
            .sections
            .iter()
            .map(|sec| {
                let s = to_session_section(sec);
                ProgressSection {
                    start_percent: (sec.start_sec / dur * 100.0).clamp(0.0, 100.0),
                    end_percent: (sec.end_sec / dur * 100.0).clamp(0.0, 100.0),
                    color: s.bright_color(),
                    name: s.display_name(),
                    short_name: s.short_display(),
                    comment: None,
                }
            })
            .collect()
    }

    /// Progress-bar segments from a hydrated `Song`'s sections (which prefer the
    /// KEYFLOW chart's labelling). Used by the setlist so the bars read the same
    /// `VS 1 A` / `CH 2 A` as the navigator + the engraved chart.
    pub(crate) fn progress_sections_from_song(song: &SessionSong) -> Vec<ProgressSection> {
        let dur = (song.end_seconds - song.start_seconds).max(0.001);
        song.sections
            .iter()
            .map(|sec| ProgressSection {
                start_percent: ((sec.start_seconds - song.start_seconds) / dur * 100.0)
                    .clamp(0.0, 100.0),
                end_percent: ((sec.end_seconds - song.start_seconds) / dur * 100.0)
                    .clamp(0.0, 100.0),
                color: sec.bright_color(),
                name: sec.display_name(),
                short_name: sec.short_display(),
                comment: sec.comment.clone(),
            })
            .collect()
    }

    /// Musical position (measure.beat.subdivision) from seconds + tempo/meter.
    pub(crate) fn musical_at(seconds: f64, bpm: f64, ts_num: u32) -> MusicalPosition {
        let bpm = if bpm > 0.0 { bpm } else { 120.0 };
        let num = ts_num.max(1) as f64;
        let beats_total = (seconds.max(0.0)) * bpm / 60.0;
        let measure = (beats_total / num).floor();
        let beat_in_measure = beats_total - measure * num;
        let beat = beat_in_measure.floor();
        let subdivision = ((beat_in_measure - beat) * 1000.0).round().clamp(0.0, 999.0);
        MusicalPosition::new(measure as i32, beat as i32, subdivision as i32)
    }

    /// Whether a stem is part of the click/guide bus (Click + Cue/Guide stems).
    pub(crate) fn is_guide_stem(spec: &StemSpec) -> bool {
        let hay = format!(
            "{} {}",
            spec.name.to_lowercase(),
            spec.group.as_deref().unwrap_or("").to_lowercase()
        );
        hay.contains("click") || hay.contains("guide") || hay.contains("cue")
    }

    /// Deterministic accent color (0xRRGGBB) for a stem group.
    fn group_color(group: &str) -> u32 {
        // The Reference group is always neutral gray — the reference / original
        // track is a backing reference, not an instrument, so it reads quiet and
        // sits apart from the colored instrument folders.
        if group.eq_ignore_ascii_case("reference") {
            return 0x71717a; // zinc-500
        }
        // Small fixed palette, hashed by group name for stable coloring.
        const PALETTE: [u32; 8] = [
            0x3B82F6, // blue
            0x22C55E, // green
            0xF59E0B, // amber
            0xEF4444, // red
            0xA855F7, // purple
            0x14B8A6, // teal
            0xEC4899, // pink
            0xF97316, // orange
        ];
        let sum: u32 = group.bytes().map(|b| b as u32).sum();
        PALETTE[(sum as usize) % PALETTE.len()]
    }

    /// Map the per-stem UI state to a flat `daw_proto::Track` list for MixerView.
    pub(crate) fn stems_to_tracks(manifest: &Manifest, ui: &[StemUi]) -> Vec<Track> {
        manifest
            .stems
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let st = ui.get(i).copied().unwrap_or(StemUi {
                    muted: spec.default_muted,
                    soloed: false,
                    volume: 1.0,
                });
                let mut t = Track::new(spec.file.clone(), i as u32, spec.name.clone());
                t.color = Some(group_color(spec.group.as_deref().unwrap_or("Other")));
                t.muted = st.muted;
                t.soloed = st.soloed;
                t.volume = st.volume as f64;
                t
            })
            .collect()
    }

    // ── small format helpers ────────────────────────────────────────────────

    #[allow(dead_code)] // kept as a shared helper; the inline scrubber that
    /// Which tab is showing in the player.
    #[derive(Clone, Copy, PartialEq)]
    enum Tab {
        Chart,
        Mixer,
    }

    // ── the component ───────────────────────────────────────────────────────

    /// The `type: song` view. A song plays through the SAME session player
    /// as a setlist — as a one-song set — so the single-song page and the
    /// setlist viewer are one identical experience (header + timeline +
    /// Session/Chart tabs + transport), not two divergent players.
    ///
    /// Media-served songs (a `song.md` / `manifest.json` at
    /// `/org/{org}/media/songs/{slug}`) take that path. A song whose stems
    /// exist only as content-addressed blobs (a frontmatter `stems:` block,
    /// no `/media` folder) falls back to the standalone streaming player
    /// ([`StandaloneSongPlayer`]).
    #[component]
    pub fn SongView(
        slug: String,
        org: String,
        title: String,
        front: task_ui_core::frontmatter::SongFront,
    ) -> Element {
        // Probe whether the song is media-served (the common, migrated case).
        // Cheap: one HEAD-ish GET, browser-cached and re-used by SetlistPlayer.
        let slug_p = slug.clone();
        let org_p = org.clone();
        let media = use_resource(use_reactive!(|slug_p, org_p| {
            let slug = slug_p.clone();
            let org = org_p.clone();
            async move {
                let base = format!("/org/{org}/media/songs/{slug}");
                let tok = crate::media_grant::suffix(&org, &slug).await;
                fetch_text(&format!("{base}/song.md{tok}")).await.is_ok()
                    || fetch_text(&format!("{base}/manifest.json{tok}")).await.is_ok()
            }
        }));

        match *media.read_unchecked() {
            None => rsx! {
                div { class: "flex flex-col gap-2 py-10",
                    span { class: "text-sm text-muted-foreground", "Loading song…" }
                }
            },
            // Media-served → the one-song setlist session player (embedded).
            Some(true) => rsx! {
                crate::setlist_session::SetlistPlayer {
                    songs: vec![slug.clone()],
                    org: org.clone(),
                    fullscreen: false,
                }
            },
            // Blob-only → the standalone streaming player (frontmatter stems).
            Some(false) => rsx! {
                StandaloneSongPlayer {
                    slug: slug.clone(),
                    org: org.clone(),
                    title: title.clone(),
                    front: front.clone(),
                }
            },
        }
    }

    /// The standalone single-song streaming player: resolves stems (colocated
    /// `song.md` → legacy `manifest.json` → content-addressed blobs) and plays
    /// them through its own Web-Audio graph. Retained as the fallback for
    /// blob-only songs that have no `/media` folder; media-served songs go
    /// through [`SetlistPlayer`] instead (see [`SongView`]).
    #[component]
    fn StandaloneSongPlayer(
        slug: String,
        org: String,
        title: String,
        front: task_ui_core::frontmatter::SongFront,
    ) -> Element {
        let mut playing = use_signal(|| false);
        let mut position = use_signal(|| 0.0_f64);
        let mut buffering = use_signal(|| true);
        // Per-stem mixer state; filled once the manifest lands (see effect).
        let mut stem_ui = use_signal(Vec::<StemUi>::new);

        // Resolve stems → build the streaming graph. Keyed on the props via
        // `use_reactive!`: the future reads ONLY props (no signals), so it
        // runs once per song (re-firing if the frontmatter stems change) and
        // never on ticks.
        let slug_r = slug.clone();
        let org_r = org.clone();
        let title_r = title.clone();
        let front_r = front.clone();
        let loaded = use_resource(use_reactive!(|slug_r, org_r, title_r, front_r| {
            let slug = slug_r.clone();
            let org = org_r.clone();
            let title = title_r.clone();
            let front = front_r.clone();
            async move {
                // Colocated-schema-first: derive the song model from the
                // `song` folder (song.md → arrangement.md → chart + stems).
                // The legacy `manifest.json` is now only a fallback for songs
                // not yet migrated, so migrated songs can drop it entirely
                // (#57 manifest retirement). A frontmatter `stems:` block (blob
                // resolver) is the last resort. Refs #56/#57/#59.
                // Every stem URL below carries this song's grant.
                let tok = crate::media_grant::suffix(&org, &slug).await;
                let manifest_url =
                    format!("/org/{org}/media/songs/{slug}/manifest.json{tok}");
                let url_sources = |m: &Manifest| -> Vec<StemSource> {
                    m.stems
                        .iter()
                        .map(|s| {
                            StemSource::Url(format!(
                                "/org/{org}/media/songs/{slug}/{}{tok}",
                                s.file
                            ))
                        })
                        .collect()
                };
                let (manifest, sources) = match fetch_kf_manifest(&org, &slug).await {
                    Ok(manifest) => {
                        let sources = url_sources(&manifest);
                        (manifest, sources)
                    }
                    // No song.md → legacy manifest.json, then the frontmatter
                    // blob resolver.
                    Err(_) => match fetch_manifest(&manifest_url).await {
                        Ok(manifest) => {
                            let sources = url_sources(&manifest);
                            (manifest, sources)
                        }
                        Err(_) if !front.stems.is_empty() => {
                            resolve_front(&org, &title, &front).await?
                        }
                        Err(e) => return Err(e),
                    },
                };
                let eng = build_engine(&manifest, &sources)?;
                Ok::<(Manifest, Engine), String>((manifest, eng))
            }
        }));

        // Chart source (optional). Fetched async; populates SONG_CHARTS +
        // SETLIST_STRUCTURE once present (see effect below).
        let slug_c = slug.clone();
        let org_c = org.clone();
        let mut chart_src = use_signal(String::new);
        use_future(move || {
            let slug = slug_c.clone();
            let org = org_c.clone();
            async move {
                let tok = crate::media_grant::suffix(&org, &slug).await;
                if let Ok(txt) =
                    fetch_text(&format!("/org/{org}/media/songs/{slug}/chart.kf{tok}")).await
                {
                    chart_src.set(txt);
                }
            }
        });

        // Clone the engine Rc out of the resource (or None while loading).
        let engine_of = move || -> Option<Engine> {
            loaded
                .read()
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|(_, e)| e.clone())
        };

        // Initialize the mixer state + populate SETLIST_STRUCTURE / SONG_CHARTS
        // once the engine is ready. Runs once per song (guarded by empty ui).
        let slug_for_setlist = slug.clone();
        use_effect(move || {
            if let Some(Ok((m, eng))) = &*loaded.read() {
                if stem_ui.read().is_empty() && !m.stems.is_empty() {
                    let v: Vec<StemUi> = m
                        .stems
                        .iter()
                        .map(|s| StemUi {
                            muted: s.default_muted,
                            soloed: false,
                            volume: 1.0,
                        })
                        .collect();
                    apply_mix(eng, &v);
                    stem_ui.set(v);

                    // Populate the session-ui structural signals.
                    let chart = chart_src.peek().clone();
                    let chart_opt = (!chart.is_empty()).then(|| chart.clone());
                    let setlist = build_setlist(&slug_for_setlist, m, chart_opt);
                    let guid = setlist
                        .songs
                        .first()
                        .map(|s| s.project_guid.clone())
                        .unwrap_or_default();
                    *SETLIST_STRUCTURE.write() = setlist;
                    if !chart.is_empty() {
                        SONG_CHARTS.write().insert(
                            guid,
                            SongChartHydration {
                                project_guid: String::new(),
                                chart_text: chart,
                                detected_chords: Vec::new(),
                                chart_fingerprint: String::new(),
                            },
                        );
                    }
                }
            }
        });

        // Chart arrived after the structure was built: backfill SONG_CHARTS +
        // the song's chart_text so SessionChartPane picks it up.
        use_effect(move || {
            let chart = chart_src.read().clone();
            if chart.is_empty() {
                return;
            }
            let mut setlist = SETLIST_STRUCTURE.write();
            if let Some(song) = setlist.songs.first_mut() {
                let guid = song.project_guid.clone();
                if song.chart_text.as_deref() != Some(chart.as_str()) {
                    song.chart_text = Some(chart.clone());
                    drop(setlist);
                    SONG_CHARTS.write().insert(
                        guid,
                        SongChartHydration {
                            project_guid: String::new(),
                            chart_text: chart,
                            detected_chords: Vec::new(),
                            chart_fingerprint: String::new(),
                        },
                    );
                }
            }
        });

        // ~10 Hz loop: readiness (buffering), drift correction, playhead, end,
        // and session-ui signal population (ACTIVE_INDICES / SONG_TRANSPORT).
        // A short timeout (~4s) enables Play even if a stem is slow to buffer.
        use_future(move || async move {
            let mut ticks: u32 = 0;
            loop {
                gloo_timers::future::TimeoutFuture::new(TICK_MS).await;
                ticks += 1;
                let Some(eng) = engine_of() else {
                    continue;
                };
                // Self-correct the total from the real audio once it loads —
                // a chart-derived duration omits any outro past the last
                // charted section, and the end-detect below must not stop
                // playback early. (The authoritative length is the audio.)
                {
                    let mut e = eng.borrow_mut();
                    if let Some(real) = e.stems.first().map(|s| s.el.duration()) {
                        if real.is_finite() && real > e.duration {
                            e.duration = real;
                        }
                    }
                }
                let (rc, total, is_playing, pos, dur) = {
                    let e = eng.borrow();
                    (
                        e.ready_count(),
                        e.stems.len(),
                        e.playing,
                        e.position(),
                        e.duration,
                    )
                };
                let all_ready = total > 0 && rc >= total;
                let timed_out = ticks > (4000 / TICK_MS);
                buffering.set(!(all_ready || timed_out));

                let mut pos = pos;
                if is_playing {
                    eng.borrow().correct_drift();
                    position.set(pos);
                    if dur > 0.0 && pos >= dur - 0.25 {
                        eng.borrow_mut().pause();
                        playing.set(false);
                        position.set(dur);
                        pos = dur;
                    }
                }

                push_session_signals(pos, is_playing);
            }
        });

        // ── transport actions (as Callbacks for the session-ui components) ────
        let play_pause: Callback<()> = use_callback(move |()| {
            if let Some(eng) = engine_of() {
                if playing() {
                    eng.borrow_mut().pause();
                    playing.set(false);
                } else {
                    let off = position();
                    eng.borrow_mut().play(off);
                    playing.set(true);
                }
            }
        });
        let seek: Callback<f64> = use_callback(move |off: f64| {
            if let Some(eng) = engine_of() {
                eng.borrow_mut().seek(off);
            }
            position.set(off);
            push_session_signals(off, playing());
        });

        // ── mixer mutators (by stem index) ───────────────────────────────────
        let toggle_mute: Callback<usize> = use_callback(move |i: usize| {
            let mut ui = stem_ui();
            if let Some(s) = ui.get_mut(i) {
                s.muted = !s.muted;
            }
            if let Some(eng) = engine_of() {
                apply_mix(&eng, &ui);
            }
            stem_ui.set(ui);
        });
        let toggle_solo: Callback<usize> = use_callback(move |i: usize| {
            let mut ui = stem_ui();
            if let Some(s) = ui.get_mut(i) {
                s.soloed = !s.soloed;
            }
            if let Some(eng) = engine_of() {
                apply_mix(&eng, &ui);
            }
            stem_ui.set(ui);
        });
        let set_volume: Callback<(usize, f32)> = use_callback(move |(i, v): (usize, f32)| {
            let mut ui = stem_ui();
            if let Some(s) = ui.get_mut(i) {
                s.volume = v;
            }
            if let Some(eng) = engine_of() {
                apply_mix(&eng, &ui);
            }
            stem_ui.set(ui);
        });
        // Set an explicit mute value for a set of stems (used by the Guide
        // toggle so it deterministically un/mutes the click + cue bus).
        let set_mutes: Callback<(Vec<usize>, bool)> =
            use_callback(move |(idxs, muted): (Vec<usize>, bool)| {
                let mut ui = stem_ui();
                for i in idxs {
                    if let Some(s) = ui.get_mut(i) {
                        s.muted = muted;
                    }
                }
                if let Some(eng) = engine_of() {
                    apply_mix(&eng, &ui);
                }
                stem_ui.set(ui);
            });

        // ── render ──────────────────────────────────────────────────────────
        let body = match &*loaded.read_unchecked() {
            None => rsx! {
                div { class: "flex flex-col gap-2 py-10",
                    span { class: "text-sm text-muted-foreground", "Loading song…" }
                }
            },
            Some(Err(msg)) => rsx! {
                div { class: "flex flex-col gap-2 py-10",
                    span { class: "text-sm font-semibold text-destructive", "Could not load song" }
                    span { class: "text-sm text-muted-foreground", "{msg}" }
                }
            },
            Some(Ok((manifest, _))) => {
                let manifest = manifest.clone();
                rsx! {
                    Player {
                        manifest,
                        playing,
                        position,
                        buffering,
                        stem_ui,
                        play_pause,
                        seek,
                        toggle_mute,
                        toggle_solo,
                        set_volume,
                        set_mutes,
                    }
                }
            }
        };

        rsx! {
            // Full-width: the embedded player fills the note column (no
            // artificial max-width) so the progress bars get real room.
            div { class: "w-full px-4 py-6 flex flex-col gap-5", {body} }
        }
    }

    /// Populate the session-ui global signals from the player's current state.
    /// Called each transport tick and after seeks. Uses `apply_active_indices`
    /// for the cursor + `PLAYBACK_STATE`, and writes `SONG_TRANSPORT[0]`.
    fn push_session_signals(pos: f64, is_playing: bool) {
        // Read structural facts from the setlist we populated on load.
        let (dur, count_in, bpm, ts_num, ts_denom, section_index, section_prog) = {
            let setlist = SETLIST_STRUCTURE.read();
            let Some(song) = setlist.songs.first() else {
                return;
            };
            let dur = song.duration().max(0.001);
            let bpm = song.tempo.unwrap_or(120.0);
            let ts = song.time_signature.unwrap_or(TimeSignature::COMMON_TIME);
            let (sec_idx, sec_prog) = song
                .section_at_position_with_index(pos)
                .map(|(i, s)| {
                    let d = s.duration().max(0.001);
                    (Some(i), ((pos - s.start_seconds) / d).clamp(0.0, 1.0))
                })
                // Before the first section's start (e.g. 0:00 with a late
                // first section) the cursor clamps to section 0 instead of
                // reading no/the-wrong section.
                .unwrap_or_else(|| {
                    if song.sections.first().is_some_and(|s| pos < s.start_seconds) {
                        (Some(0), 0.0)
                    } else {
                        (None, 0.0)
                    }
                });
            (
                dur,
                song.count_in_seconds.unwrap_or(0.0),
                bpm,
                ts.numerator(),
                ts.denominator(),
                sec_idx,
                sec_prog,
            )
        };

        let song_progress = (pos / dur).clamp(0.0, 1.0);
        let indices = ActiveIndices {
            song_index: Some(0),
            section_index,
            slide_index: None,
            song_progress: Some(song_progress),
            section_progress: Some(section_prog),
            is_playing,
            looping: false,
            loop_selection: None,
            queued_target: None,
        };
        apply_active_indices(&indices);

        let musical = musical_at(count_in + pos, bpm, ts_num);
        let transport = TransportState {
            position: Position::from_time_and_musical(PositionInSeconds::from_seconds(pos), musical),
            bpm,
            time_sig_num: ts_num as i32,
            time_sig_denom: ts_denom as i32,
            is_playing,
            is_looping: false,
            loop_region: None,
        };
        // Only write when the state actually changed to limit render fanout.
        let changed = SONG_TRANSPORT
            .peek()
            .get(&0)
            .map(|e| *e != transport)
            .unwrap_or(true);
        if changed {
            SONG_TRANSPORT.write().insert(0, transport);
        }
        // `PLAYBACK_STATE` is set by `apply_active_indices`; kept here as the
        // documented contract (Playing/Stopped).
        let _ = &*PLAYBACK_STATE;
    }

    /// The ready player: header, transport/progress (always visible), then a
    /// Chart / Mixer **tab** switcher. Split out so the reactive reads
    /// (position/stem_ui) live in a child scope.
    #[component]
    fn Player(
        manifest: Manifest,
        playing: Signal<bool>,
        position: Signal<f64>,
        buffering: Signal<bool>,
        stem_ui: Signal<Vec<StemUi>>,
        play_pause: Callback<()>,
        seek: Callback<f64>,
        toggle_mute: Callback<usize>,
        toggle_solo: Callback<usize>,
        set_volume: Callback<(usize, f32)>,
        set_mutes: Callback<(Vec<usize>, bool)>,
    ) -> Element {
        // Which tab is showing (Chart default).
        let mut tab = use_signal(|| Tab::Chart);

        let duration = manifest.duration_sec.max(0.001);
        let pos = position();
        let is_playing = playing();
        let is_buffering = buffering();

        let sections = progress_sections(&manifest);

        // Song / section progress (0-100) for the session-ui progress bars.
        let song_progress = (pos / duration * 100.0).clamp(0.0, 100.0);
        let cur_section = manifest
            .sections
            .iter()
            .position(|s| pos >= s.start_sec && pos < s.end_sec);
        let section_progress = cur_section
            .and_then(|i| manifest.sections.get(i))
            .map(|s| {
                let d = (s.end_sec - s.start_sec).max(0.001);
                ((pos - s.start_sec) / d * 100.0).clamp(0.0, 100.0)
            })
            .unwrap_or(0.0);

        // Guide (click + cue) stem indices, and whether the bus is on.
        let guide_idxs: Vec<usize> = manifest
            .stems
            .iter()
            .enumerate()
            .filter(|(_, s)| is_guide_stem(s))
            .map(|(i, _)| i)
            .collect();
        let guide_on = {
            let ui = stem_ui.read();
            guide_idxs
                .iter()
                .any(|&i| ui.get(i).map(|s| !s.muted).unwrap_or(false))
        };

        // ── session-ui callback adapters (guid ↔ stem index) ─────────────────
        let stems_for_lookup = manifest.stems.clone();
        let index_of = move |guid: &str| stems_for_lookup.iter().position(|s| s.file == guid);

        let mixer_volume: Callback<(String, f64)> = use_callback({
            let index_of = index_of.clone();
            move |(guid, v): (String, f64)| {
                if let Some(i) = index_of(&guid) {
                    set_volume.call((i, v as f32));
                }
            }
        });
        let mixer_mute: Callback<String> = use_callback({
            let index_of = index_of.clone();
            move |guid: String| {
                if let Some(i) = index_of(&guid) {
                    toggle_mute.call(i);
                }
            }
        });
        let mixer_solo: Callback<String> = use_callback({
            let index_of = index_of.clone();
            move |guid: String| {
                if let Some(i) = index_of(&guid) {
                    toggle_solo.call(i);
                }
            }
        });

        // Transport bar adapters.
        let on_play_pause: Callback<()> = use_callback(move |()| play_pause.call(()));
        let noop: Callback<()> = use_callback(move |()| {});
        let sections_for_back = manifest.sections.clone();
        let on_back: Callback<()> = use_callback(move |()| {
            let p = position();
            // Jump to the previous section boundary (or the current section's
            // start if we're already past its head).
            let target = sections_for_back
                .iter()
                .map(|s| s.start_sec)
                .filter(|&s| s < p - 1.0)
                .fold(0.0_f64, f64::max);
            seek.call(target);
        });
        let sections_for_fwd = manifest.sections.clone();
        let on_forward: Callback<()> = use_callback(move |()| {
            let p = position();
            if let Some(next) = sections_for_fwd
                .iter()
                .map(|s| s.start_sec)
                .find(|&s| s > p + 0.5)
            {
                seek.call(next);
            }
        });

        // Guide toggle: un/mute the click + cue stems together.
        let guide_idxs_for_toggle = guide_idxs.clone();
        let on_guide: Callback<()> = use_callback(move |()| {
            // If currently on (unmuted), mute; else unmute.
            set_mutes.call((guide_idxs_for_toggle.clone(), guide_on));
        });

        // Section-click seeks to the section start.
        let sections_for_click = manifest.sections.clone();
        let on_section_click: Callback<usize> = use_callback(move |i: usize| {
            if let Some(s) = sections_for_click.get(i) {
                seek.call(s.start_sec);
            }
        });

        let tracks = stems_to_tracks(&manifest, &stem_ui.read());
        let active = tab();

        rsx! {
            // Song progress (segmented sections; click to seek). Always visible.
            if !manifest.sections.is_empty() {
                div { class: "pt-2",
                    SongProgressBar {
                        progress: song_progress,
                        sections: sections.clone(),
                        song_key: manifest.key.clone(),
                        on_section_click,
                    }
                }
                // Progress within the current section.
                SectionProgressBar {
                    progress: section_progress,
                    sections: sections.clone(),
                    song_key: manifest.key.clone(),
                }
            }

            // Buffering hint (the scrubbing UI lives in the progress bars above).
            if is_buffering {
                div { class: "text-[11px] text-muted-foreground/70", "buffering…" }
            }

            // Transport bar (real session-ui component) — playback only, so
            // Arm/Record are hidden. Loop is a no-op in the browser player;
            // play/back/forward drive the engine.
            div { class: "h-16 rounded-lg overflow-hidden border border-border",
                TransportControlBar {
                    is_playing,
                    is_looping: false,
                    is_recording: false,
                    is_armed: false,
                    show_recording: false,
                    on_play_pause,
                    on_loop_toggle: noop,
                    on_record_toggle: noop,
                    on_arm_toggle: noop,
                    on_back,
                    on_forward,
                }
            }

            // ── Chart / Mixer tab switcher ───────────────────────────────────
            div { class: "flex items-center gap-1 border-b border-border",
                button {
                    class: if active == Tab::Chart {
                        "px-4 py-2 text-sm font-semibold border-b-2 border-primary text-foreground"
                    } else {
                        "px-4 py-2 text-sm font-semibold border-b-2 border-transparent text-muted-foreground hover:text-foreground transition-colors"
                    },
                    onclick: move |_| tab.set(Tab::Chart),
                    "Chart"
                }
                button {
                    class: if active == Tab::Mixer {
                        "px-4 py-2 text-sm font-semibold border-b-2 border-primary text-foreground"
                    } else {
                        "px-4 py-2 text-sm font-semibold border-b-2 border-transparent text-muted-foreground hover:text-foreground transition-colors"
                    },
                    onclick: move |_| tab.set(Tab::Mixer),
                    "Mixer"
                }
            }

            // Chart tab — engraver SVG rendered INLINE + synced playhead.
            div { class: if active == Tab::Chart { "block" } else { "hidden" },
                div { class: "border border-border rounded-lg overflow-hidden bg-white",
                    SessionChartPane {}
                }
            }

            // Mixer tab — per-stem MixerView + the Guide/click toggle.
            div { class: if active == Tab::Mixer { "flex flex-col gap-3" } else { "hidden" },
                // Guide (click) toggle — un/mutes the Click + Cue stems together.
                if !guide_idxs.is_empty() {
                    div { class: "flex items-center gap-3 p-3 border border-border rounded-lg bg-card",
                        span { class: "text-sm font-semibold text-foreground flex-1", "Guide / Click" }
                        button {
                            class: if guide_on {
                                "px-4 py-1.5 rounded-md text-sm font-semibold bg-primary text-primary-foreground hover:bg-primary/90 transition-colors"
                            } else {
                                "px-4 py-1.5 rounded-md text-sm font-semibold bg-muted text-muted-foreground hover:bg-accent transition-colors"
                            },
                            onclick: move |_| on_guide.call(()),
                            if guide_on { "On" } else { "Off" }
                        }
                    }
                }
                // Per-stem mixer (real session-ui MixerView, backed by the
                // Web-Audio GainNodes / mute / solo state).
                div { class: "h-56 rounded-lg overflow-hidden border border-border bg-card",
                    MixerView {
                        tracks,
                        on_volume: mixer_volume,
                        on_mute: mixer_mute,
                        on_solo: mixer_solo,
                    }
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::SongView;

// ─────────────────────────────────────────────────────────────────────────────
// Non-wasm: a stub so the crate still compiles on native. The session player
// is a browser-only feature (Web Audio + media elements).
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(not(target_arch = "wasm32"))]
mod stub {
    use dioxus::prelude::*;

    #[component]
    pub fn SongView(
        slug: String,
        org: String,
        title: String,
        front: task_ui_core::frontmatter::SongFront,
    ) -> Element {
        let _ = (&slug, &org, &title, &front);
        rsx! {
            div { class: "mx-auto max-w-3xl px-4 py-10",
                span { class: "text-sm text-muted-foreground",
                    "The session player runs in the browser."
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use stub::SongView;

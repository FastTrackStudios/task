//! `task collection …` + `task song …` — headless library / setlist
//! seeding (W0.5).
//!
//! A song **Library**, a **Setlist**, a **Show**, and a **Playlist** are
//! the same primitive: an ordered list of [`NodeRef`] items served by
//! `CollectionService`. These verbs create such collections, populate and
//! reorder them, and inspect them — all without a GUI, so the W7 (Alan
//! Parsons import) and W8 (Days to Praise seed) pipelines can drive them.
//!
//! `task song add` is the composite verb: it builds a durable **Song
//! folder** with the `song` crate (a `song.md` index + a default
//! arrangement), attaches chart / pdf / audio files, and then registers
//! the song in a target collection as a `song:<slug>` node.
//!
//! Lives in its own module (like `plan` / `workstream`) so concurrent
//! agents editing `main.rs` only collide on the two-line dispatch arm.
//! Client construction, org resolution, and the vox transport all reuse
//! the shared helpers in `main.rs` (`crate::establish_client`,
//! `crate::resolve_active_org`, `crate::org_ctx`).

use std::path::{Path, PathBuf};

use clap::Subcommand;
use collection_proto::{
    Collection, CollectionKind, CollectionServiceClient, NodeKind, NodeRef, Placement,
};
use eyre::Context;
use song::{Arrangement, AttachmentRef, ChartRef, Key, PartsManifest, Song};
use uuid::Uuid;

use crate::errors;

// ── CLI surface ───────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum CollectionCmd {
    /// Create an empty collection.
    Create {
        /// Human title (e.g. `"Sunday Library"`).
        title: String,
        /// What kind of collection: `library` | `setlist` | `show` |
        /// `playlist` | `other:NAME`.
        #[arg(long, default_value = "library")]
        kind: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Add a node to a collection (append, or insert after `--after`).
    Add {
        /// Target collection — id or title.
        collection: String,
        /// Node reference token (`kind:id`, e.g. `song:great-are-you-lord`).
        #[arg(long)]
        node: String,
        /// Insert immediately after this node token. Omit to append.
        #[arg(long)]
        after: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Move an existing item to sit after `--after` (or to the tail).
    Reorder {
        /// Target collection — id or title.
        collection: String,
        /// The node token to move.
        #[arg(long)]
        node: String,
        /// Move the item to sit after this node token. Omit for the tail.
        #[arg(long)]
        after: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove the item pointing at `--node` from a collection.
    Remove {
        /// Target collection — id or title.
        collection: String,
        /// The node token to remove (`kind:id`, e.g. `song:king-of-kings`).
        #[arg(long)]
        node: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List collections in the active org, optionally filtered by kind.
    List {
        /// Restrict to one kind (`library` | `setlist` | … | `other:NAME`).
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one collection and its ordered items.
    Show {
        /// Collection — id or title.
        collection: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum SongCmd {
    /// Build a Song folder and add it to a collection as a `song:` node.
    Add {
        /// Target collection — id or title.
        collection: String,
        /// Song title (e.g. `"Great Are You Lord"`).
        #[arg(long)]
        title: String,
        /// Musical key of the default arrangement (`"Bb Minor"`,
        /// `"F# Dorian"`, `"C"`). Defaults to `C Major`.
        #[arg(long)]
        key: Option<String>,
        /// Chart file (e.g. a `.kf` keyflow chart) — copied into the
        /// default arrangement folder.
        #[arg(long, value_name = "FILE")]
        chart: Option<PathBuf>,
        /// PDF attachment(s). Repeatable.
        #[arg(long = "pdf", value_name = "FILE")]
        pdf: Vec<PathBuf>,
        /// Audio attachment(s). Repeatable.
        #[arg(long = "audio", value_name = "FILE")]
        audio: Vec<PathBuf>,
        /// Name of the default arrangement (default `Default`).
        #[arg(long)]
        arrangement: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Ingest a folder of stems for a `type: song` note: transcode
    /// WAV→Opus, upload each as a content-addressed attachment, and
    /// write the `stems:` block into the note's frontmatter.
    Ingest {
        /// Vault-relative path of the song note (e.g. `Songs/Praise.md`).
        /// Created with `type: song` frontmatter if it doesn't exist.
        note: String,
        /// Directory of stem audio files (wav/aiff/flac/ogg/…), one per
        /// stem, sorted by filename. A leading `NN - ` index is stripped
        /// from the display name.
        #[arg(long, value_name = "DIR")]
        stems: PathBuf,
        /// Artist written into the frontmatter.
        #[arg(long)]
        artist: Option<String>,
        /// Musical key written into the frontmatter (e.g. `"Bb"`).
        #[arg(long)]
        key: Option<String>,
        /// Tempo written into the frontmatter.
        #[arg(long)]
        bpm: Option<f64>,
        /// Time signature written into the frontmatter (e.g. `4/4`).
        #[arg(long)]
        time_signature: Option<String>,
        /// Song duration in seconds. Probed from the first stem via
        /// ffprobe when omitted.
        #[arg(long)]
        duration_sec: Option<f64>,
        /// Opus bitrate for the transcode.
        #[arg(long, default_value = "96k")]
        bitrate: String,
        /// Plan only: list stems/groups and what would upload, touch nothing.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

// ── parsing helpers ───────────────────────────────────────────────────

/// Parse a `--kind` value into a [`CollectionKind`]. `other:NAME` maps to
/// [`CollectionKind::Other`]; the four named kinds match case-insensitively;
/// anything else falls through to `Other(<verbatim>)`.
fn parse_kind(s: &str) -> CollectionKind {
    if let Some(rest) = s
        .strip_prefix("other:")
        .or_else(|| s.strip_prefix("Other:"))
        .or_else(|| s.strip_prefix("OTHER:"))
    {
        return CollectionKind::Other(rest.to_string());
    }
    match s.to_ascii_lowercase().as_str() {
        "library" => CollectionKind::Library,
        "setlist" => CollectionKind::Setlist,
        "show" => CollectionKind::Show,
        "playlist" => CollectionKind::Playlist,
        other => CollectionKind::Other(other.to_string()),
    }
}

/// Parse a `kind:id[#anchor]` node token into a [`NodeRef`].
fn parse_node(token: &str) -> eyre::Result<NodeRef> {
    NodeRef::parse(token).ok_or_else(|| {
        errors::usage("parse node")
            .cause(format!("`{token}` is not a `kind:id` token"))
            .hint("e.g. `song:great-are-you-lord`, `note:Journal/2026.md`")
            .report()
    })
}

/// Slugify a title into a folder-safe token (mirrors the `song` crate's
/// internal slug rule for the on-disk arrangement dirs).
fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let cleaned = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if cleaned.is_empty() {
        "song".to_string()
    } else {
        cleaned
    }
}

// ── shared client / resolution ────────────────────────────────────────

async fn client_for(
    org: Option<String>,
    server: Option<String>,
) -> eyre::Result<(CollectionServiceClient, String)> {
    let slug = crate::resolve_active_org(org)?;
    let client = crate::establish_client::<CollectionServiceClient>(server, &slug).await?;
    Ok((client, slug))
}

/// Resolve a `<collection>` argument (an id or a title) to a [`Collection`].
async fn resolve_collection(
    client: &CollectionServiceClient,
    org_slug: &str,
    target: &str,
) -> eyre::Result<Collection> {
    // Fast path: treat the argument as an id.
    if let Some(c) = client
        .get(target.to_owned())
        .await
        .map_err(|e| eyre::eyre!("get: {e:?}"))?
    {
        return Ok(c);
    }
    // Fall back to a title match within the org.
    let all = client
        .list(org_slug.to_owned(), None)
        .await
        .map_err(|e| eyre::eyre!("list: {e:?}"))?;
    all.into_iter()
        .find(|c| c.id == target || c.title == target)
        .ok_or_else(|| {
            errors::not_found("resolve collection", target)
                .cause("no id or title match in this org")
                .report()
        })
}

fn print_collection(c: &Collection) {
    println!("{} [{}]", c.title, c.kind.as_str());
    println!("  id:    {}", c.id);
    println!("  org:   {}", c.org);
    println!("  items: {}", c.items.len());
    for (i, item) in c.items.iter().enumerate() {
        println!("    {:>3}. {}", i + 1, item.node.to_token());
    }
}

fn emit_json<T: serde::Serialize>(v: &T) -> eyre::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| eyre::eyre!("json: {e}"))?
    );
    Ok(())
}

// ── run: collection ───────────────────────────────────────────────────

pub async fn run_collection(cmd: CollectionCmd) -> eyre::Result<()> {
    match cmd {
        CollectionCmd::Create {
            title,
            kind,
            org,
            server,
            json,
        } => {
            let (client, slug) = client_for(org, server).await?;
            let created = client
                .create(slug, title, parse_kind(&kind))
                .await
                .map_err(|e| eyre::eyre!("create: {e:?}"))?;
            if json {
                return emit_json(&created);
            }
            println!("created collection {}", created.id);
            print_collection(&created);
        }
        CollectionCmd::Add {
            collection,
            node,
            after,
            org,
            server,
            json,
        } => {
            let (client, slug) = client_for(org, server).await?;
            let coll = resolve_collection(&client, &slug, &collection).await?;
            let node = parse_node(&node)?;
            let after = after.as_deref().map(parse_node).transpose()?;
            let updated = client
                .add_item(Placement {
                    collection_id: coll.id,
                    node,
                    after,
                })
                .await
                .map_err(|e| eyre::eyre!("add_item: {e:?}"))?;
            if json {
                return emit_json(&updated);
            }
            print_collection(&updated);
        }
        CollectionCmd::Reorder {
            collection,
            node,
            after,
            org,
            server,
            json,
        } => {
            let (client, slug) = client_for(org, server).await?;
            let coll = resolve_collection(&client, &slug, &collection).await?;
            let node = parse_node(&node)?;
            let after = after.as_deref().map(parse_node).transpose()?;
            let updated = client
                .reorder(Placement {
                    collection_id: coll.id,
                    node,
                    after,
                })
                .await
                .map_err(|e| eyre::eyre!("reorder: {e:?}"))?;
            if json {
                return emit_json(&updated);
            }
            print_collection(&updated);
        }
        CollectionCmd::Remove {
            collection,
            node,
            org,
            server,
            json,
        } => {
            let (client, slug) = client_for(org, server).await?;
            let coll = resolve_collection(&client, &slug, &collection).await?;
            let node = parse_node(&node)?;
            let updated = client
                .remove_item(coll.id, node)
                .await
                .map_err(|e| eyre::eyre!("remove_item: {e:?}"))?;
            if json {
                return emit_json(&updated);
            }
            print_collection(&updated);
        }
        CollectionCmd::List {
            kind,
            org,
            server,
            json,
        } => {
            let (client, slug) = client_for(org, server).await?;
            let filter = kind.as_deref().map(parse_kind);
            let rows = client
                .list(slug, filter)
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            if json {
                return emit_json(&rows);
            }
            println!("{} collections", rows.len());
            for c in &rows {
                println!(
                    "  {} [{}]  {} items  ({})",
                    c.title,
                    c.kind.as_str(),
                    c.items.len(),
                    c.id
                );
            }
        }
        CollectionCmd::Show {
            collection,
            org,
            server,
            json,
        } => {
            let (client, slug) = client_for(org, server).await?;
            let coll = resolve_collection(&client, &slug, &collection).await?;
            if json {
                return emit_json(&coll);
            }
            print_collection(&coll);
        }
    }
    Ok(())
}

// ── run: song ─────────────────────────────────────────────────────────

pub async fn run_song(cmd: SongCmd) -> eyre::Result<()> {
    match cmd {
        SongCmd::Add {
            collection,
            title,
            key,
            chart,
            pdf,
            audio,
            arrangement,
            org,
            server,
            json,
        } => {
            // Resolve the org once: its slug names the collection service's
            // scope, and its on-disk root is where the Song folder is
            // written (`<org>/resources/songs/<slug>`).
            let active = crate::org_ctx::resolve_active(org.as_deref())?;
            let org_slug = active.root.slug().to_string();
            let song_slug = slug(&title);
            let song_root = active
                .root
                .resources_dir()
                .join("songs")
                .join(&song_slug);

            let key: Key = match key.as_deref() {
                Some(k) => k
                    .parse()
                    .map_err(|e| errors::usage("parse key").cause(format!("{e}")).report())?,
                None => Key::default(),
            };
            let arr_name = arrangement.unwrap_or_else(|| "Default".to_string());
            let arr_slug = slug(&arr_name);
            let arr_id = Uuid::new_v4();

            // Chart lands inside the default arrangement folder; pdfs +
            // audio land under `attachments/`. Refs are relative paths.
            let chart_ref = chart.as_ref().map(|src| {
                let fname = file_name(src);
                ChartRef::from_path(format!("arrangements/{arr_slug}/{fname}"))
            });

            let mut attachment_refs = Vec::new();
            for src in &pdf {
                attachment_refs.push(attachment_ref(src, "pdf"));
            }
            for src in &audio {
                attachment_refs.push(attachment_ref(src, "audio"));
            }

            let arrangement = Arrangement {
                id: arr_id,
                name: arr_name,
                key,
                chart_ref: chart_ref.clone(),
                parts: PartsManifest::default(),
                attachment_refs: attachment_refs.clone(),
            };
            let song = Song {
                id: Uuid::new_v4(),
                title: title.clone(),
                tags: Vec::new(),
                default_arrangement: arr_id,
                arrangements: vec![arrangement],
            };

            // Write the folder skeleton (song.md + arrangement.md + dirs)…
            song::to_folder(&song, &song_root)
                .map_err(|e| eyre::eyre!("write song folder {}: {e}", song_root.display()))?;

            // …then copy the referenced bytes into place.
            if let (Some(src), Some(cref)) = (chart.as_ref(), chart_ref.as_ref()) {
                if let Some(rel) = &cref.path {
                    copy_into(src, &song_root.join(rel))?;
                }
            }
            for (src, aref) in pdf.iter().chain(audio.iter()).zip(&attachment_refs) {
                if let Some(rel) = &aref.path {
                    copy_into(src, &song_root.join(rel))?;
                }
            }

            // Register the song in the target collection.
            let client =
                crate::establish_client::<CollectionServiceClient>(server, &org_slug).await?;
            let coll = resolve_collection(&client, &org_slug, &collection).await?;
            let node = NodeRef::new(NodeKind::Song, song_slug.clone());
            let updated = client
                .add_item(Placement {
                    collection_id: coll.id,
                    node,
                    after: None,
                })
                .await
                .map_err(|e| eyre::eyre!("add_item: {e:?}"))?;

            if json {
                return emit_json(&updated);
            }
            println!(
                "wrote song `{title}` → {}",
                song_root.display()
            );
            println!("added song:{song_slug} to `{}`", updated.title);
            print_collection(&updated);
        }
        SongCmd::Ingest {
            note,
            stems,
            artist,
            key,
            bpm,
            time_signature,
            duration_sec,
            bitrate,
            dry_run,
            org,
            server,
            json,
        } => {
            ingest_song_stems(IngestArgs {
                note,
                stems,
                artist,
                key,
                bpm,
                time_signature,
                duration_sec,
                bitrate,
                dry_run,
                org,
                server,
                json,
            })
            .await?;
        }
    }
    Ok(())
}

// ── song ingest (stems → attachments + frontmatter) ───────────────────

struct IngestArgs {
    note: String,
    stems: PathBuf,
    artist: Option<String>,
    key: Option<String>,
    bpm: Option<f64>,
    time_signature: Option<String>,
    duration_sec: Option<f64>,
    bitrate: String,
    dry_run: bool,
    org: Option<String>,
    server: Option<String>,
    json: bool,
}

/// One stem discovered in the `--stems` dir.
struct IngestStem {
    /// Display name (filename minus extension and any `NN - ` index).
    name: String,
    /// Instrument group (keyword heuristic — same table the demo
    /// export / app seeding uses).
    group: Option<&'static str>,
    /// Click/cue/count/guide stems start muted.
    default_muted: bool,
    path: PathBuf,
    /// Already Opus/ogg — upload as-is, no transcode.
    passthrough: bool,
}

async fn ingest_song_stems(args: IngestArgs) -> eyre::Result<()> {
    use attachments_proto::{AttachmentServiceClient, CompleteUpload, InitiateUpload};

    let active = crate::org_ctx::resolve_active(args.org.as_deref())?;
    let org_slug = active.root.slug().to_string();
    let note_rel = args.note.trim_start_matches('/').to_string();
    if !note_rel.ends_with(".md") {
        return Err(errors::usage("song ingest")
            .cause(format!("`{note_rel}` is not a .md note path"))
            .hint("pass the vault-relative note path, e.g. Songs/Praise.md")
            .report());
    }
    let note_path = active.root.vault_dir().join(&note_rel);

    let found = discover_stems(&args.stems)?;
    if found.is_empty() {
        return Err(errors::usage("song ingest")
            .cause(format!("no audio files in {}", args.stems.display()))
            .report());
    }

    if args.dry_run {
        println!("would ingest {} stems into `{note_rel}`:", found.len());
        for s in &found {
            println!(
                "  {:<24} group={:<10} muted={} {}{}",
                s.name,
                s.group.unwrap_or("-"),
                s.default_muted,
                s.path.display(),
                if s.passthrough { "" } else { "  (→ opus)" },
            );
        }
        return Ok(());
    }

    // Transcode into a temp dir (Opus ~96k streams + seeks well and
    // keeps 20-36-stem songs small); passthrough for already-Opus.
    let tmp = tempfile::tempdir().map_err(|e| eyre::eyre!("tempdir: {e}"))?;
    let mut uploads: Vec<(usize, PathBuf)> = Vec::with_capacity(found.len());
    for (i, s) in found.iter().enumerate() {
        if s.passthrough {
            uploads.push((i, s.path.clone()));
            continue;
        }
        let dst = tmp.path().join(format!("{i:02}.webm"));
        transcode_opus(&s.path, &dst, &args.bitrate)?;
        uploads.push((i, dst));
    }

    // Duration: explicit flag, else ffprobe the first stem.
    let duration_sec = match args.duration_sec {
        Some(d) => Some(d),
        None => probe_duration(&uploads[0].1),
    };

    // Upload each transcoded stem: initiate → PUT → complete.
    let client =
        crate::establish_client::<AttachmentServiceClient>(args.server.clone(), &org_slug).await?;
    let http = reqwest::Client::new();
    let http_base = crate::resolve_server_http_base(args.server.as_deref());
    let mut hashes: Vec<String> = Vec::with_capacity(uploads.len());
    for (i, file) in &uploads {
        let stem = &found[*i];
        let bytes = std::fs::read(file).map_err(|e| eyre::eyre!("read {}: {e}", file.display()))?;
        let filename = format!("{}.webm", slug(&stem.name));
        let ticket = client
            .initiate_upload(InitiateUpload {
                doc_id: note_rel.clone(),
                filename,
                mime_type: "audio/webm".to_string(),
                size_bytes: bytes.len() as u64,
            })
            .await
            .map_err(|e| eyre::eyre!("initiate_upload `{}`: {e:?}", stem.name))?;
        let put_url = absolutize_blob_url(&ticket.upload_url, &http_base);
        let resp = http
            .put(&put_url)
            .body(bytes)
            .send()
            .await
            .map_err(|e| eyre::eyre!("PUT {put_url}: {e}"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(eyre::eyre!("PUT `{}`: HTTP {status}: {body}", stem.name));
        }
        let content_hash = body.trim().to_string();
        client
            .complete_upload(CompleteUpload {
                upload_id: ticket.upload_id,
                content_hash: content_hash.clone(),
            })
            .await
            .map_err(|e| eyre::eyre!("complete_upload `{}`: {e:?}", stem.name))?;
        println!("uploaded {:<24} {}", stem.name, &content_hash[..16.min(content_hash.len())]);
        hashes.push(content_hash);
    }

    // Write the note frontmatter (scalars only when provided; the
    // stems block always replaced wholesale).
    let mut scalars: Vec<(&str, String)> = vec![("type", "song".into())];
    if let Some(v) = &args.artist {
        scalars.push(("artist", v.clone()));
    }
    if let Some(v) = &args.key {
        scalars.push(("key", v.clone()));
    }
    if let Some(v) = args.bpm {
        scalars.push(("bpm", format!("{v}")));
    }
    if let Some(v) = &args.time_signature {
        scalars.push(("time_signature", format!("\"{v}\"")));
    }
    if let Some(v) = duration_sec {
        scalars.push(("duration_sec", format!("{v:.3}")));
    }
    let mut stems_yaml = String::from("stems:\n");
    for (s, hash) in found.iter().zip(&hashes) {
        stems_yaml.push_str(&format!("  - name: \"{}\"\n", s.name));
        if let Some(g) = s.group {
            stems_yaml.push_str(&format!("    group: {g}\n"));
        }
        if s.default_muted {
            stems_yaml.push_str("    default_muted: true\n");
        }
        stems_yaml.push_str(&format!("    content_hash: {hash}\n"));
    }
    upsert_note_frontmatter(&note_path, &scalars, &stems_yaml)?;

    if args.json {
        return emit_json(&serde_json::json!({
            "note": note_rel,
            "stems": found
                .iter()
                .zip(&hashes)
                .map(|(s, h)| serde_json::json!({
                    "name": s.name,
                    "group": s.group,
                    "default_muted": s.default_muted,
                    "content_hash": h,
                }))
                .collect::<Vec<_>>(),
            "duration_sec": duration_sec,
        }));
    }
    println!(
        "wrote {} stems into `{}` frontmatter",
        hashes.len(),
        note_path.display()
    );
    Ok(())
}

/// Scan a directory for stem audio files, sorted by filename.
fn discover_stems(dir: &Path) -> eyre::Result<Vec<IngestStem>> {
    const AUDIO_EXT: &[&str] = &["wav", "aif", "aiff", "flac", "mp3", "m4a", "ogg", "opus"];
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| eyre::eyre!("read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| AUDIO_EXT.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    files.sort();
    Ok(files
        .into_iter()
        .map(|path| {
            let base = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Strip a leading track index: `NN - `, `NN-`, `NN.`, `NN_`.
            let stripped = base
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start_matches([' ', '-', '.', '_'])
                .trim();
            let name = if stripped.is_empty() || stripped.len() == base.len() {
                base.clone()
            } else {
                stripped.to_string()
            };
            let group = stem_group_for(&name);
            let default_muted = {
                let hay = format!("{} {}", group.unwrap_or(""), name).to_ascii_lowercase();
                ["click", "guide", "cue", "count"].iter().any(|k| hay.contains(k))
            };
            // Only webm passes through untranscoded — the browser player's
            // vox-MSE path speaks audio/webm; ogg/opus fall back to HTTP.
            let passthrough = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("webm"));
            IngestStem {
                name,
                group,
                default_muted,
                path,
                passthrough,
            }
        })
        .collect())
}

/// Instrument-group keyword heuristic — the same table the demo export
/// and the app's session-engine seeding use.
fn stem_group_for(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    let any = |ks: &[&str]| ks.iter().any(|k| n.contains(k));
    if any(&["click", "cue", "guide"]) {
        Some("Guide")
    } else if any(&["loop"]) {
        Some("Tracks")
    } else if any(&["bass"]) {
        Some("Bass")
    } else if any(&["drum", "perc"]) {
        Some("Drums")
    } else if any(&["organ", "key", "piano", "rhodes", "synth", "pad"]) {
        Some("Keys")
    } else if any(&["guitar", "gtr", "acoustic", "electric"]) {
        Some("Guitars")
    } else if any(&["vocal", "vox", "bgv", "choir"]) {
        Some("Vocals")
    } else if any(&["sax", "horn", "brass", "string", "orch"]) {
        Some("Orchestra")
    } else if any(&["fx"]) {
        Some("FX")
    } else {
        None
    }
}

fn ffmpeg_bin() -> String {
    std::env::var("FTS_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string())
}

fn transcode_opus(src: &Path, dst: &Path, bitrate: &str) -> eyre::Result<()> {
    let status = std::process::Command::new(ffmpeg_bin())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(["-c:a", "libopus", "-b:a", bitrate])
        .arg(dst)
        .status()
        .map_err(|e| {
            errors::usage("song ingest")
                .cause(format!("run ffmpeg: {e}"))
                .hint("install ffmpeg or set FTS_FFMPEG to its path")
                .report()
        })?;
    if !status.success() {
        return Err(eyre::eyre!("ffmpeg failed on {}", src.display()));
    }
    Ok(())
}

/// Duration of an audio file in seconds via ffprobe. Best-effort.
fn probe_duration(path: &Path) -> Option<f64> {
    let ffprobe = std::env::var("FTS_FFPROBE").unwrap_or_else(|_| "ffprobe".to_string());
    let out = std::process::Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// An upload/download URL from the server may be relative (no
/// `TASK_SERVER_PUBLIC_URL` configured); resolve it against the HTTP
/// base derived from the vox server URL.
fn absolutize_blob_url(url: &str, http_base: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("{http_base}{url}")
    }
}

/// Create or update the note's leading `---` frontmatter: set the given
/// scalar keys (replacing existing lines) and replace any existing
/// `stems:` block with `stems_yaml` (which must be a full `stems:`
/// block, trailing newline included).
fn upsert_note_frontmatter(
    note_path: &Path,
    scalars: &[(&str, String)],
    stems_yaml: &str,
) -> eyre::Result<()> {
    let existing = std::fs::read_to_string(note_path).unwrap_or_default();
    let (front, body) = match existing.strip_prefix("---") {
        Some(rest) => match rest.split_once("\n---") {
            Some((f, b)) => (f.to_string(), b.trim_start_matches('\n').to_string()),
            None => (String::new(), existing.clone()),
        },
        None => (String::new(), existing.clone()),
    };

    // Drop lines we're about to re-set + any previous stems block.
    let mut kept: Vec<String> = Vec::new();
    let mut in_stems = false;
    for line in front.lines() {
        if line.trim().is_empty() && kept.is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if in_stems && indented {
            continue;
        }
        in_stems = false;
        if line.trim_end() == "stems:" {
            in_stems = true;
            continue;
        }
        let key = line.split_once(':').map(|(k, _)| k.trim());
        if key.is_some_and(|k| scalars.iter().any(|(sk, _)| *sk == k)) {
            continue;
        }
        kept.push(line.to_string());
    }

    let mut front_out = String::new();
    for (k, v) in scalars {
        front_out.push_str(&format!("{k}: {v}\n"));
    }
    for line in kept {
        front_out.push_str(&line);
        front_out.push('\n');
    }
    front_out.push_str(stems_yaml);

    let body = if body.is_empty() {
        let title = note_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("\n# {title}\n")
    } else {
        format!("\n{body}")
    };
    if let Some(parent) = note_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| eyre::eyre!("mkdir {}: {e}", parent.display()))?;
    }
    std::fs::write(note_path, format!("---\n{front_out}---\n{body}"))
        .map_err(|e| eyre::eyre!("write {}: {e}", note_path.display()))?;
    Ok(())
}

// ── song file helpers ─────────────────────────────────────────────────

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".to_string())
}

fn attachment_ref(src: &Path, kind: &str) -> AttachmentRef {
    let fname = file_name(src);
    AttachmentRef {
        id: Uuid::new_v4().simple().to_string(),
        path: Some(format!("attachments/{fname}")),
        sha256: None,
        kind: Some(kind.to_string()),
    }
}

/// Copy `src` to `dest`, creating parent dirs. Errors if `src` is missing.
fn copy_into(src: &Path, dest: &Path) -> eyre::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("mkdir {}", parent.display()))?;
    }
    std::fs::copy(src, dest)
        .wrap_err_with(|| format!("copy {} → {}", src.display(), dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind_is_case_insensitive_with_an_other_escape() {
        assert!(matches!(parse_kind("library"), CollectionKind::Library));
        assert!(matches!(parse_kind("Setlist"), CollectionKind::Setlist));
        assert!(matches!(parse_kind("SHOW"), CollectionKind::Show));
        assert!(matches!(parse_kind("playlist"), CollectionKind::Playlist));
        // `other:` preserves the caller's casing for the free-text
        // label; a bare unknown word is lowercased.
        assert!(
            matches!(parse_kind("other:Rehearsal Pool"), CollectionKind::Other(s) if s == "Rehearsal Pool")
        );
        assert!(matches!(parse_kind("Archive"), CollectionKind::Other(s) if s == "archive"));
    }

    #[test]
    fn slug_folds_to_a_folder_safe_token() {
        assert_eq!(slug("Great Are You Lord"), "great-are-you-lord");
        assert_eq!(slug("  Trailing / Slashes  "), "trailing-slashes");
        assert_eq!(slug("Don't Stop"), "don-t-stop");
        assert_eq!(slug("A---B"), "a-b");
        // Never yields an empty path segment.
        assert_eq!(slug(""), "song");
        assert_eq!(slug("!!!"), "song");
    }

    /// `slug` here is deliberately ASCII-only, unlike
    /// `vault_entity::slugify`, which keeps non-ASCII alphanumerics
    /// (`"Café" → "café"`). The two disagree only outside ASCII. This
    /// pins the divergence rather than blessing it: these slugs are
    /// baked into on-disk media paths, so unifying them is a migration,
    /// not a refactor. See issue #68 (Phase 1 slugify consolidation).
    #[test]
    fn slug_is_ascii_only_and_diverges_from_vault_entity() {
        assert_eq!(slug("Café Sessions"), "caf-sessions");
        assert_eq!(vault_entity::slugify("Café Sessions", "song"), "café-sessions");
        // Inside ASCII the two agree, which is why every existing path
        // is unaffected.
        for name in ["Great Are You Lord", "Don't Stop", "A---B"] {
            assert_eq!(slug(name), vault_entity::slugify(name, "song"), "{name}");
        }
    }

    #[test]
    fn stem_group_for_matches_the_first_rule_not_the_best_one() {
        assert_eq!(stem_group_for("01 Click.wav"), Some("Guide"));
        assert_eq!(stem_group_for("Guide Vox.wav"), Some("Guide"));
        assert_eq!(stem_group_for("Kick In.wav"), None);
        assert_eq!(stem_group_for("Bass DI.wav"), Some("Bass"));
        assert_eq!(stem_group_for("Drum OH L.wav"), Some("Drums"));
        assert_eq!(stem_group_for("Percussion.wav"), Some("Drums"));
        assert_eq!(stem_group_for("Rhodes.wav"), Some("Keys"));
        assert_eq!(stem_group_for("Acoustic Gtr.wav"), Some("Guitars"));
        assert_eq!(stem_group_for("BGV 1.wav"), Some("Vocals"));
        assert_eq!(stem_group_for("Strings.wav"), Some("Orchestra"));
        assert_eq!(stem_group_for("FX Riser.wav"), Some("FX"));
        assert_eq!(stem_group_for("Untitled 3.wav"), None);

        // Rules are checked in order, so a name matching two rules
        // lands in the earlier one. `loop` beats `bass`, and `guide`
        // beats `vocal` — both are intentional (a bass loop is a
        // backing track, a guide vocal is a guide), but they are
        // order-dependent and would silently change if the chain were
        // reordered.
        assert_eq!(stem_group_for("Bass Loop.wav"), Some("Tracks"));
        assert_eq!(stem_group_for("Guide Vocal.wav"), Some("Guide"));
    }

    #[test]
    fn absolutize_blob_url_only_prefixes_relative_urls() {
        assert_eq!(
            absolutize_blob_url("/blob/abc123", "http://localhost:8080"),
            "http://localhost:8080/blob/abc123"
        );
        // An already-absolute URL must survive untouched, or the blob
        // becomes unreachable under a double prefix.
        assert_eq!(
            absolutize_blob_url("https://cdn.example/blob/abc", "http://localhost:8080"),
            "https://cdn.example/blob/abc"
        );
        assert_eq!(
            absolutize_blob_url("http://other/blob/abc", "http://localhost:8080"),
            "http://other/blob/abc"
        );
    }
}

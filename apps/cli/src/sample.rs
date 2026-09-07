//! `task sample …` — the Signal sample lane of the Resource Library
//! from the shell.
//!
//! **This command never moves audio.** A sample's manifest lives at
//! `<org>/resources/samples/<slug>/sample.md` and says what the sample
//! *is*; the audio lives in a File Root, named by `--content-root` and
//! `--content-path`, because the Files layer has the versioning,
//! selective sync, Peaks renditions and chunked streaming that a
//! sample library needs and `resources/` has none of. Put the bytes
//! there with `task files`, then bind them here.
//!
//! A sample *library* is then an ordinary `Collection` of kind
//! `Library` over `sample:<slug>` references (ADR 0003) — which is why
//! subscribing to one costs manifests rather than gigabytes.

use clap::Subcommand;
use resources_proto::{ResourcesServiceClient, SampleDoc};

use crate::asset::{client, content_cell, content_ref, read_body};

#[derive(Subcommand, Debug)]
pub enum SampleCmd {
    /// Create or update a sample's manifest. The slug is the identity.
    Save {
        /// Sample slug (`room-kick-48k`) — or the metadata file, whose
        /// stem becomes the slug when `--from` is absent.
        slug_or_file: String,
        /// Sample title. Required: it is what a library lists.
        #[arg(long)]
        title: String,
        /// Free tag, repeatable (`--tag kick --tag room`).
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Length in seconds — what a `sample:<slug>#t:0-2:400` region
        /// anchor is measured against.
        #[arg(long)]
        duration_secs: Option<u64>,
        /// Sample rate in Hz (`48000`).
        #[arg(long)]
        sample_rate: Option<u32>,
        /// File Root holding the audio. The bytes are not uploaded by
        /// this command — `task files` puts them there and this records
        /// where they went.
        #[arg(long)]
        content_root: Option<String>,
        /// Root-relative path of the audio
        /// (`Samples/Kicks/Room Kick 48k.wav`).
        #[arg(long)]
        content_path: Option<String>,
        /// Read the metadata notes from this file, or `-` for stdin.
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print a sample's metadata notes (or the whole document with
    /// `--json`, which is where the File Root binding shows).
    Get {
        slug: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every sample the org holds.
    List {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Un-declare a sample: its manifest directory goes. The audio in
    /// its File Root is not touched — deleting a declaration is not the
    /// same act as destroying content this lane never held.
    Rm {
        slug: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

pub async fn run_sample(cmd: SampleCmd, global_org: Option<&str>) -> eyre::Result<()> {
    let org_of = |org: Option<String>| org.or_else(|| global_org.map(str::to_owned));
    match cmd {
        SampleCmd::Save {
            slug_or_file,
            title,
            tags,
            duration_secs,
            sample_rate,
            content_root,
            content_path,
            from,
            org,
            server,
            json,
        } => {
            let (slug, body) = read_body(&slug_or_file, from.as_deref())?;
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let out = client
                .upsert_sample(SampleDoc {
                    slug,
                    title,
                    tags,
                    duration_secs: duration_secs.unwrap_or_default(),
                    sample_rate: sample_rate.unwrap_or_default(),
                    body,
                    content: content_ref(content_root, content_path),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                })
                .await
                .map_err(|e| eyre::eyre!("upsert_sample: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let verb = if out.created { "created" } else { "updated" };
                println!("{verb} sample:{} → {}", out.slug, out.rel_path);
            }
            Ok(())
        }
        SampleCmd::Get {
            slug,
            org,
            server,
            json,
        } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let doc = client
                .sample(slug.clone())
                .await
                .map_err(|e| eyre::eyre!("sample `{slug}`: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else {
                print!("{}", doc.body);
            }
            Ok(())
        }
        SampleCmd::List { org, server, json } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let list = client
                .list_samples()
                .await
                .map_err(|e| eyre::eyre!("list_samples: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            if list.is_empty() {
                println!("no samples yet");
                return Ok(());
            }
            for s in &list {
                println!(
                    "{:<32} {:>5}s {:>7}Hz {:<20} {:<36} {}",
                    s.slug,
                    s.duration_secs,
                    s.sample_rate,
                    s.tags.join(","),
                    content_cell(&s.content),
                    s.title
                );
            }
            Ok(())
        }
        SampleCmd::Rm { slug, org, server } => {
            let client: ResourcesServiceClient = client(org_of(org), server).await?;
            let gone = client
                .delete_sample(slug.clone())
                .await
                .map_err(|e| eyre::eyre!("delete_sample `{slug}`: {e:?}"))?;
            if gone {
                println!("deleted sample:{slug} (its audio is untouched)");
            } else {
                println!("no sample `{slug}`");
            }
            Ok(())
        }
    }
}

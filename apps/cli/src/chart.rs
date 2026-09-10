//! `task chart …` — the chart lane of the Resource Library from the
//! shell.
//!
//! A chart is Keyflow's document, and Task is where it lives between
//! sessions: since ADR 0004 that is one **vault document** on the
//! Assets shelf, `<vault>/Assets/Charts/<slug>.md`, holding the source
//! verbatim in a ` ```keyflow ` fence and whatever prose somebody wrote
//! around it. Being a vault file is what gives a chart collaborative
//! editing, wikilinks, tags and search — none of which was built for
//! charts.
//!
//! These verbs are the same four RPCs Keyflow itself calls — nothing
//! here is a private lane (ADR 0003).
//!
//! `save` takes the source on `--from <file>` or stdin (`--from -`), so
//! a chart round-trips through a pipe:
//!
//! ```text
//! task chart get hosanna | task chart save hosanna --title Hosanna --from -
//! ```
//!
//! **One chart is one arrangement.** `--song` says which song a chart
//! arranges, `--arrangement` says which reading of it this is, and
//! `--default` makes it the one somebody gets when they ask for "the
//! chart" of that song. `task chart list --song doxology` is then the
//! song and its arrangements in one call, with the main one starred:
//!
//! ```text
//! task chart save doxology.kf --title Doxology --song doxology --arrangement original --default
//! task chart save live.kf --title Doxology --song doxology --arrangement 'condensed live'
//! task chart list --song doxology
//! ```
//!
//! Lives in its own module (like `plan` / `collection`) so concurrent
//! agents editing `main.rs` only collide on the two-line dispatch arm.

use std::io::Read as _;
use std::path::PathBuf;

use clap::Subcommand;
use resources_proto::{ChartDoc, ResourcesServiceClient};

use crate::{establish_for_url, resolve_active_org, resolve_org_vox_url};

#[derive(Subcommand, Debug)]
pub enum ChartCmd {
    /// Create or update a chart. The slug is the identity: pass one to
    /// update, omit it (naming a source file instead) to create.
    Save {
        /// Chart slug (`hosanna`) — or the source file, whose stem
        /// becomes the slug when `--from` is absent.
        slug_or_file: String,
        /// Chart title. Required: it is what a library lists.
        #[arg(long)]
        title: String,
        /// Musical key as written (`A`, `Bb`, `f#m`).
        #[arg(long)]
        key: Option<String>,
        /// Notation dialect (`keyflow`, `chordpro`, `nashville`).
        #[arg(long, default_value = "keyflow")]
        notation: String,
        /// Section names in chart order, repeatable (`--section chorus`).
        /// The server does not parse the source — a `chart:<slug>#chorus`
        /// anchor is only addressable if the sections are declared here.
        #[arg(long = "section")]
        sections: Vec<String>,
        /// The song this chart arranges: `song:doxology`, a bare
        /// `doxology` (read as this org's own), or a qualified
        /// `guest.example/song:hosanna`. Leave it off for a chart
        /// nobody has attached to a song yet.
        #[arg(long)]
        song: Option<String>,
        /// Which arrangement of that song this is — `original`,
        /// `condensed live`. It also names the derived slug, so a
        /// song's second chart is `doxology-condensed-live` rather than
        /// `doxology-2`.
        #[arg(long)]
        arrangement: Option<String>,
        /// Make this the song's main chart, clearing the flag on its
        /// other arrangements. Leaving it off means *no opinion*: a
        /// chart that is already the default stays it, and a song's
        /// first chart becomes it regardless.
        #[arg(long = "default")]
        is_default: bool,
        /// Read the source from this file, or `-` for stdin. Without
        /// it, `slug_or_file` is read as the file.
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print a chart's source (or the whole document with `--json`).
    Get {
        slug: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every chart the org holds — or, with `--song`, one song's
    /// arrangements, marked with which is the main one.
    List {
        /// Only this song's charts: `song:doxology`, a bare
        /// `doxology`, or a qualified `guest.example/song:hosanna`.
        #[arg(long)]
        song: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Delete a chart's vault document. References to
    /// it from collections are left alone; a dangling reference reads
    /// as unresolved, which is a legible state.
    Rm {
        slug: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
}

pub async fn run_chart(cmd: ChartCmd, global_org: Option<&str>) -> eyre::Result<()> {
    let org_of = |org: Option<String>| org.or_else(|| global_org.map(str::to_owned));
    match cmd {
        ChartCmd::Save {
            slug_or_file,
            title,
            key,
            notation,
            sections,
            song,
            arrangement,
            is_default,
            from,
            org,
            server,
            json,
        } => {
            let (slug, source) = read_source(&slug_or_file, from.as_deref())?;
            let client = client(org_of(org), server).await?;
            let out = client
                .upsert_chart(ChartDoc {
                    slug,
                    title,
                    source,
                    key: key.unwrap_or_default(),
                    notation,
                    sections,
                    song: song.unwrap_or_default(),
                    arrangement: arrangement.unwrap_or_default(),
                    is_default,
                    updated_at: chrono::Utc::now().to_rfc3339(),
                })
                .await
                .map_err(|e| eyre::eyre!("upsert_chart: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let verb = if out.created { "created" } else { "updated" };
                println!("{verb} chart:{} → {}", out.slug, out.rel_path);
            }
            Ok(())
        }
        ChartCmd::Get {
            slug,
            org,
            server,
            json,
        } => {
            let client = client(org_of(org), server).await?;
            let doc = client
                .chart(slug.clone())
                .await
                .map_err(|e| eyre::eyre!("chart `{slug}`: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else {
                print!("{}", doc.source);
            }
            Ok(())
        }
        ChartCmd::List {
            song,
            org,
            server,
            json,
        } => {
            let client = client(org_of(org), server).await?;
            let filter = song.unwrap_or_default();
            let list = client
                .list_charts(filter.clone())
                .await
                .map_err(|e| eyre::eyre!("list_charts: {e:?}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            if list.is_empty() {
                if filter.is_empty() {
                    println!("no charts yet");
                } else {
                    println!("no charts of `{filter}`");
                }
                return Ok(());
            }
            for c in &list {
                // A leading `*` is the song's main chart, and the
                // arrangement label is what tells the rest apart.
                println!(
                    "{} {:<32} {:<4} {:<9} {:>2} sections  {}{}",
                    if c.is_default { "*" } else { " " },
                    c.slug,
                    c.key,
                    c.notation,
                    c.sections.len(),
                    c.title,
                    if c.arrangement.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", c.arrangement)
                    }
                );
            }
            Ok(())
        }
        ChartCmd::Rm { slug, org, server } => {
            let client = client(org_of(org), server).await?;
            let gone = client
                .delete_chart(slug.clone())
                .await
                .map_err(|e| eyre::eyre!("delete_chart `{slug}`: {e:?}"))?;
            if gone {
                println!("deleted chart:{slug}");
            } else {
                println!("no chart `{slug}`");
            }
            Ok(())
        }
    }
}

/// `(slug, source)` for a save: `--from` names the source and the
/// positional argument is the slug; without it the positional argument
/// is the source file and its stem is the slug.
fn read_source(slug_or_file: &str, from: Option<&str>) -> eyre::Result<(String, String)> {
    match from {
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            Ok((slug_or_file.to_string(), buf))
        }
        Some(path) => Ok((slug_or_file.to_string(), read_file(path.into())?)),
        None => {
            let path = PathBuf::from(slug_or_file);
            let slug = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((slug, read_file(path)?))
        }
    }
}

fn read_file(path: PathBuf) -> eyre::Result<String> {
    std::fs::read_to_string(&path)
        .map_err(|e| eyre::eyre!("reading chart source `{}`: {e}", path.display()))
}

async fn client(
    org: Option<String>,
    server: Option<String>,
) -> eyre::Result<ResourcesServiceClient> {
    let slug = resolve_active_org(org)?;
    establish_for_url(&resolve_org_vox_url(server, &slug)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_file_names_its_own_slug() {
        let dir = tempfile::tempdir().unwrap();
        let kf = dir.path().join("hosanna.kf");
        std::fs::write(&kf, "| A |\n").unwrap();
        let (slug, source) = read_source(&kf.to_string_lossy(), None).unwrap();
        assert_eq!(slug, "hosanna");
        assert_eq!(source, "| A |\n");

        // With `--from`, the positional argument is the slug.
        let (slug, source) = read_source("other-name", Some(&kf.to_string_lossy())).unwrap();
        assert_eq!(slug, "other-name");
        assert_eq!(source, "| A |\n");
    }
}

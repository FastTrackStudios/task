//! `task-server admin bible …` — putting a corpus in an org's resource
//! library.
//!
//! Corpora do not live in the git repo (`OrgRoot::resources_dir` says
//! so, and a 66-book canon is not a thing to commit), so there has to
//! be a first-class way to install one. Before this, the only route was
//! `cargo run -p scripture --example install_bible` with a USFM export
//! you had already found yourself — which meant a fresh demo had a
//! scripture reader and nothing to read.
//!
//! Only public-domain editions can be pulled. That is not a policy
//! implemented here: `scripture::source_for` refuses a licensed edition
//! outright, because a licensed one has no corpus to install — it is
//! read per passage under the reader's own key
//! (`wiki.resource.rights`).

use eyre::bail;

use crate::admin_cli::flag;

/// `admin bible <install|list> …`.
///
/// # Errors
///
/// An unknown subcommand, an org that is not on this data root, a
/// licensed or unknown edition, or any failure downloading or
/// installing the corpus.
pub async fn bible(args: &[String]) -> eyre::Result<()> {
    match args.first().map(String::as_str) {
        Some("list") => {
            list();
            Ok(())
        }
        Some("install") => install(&args[1..]).await,
        other => {
            eprintln!(
                "usage:\n  \
                 task-server admin bible list\n  \
                 task-server admin bible install --org <slug> [--translation WEB] \\\n    \
                 [--from <usfm-dir-or-zip>]\n    \
                 (downloads from the recorded public-domain source when --from is omitted;\n     \
                 archives are cached under $TASK_BIBLE_CACHE so a re-plant does not refetch)\n"
            );
            bail!("unknown bible subcommand: {}", other.unwrap_or("(none)"));
        }
    }
}

fn list() {
    println!("editions that can be installed whole:");
    for s in scripture::installable() {
        let tx = scripture::Translation::lookup(s.id);
        let name = tx.map_or(s.id, |t| t.name);
        let license = tx.map_or("", |t| t.license);
        let cached = if scripture::pull::cached(s).is_some() {
            " (cached)"
        } else {
            ""
        };
        println!("  {:<4} {name} — {license}{cached}", s.id);
    }
    println!(
        "\nlicensed editions (NIV, ESV, …) are read per passage through their own API \
         with your key, and are never installed."
    );
}

async fn install(args: &[String]) -> eyre::Result<()> {
    let Some(slug) = flag(args, "--org") else {
        bail!("--org is required");
    };
    let translation = flag(args, "--translation").unwrap_or_else(|| "WEB".to_owned());

    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let org = data_root.org(&slug);
    if !org.path().is_dir() {
        bail!(
            "no org `{slug}` on this data root ({}). \
             `task-server admin demo --org {slug}` plants one.",
            data_root.path().display()
        );
    }
    let dest = org.bible_dir(&translation);

    let pulled = match flag(args, "--from") {
        Some(from) => {
            let path = std::path::PathBuf::from(&from);
            if path.is_dir() {
                // A directory of USFM installs directly; no archive,
                // nothing to cache.
                let books = scripture::install_usfm_dir(&path, &dest)
                    .map_err(|e| eyre::eyre!("install from {from}: {e}"))?;
                println!(
                    "installed {} books of {translation} into {} (from {from})",
                    books.len(),
                    dest.display()
                );
                return Ok(());
            }
            scripture::pull_from_archive(&translation, &path, &dest)
                .map_err(|e| eyre::eyre!("install from {from}: {e}"))?
        }
        None => scripture::pull(&translation, &dest)
            .await
            .map_err(|e| eyre::eyre!("pull {translation}: {e}"))?,
    };

    println!(
        "installed {} books of {} into {} ({})",
        pulled.books.len(),
        pulled.id,
        dest.display(),
        if pulled.from_cache {
            "from cache"
        } else {
            "downloaded"
        }
    );
    Ok(())
}

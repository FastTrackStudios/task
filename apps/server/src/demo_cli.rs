//! `admin demo` — the example studio, as servers you can sign into.
//!
//! [`crate::example_org`] holds the tree and the cast; this plants them
//! and hands back the endpoint ids. What comes out is the world the
//! integration suite asserts against, on disk, reachable, with known
//! passwords.
//!
//! # One org per data root, because one server hosts one org here
//!
//! `admin seed` puts several orgs in one data root, which is the right
//! shape for eyeballing multi-org UI on one process. This is the other
//! arrangement and the one the product is actually about: **two
//! companies on two machines**, neither a member of the other, sharing
//! one project across a boundary. Federation between two orgs in one
//! process is federation with the interesting part removed — the two
//! failures this repo has actually had were both "it works because both
//! sides share a process".
//!
//! So `--org <slug>` plants exactly one org, and the demo script runs
//! this twice against two data roots and starts two servers.
//!
//! # What it does not do
//!
//! Adopt anything, offer anything, or accept anything. Those are calls a
//! signed-in person makes over the wire, and making them here would bake
//! the interesting half of the demo into a seeder — the same mistake as
//! a scenario that asserts against a world it built with the backend
//! rather than through the router. The projects are on disk; adopting
//! one is the first thing you do in the app.
//!
//! DEV ONLY, like `seed`: it plants known-password accounts, so it is
//! compiled out of release builds entirely.

use eyre::{Context, bail};

use crate::example_org::{self, Holds, PASSWORD};

/// `admin demo --org <slug>` — plant one example org on this data root.
///
/// # Errors
///
/// Refuses without an explicit `TASK_DATA_ROOT`, for the reason `seed`
/// refuses: this plants accounts whose passwords are in the source tree.
pub async fn demo(args: &[String]) -> eyre::Result<()> {
    match std::env::var("TASK_DATA_ROOT") {
        Ok(v) if !v.trim().is_empty() => {}
        _ => bail!(
            "refusing to plant a demo: set TASK_DATA_ROOT to a throwaway dir first \
             (this plants known-password accounts — never point it at real data). \
             `just demo` does this for you."
        ),
    }

    let slug = crate::admin_cli::flag(args, "--org").unwrap_or_else(|| "acme-audio".to_owned());
    let Some((_, display)) = example_org::ORGS.iter().find(|(s, _)| *s == slug) else {
        bail!(
            "`{slug}` is not in the example. It has: {}",
            example_org::ORGS
                .iter()
                .map(|(s, _)| *s)
                .collect::<Vec<_>>()
                .join(", ")
        );
    };

    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .ensure()
        .map_err(|e| eyre::eyre!("ensure data root: {e}"))?;
    println!("planting {slug} into {}", data_root.path().display());

    // The org is home on its own server. It is the only one here, and
    // "home" is what makes it the identity authority for its own
    // accounts.
    match data_root.init_org(&slug, display, true) {
        Ok(_) => println!("  org: created on disk"),
        Err(e) => println!("  org: already present ({e})"),
    }
    let org = data_root.org(&slug);

    let planted = example_org::install(&org, &slug).wrap_err("plant the example tree")?;
    println!(
        "  tree: {} file(s) written, {} already there",
        planted.written, planted.kept
    );

    // Accounts, in this org's own auth store.
    let url = format!("sqlite://{}?mode=rwc", org.auth_db().display());
    let auth = crate::AuthState::open(&url, &crate::auth_secret())
        .await
        .wrap_err_with(|| format!("open auth store for `{slug}`"))?;
    for member in example_org::cast_of(&slug) {
        crate::admin_cli::seed_account(
            &auth,
            member.email,
            PASSWORD,
            member.name,
            None,
            // The owner administers their own company. Nobody else does
            // — Sam and Casey are the two who carry the access story,
            // and an admin role would make both of them meaningless.
            matches!(member.holds, Holds::Owner).then_some("admin"),
            match member.holds {
                Holds::Owner => "owner",
                Holds::Employee => "employee",
                Holds::Client => "client",
            },
        )
        .await?;
    }

    println!("\nplanted. sign in with any of:");
    for member in example_org::cast_of(&slug) {
        println!(
            "  {:<20} {PASSWORD}   ({:?}{})",
            member.email,
            member.holds,
            if member.scope.is_empty() {
                String::new()
            } else {
                format!(" of {}", member.scope)
            }
        );
    }
    println!("\nprojects on disk, waiting to be adopted:");
    let projects = org.path().join("files").join("Projects");
    if let Ok(entries) = std::fs::read_dir(&projects) {
        let mut names: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            println!("  {}", projects.join(name).display());
        }
    }
    Ok(())
}

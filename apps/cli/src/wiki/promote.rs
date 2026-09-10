//! `task wiki promote` — a vetted page moves up from a working wiki
//! into a curated one.
//!
//! The reasoning behind the verb — why it copies rather than moves, why
//! provenance is written on both ends, why a type the target does not
//! declare is a refusal rather than a guess — lives with the code that
//! makes those decisions, in [`wiki_proto::promote`]. Read that first;
//! this file is the plumbing around it.
//!
//! # Why this is not a server RPC
//!
//! Everything a promotion does is already on the wire. `Pages::read_page`
//! reads the source, `Schema::read_schema` reads the target's contract,
//! `Pages::list_pages` says what the target can resolve a link to, and
//! `Pages::write_page` — sha-guarded — writes both ends. A `promote`
//! RPC would be a fifth method whose only content is the order of those
//! four, and it would put the schema-translation policy on the server,
//! where every client would then be stuck with one version of it.
//!
//! Composing instead has one real cost and it is worth naming: a
//! promotion is two writes and there is no transaction across them. The
//! order is chosen so the failure is recoverable rather than confusing
//! — see [`run`].
//!
//! # Why the guard is the sha the caller already read
//!
//! `write_page`'s optimistic guard is a content hash, so a promotion
//! gets clobber-safety for free: the source is written back against the
//! sha it was read at, and the target — when `--force` overwrites one —
//! against the sha the existence check just returned. A page that
//! changed under a slow promotion is a conflict the caller is told
//! about, not a silent overwrite. Without `--force` an existing target
//! is refused before either write happens, so the common mistake
//! (promoting the same page twice, or onto a curated page someone
//! already wrote) costs nothing.

use std::collections::BTreeSet;

use wiki_proto::promote::{self, PromoteRequest};
use wiki_proto::service::pages::PagesClient;
use wiki_proto::service::schema::SchemaClient;

use crate::establish_for_url;
use crate::resolve_active_org;
use crate::resolve_org_vox_url;

/// Everything `task wiki promote` was invoked with.
pub(super) struct Args {
    pub from_wiki: String,
    pub path: String,
    pub to_wiki: String,
    pub as_path: Option<String>,
    pub as_type: Option<String>,
    pub base_sha256: String,
    pub dry_run: bool,
    pub force: bool,
    pub org: Option<String>,
    pub server: Option<String>,
}

/// Read both wikis, plan the promotion, and — unless this is a dry run
/// — write the two documents.
///
/// t[impl wiki.promote.no-clobber] — an occupied target is refused
/// before either write, and a forced overwrite still carries the guard
/// the existence check just read.
///
/// # Order of writes
///
/// The promoted page is written first and the source's back-reference
/// second. If the second write fails the world is: a curated page that
/// exists and records where it came from, and a research page that does
/// not yet know it was promoted. That is a missing annotation, and
/// re-running with `--force` repairs it.
///
/// The other order fails worse. A source that claims it was promoted to
/// a page that does not exist is a lie in the curated direction — the
/// direction where a reader is relying on the wiki to be trustworthy —
/// and nothing about it looks broken.
pub(super) async fn run(args: Args) -> eyre::Result<()> {
    let org = resolve_active_org(args.org)?;
    let url = resolve_org_vox_url(args.server, &org);
    let pages: PagesClient = establish_for_url(&url).await?;
    let schemas: SchemaClient = establish_for_url(&url).await?;

    let source = pages
        .read_page(args.from_wiki.clone(), args.path.clone())
        .await
        .map_err(|e| eyre::eyre!("read `{}::{}`: {e:?}", args.from_wiki, args.path))?;

    let schema = schemas
        .read_schema(args.to_wiki.clone())
        .await
        .map_err(|e| {
            eyre::eyre!(
                "read `{}`'s schema: {e:?} — a promotion is checked against the target's \
                 declared page types, so a wiki with no schema cannot be promoted into",
                args.to_wiki
            )
        })?;

    // Every spelling a bare `[[link]]` in the target could resolve to.
    // Both the frontmatter title and the file stem are legal targets,
    // so both count as "the target can resolve this".
    let target_names: BTreeSet<String> = pages
        .list_pages(args.to_wiki.clone())
        .await
        .map_err(|e| eyre::eyre!("list `{}`'s pages: {e:?}", args.to_wiki))?
        .into_iter()
        .flat_map(|p| {
            let stem = std::path::Path::new(&p.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned();
            [p.title, stem]
        })
        .filter(|n| !n.is_empty())
        .collect();

    let plan = promote::plan(
        &source.markdown,
        &PromoteRequest {
            from_wiki: &args.from_wiki,
            from_path: &args.path,
            to_wiki: &args.to_wiki,
            to_path: args.as_path.as_deref(),
            as_type: args.as_type.as_deref(),
            target_schema: &schema.markdown,
            target_names: &target_names,
            at: chrono::Utc::now(),
        },
    )
    .map_err(|e| eyre::eyre!("{e}"))?;

    // Does the target already hold this page? Asked before anything is
    // written, so a refusal leaves both wikis byte-identical.
    //
    // Only a genuine "no such page" counts as absent. Any other error —
    // a refusal, a wiki that does not exist, a backend that is unwell —
    // is reported rather than read as an empty slot: treating "I could
    // not look" as "nothing is there" is exactly how a clobber-safe
    // verb stops being clobber-safe.
    let existing = match pages
        .read_page(args.to_wiki.clone(), plan.to_path.clone())
        .await
    {
        Ok(doc) => Some(doc),
        Err(vox::VoxError::User(e)) if matches!(*e, wiki_proto::WikiError::NotFound(_)) => None,
        Err(e) => {
            return Err(eyre::eyre!(
                "could not check whether `{}::{}` is already taken: {e:?} — refusing rather \
                 than writing over a page this could not read",
                args.to_wiki,
                plan.to_path,
            ));
        }
    };
    //
    // A dry run is exempt: its job is to say what *would* happen, and
    // "it would be refused, here is the page you would be replacing" is
    // more use than the refusal on its own. It still writes nothing.
    if let Some(doc) = &existing
        && !args.force
        && !args.dry_run
    {
        return Err(eyre::eyre!(
            "`{}::{}` already exists (sha256 {}). A promotion never overwrites a curated \
             page it did not just write — that is the whole point of the verb. Re-run with \
             `--force` to replace it, `--as <other path>` to land beside it, or \
             `--force --base-sha256 {}` to replace it only if nobody has touched it since \
             you read this.",
            args.to_wiki,
            plan.to_path,
            doc.sha256,
            doc.sha256,
        ));
    }

    // The guard for the target write. An explicit `--base-sha256` is
    // the caller's own; otherwise a forced overwrite is guarded by what
    // the existence check just saw, so `--force` still refuses to
    // clobber a change that landed in between.
    let target_base = if !args.base_sha256.trim().is_empty() {
        args.base_sha256.trim().to_owned()
    } else {
        existing
            .as_ref()
            .map(|d| d.sha256.clone())
            .unwrap_or_default()
    };

    for link in &plan.requalified_links {
        eprintln!(
            "  link  [[{link}]] → [[{}::{link}]]  (not a page `{}` holds)",
            args.from_wiki, args.to_wiki
        );
    }

    if args.dry_run {
        if let Some(doc) = &existing
            && !args.force
        {
            eprintln!(
                "  REFUSED  `{}::{}` already exists (sha256 {}) — this promotion would not \
                 run without `--force` or a different `--as <path>`",
                args.to_wiki, plan.to_path, doc.sha256
            );
        }
        println!(
            "would write {}::{}  (type {}{})",
            args.to_wiki,
            plan.to_path,
            plan.to_type,
            if existing.is_some() {
                ", replacing"
            } else {
                ", new"
            }
        );
        println!("─── {}::{} ───", args.to_wiki, plan.to_path);
        print!("{}", plan.promoted_markdown);
        println!(
            "─── {}::{} (back-reference only) ───",
            args.from_wiki, args.path
        );
        print!("{}", plan.annotated_source);
        eprintln!("dry run — nothing was written");
        return Ok(());
    }

    let written = pages
        .write_page(
            args.to_wiki.clone(),
            plan.to_path.clone(),
            plan.promoted_markdown.clone(),
            target_base,
        )
        .await
        .map_err(|e| eyre::eyre!("write `{}::{}`: {e:?}", args.to_wiki, plan.to_path))?;
    println!(
        "{}::{}  sha256 {}",
        args.to_wiki, plan.to_path, written.sha256
    );

    pages
        .write_page(
            args.from_wiki.clone(),
            args.path.clone(),
            plan.annotated_source.clone(),
            source.sha256.clone(),
        )
        .await
        .map_err(|e| {
            eyre::eyre!(
                "the page was promoted to `{}::{}`, but writing the back-reference onto \
                 `{}::{}` failed: {e:?}. The research page does not yet record where its \
                 vetted form went; re-run with `--force` to repair it.",
                args.to_wiki,
                plan.to_path,
                args.from_wiki,
                args.path,
            )
        })?;
    println!(
        "{}::{}  promoted_to {}::{}",
        args.from_wiki, args.path, args.to_wiki, plan.to_path
    );
    Ok(())
}

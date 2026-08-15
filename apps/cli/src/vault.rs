//! `task vault …` — vault queries/edits + sync.
//!
//! The query/edit subcommands address a vault by positional path.
//! Routing (see [`run_vault`]): an EXISTING local directory — or
//! `task vault --fs …` — runs the original FS-native logic
//! directly on it; a missing path routes to the ACTIVE ORG's
//! vault over vox (`VaultSync`, `vault_id = "default"`), by
//! mirroring it into a scratch dir, running the very same logic,
//! and pushing any mutations back file-by-file. Same commands,
//! same output, local and remote.

use clap::Subcommand;
use std::collections::HashMap;

use crate::establish_for_url;

// ── Obsidian vault subcommands ───────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum VaultCmd {
    /// Open a vault and print a one-line summary.
    Open { path: std::path::PathBuf },
    /// List pages in a vault, optionally filtered by folder, tag, or
    /// `key=value` frontmatter match. Prints one vault-relative path
    /// per line so output is pipe-friendly.
    Pages {
        path: std::path::PathBuf,
        /// Restrict to pages whose folder equals (or starts with) this.
        #[arg(long)]
        folder: Option<String>,
        /// Restrict to pages tagged `<tag>` (frontmatter `tags` or
        /// inline `#tag`).
        #[arg(long)]
        tag: Option<String>,
        /// `key=value` frontmatter match. Value is parsed as JSON,
        /// falling back to a string. Repeatable; all must match.
        #[arg(long = "fm", value_name = "KEY=VAL")]
        fm: Vec<String>,
        /// Emit one JSON object per line instead of the path.
        #[arg(long)]
        json: bool,
    },
    /// List every tag in the vault with a count.
    Tags { path: std::path::PathBuf },
    /// List `.base` files and (if parsed cleanly) their view names.
    Bases { path: std::path::PathBuf },
    /// Print a single page's raw markdown.
    Cat {
        path: std::path::PathBuf,
        /// Vault-relative path of the page, e.g. `Music/Charts.md`.
        rel_path: String,
    },
    /// Substring search across page bodies (case-insensitive).
    /// Prints `path:line:content` like grep.
    Grep {
        path: std::path::PathBuf,
        pattern: String,
    },
    /// Pages that link TO the given page.
    Backlinks {
        path: std::path::PathBuf,
        rel_path: String,
    },
    /// Outgoing wikilinks from a page (resolved + raw target).
    Links {
        path: std::path::PathBuf,
        rel_path: String,
    },
    /// Pages with no incoming links.
    Orphans { path: std::path::PathBuf },
    /// Pages with no outgoing links.
    Deadends { path: std::path::PathBuf },
    /// Wikilink targets that don't resolve to any page.
    Unresolved { path: std::path::PathBuf },
    /// Heading outline of a single page.
    Outline {
        path: std::path::PathBuf,
        rel_path: String,
    },
    /// List distinct frontmatter property keys across the vault.
    Properties { path: std::path::PathBuf },
    /// Read one frontmatter property from a page.
    PropertyRead {
        path: std::path::PathBuf,
        rel_path: String,
        key: String,
    },
    /// Set a frontmatter property on a page (creates key if absent).
    PropertySet {
        path: std::path::PathBuf,
        rel_path: String,
        key: String,
        /// Value parsed as JSON; falls back to a string literal.
        value: String,
    },
    /// Remove a frontmatter property from a page (no-op if absent).
    PropertyRemove {
        path: std::path::PathBuf,
        rel_path: String,
        key: String,
    },
    /// All aliases declared via frontmatter, sorted.
    Aliases { path: std::path::PathBuf },
    /// All `- [ ]` task items across the vault. `path:line marker text`.
    Tasks { path: std::path::PathBuf },
    /// Word + character count for the vault (or one page when `--page` set).
    Wordcount {
        path: std::path::PathBuf,
        #[arg(long)]
        page: Option<String>,
    },
    /// Run all views in a `.base` file over the vault's pages.
    /// `--view <name>` runs only the matching view (case-insensitive).
    BaseQuery {
        path: std::path::PathBuf,
        /// Vault-relative path to the `.base` file.
        base: String,
        #[arg(long)]
        view: Option<String>,
    },
    /// Create a new `.md` page. Fails if the file already exists.
    Create {
        path: std::path::PathBuf,
        rel_path: String,
        /// Optional initial body. Reads stdin when omitted.
        #[arg(long)]
        body: Option<String>,
    },
    /// Append text to an existing page.
    Append {
        path: std::path::PathBuf,
        rel_path: String,
        text: String,
        /// No leading newline.
        #[arg(long)]
        inline: bool,
    },
    /// Prepend text immediately after the frontmatter (or at top
    /// when none).
    Prepend {
        path: std::path::PathBuf,
        rel_path: String,
        text: String,
        #[arg(long)]
        inline: bool,
    },
    /// Delete a page.
    Delete {
        path: std::path::PathBuf,
        rel_path: String,
    },
    /// Move / rename a page.
    Move {
        path: std::path::PathBuf,
        from: String,
        to: String,
    },
    /// Sync a local vault directory against the active org's
    /// vault on the server. Pulls remote-only files, pushes
    /// local-only files, and resolves conflicts via
    /// newer-mtime-wins. See `features/vault/vault-sync-client/`
    /// for the orchestrator.
    Sync {
        /// Local vault root. Defaults to the active org's
        /// `vault/` dir under the data root.
        #[arg(long)]
        local: Option<std::path::PathBuf>,
        /// Server URL. Falls back to `ws://127.0.0.1:18080`.
        #[arg(long)]
        server: Option<String>,
        /// Org slug to sync against. Defaults to the active
        /// org from the session.
        #[arg(long)]
        org: Option<String>,
        /// Remote vault id under that org. Server-side currently
        /// runs one vault per org keyed by `"default"`.
        #[arg(long, default_value = "default")]
        vault_id: String,
        /// Show the plan but don't apply it.
        #[arg(long)]
        dry_run: bool,
    },
    /// One-way pull — download every server-only file. Local
    /// files that already match the server are skipped; local
    /// files not on the server are left in place.
    Pull {
        #[arg(long)]
        local: Option<std::path::PathBuf>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long, default_value = "default")]
        vault_id: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// One-way push — upload every local-only file. Remote
    /// files not present locally are left alone (no delete).
    Push {
        #[arg(long)]
        local: Option<std::path::PathBuf>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long, default_value = "default")]
        vault_id: String,
        #[arg(long)]
        dry_run: bool,
    },
}

/// Server-side vault id for the flat commands' vox route — the
/// one-vault-per-org convention (`<org>/vault/`).
const ORG_VAULT_ID: &str = "default";

/// The positional vault path of a query/edit subcommand (the sync
/// ops have none).
fn cmd_path_mut(cmd: &mut VaultCmd) -> Option<&mut std::path::PathBuf> {
    match cmd {
        VaultCmd::Open { path }
        | VaultCmd::Pages { path, .. }
        | VaultCmd::Tags { path }
        | VaultCmd::Bases { path }
        | VaultCmd::Cat { path, .. }
        | VaultCmd::Grep { path, .. }
        | VaultCmd::Backlinks { path, .. }
        | VaultCmd::Links { path, .. }
        | VaultCmd::Orphans { path }
        | VaultCmd::Deadends { path }
        | VaultCmd::Unresolved { path }
        | VaultCmd::Outline { path, .. }
        | VaultCmd::Properties { path }
        | VaultCmd::PropertyRead { path, .. }
        | VaultCmd::PropertySet { path, .. }
        | VaultCmd::PropertyRemove { path, .. }
        | VaultCmd::Aliases { path }
        | VaultCmd::Tasks { path }
        | VaultCmd::Wordcount { path, .. }
        | VaultCmd::BaseQuery { path, .. }
        | VaultCmd::Create { path, .. }
        | VaultCmd::Append { path, .. }
        | VaultCmd::Prepend { path, .. }
        | VaultCmd::Delete { path, .. }
        | VaultCmd::Move { path, .. } => Some(path),
        VaultCmd::Sync { .. } | VaultCmd::Pull { .. } | VaultCmd::Push { .. } => None,
    }
}

/// Does this subcommand write the vault? Decides whether the vox
/// route pushes the mirror back after running.
fn cmd_mutates(cmd: &VaultCmd) -> bool {
    matches!(
        cmd,
        VaultCmd::PropertySet { .. }
            | VaultCmd::PropertyRemove { .. }
            | VaultCmd::Create { .. }
            | VaultCmd::Append { .. }
            | VaultCmd::Prepend { .. }
            | VaultCmd::Delete { .. }
            | VaultCmd::Move { .. }
    )
}

/// Route a query/edit subcommand:
///
/// - `--fs`, or the positional path names an existing directory →
///   [`run_vault_fs`] on that directory, byte-identical to the
///   pre-vox behaviour. An explicit on-disk path is authoritative
///   (these commands work on ANY Obsidian vault, org or not); the
///   flag pins it for recovery / offline inspection even though
///   it is also the default for existing paths.
/// - the path does not exist (previously a hard `open:` error) →
///   the active org's vault over vox: mirror it into a scratch
///   dir via `VaultSync::manifest` + `get_file` (remote server or
///   embedded backend alike), run the identical FS logic on the
///   mirror, then push mutations back (`put_file` sha-guarded /
///   create-only, `delete_file` for removals). Data over the
///   wire; computation local; output unchanged.
pub(crate) async fn run_vault(mut cmd: VaultCmd, force_fs: bool) -> eyre::Result<()> {
    use vault_proto::{IfMatch, VaultSyncClient};

    let path = cmd_path_mut(&mut cmd).expect("sync ops never reach run_vault");
    if force_fs || path.exists() {
        return run_vault_fs(cmd);
    }

    let slug = crate::resolve_active_org(None)?;
    let url = crate::resolve_org_vox_url(None, &slug);
    let client: VaultSyncClient = establish_for_url(&url).await?;
    let manifest = client
        .manifest(ORG_VAULT_ID.to_owned())
        .await
        .map_err(|e| eyre::eyre!("fetch manifest: {e:?}"))?;

    // Mirror the org vault into a scratch dir.
    let tmp = tempfile::tempdir().map_err(|e| eyre::eyre!("scratch dir: {e}"))?;
    for entry in &manifest.files {
        let bytes = client
            .get_file(ORG_VAULT_ID.to_owned(), entry.path.clone())
            .await
            .map_err(|e| eyre::eyre!("get {}: {e:?}", entry.path))?;
        let abs = tmp.path().join(&entry.path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| eyre::eyre!("mirror {}: {e}", entry.path))?;
        }
        std::fs::write(&abs, &bytes.0).map_err(|e| eyre::eyre!("mirror {}: {e}", entry.path))?;
    }

    let mutating = cmd_mutates(&cmd);
    *cmd_path_mut(&mut cmd).expect("still a query/edit cmd") = tmp.path().to_path_buf();
    run_vault_fs(cmd)?;

    if mutating {
        // Push the mirror's delta back: sha-guarded overwrites,
        // create-only for new pages, delete for removals (a
        // `move` shows up as one of each).
        let before: HashMap<&str, &str> = manifest
            .files
            .iter()
            .map(|e| (e.path.as_str(), e.sha256.as_str()))
            .collect();
        let after = vault_sync_client::index_local(tmp.path())
            .map_err(|e| eyre::eyre!("index mirror: {e}"))?;
        let mut seen = std::collections::HashSet::new();
        for entry in &after {
            seen.insert(entry.path.clone());
            let if_match = match before.get(entry.path.as_str()) {
                Some(sha) if *sha == entry.sha256 => continue,
                Some(sha) => IfMatch::Sha((*sha).to_owned()),
                None => IfMatch::CreateOnly,
            };
            let bytes = std::fs::read(tmp.path().join(&entry.path))
                .map_err(|e| eyre::eyre!("read mirror {}: {e}", entry.path))?;
            client
                .put_file(ORG_VAULT_ID.to_owned(), entry.path.clone(), bytes, if_match)
                .await
                .map_err(|e| eyre::eyre!("put {}: {e:?}", entry.path))?;
        }
        for entry in &manifest.files {
            if !seen.contains(&entry.path) {
                client
                    .delete_file(
                        ORG_VAULT_ID.to_owned(),
                        entry.path.clone(),
                        IfMatch::Sha(entry.sha256.clone()),
                    )
                    .await
                    .map_err(|e| eyre::eyre!("delete {}: {e:?}", entry.path))?;
            }
        }
    }
    Ok(())
}

fn run_vault_fs(cmd: VaultCmd) -> eyre::Result<()> {
    use vault_obsidian::Vault;
    match cmd {
        VaultCmd::Open { path } => {
            let t0 = std::time::Instant::now();
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            println!(
                "vault: {}\n  pages:       {}\n  bases:       {}\n  attachments: {}\n  loaded in:   {:?}",
                v.root.display(),
                v.pages.len(),
                v.bases.len(),
                v.attachments.len(),
                t0.elapsed(),
            );
        }
        VaultCmd::Pages {
            path,
            folder,
            tag,
            fm,
            json,
        } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let fm_pairs = parse_fm_pairs(&fm)?;
            for page in &v.pages {
                if let Some(f) = &folder {
                    if !page.folder.starts_with(f) {
                        continue;
                    }
                }
                if let Some(t) = &tag {
                    if !page_matches_tag(page, t) {
                        continue;
                    }
                }
                if !fm_pairs.iter().all(|(k, v)| page_matches_fm(page, k, v)) {
                    continue;
                }
                if json {
                    let obj = serde_json::json!({
                        "path": page.rel_path,
                        "basename": page.basename,
                        "folder": page.folder,
                        "frontmatter": page
                            .parsed
                            .frontmatter
                            .iter()
                            .map(|e| (e.key.clone(), e.value.clone()))
                            .collect::<serde_json::Map<_, _>>(),
                    });
                    println!("{obj}");
                } else {
                    println!("{}", page.rel_path);
                }
            }
        }
        VaultCmd::Tags { path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let mut counts: HashMap<String, usize> = HashMap::new();
            for page in &v.pages {
                for t in collect_page_tags(page) {
                    *counts.entry(t).or_insert(0) += 1;
                }
            }
            let mut rows: Vec<_> = counts.into_iter().collect();
            rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            for (tag, n) in rows {
                println!("{n:>5}  #{tag}");
            }
        }
        VaultCmd::Bases { path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            for base in &v.bases {
                match &base.parsed {
                    Ok(p) => {
                        let views: Vec<&str> = p.views.iter().map(|v| v.name.as_str()).collect();
                        println!("{}  [{}]", base.rel_path, views.join(", "));
                    }
                    Err(e) => println!("{}  (parse error: {e})", base.rel_path),
                }
            }
        }
        VaultCmd::Cat { path, rel_path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let page = v
                .page(&rel_path)
                .ok_or_else(|| eyre::eyre!("page not found: {rel_path}"))?;
            print!("{}", page.raw);
        }
        VaultCmd::Grep { path, pattern } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let needle = pattern.to_lowercase();
            for page in &v.pages {
                for (i, line) in page.raw.lines().enumerate() {
                    if line.to_lowercase().contains(&needle) {
                        println!("{}:{}:{}", page.rel_path, i + 1, line);
                    }
                }
            }
        }
        VaultCmd::Backlinks { path, rel_path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let idx = vault_obsidian::LinkIndex::build(&v);
            for p in idx.backlinks(&rel_path) {
                println!("{p}");
            }
        }
        VaultCmd::Links { path, rel_path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let idx = vault_obsidian::LinkIndex::build(&v);
            for link in idx.outgoing(&rel_path) {
                match link.resolved {
                    Some(target) => println!("{}\t→ {target}", link.linkpath),
                    None => println!("{}\t(unresolved)", link.linkpath),
                }
            }
        }
        VaultCmd::Orphans { path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let idx = vault_obsidian::LinkIndex::build(&v);
            for p in idx.orphans() {
                println!("{p}");
            }
        }
        VaultCmd::Deadends { path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let idx = vault_obsidian::LinkIndex::build(&v);
            for p in idx.deadends() {
                println!("{p}");
            }
        }
        VaultCmd::Unresolved { path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let idx = vault_obsidian::LinkIndex::build(&v);
            for u in idx.unresolved() {
                println!("{}\t{}", u.source, u.linkpath);
            }
        }
        VaultCmd::Outline { path, rel_path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let page = v
                .page(&rel_path)
                .ok_or_else(|| eyre::eyre!("page not found: {rel_path}"))?;
            for h in vault_obsidian::outline(page).headings {
                let bar = "#".repeat(h.level as usize);
                println!("{:>5}  {bar} {}", h.line, h.text);
            }
        }
        VaultCmd::Properties { path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            for k in vault_obsidian::list_property_keys(&v) {
                println!("{k}");
            }
        }
        VaultCmd::PropertyRead {
            path,
            rel_path,
            key,
        } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let page = v
                .page(&rel_path)
                .ok_or_else(|| eyre::eyre!("page not found: {rel_path}"))?;
            if let Some(v) = vault_obsidian::read_property(page, &key) {
                println!("{v}");
            }
        }
        VaultCmd::PropertySet {
            path,
            rel_path,
            key,
            value,
        } => {
            let mut v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let parsed: serde_json::Value = serde_json::from_str(&value)
                .unwrap_or_else(|_| serde_json::Value::String(value.clone()));
            let guard = vault_obsidian::SelfWriteGuard::new();
            vault_obsidian::set_property(&mut v, &rel_path, &key, parsed, &guard)
                .map_err(|e| eyre::eyre!("set: {e}"))?;
        }
        VaultCmd::PropertyRemove {
            path,
            rel_path,
            key,
        } => {
            let mut v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let guard = vault_obsidian::SelfWriteGuard::new();
            vault_obsidian::remove_property(&mut v, &rel_path, &key, &guard)
                .map_err(|e| eyre::eyre!("remove: {e}"))?;
        }
        VaultCmd::Aliases { path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            for a in vault_obsidian::list_aliases(&v) {
                println!("{}\t{}", a.alias, a.page);
            }
        }
        VaultCmd::Tasks { path } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            for t in vault_obsidian::list_tasks(&v) {
                println!("{}:{}\t[{}] {}", t.page, t.line, t.marker, t.text);
            }
        }
        VaultCmd::Wordcount { path, page } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let wc = match page {
                Some(rel) => {
                    let p = v
                        .page(&rel)
                        .ok_or_else(|| eyre::eyre!("page not found: {rel}"))?;
                    vault_obsidian::page_wordcount(p)
                }
                None => vault_obsidian::vault_wordcount(&v),
            };
            println!(
                "pages: {}\nwords: {}\ncharacters: {}",
                wc.pages, wc.words, wc.characters
            );
        }
        VaultCmd::BaseQuery { path, base, view } => {
            let v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            if let Some(view_name) = view {
                let ev = vault_obsidian::query_view(&v, &base, &view_name)
                    .map_err(|e| eyre::eyre!("query: {e}"))?;
                print_executed_view(&view_name, &ev);
            } else {
                let results = vault_obsidian::query_all_views(&v, &base)
                    .map_err(|e| eyre::eyre!("query: {e}"))?;
                for (name, ev) in results {
                    print_executed_view(&name, &ev);
                }
            }
        }
        VaultCmd::Create {
            path,
            rel_path,
            body,
        } => {
            let mut v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let body = if let Some(b) = body {
                b
            } else {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf
            };
            let guard = vault_obsidian::SelfWriteGuard::new();
            vault_obsidian::create_page(&mut v, &rel_path, &[], &body, &guard)
                .map_err(|e| eyre::eyre!("create: {e}"))?;
        }
        VaultCmd::Append {
            path,
            rel_path,
            text,
            inline,
        } => {
            let mut v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let guard = vault_obsidian::SelfWriteGuard::new();
            vault_obsidian::append_to_page(&mut v, &rel_path, &text, inline, &guard)
                .map_err(|e| eyre::eyre!("append: {e}"))?;
        }
        VaultCmd::Prepend {
            path,
            rel_path,
            text,
            inline,
        } => {
            let mut v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let guard = vault_obsidian::SelfWriteGuard::new();
            vault_obsidian::prepend_to_page(&mut v, &rel_path, &text, inline, &guard)
                .map_err(|e| eyre::eyre!("prepend: {e}"))?;
        }
        VaultCmd::Delete { path, rel_path } => {
            let mut v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let guard = vault_obsidian::SelfWriteGuard::new();
            vault_obsidian::delete_page(&mut v, &rel_path, &guard)
                .map_err(|e| eyre::eyre!("delete: {e}"))?;
        }
        VaultCmd::Move { path, from, to } => {
            let mut v = Vault::open(&path).map_err(|e| eyre::eyre!("open: {e}"))?;
            let guard = vault_obsidian::SelfWriteGuard::new();
            vault_obsidian::move_page(&mut v, &from, &to, &guard)
                .map_err(|e| eyre::eyre!("move: {e}"))?;
        }
        VaultCmd::Sync { .. } | VaultCmd::Pull { .. } | VaultCmd::Push { .. } => {
            // Routed to `run_vault_sync` from the async
            // dispatch above. Should never hit this arm.
            unreachable!("sync ops routed through run_vault_sync");
        }
    }
    Ok(())
}

/// Async vault sync handler — talks to `/org/<slug>/vox` and
/// applies pull/push/sync ops via the architect-generated
/// `VaultSyncClient`. Logic lives in `vault_sync_client`; this
/// wrapper handles the I/O + the CLI's flag plumbing.
pub(crate) async fn run_vault_sync(cmd: VaultCmd) -> eyre::Result<()> {
    use vault_proto::{IfMatch, VaultSyncClient};
    use vault_sync_client::{LocalEntry, Side, SyncOp, SyncSummary, index_local, plan_sync};

    enum Mode {
        Sync,
        Pull,
        Push,
    }

    let (mode, local, server, org_slug, vault_id, dry_run) = match cmd {
        VaultCmd::Sync {
            local,
            server,
            org,
            vault_id,
            dry_run,
        } => (Mode::Sync, local, server, org, vault_id, dry_run),
        VaultCmd::Pull {
            local,
            server,
            org,
            vault_id,
            dry_run,
        } => (Mode::Pull, local, server, org, vault_id, dry_run),
        VaultCmd::Push {
            local,
            server,
            org,
            vault_id,
            dry_run,
        } => (Mode::Push, local, server, org, vault_id, dry_run),
        _ => unreachable!("only sync ops reach this handler"),
    };

    // Resolve the org slug (active session if not overridden).
    let org_slug = match org_slug {
        Some(s) => s,
        None => crate::session_store::load()?
            .map(|s| s.active_slug())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| eyre::eyre!("no active org — pass --org or sign in first"))?,
    };

    // Resolve local vault root (org's `vault/` dir if not overridden).
    let local_root = if let Some(p) = local {
        p
    } else {
        let root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
        root.org(&org_slug).vault_dir()
    };

    // Resolve the per-org vox URL.
    let base = server.unwrap_or_else(|| "ws://127.0.0.1:18080".to_owned());
    let url = if base.ends_with("/vox") {
        base
    } else {
        let stripped = base.trim_end_matches('/');
        format!("{stripped}/org/{org_slug}/vox")
    };

    println!("Local:  {}", local_root.display());
    println!("Server: {url}");
    println!("Vault:  {vault_id}\n");

    let client: VaultSyncClient = establish_for_url(&url).await?;

    // Index local + fetch remote manifest in parallel-ish (the
    // local walk is sync, but cheap; do it before the network
    // round-trip).
    let local_entries: Vec<LocalEntry> =
        index_local(&local_root).map_err(|e| eyre::eyre!("index local: {e}"))?;
    let remote_manifest = client
        .manifest(vault_id.clone())
        .await
        .map_err(|e| eyre::eyre!("fetch manifest: {e:?}"))?;

    println!(
        "indexed: local={} remote={}",
        local_entries.len(),
        remote_manifest.files.len()
    );

    // Plan, then filter by mode.
    let plan = plan_sync(&local_entries, &remote_manifest);
    let plan: Vec<SyncOp> = plan
        .into_iter()
        .filter(|op| {
            matches!(
                (op, &mode),
                (SyncOp::InSync { .. }, _)
                    | (SyncOp::Pull { .. }, Mode::Sync | Mode::Pull)
                    | (SyncOp::Push { .. }, Mode::Sync | Mode::Push)
                    | (SyncOp::Conflict { .. }, Mode::Sync)
            )
        })
        .collect();

    let mut summary = SyncSummary::default();
    for op in &plan {
        summary.record(op);
    }

    println!(
        "plan: {} push · {} pull · {} in-sync · {} conflicts (local/remote: {}/{})\n",
        summary.pushed,
        summary.pulled,
        summary.in_sync,
        summary.conflicts_local_won + summary.conflicts_remote_won,
        summary.conflicts_local_won,
        summary.conflicts_remote_won,
    );

    if dry_run {
        for op in &plan {
            describe_op(op);
        }
        return Ok(());
    }

    // Apply.
    for op in &plan {
        match op {
            SyncOp::InSync { .. } => {}
            SyncOp::Pull { path, .. } => {
                let bytes = client
                    .get_file(vault_id.clone(), path.clone())
                    .await
                    .map_err(|e| eyre::eyre!("get_file {path}: {e:?}"))?;
                write_local(&local_root, path, &bytes.0)?;
                println!("PULL  {path}");
            }
            SyncOp::Push { path, .. } => {
                let abs = local_root.join(path);
                let bytes =
                    std::fs::read(&abs).map_err(|e| eyre::eyre!("read {}: {e}", abs.display()))?;
                client
                    .put_file(vault_id.clone(), path.clone(), bytes, IfMatch::CreateOnly)
                    .await
                    .map_err(|e| eyre::eyre!("put_file {path}: {e:?}"))?;
                println!("PUSH  {path}");
            }
            SyncOp::Conflict {
                path,
                remote_sha,
                winning_side,
                ..
            } => match winning_side {
                Side::Local => {
                    let abs = local_root.join(path);
                    let bytes = std::fs::read(&abs)
                        .map_err(|e| eyre::eyre!("read {}: {e}", abs.display()))?;
                    client
                        .put_file(
                            vault_id.clone(),
                            path.clone(),
                            bytes,
                            IfMatch::Sha(remote_sha.clone()),
                        )
                        .await
                        .map_err(|e| eyre::eyre!("put_file (conflict) {path}: {e:?}"))?;
                    println!("PUSH! {path}  (conflict: local won)");
                }
                Side::Remote => {
                    let bytes = client
                        .get_file(vault_id.clone(), path.clone())
                        .await
                        .map_err(|e| eyre::eyre!("get_file (conflict) {path}: {e:?}"))?;
                    write_local(&local_root, path, &bytes.0)?;
                    println!("PULL! {path}  (conflict: remote won)");
                }
            },
        }
    }

    println!(
        "\ndone: {} pushed · {} pulled · {} in-sync",
        summary.pushed + summary.conflicts_local_won,
        summary.pulled + summary.conflicts_remote_won,
        summary.in_sync,
    );
    Ok(())
}

fn describe_op(op: &vault_sync_client::SyncOp) {
    use vault_sync_client::{Side, SyncOp};
    match op {
        SyncOp::InSync { path } => println!("OK    {path}"),
        SyncOp::Pull { path, .. } => println!("PULL  {path}"),
        SyncOp::Push { path, .. } => println!("PUSH  {path}"),
        SyncOp::Conflict {
            path, winning_side, ..
        } => {
            let side = match winning_side {
                Side::Local => "local",
                Side::Remote => "remote",
            };
            println!("CONF  {path}  (winner: {side})");
        }
    }
}

fn write_local(local_root: &std::path::Path, path: &str, bytes: &[u8]) -> eyre::Result<()> {
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(eyre::eyre!("refused path with `..`: {path}"));
    }
    let abs = local_root.join(path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| eyre::eyre!("mkdir {}: {e}", parent.display()))?;
    }
    std::fs::write(&abs, bytes).map_err(|e| eyre::eyre!("write {}: {e}", abs.display()))?;
    Ok(())
}

fn print_executed_view(name: &str, ev: &vault_live::bases::ExecutedView) {
    let total: usize = ev.groups.iter().map(|(_, r)| r.len()).sum();
    println!("## {name}  ({total} rows)");
    for (bucket, rows) in &ev.groups {
        if !bucket.is_empty() {
            println!("  [{bucket}]");
        }
        for row in rows {
            println!("    {}", row.basename);
        }
    }
}

fn parse_fm_pairs(raw: &[String]) -> eyre::Result<Vec<(String, serde_json::Value)>> {
    raw.iter()
        .map(|s| {
            let (k, v) = s
                .split_once('=')
                .ok_or_else(|| eyre::eyre!("--fm expects KEY=VALUE, got `{s}`"))?;
            let parsed: serde_json::Value = serde_json::from_str(v)
                .unwrap_or_else(|_| serde_json::Value::String(v.to_string()));
            Ok((k.to_string(), parsed))
        })
        .collect()
}

fn page_matches_fm(page: &vault_obsidian::VaultPage, key: &str, value: &serde_json::Value) -> bool {
    page.parsed
        .frontmatter
        .iter()
        .any(|e| e.key == key && &e.value == value)
}

fn page_matches_tag(page: &vault_obsidian::VaultPage, tag: &str) -> bool {
    // Match Obsidian: a query for `#parent` also includes any
    // `#parent/child` nested tags.
    let prefix = format!("{tag}/");
    collect_page_tags(page)
        .into_iter()
        .any(|t| t == tag || t.starts_with(&prefix))
}

fn collect_page_tags(page: &vault_obsidian::VaultPage) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in &page.parsed.frontmatter {
        if e.key == "tags" || e.key == "tag" {
            match &e.value {
                serde_json::Value::String(s) => {
                    for t in s.split([',', ' ']) {
                        let t = t.trim().trim_start_matches('#');
                        if !t.is_empty() {
                            out.push(t.to_string());
                        }
                    }
                }
                serde_json::Value::Array(arr) => {
                    for v in arr {
                        if let Some(s) = v.as_str() {
                            out.push(s.trim_start_matches('#').to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for b in &page.parsed.blocks {
        for r in &b.refs {
            if let vault_live::refs::Ref::Tag(t) = r {
                out.push(t.path.join("/"));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

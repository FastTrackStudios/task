//! `task share …` — share links from the shell: mint one for a folder of
//! a File Root, list them, pause, resume and revoke them, read who used
//! them.
//!
//! The same `ShareService` the web app's Share panel calls, on the org
//! lane — nothing here is a private lane (ADR 0003). A link is what lets
//! somebody with no account reach one folder: a band member's stems, a
//! client's cut, or a public demo that opens a session and streams its
//! proxies:
//!
//! ```text
//! task share folder <root-id> "Always On Time" --label demo --documents
//! ```
//!
//! Lives in its own module so concurrent agents editing `main.rs` only
//! collide on the two-line dispatch arm.

use clap::Subcommand;
use share_proto::{NewShareLink, ShareCapabilities, ShareLinkInfo, ShareServiceClient, ShareTarget};

use crate::{establish_for_url, resolve_org_vox_url};

#[derive(Subcommand, Debug)]
pub enum ShareCmd {
    /// Mint a link to a folder of a File Root (the whole root with no
    /// subpath). View-only unless a capability says otherwise: media
    /// streams as renditions, originals never.
    Folder {
        root_id: uuid::Uuid,
        /// Root-relative folder; empty shares the whole root.
        #[arg(default_value = "")]
        subpath: String,
        /// What the link is called in the Links registry.
        #[arg(long, default_value = "")]
        label: String,
        /// The folder's documents (a session, a chart) are served whole —
        /// what an app opens the folder by. Media stays renditions-only.
        #[arg(long)]
        documents: bool,
        /// Originals may be downloaded (receipted in the access log).
        #[arg(long)]
        download: bool,
        /// Visitors may comment.
        #[arg(long)]
        comment: bool,
        /// Gate the link with a password. Reads `TASK_SHARE_PASSWORD`
        /// when the flag is absent — prefer it in scripts, since an
        /// argument is visible to `ps` and lands in shell history.
        #[arg(long, env = "TASK_SHARE_PASSWORD", hide_env_values = true)]
        password: Option<String>,
        /// Stop resolving after this many days.
        #[arg(long)]
        expires_days: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// Mint a live link to a setlist: whoever opens it joins the set's
    /// live session (Session, in a browser or the app) with no account —
    /// its songs streamed, everyone in one session. `--reset-minutes`
    /// makes it a playground that starts over that often (the public
    /// demo).
    ///
    /// ```text
    /// task share live <setlist-id> --label demo --reset-minutes 5
    /// ```
    Live {
        setlist: String,
        #[arg(long, default_value = "")]
        label: String,
        /// Start the set over this often (0: keep what is done in it).
        #[arg(long, default_value_t = 0)]
        reset_minutes: u32,
        #[arg(long, env = "TASK_SHARE_PASSWORD", hide_env_values = true)]
        password: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Every link in the org, newest first.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Pause a link: it stops resolving at once, and can be resumed.
    Disable { token: String },
    /// Resume a paused link.
    Enable { token: String },
    /// Delete a link for good.
    Revoke { token: String },
    /// Who used a link: views, browses, streams, documents, downloads.
    Log {
        token: String,
        #[arg(long)]
        json: bool,
    },
}

pub async fn run_share(cmd: ShareCmd, org_override: Option<&str>) -> eyre::Result<()> {
    let slug = crate::resolve_slug(org_override)?;
    let share: ShareServiceClient = establish_for_url(&resolve_org_vox_url(None, &slug)).await?;
    match cmd {
        ShareCmd::Folder {
            root_id,
            subpath,
            label,
            documents,
            download,
            comment,
            password,
            expires_days,
            json,
        } => {
            let expires_unix = expires_days.map(|days| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                i64::try_from(now + u64::from(days) * 86_400).unwrap_or(i64::MAX)
            });
            let link = share
                .create_link(
                    ShareTarget::Slice {
                        root_id,
                        subpath: subpath.trim_matches('/').to_owned(),
                    },
                    NewShareLink {
                        label,
                        capabilities: Some(ShareCapabilities {
                            comment,
                            download,
                            file_request: false,
                            documents,
                        }),
                        password,
                        expires_unix,
                    },
                )
                .await
                .map_err(|e| eyre::eyre!("create link: {e}"))?;
            print_link(&link, json)?;
        }
        ShareCmd::Live { setlist, label, reset_minutes, password, json } => {
            let link = share
                .create_link(
                    ShareTarget::Live { setlist, reset_secs: reset_minutes.saturating_mul(60) },
                    NewShareLink { label, capabilities: None, password, expires_unix: None },
                )
                .await
                .map_err(|e| eyre::eyre!("create link: {e}"))?;
            print_link(&link, json)?;
        }
        ShareCmd::List { json } => {
            let links = share
                .list_links()
                .await
                .map_err(|e| eyre::eyre!("list links: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&links).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                for link in &links {
                    print_link(link, false)?;
                }
            }
        }
        ShareCmd::Disable { token } => share
            .set_link_disabled(token, true)
            .await
            .map_err(|e| eyre::eyre!("disable: {e}"))?,
        ShareCmd::Enable { token } => share
            .set_link_disabled(token, false)
            .await
            .map_err(|e| eyre::eyre!("enable: {e}"))?,
        ShareCmd::Revoke { token } => share
            .delete_link(token)
            .await
            .map_err(|e| eyre::eyre!("revoke: {e}"))?,
        ShareCmd::Log { token, json } => {
            let log = share
                .access_log(token)
                .await
                .map_err(|e| eyre::eyre!("access log: {e}"))?;
            if json {
                println!(
                    "{}",
                    facet_json::to_string(&log).map_err(|e| eyre::eyre!("{e}"))?
                );
            } else {
                for row in log {
                    println!("{}  {:<9}  {}", row.at, row.kind, row.path);
                }
            }
        }
    }
    Ok(())
}

fn print_link(link: &ShareLinkInfo, json: bool) -> eyre::Result<()> {
    if json {
        println!(
            "{}",
            facet_json::to_string(link).map_err(|e| eyre::eyre!("{e}"))?
        );
        return Ok(());
    }
    let caps = link.capabilities;
    let granted: Vec<&str> = [
        ("documents", caps.documents),
        ("download", caps.download),
        ("comment", caps.comment),
        ("file-request", caps.file_request),
    ]
    .into_iter()
    .filter_map(|(name, on)| on.then_some(name))
    .collect();
    let target = match &link.target {
        ShareTarget::Slice { root_id, subpath } => format!("{root_id}/{subpath}"),
        ShareTarget::Note { path, .. } => format!("note {path}"),
        ShareTarget::NamedVersion { id } => format!("version {id}"),
        other => format!("{other:?}"),
    };
    let state = if link.disabled { "  [disabled]" } else { "" };
    let caps = if granted.is_empty() {
        "view".to_owned()
    } else {
        format!("view+{}", granted.join("+"))
    };
    println!("{}  {caps}  {target}{state}\n  {}", link.label, link.url);
    Ok(())
}

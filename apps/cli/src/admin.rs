//! `task admin …` — server-native git snapshots of the data root.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_server_client;

#[derive(Subcommand)]
pub(crate) enum AdminCmd {
    /// Run one server-native snapshot cycle: quiesce writes,
    /// WAL-checkpoint every open sqlite, commit the per-org +
    /// full-state git repos under `<data_root>/.gitstate/`, push
    /// when the server has a backup remote configured.
    Snapshot {
        /// Server URL (defaults like `task org create`).
        #[arg(long)]
        server: Option<String>,
    },
    /// Recent snapshot commits on the full-state repo.
    Log {
        /// Max commits to show.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        server: Option<String>,
    },
    /// Create a branch at the full-state repo's HEAD (and push it)
    /// — "branch the data".
    Branch {
        /// Branch name (a valid git ref name).
        name: String,
        #[arg(long)]
        server: Option<String>,
    },
    /// Restore the server's data root to a snapshot commit, then
    /// the server EXITS so its supervisor restarts it on the
    /// restored data (local dev: restart task-server manually).
    /// By default a rescue snapshot runs first; requires --yes.
    Restore {
        /// Full-repo commit (sha or ref) to restore to.
        commit: String,
        /// Skip the rescue snapshot and proceed even if the
        /// server's work tree has uncommitted changes.
        #[arg(long)]
        force: bool,
        /// Confirm the restore (sends the confirmation token).
        /// Without it the command only prints what would happen.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        server: Option<String>,
    },
}

pub(crate) async fn run_admin(cmd: AdminCmd) -> eyre::Result<()> {
    // Same connection style as `task org create`: the
    // server-management endpoint at `<server>/server/vox`, the
    // active session's token for auth. An explicitly-set
    // `TASK_BACKUP_GIT_TOKEN` wins over the stored session so
    // headless automation (and admins targeting a server their
    // session wasn't minted on) can drive the verbs directly.
    async fn connect_snapshot(
        server: Option<&str>,
    ) -> eyre::Result<(org_proto::SnapshotServiceClient, String)> {
        let token = std::env::var("TASK_BACKUP_GIT_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .or_else(|| {
                crate::session_store::load()
                    .ok()
                    .flatten()
                    .and_then(|s| s.servers.get(&s.active).map(|e| e.token.clone()))
            })
            .unwrap_or_default();
        let (client, _url): (org_proto::SnapshotServiceClient, _) =
            establish_server_client(server).await?;
        Ok((client, token))
    }

    fn print_repos(repos: &[org_proto::RepoResult]) {
        for r in repos {
            if !r.error.is_empty() {
                println!("  {}  ERROR: {}", r.repo, r.error);
            } else if r.clean {
                println!("  {}  clean — skip{}", r.repo, push_badge(r.pushed));
            } else {
                println!(
                    "  {}  committed {}{}",
                    r.repo,
                    &r.committed[..r.committed.len().min(12)],
                    push_badge(r.pushed)
                );
            }
        }
    }
    fn push_badge(pushed: bool) -> &'static str {
        if pushed { "  [pushed]" } else { "" }
    }

    match cmd {
        AdminCmd::Snapshot { server } => {
            let (client, token) = connect_snapshot(server.as_deref()).await?;
            let report = client
                .snapshot(token)
                .await
                .map_err(|e| eyre::eyre!("snapshot: {e:?}"))?;
            println!("snapshot {}", report.stamp);
            print_repos(&report.repos);
            if report.repos.iter().any(|r| !r.error.is_empty()) {
                return Err(eyre::eyre!("snapshot cycle reported per-repo errors"));
            }
        }
        AdminCmd::Log { limit, server } => {
            let (client, token) = connect_snapshot(server.as_deref()).await?;
            let entries = client
                .log(token, limit)
                .await
                .map_err(|e| eyre::eyre!("log: {e:?}"))?;
            if entries.is_empty() {
                println!("(no snapshots yet — run `task admin snapshot`)");
                return Ok(());
            }
            for e in entries {
                println!(
                    "{}  {}  {}",
                    &e.commit[..e.commit.len().min(12)],
                    e.timestamp,
                    e.message
                );
            }
        }
        AdminCmd::Branch { name, server } => {
            let (client, token) = connect_snapshot(server.as_deref()).await?;
            let res = client
                .branch(token, name)
                .await
                .map_err(|e| eyre::eyre!("branch: {e:?}"))?;
            println!(
                "branched `{}` at {}{}",
                res.name,
                &res.commit[..res.commit.len().min(12)],
                push_badge(res.pushed)
            );
        }
        AdminCmd::Restore {
            commit,
            force,
            yes,
            server,
        } => {
            if !yes {
                println!("Would restore the server's data root to `{commit}`.");
                println!("This rewrites EVERY org's files + sqlites on the server, then the");
                println!("server process exits so its supervisor restarts it (local dev: restart");
                println!("task-server manually).");
                if force {
                    println!(
                        "--force: skips the rescue snapshot — pre-restore state is NOT saved."
                    );
                } else {
                    println!("A rescue snapshot of the current state runs first.");
                }
                println!("\nRe-run with --yes to proceed.");
                return Ok(());
            }
            let (client, token) = connect_snapshot(server.as_deref()).await?;
            let report = client
                .restore(token, commit.clone(), commit, force)
                .await
                .map_err(|e| eyre::eyre!("restore: {e:?}"))?;
            if !report.pre_restore.is_empty() {
                println!("rescue snapshot:");
                print_repos(&report.pre_restore);
            }
            println!("restored data root to {}", report.commit);
            if report.restarting {
                println!("server is exiting for restart — give it a few seconds to come back");
            }
        }
    }
    Ok(())
}

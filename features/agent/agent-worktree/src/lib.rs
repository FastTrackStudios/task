//! What a runner physically does for one ticket: cut a worktree and
//! a branch, run an agent in it, run the verify command, commit, and
//! report where the worktree is.
//!
//! No Task types and no vox here — this is git and processes. That
//! keeps it testable against a temporary repository, which matters
//! because every failure mode in this file is a filesystem or exit
//! code, not a wire shape.
//!
//! # One worktree per ticket
//!
//! Tickets under one workstream usually touch the same crates, so
//! their branches merge into the workstream branch rather than each
//! going its own way. But the *work* happens in a worktree of its
//! own, so two tickets running at once cannot corrupt each other's
//! index.
//!
//! # Nothing merges to a mainline
//!
//! The branch is the handback. This module never touches the
//! repository's checked-out branch, and never pushes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git {op} failed ({code}): {stderr}")]
    Git {
        op: &'static str,
        code: String,
        stderr: String,
    },
    #[error("io during {op}: {source}")]
    Io {
        op: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("`{0}` is not inside a git repository")]
    NotARepo(PathBuf),
}

/// A worktree cut for one ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute path on the runner's disk. Reported to the server
    /// afterwards as an observation, never configured there.
    pub path: PathBuf,
    /// The branch checked out in it.
    pub branch: String,
    /// The repository it was cut from.
    pub repo: PathBuf,
}

/// Outcome of running the verify command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// `true` only on exit zero. The whole point: done is a fact.
    pub passed: bool,
    pub code: Option<i32>,
    /// Tail of combined output, bounded — full transcripts do not
    /// belong anywhere near the vault.
    pub tail: String,
}

/// How many bytes of verify output to keep.
pub const TAIL_BYTES: usize = 8 * 1024;

fn git(repo: &Path, op: &'static str, args: &[&str]) -> Result<String, WorktreeError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|source| WorktreeError::Io { op, source })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(WorktreeError::Git {
            op,
            code: out
                .status
                .code()
                .map_or_else(|| "signal".into(), |c| c.to_string()),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }
}

/// The branch name for a ticket.
///
/// Short ids keep the branch readable; the full id lives on the run
/// record, so this only has to be unique among live work.
#[must_use]
pub fn branch_for(ticket_short_id: &str) -> String {
    format!("agent/{ticket_short_id}")
}

/// Cut a worktree for one ticket, on a new branch off `base`.
///
/// The worktree lands under `worktree_root`, which the runner
/// chooses — the server never dictates a path.
///
/// # Errors
///
/// [`WorktreeError`] when the repo is not a repo, or git refuses.
pub fn create(
    repo: &Path,
    worktree_root: &Path,
    ticket_short_id: &str,
    base: &str,
) -> Result<Worktree, WorktreeError> {
    if !repo.join(".git").exists() {
        // A worktree's `.git` is a file, not a directory, so this
        // also accepts cutting from inside another worktree.
        return Err(WorktreeError::NotARepo(repo.to_path_buf()));
    }
    let branch = branch_for(ticket_short_id);
    let path = worktree_root.join(ticket_short_id);

    std::fs::create_dir_all(worktree_root).map_err(|source| WorktreeError::Io {
        op: "create worktree root",
        source,
    })?;

    git(
        repo,
        "worktree add",
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &path.to_string_lossy(),
            base,
        ],
    )?;

    Ok(Worktree {
        path,
        branch,
        repo: repo.to_path_buf(),
    })
}

/// Run the verify command inside the worktree.
///
/// Exit zero is the only pass. The command runs through a shell so a
/// project can declare a pipeline (`cargo check -p x && cargo test`)
/// without this module parsing it.
///
/// # Errors
///
/// [`WorktreeError::Io`] if the shell cannot be spawned at all.
pub fn verify(worktree: &Worktree, command: &str) -> Result<Verdict, WorktreeError> {
    let out = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&worktree.path)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| WorktreeError::Io {
            op: "spawn verify",
            source,
        })?;

    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));

    Ok(Verdict {
        passed: out.status.success(),
        code: out.status.code(),
        tail: tail(&combined, TAIL_BYTES),
    })
}

/// Last `max` bytes, cut on a char boundary.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// Is there anything to commit?
///
/// # Errors
///
/// [`WorktreeError`] when git refuses.
pub fn is_dirty(worktree: &Worktree) -> Result<bool, WorktreeError> {
    Ok(!git(&worktree.path, "status", &["status", "--porcelain"])?.is_empty())
}

/// Commit everything in the worktree onto its own branch.
///
/// Returns `None` when there was nothing to commit — an agent that
/// changed nothing is not a failure, it is a no-op, and inventing an
/// empty commit would make the branch lie.
///
/// # Errors
///
/// [`WorktreeError`] when git refuses.
pub fn commit_all(worktree: &Worktree, message: &str) -> Result<Option<String>, WorktreeError> {
    if !is_dirty(worktree)? {
        return Ok(None);
    }
    git(&worktree.path, "add", &["add", "-A"])?;
    git(&worktree.path, "commit", &["commit", "-m", message])?;
    Ok(Some(git(
        &worktree.path,
        "rev-parse",
        &["rev-parse", "HEAD"],
    )?))
}

/// Commits on this worktree's branch that are not on `base`.
///
/// # Errors
///
/// [`WorktreeError`] when git refuses.
pub fn commits_ahead(worktree: &Worktree, base: &str) -> Result<Vec<String>, WorktreeError> {
    let out = git(
        &worktree.path,
        "rev-list",
        &["rev-list", &format!("{base}..HEAD"), "--oneline"],
    )?;
    Ok(out.lines().map(str::to_string).collect())
}

/// Merge a ticket branch into the workstream branch.
///
/// Run by the workstream manager once a ticket goes green, so the
/// human reviews one branch per workstream rather than one per
/// ticket.
///
/// # Errors
///
/// [`WorktreeError`] when the merge conflicts or git refuses.
pub fn merge_into(
    workstream_worktree: &Worktree,
    ticket_branch: &str,
) -> Result<(), WorktreeError> {
    git(
        &workstream_worktree.path,
        "merge",
        &[
            "merge",
            "--no-ff",
            "-m",
            &format!("merge {ticket_branch}"),
            ticket_branch,
        ],
    )?;
    Ok(())
}

/// Remove the worktree from disk, keeping the branch.
///
/// The branch is the deliverable; the directory is scratch. A
/// worktree still on disk after a terminal run is what
/// `needs-cleanup` means.
///
/// # Errors
///
/// [`WorktreeError`] when git refuses.
pub fn remove(worktree: &Worktree) -> Result<(), WorktreeError> {
    git(
        &worktree.repo,
        "worktree remove",
        &[
            "worktree",
            "remove",
            "--force",
            &worktree.path.to_string_lossy(),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo with one commit on `main`.
    fn repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("repo");
        std::fs::create_dir_all(&path).unwrap();
        run(&path, &["git", "init", "-q", "-b", "main"]);
        run(&path, &["git", "config", "user.email", "t@example.com"]);
        run(&path, &["git", "config", "user.name", "T"]);
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        run(&path, &["git", "add", "-A"]);
        run(&path, &["git", "commit", "-qm", "init"]);
        (tmp, path)
    }

    #[test]
    fn a_worktree_is_cut_on_its_own_branch() {
        let (tmp, path) = repo();
        let wt = create(&path, &tmp.path().join("wt"), "758b2ae8", "main").unwrap();

        assert!(wt.path.join("README.md").exists());
        assert_eq!(wt.branch, "agent/758b2ae8");
        let head = git(&wt.path, "branch", &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(head, "agent/758b2ae8");
    }

    #[test]
    fn the_repos_own_branch_is_untouched() {
        let (tmp, path) = repo();
        create(&path, &tmp.path().join("wt"), "aaa", "main").unwrap();
        let head = git(&path, "branch", &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(head, "main", "the runner must never move the checkout");
    }

    #[test]
    fn two_tickets_get_separate_worktrees() {
        let (tmp, path) = repo();
        let root = tmp.path().join("wt");
        let a = create(&path, &root, "aaa", "main").unwrap();
        let b = create(&path, &root, "bbb", "main").unwrap();
        assert_ne!(a.path, b.path);
        assert_ne!(a.branch, b.branch);

        // Work in one is invisible to the other.
        std::fs::write(a.path.join("only-a.txt"), "x").unwrap();
        assert!(!b.path.join("only-a.txt").exists());
    }

    #[test]
    fn a_path_that_is_not_a_repo_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = create(tmp.path(), &tmp.path().join("wt"), "aaa", "main").unwrap_err();
        assert!(matches!(err, WorktreeError::NotARepo(_)), "{err:?}");
    }

    #[test]
    fn exit_zero_passes_and_anything_else_fails() {
        let (tmp, path) = repo();
        let wt = create(&path, &tmp.path().join("wt"), "aaa", "main").unwrap();

        let ok = verify(&wt, "true").unwrap();
        assert!(ok.passed);
        assert_eq!(ok.code, Some(0));

        let bad = verify(&wt, "exit 3").unwrap();
        assert!(!bad.passed);
        assert_eq!(bad.code, Some(3));
    }

    #[test]
    fn verify_runs_inside_the_worktree_not_the_repo() {
        let (tmp, path) = repo();
        let wt = create(&path, &tmp.path().join("wt"), "aaa", "main").unwrap();
        std::fs::write(wt.path.join("marker.txt"), "x").unwrap();
        assert!(verify(&wt, "test -f marker.txt").unwrap().passed);
    }

    #[test]
    fn verify_captures_output_and_bounds_it() {
        let (tmp, path) = repo();
        let wt = create(&path, &tmp.path().join("wt"), "aaa", "main").unwrap();

        let v = verify(&wt, "echo hello; echo oops >&2; exit 1").unwrap();
        assert!(v.tail.contains("hello") && v.tail.contains("oops"));

        let big = verify(&wt, "head -c 100000 /dev/zero | tr '\\0' 'x'").unwrap();
        assert!(big.tail.len() <= TAIL_BYTES, "{}", big.tail.len());
    }

    #[test]
    fn a_shell_pipeline_is_a_valid_verify_command() {
        let (tmp, path) = repo();
        let wt = create(&path, &tmp.path().join("wt"), "aaa", "main").unwrap();
        assert!(verify(&wt, "true && true").unwrap().passed);
        assert!(!verify(&wt, "true && false").unwrap().passed);
    }

    #[test]
    fn work_is_committed_to_the_ticket_branch() {
        let (tmp, path) = repo();
        let wt = create(&path, &tmp.path().join("wt"), "aaa", "main").unwrap();
        run(&wt.path, &["git", "config", "user.email", "t@example.com"]);
        run(&wt.path, &["git", "config", "user.name", "T"]);

        std::fs::write(wt.path.join("new.txt"), "work\n").unwrap();
        let sha = commit_all(&wt, "agent: did the thing").unwrap();
        assert!(sha.is_some());

        let ahead = commits_ahead(&wt, "main").unwrap();
        assert_eq!(ahead.len(), 1);
        assert!(ahead[0].contains("did the thing"));
    }

    #[test]
    fn an_agent_that_changed_nothing_makes_no_commit() {
        let (tmp, path) = repo();
        let wt = create(&path, &tmp.path().join("wt"), "aaa", "main").unwrap();
        assert!(!is_dirty(&wt).unwrap());
        assert_eq!(
            commit_all(&wt, "nothing").unwrap(),
            None,
            "an empty commit would make the branch lie"
        );
        assert!(commits_ahead(&wt, "main").unwrap().is_empty());
    }

    #[test]
    fn ticket_branches_merge_into_a_workstream_branch() {
        let (tmp, path) = repo();
        let root = tmp.path().join("wt");

        let ws = create(&path, &root, "ws", "main").unwrap();
        run(&ws.path, &["git", "config", "user.email", "t@example.com"]);
        run(&ws.path, &["git", "config", "user.name", "T"]);

        for (id, file) in [("t1", "one.txt"), ("t2", "two.txt")] {
            let t = create(&path, &root, id, "main").unwrap();
            run(&t.path, &["git", "config", "user.email", "t@example.com"]);
            run(&t.path, &["git", "config", "user.name", "T"]);
            std::fs::write(t.path.join(file), "x").unwrap();
            commit_all(&t, &format!("agent: {id}")).unwrap();
            merge_into(&ws, &t.branch).unwrap();
        }

        assert!(ws.path.join("one.txt").exists());
        assert!(ws.path.join("two.txt").exists());
        // Two ticket commits plus two merge commits.
        assert_eq!(commits_ahead(&ws, "main").unwrap().len(), 4);
    }

    #[test]
    fn removing_a_worktree_keeps_its_branch() {
        let (tmp, path) = repo();
        let wt = create(&path, &tmp.path().join("wt"), "aaa", "main").unwrap();
        run(&wt.path, &["git", "config", "user.email", "t@example.com"]);
        run(&wt.path, &["git", "config", "user.name", "T"]);
        std::fs::write(wt.path.join("new.txt"), "work\n").unwrap();
        commit_all(&wt, "agent: work").unwrap();

        remove(&wt).unwrap();
        assert!(!wt.path.exists());

        let branches = git(&path, "branch", &["branch", "--list", &wt.branch]).unwrap();
        assert!(
            branches.contains("agent/aaa"),
            "the branch is the deliverable: {branches}"
        );
    }
}

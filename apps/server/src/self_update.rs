//! `task-server self-update`: keep a non-Nix node on the newest release.
//!
//! The check, the verification and the swap are `architect::release`
//! ([`Updater::run`]): the newest `v<X.Y.Z>` release carrying this
//! platform's `task-server-<os>-<arch>` asset, its `SHA256SUMS` signed by
//! [`RELEASE_SIGNING_KEY`] for that tag, the download matching those sums,
//! and the new binary answering `task-server <version>` before it is
//! renamed over this one. A check that fails for any reason leaves the
//! installed binary exactly as it was; `--then <command…>` (the service
//! files restart the node with it) runs only when a new binary went in.
//!
//! This file is Task's half of the contract: the app name, the repo and
//! the pinned key. The release workflow (`.github/workflows/node-binaries.yml`)
//! and `scripts/install-node.sh` must agree with them; the tests below
//! check both.
//!
//! A binary in the Nix store is never touched: a Nix host follows the
//! release by moving its flake pin (docs/headless-node.md).

use architect::release::{self, Args, Options, ReleaseError, Target, Updater};

/// The binary's name: the asset prefix, what `--version` prints and what
/// the signature's trusted comment names (`task-server v0.2.4 SHA256SUMS`).
pub const APP: &str = "task-server";

/// Where releases live (a public repo: no token needed, but
/// `GITHUB_TOKEN` is sent to the API when set, for the rate limit).
pub const REPO: &str = "FastTrackStudios/task";

/// The release signing key: the minisign public key whose secret half
/// signs every release's `SHA256SUMS` (the `TASK_RELEASE_MINISIGN_KEY`
/// Actions secret). `self-update` installs nothing it does not verify
/// against this key, so a release whose assets were swapped by someone
/// who can write releases, but does not hold the key, is refused.
///
/// The one place the key lives in Task. Provisioned by Keeper (STA-178);
/// the secret half never leaves the `release-signing` environment and
/// nix-secrets. A build whose const is `None` installs nothing, and
/// `self-update` says why. Rotating it (e.g. to the org-wide key, STA-381)
/// is a change to this literal, released like any other; a node moves to
/// the new key by installing a release signed with the old one. The
/// release workflow reads this const to check the secret it signs with is
/// this key's, and `scripts/install-node.sh` pins the same key (a test
/// keeps them equal): keep it a `Some("<base64>")` literal.
pub const RELEASE_SIGNING_KEY: Option<&str> =
    Some("RWRh9Gk9qaStQMhku4THMidEYrQsmAEsm2dDprUkZxShEtqfIcroM8Od");

/// Task's releases, as `architect::release` reads them.
#[must_use]
pub fn updater(token: Option<String>) -> Updater {
    Updater::new(APP, REPO, release::platform_asset(APP)).token(token)
}

/// `task-server self-update …`.
///
/// # Errors
///
/// Bad arguments, or any refusal from [`Updater::run`] (every one but a
/// failed `--then` hook leaves the installed binary unchanged).
pub async fn cli(args: &[String]) -> eyre::Result<()> {
    let opts = match Options::parse(args) {
        Ok(Args::Run(opts)) => opts,
        Ok(Args::Help) => {
            println!("{}", release::usage(APP));
            return Ok(());
        }
        Err(ReleaseError::Usage(why)) => eyre::bail!("{why}\n\n{}", release::usage(APP)),
        Err(e) => return Err(e.into()),
    };
    let exe = release::current_exe()?;
    // The releases API over rustls: the graph carries two backends, so one
    // has to be chosen before the first TLS dial.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let target = Target {
        current: crate::node_cli::version(),
        exe: &exe,
        key: RELEASE_SIGNING_KEY,
    };
    let outcome = updater(std::env::var("GITHUB_TOKEN").ok())
        .run(target, &opts)
        .await?;
    println!("{APP} {outcome}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::path::Path;

    use super::*;

    fn repo_file(path: &str) -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(path),
        )
        .unwrap()
    }

    /// The assets `node-binaries.yml` publishes.
    const PUBLISHED: [&str; 3] = [
        "task-server-linux-x86_64",
        "task-server-linux-aarch64",
        "task-server-macos-aarch64",
    ];

    #[test]
    fn the_release_workflow_builds_this_platforms_asset() {
        // node-binaries.yml names the assets; self-update looks them up by
        // `release::platform_asset`. One list, checked from both ends.
        let wf = repo_file(".github/workflows/node-binaries.yml");
        for a in PUBLISHED {
            assert!(
                wf.contains(&format!("asset: {a}")),
                "node-binaries.yml does not build {a}"
            );
        }
        if cfg!(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "aarch64"),
        )) {
            let asset = updater(None).asset().to_owned();
            assert!(PUBLISHED.contains(&asset.as_str()), "{asset}");
        }
        assert!(wf.contains(release::SUMS_ASSET));
    }

    #[test]
    fn the_service_units_arguments_still_parse() {
        // service_units.rs and deploy/node run exactly this.
        let args: Vec<String> = ["--then", "systemctl", "try-restart", "task-server.service"]
            .map(str::to_owned)
            .into();
        let Ok(Args::Run(opts)) = Options::parse(&args) else {
            panic!("the update unit's arguments no longer parse");
        };
        assert_eq!(opts.then, args[1..]);
        let check: Vec<String> = ["--check", "--to", "v0.2.4"].map(str::to_owned).into();
        let Ok(Args::Run(opts)) = Options::parse(&check) else {
            panic!("--check --to no longer parse");
        };
        assert!(opts.check);
        assert_eq!(opts.to.as_deref(), Some("0.2.4"));
    }

    /// A throwaway minisign key (its secret half was discarded) and two
    /// signatures it made over [`SUMS`], one for v9.9.9 and one for v9.9.8,
    /// with the trusted comment the release workflow writes.
    const TEST_KEY: &str = "RWRUN7wS6cvax7nVK8QgbSwxJb6P55M+ZNlJ0hTBX3rBaITOw8rqgj6T";
    const SUMS: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  task-server-linux-x86_64\n";
    const SIG_999: &str = "untrusted comment: signature from minisign secret key
RURUN7wS6cvax5cQlQV3dC+3yh2F6SdKL3DdhTxkIIfhuiNQQifK6SbByssBWrUjeIqVBdA4IscSsM3kSXdDUsLLqwmohqMPfAg=
trusted comment: task-server v9.9.9 SHA256SUMS
8JydgklHgR68fyD2Rf64Y1AU1AKfogNbIet0hmJVuJBF+qfBGUR4NhDew6geVZBOjY7WOXskZt2Cmh0FY9+OAw==
";
    const SIG_998: &str = "untrusted comment: signature from minisign secret key
RURUN7wS6cvax5cQlQV3dC+3yh2F6SdKL3DdhTxkIIfhuiNQQifK6SbByssBWrUjeIqVBdA4IscSsM3kSXdDUsLLqwmohqMPfAg=
trusted comment: task-server v9.9.8 SHA256SUMS
TIeWoirwEQq6d9jQVGiSIWZwl0LKU3S9HmhRvL1iyUcoHPuiJx9ShisEh1e0TOKyROLeEZP6rotFyl3XC+pKAQ==
";

    #[test]
    fn task_signatures_verify_for_their_own_tag_only() {
        let for_tag = |tag: &str| release::trusted_comment(APP, tag);
        release::verify_sums(TEST_KEY, SUMS.as_bytes(), SIG_999, &for_tag("v9.9.9")).unwrap();
        // A real signature, lifted from an older release.
        assert!(matches!(
            release::verify_sums(TEST_KEY, SUMS.as_bytes(), SIG_998, &for_tag("v9.9.9")),
            Err(ReleaseError::BadSignature { .. })
        ));
    }

    #[test]
    fn the_pinned_release_key_decodes() {
        let key = RELEASE_SIGNING_KEY.expect("the release key is pinned (STA-446)");
        // The key decodes, so verification gets as far as the signature.
        assert!(matches!(
            release::verify_sums(key, b"", "not a signature", "x"),
            Err(ReleaseError::BadSignature { .. })
        ));
        assert_eq!(
            release::trusted_comment(APP, "v0.2.4"),
            "task-server v0.2.4 SHA256SUMS"
        );
    }

    #[test]
    fn the_install_script_pins_the_same_release_key() {
        let script = repo_file("scripts/install-node.sh");
        let key = RELEASE_SIGNING_KEY.expect("the release key is pinned (STA-446)");
        assert!(
            script.contains(&format!("RELEASE_KEY=\"{key}\"")),
            "scripts/install-node.sh pins another release key than RELEASE_SIGNING_KEY"
        );
        assert!(
            script.contains(r#"task-server $tag SHA256SUMS"#),
            "scripts/install-node.sh checks another trusted comment than `trusted_comment`"
        );
    }

    #[test]
    fn the_release_workflow_signs_what_self_update_verifies() {
        let wf = repo_file(".github/workflows/node-binaries.yml");
        assert!(
            wf.contains(release::SIG_ASSET),
            "node-binaries.yml does not attach {}",
            release::SIG_ASSET
        );
        assert!(
            wf.contains(r#"-t "task-server $TAG SHA256SUMS""#),
            "node-binaries.yml signs another trusted comment than `trusted_comment`"
        );
        assert!(
            wf.contains("RELEASE_SIGNING_KEY"),
            "the workflow checks its key against the pinned one"
        );
        assert!(
            wf.contains("pins RELEASE_SIGNING_KEY but TASK_RELEASE_MINISIGN_KEY is not set"),
            "a tag that pins a key must fail, not warn, when the signing secret is unset"
        );
    }

    /// Stage `bytes` beside `exe` the way the download does.
    fn stage(exe: &Path, bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut staged = release::staging_file(exe).unwrap();
        staged.write_all(bytes).unwrap();
        staged
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_staged_binary_that_lies_about_its_version_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(APP);
        std::fs::write(&exe, b"old").unwrap();
        let expected = format!("{APP} 9.9.9");
        // A "binary" that answers the wrong version.
        let liar = b"#!/bin/sh\necho 'task-server 0.0.1'\n";
        let err = release::swap_in(stage(&exe, liar), &exe, &expected)
            .await
            .unwrap_err();
        assert!(matches!(err, ReleaseError::SmokeTest { .. }), "{err}");
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"old",
            "the installed binary is untouched"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(leftovers.len(), 1, "the staged file is cleaned up");
        // And one that tells the truth is swapped in.
        let honest = b"#!/bin/sh\necho 'task-server 9.9.9'\n";
        release::swap_in(stage(&exe, honest), &exe, &expected)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), honest);
    }
}

//! Standalone testing harness for the email feature. Not part
//! of the main `task` app — built so we can iterate on
//! backends + storage in isolation. Mirrors
//! `features/editor/examples/playground`.
//!
//! Usage:
//!
//! ```text
//! email-client <path-to-maildir>
//! ```
//!
//! Walks the given maildir via `email-maildir::Backend` and
//! prints folder listings + recent envelopes for each. Phase-1
//! shape — no UI yet, just CLI output to validate the read
//! path end-to-end against a fixture maildir.

use email_maildir::Backend;
use email_proto::{Account, AccountId, EmailSync, SeqRange};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: email-client <path-to-maildir>")?;

    let account = Account {
        id: AccountId("scratch".into()),
        name: "scratch".into(),
        address: "you@example.com".into(),
        display_name: None,
    };

    let backend = Backend::single(account.clone(), root.clone())?;
    let folders = backend.list_folders(&account.id.0)?;

    println!("maildir: {}", root.display());
    println!("{} folder(s):", folders.len());
    for f in &folders {
        println!(
            "  {:<24} role={:?}  msgs={:?}  unread={:?}",
            f.id, f.role, f.message_count, f.unread_count
        );
    }
    println!();

    for f in &folders {
        let envs = backend.fetch_envelopes(&account.id.0, &f.id, SeqRange::Recent(10))?;
        if envs.is_empty() {
            continue;
        }
        println!("recent in {}:", f.id);
        for e in envs {
            let from = e.from.first().map_or("?", |a| a.email.as_str());
            println!("  [{}] {}  —  {}", e.date_ms, from, e.subject);
        }
        println!();
    }

    Ok(())
}

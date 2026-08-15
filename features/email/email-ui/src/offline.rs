//! Offline read cache for `/email`.
//!
//! The server stays authoritative — every write goes to it, and it owns
//! the sqlite index and the IMAP connection. This is only about being
//! able to *read* your mail with the server unreachable: on a plane, on
//! hotel wifi, or when the home box is down.
//!
//! What is cached, and why only this much:
//!
//! - **Envelope lists**, per `(org, account, folder)`. Small (~50
//!   headers) and the thing you need to see a mailbox at all.
//! - **Message bodies you actually opened.** Capped, newest-first —
//!   caching every body would mean pulling whole mailboxes onto the
//!   device, which is the local-first design we explicitly did not
//!   choose.
//!
//! Reads fall back to the cache only when the live call *fails*. A
//! successful call always wins and refreshes the entry, so the cache
//! can never serve stale mail to an online client.
//!
//! Writes are not queued. Flagging or archiving while offline fails, as
//! it should: the server is authoritative, and silently accepting a
//! mutation we cannot deliver would show you a state your mailbox does
//! not have.
//!
//! Storage is per-platform because the platforms genuinely differ —
//! `localStorage` on web, a file under the OS cache dir on desktop.
//! Note the desktop arm is real here, unlike `prefs.rs`'s cache, which
//! no-ops off wasm; offline matters most on the laptop.

use email_proto::{Envelope, Message};

/// How many opened message bodies to keep. Bodies dominate the cache
/// size, so this is the knob that bounds it.
const MAX_BODIES: usize = 100;

fn accounts_key(slug: &str) -> String {
    format!("task.email.accounts.{slug}")
}

/// Remember the account list.
///
/// Easy to think this one is not worth caching — it is tiny and rarely
/// changes. But without it the whole offline path is dead: with no
/// accounts the page has nothing selected, so it never asks for
/// envelopes, so the cached listings are never read. Offline reading
/// silently did nothing until this was added.
pub fn put_accounts(slug: &str, accounts: &[email_proto::Account]) {
    if let Ok(json) = serde_json::to_string(accounts) {
        write(&accounts_key(slug), &json);
    }
}

pub fn get_accounts(slug: &str) -> Option<Vec<email_proto::Account>> {
    let json = read(&accounts_key(slug))?;
    serde_json::from_str(&json).ok()
}

fn envelopes_key(slug: &str, account: &str, folder: &str) -> String {
    format!("task.email.envs.{slug}.{account}.{folder}")
}

fn bodies_key(slug: &str, account: &str) -> String {
    format!("task.email.bodies.{slug}.{account}")
}

/// Store the listing just fetched.
pub fn put_envelopes(slug: &str, account: &str, folder: &str, envelopes: &[Envelope]) {
    if let Ok(json) = serde_json::to_string(envelopes) {
        write(&envelopes_key(slug, account, folder), &json);
    }
}

/// The last listing we saw for this folder, if any.
pub fn get_envelopes(slug: &str, account: &str, folder: &str) -> Option<Vec<Envelope>> {
    let json = read(&envelopes_key(slug, account, folder))?;
    serde_json::from_str(&json).ok()
}

/// Remember an opened message, evicting the oldest once over
/// [`MAX_BODIES`].
pub fn put_message(slug: &str, account: &str, message: &Message) {
    let key = bodies_key(slug, account);
    let mut bodies: Vec<Message> = read(&key)
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    let id = &message.envelope.message_id;
    // Re-reading a message moves it to the front rather than duplicating
    // it — most-recently-read is the eviction order we want.
    bodies.retain(|m| &m.envelope.message_id != id);
    bodies.insert(0, message.clone());
    bodies.truncate(MAX_BODIES);
    if let Ok(json) = serde_json::to_string(&bodies) {
        write(&key, &json);
    }
}

/// A message body we have read before.
pub fn get_message(slug: &str, account: &str, message_id: &str) -> Option<Message> {
    let json = read(&bodies_key(slug, account))?;
    let bodies: Vec<Message> = serde_json::from_str(&json).ok()?;
    bodies
        .into_iter()
        .find(|m| m.envelope.message_id == message_id)
}

// ── platform storage ────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

#[cfg(target_arch = "wasm32")]
fn read(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

#[cfg(target_arch = "wasm32")]
fn write(key: &str, value: &str) {
    // A quota failure is not worth surfacing — the consequence is
    // "no offline copy of this folder", not a broken page.
    if let Some(s) = storage() {
        let _ = s.set_item(key, value);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cache_path(key: &str) -> Option<std::path::PathBuf> {
    // Keys carry `.` separators and user-controlled folder names, so
    // flatten to one safe filename rather than letting a folder called
    // `../..` pick the directory.
    let safe: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    Some(dirs::cache_dir()?.join("task").join("email").join(safe))
}

#[cfg(not(target_arch = "wasm32"))]
fn read(key: &str) -> Option<String> {
    std::fs::read_to_string(cache_path(key)?).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn write(key: &str, value: &str) {
    let Some(path) = cache_path(key) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(id: &str) -> Envelope {
        Envelope {
            message_id: id.to_owned(),
            thread_id: None,
            folder: "INBOX".into(),
            subject: format!("subject {id}"),
            from: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            date_ms: 0,
            flags: Vec::new(),
            size: 0,
            has_attachments: false,
            snippet: None,
        }
    }

    fn msg(id: &str) -> Message {
        Message {
            envelope: env(id),
            headers_raw: String::new(),
            body_text: Some(format!("body {id}")),
            body_html: None,
            attachments: Vec::new(),
            references: Vec::new(),
        }
    }

    #[test]
    fn keys_are_scoped_by_org_account_and_folder() {
        // Two orgs, or two accounts, must never read each other's mail
        // out of the cache.
        assert_ne!(
            envelopes_key("orgA", "me@x.com", "INBOX"),
            envelopes_key("orgB", "me@x.com", "INBOX")
        );
        assert_ne!(
            envelopes_key("org", "a@x.com", "INBOX"),
            envelopes_key("org", "b@x.com", "INBOX")
        );
        assert_ne!(
            envelopes_key("org", "me@x.com", "INBOX"),
            envelopes_key("org", "me@x.com", "Archive")
        );
        assert_ne!(bodies_key("orgA", "me@x.com"), bodies_key("orgB", "me@x.com"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_hostile_folder_name_cannot_escape_the_cache_dir() {
        // Folder names come from the mail server. `../` in one must not
        // decide where we write.
        let path = cache_path(&envelopes_key("org", "acct", "../../etc/passwd")).unwrap();
        assert!(!path.to_string_lossy().contains(".."));
        assert!(path.parent().unwrap().ends_with("email"));
    }

    #[test]
    fn round_trips_envelopes_and_bodies() {
        let slug = format!("test-{}", std::process::id());
        put_envelopes(&slug, "acct", "INBOX", &[env("a"), env("b")]);
        let got = get_envelopes(&slug, "acct", "INBOX").expect("cached");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].message_id, "a");

        put_message(&slug, "acct", &msg("a"));
        assert_eq!(
            get_message(&slug, "acct", "a").unwrap().body_text.as_deref(),
            Some("body a")
        );
        assert!(get_message(&slug, "acct", "never").is_none());
    }

    #[test]
    fn reopening_a_message_does_not_duplicate_it() {
        let slug = format!("dup-{}", std::process::id());
        put_message(&slug, "acct", &msg("a"));
        put_message(&slug, "acct", &msg("b"));
        put_message(&slug, "acct", &msg("a"));
        let json = read(&bodies_key(&slug, "acct")).unwrap();
        let bodies: Vec<Message> = serde_json::from_str(&json).unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].envelope.message_id, "a", "most recent first");
    }

    #[test]
    fn body_cache_is_bounded() {
        let slug = format!("cap-{}", std::process::id());
        for i in 0..(MAX_BODIES + 20) {
            put_message(&slug, "acct", &msg(&format!("m{i}")));
        }
        let json = read(&bodies_key(&slug, "acct")).unwrap();
        let bodies: Vec<Message> = serde_json::from_str(&json).unwrap();
        assert_eq!(bodies.len(), MAX_BODIES);
        // The oldest are the ones gone.
        assert!(get_message(&slug, "acct", "m0").is_none());
        assert!(get_message(&slug, "acct", &format!("m{}", MAX_BODIES + 19)).is_some());
    }
}

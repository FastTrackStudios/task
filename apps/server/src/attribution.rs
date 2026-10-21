//! Who created an inbox item or a task (`attribution.*`): the server
//! half of `created_by`.
//!
//! Every inbox item and task records the principal behind the write
//! that created it — the person signed in, or the bot whose key made
//! the call (Atlas, Commander, …). The rules:
//!
//! - **From the credentials, never the payload.** Each write path
//!   clears whatever creator the incoming item or task carries and
//!   stamps the one its caller authenticated as ([`attribute_inbox`],
//!   [`attribute_task`]). Over the router that is the permission gate's
//!   caller, carried to the blocking thread by
//!   [`crate::activity::ActorDispatcher`]; over MCP the bearer's
//!   principal; on the watch bridge the device's account.
//! - **Write-once.** The backends keep the creator already on disk on
//!   every later write (`VaultInbox::upsert_inbox_item`,
//!   `TaskBackend::update`), so neither an edit nor a stale copy can
//!   re-attribute a record.
//! - **A ref and a name.** The ref is `user:<uuid>` or `bot:<uuid>`
//!   (`workflows_proto::creator`); the name is the account's display
//!   name at creation, so a surface can say "from Atlas" without a
//!   directory of its own. [`Creators`] resolves both from the org's
//!   auth store and the home org's (where bot accounts live).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use task::workflows_proto::creator;

/// The principal a write is attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Creator {
    /// `user:<uuid>` / `bot:<uuid>`.
    pub r#ref: String,
    /// Display name, when the account has one.
    pub name: Option<String>,
}

impl Creator {
    /// A person, by account id.
    #[must_use]
    pub fn user(id: &str, name: Option<String>) -> Self {
        Self {
            r#ref: creator::user_ref(id),
            name,
        }
    }

    /// A bot account, by account id.
    #[must_use]
    pub fn bot(id: &str, name: Option<String>) -> Self {
        Self {
            r#ref: creator::bot_ref(id),
            name,
        }
    }

    /// The creator an account is: a bot when its address is under
    /// [`crate::bots::BOT_DOMAIN`], else a person.
    #[must_use]
    pub fn of_account(user: &architect_auth::proto::AuthUser) -> Self {
        let id = user.id.to_string();
        let name = user
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| user.display_username.clone())
            .or_else(|| user.email.clone());
        let bot = user
            .email
            .as_deref()
            .is_some_and(|e| e.ends_with(&format!("@{}", crate::bots::BOT_DOMAIN)));
        if bot {
            Self::bot(&id, name)
        } else {
            Self::user(&id, name)
        }
    }
}

// t[impl attribution.server-stamped]
/// Attribute an inbox item to `creator` — and only to it: whatever the
/// item already says is dropped first, so a caller can't name someone
/// else. `None` (no principal behind the write) leaves no creator.
/// Takes effect on a create; on an item already stored the backend
/// keeps the creator it has.
pub fn attribute_inbox(item: &mut inbox_proto::InboxItem, creator: Option<&Creator>) {
    item.created_by = creator.map(|c| c.r#ref.clone());
    item.created_by_name = creator.and_then(|c| c.name.clone());
}

/// [`attribute_inbox`] for a task.
pub fn attribute_task(task: &mut task::TaskInfo, creator: Option<&Creator>) {
    task.created_by = creator.map(|c| c.r#ref.clone());
    task.created_by_name = creator.and_then(|c| c.name.clone());
}

/// Account id → [`Creator`], from the org's auth store and the home
/// org's. Cached; a miss reloads both stores (at most once a second),
/// so a bot made a moment ago is named on its first write.
#[derive(Clone)]
pub struct Creators {
    org: crate::AuthState,
    home: Option<crate::AuthState>,
    cache: Arc<RwLock<Cache>>,
}

#[derive(Default)]
struct Cache {
    by_id: HashMap<String, Creator>,
    loaded: Option<Instant>,
}

impl Creators {
    /// How long a loaded directory answers before the next write
    /// reloads it (names change rarely; a rename shows on records made
    /// after it).
    const TTL: Duration = Duration::from_secs(300);
    /// The least time between two reloads on a miss.
    const MISS_BACKOFF: Duration = Duration::from_secs(1);

    #[must_use]
    pub fn new(org: crate::AuthState, home: Option<crate::AuthState>) -> Self {
        Self {
            org,
            home,
            cache: Arc::default(),
        }
    }

    /// Who `user_id` is. An id neither store knows is still a person —
    /// a central-auth account, say — with no name.
    pub async fn resolve(&self, user_id: &str) -> Creator {
        let (hit, stale, recent) = {
            let c = self.cache.read().expect("creators lock");
            let age = c.loaded.map(|at| at.elapsed());
            (
                c.by_id.get(user_id).cloned(),
                age.is_none_or(|a| a >= Self::TTL),
                age.is_some_and(|a| a < Self::MISS_BACKOFF),
            )
        };
        match hit {
            Some(c) if !stale => return c,
            None if recent => return Creator::user(user_id, None),
            _ => {}
        }
        self.reload().await;
        self.cache
            .read()
            .expect("creators lock")
            .by_id
            .get(user_id)
            .cloned()
            .or(hit)
            .unwrap_or_else(|| Creator::user(user_id, None))
    }

    async fn reload(&self) {
        let mut by_id = HashMap::new();
        for store in std::iter::once(&self.org).chain(self.home.as_ref()) {
            match store.auth.list_users_local_trusted().await {
                Ok(users) => {
                    for u in &users {
                        by_id
                            .entry(u.id.to_string())
                            .or_insert_with(|| Creator::of_account(u));
                    }
                }
                Err(e) => tracing::warn!(error = ?e, "attribution: list accounts failed"),
            }
        }
        let mut c = self.cache.write().expect("creators lock");
        c.by_id = by_id;
        c.loaded = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // t[verify attribution.server-stamped]
    #[test]
    fn a_stamp_replaces_whatever_the_payload_claims() {
        let atlas = Creator::bot("b1", Some("Atlas".into()));
        let mut item = inbox_proto::InboxItem::capture("i", "x", "mcp", "2026-10-20T00:00:00Z");
        item.created_by = Some("user:the-captain".into());
        item.created_by_name = Some("The Captain".into());
        attribute_inbox(&mut item, Some(&atlas));
        assert_eq!(item.created_by.as_deref(), Some("bot:b1"));
        assert_eq!(item.created_by_name.as_deref(), Some("Atlas"));

        // No principal: no creator, not the claimed one.
        let mut t = task::capture("y");
        t.created_by = Some("user:the-captain".into());
        attribute_task(&mut t, None);
        assert_eq!(t.created_by, None);
        assert_eq!(t.created_by_name, None);
    }
}

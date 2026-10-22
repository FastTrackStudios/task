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
//! - **On behalf of** (`attribution.on-behalf-of`). A *personal* bot —
//!   an assistant working for one person — also stamps whom it acted
//!   for: its key's owner ([`crate::bots::acts_for`]), as `user:<uuid>`
//!   plus a name, so a record reads "from Atlas (for Cody)". A team bot
//!   ([`crate::bots::Kind::Team`]) and a person stamp none. The value
//!   comes from the key the server issued, never from what a caller
//!   sends, and is write-once with the creator.

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
    /// The person a personal bot acts for (its key's owner); `None`
    /// for a person, and for a team bot.
    pub on_behalf_of: Option<Box<Creator>>,
}

impl Creator {
    /// A person, by account id.
    #[must_use]
    pub fn user(id: &str, name: Option<String>) -> Self {
        Self {
            r#ref: creator::user_ref(id),
            name,
            on_behalf_of: None,
        }
    }

    /// A bot account, by account id.
    #[must_use]
    pub fn bot(id: &str, name: Option<String>) -> Self {
        Self {
            r#ref: creator::bot_ref(id),
            name,
            on_behalf_of: None,
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

    /// This bot, acting for `person` — what a personal bot's writes are
    /// attributed to. A person acts for nobody but themselves, so on a
    /// person's creator this is a no-op.
    #[must_use]
    pub fn acting_for(mut self, person: Creator) -> Self {
        if creator::is_bot(&self.r#ref) {
            self.on_behalf_of = Some(Box::new(Creator {
                on_behalf_of: None,
                ..person
            }));
        }
        self
    }

    fn obo(&self) -> (Option<String>, Option<String>) {
        match &self.on_behalf_of {
            Some(p) => (Some(p.r#ref.clone()), p.name.clone()),
            None => (None, None),
        }
    }
}

// t[impl attribution.server-stamped]
/// Attribute an inbox item to `creator` — and only to it: whatever the
/// item already says is dropped first, so a caller can't name someone
/// else. `None` (no principal behind the write) leaves no creator.
/// Takes effect on a create; on an item already stored the backend
/// keeps the creator it has.
///
/// Whom a personal bot acted for is stamped the same way, from the
/// creator alone ([`Creator::on_behalf_of`]) — never kept from the item.
// t[impl attribution.on-behalf-of.server-stamped]
pub fn attribute_inbox(item: &mut inbox_proto::InboxItem, creator: Option<&Creator>) {
    item.created_by = creator.map(|c| c.r#ref.clone());
    item.created_by_name = creator.and_then(|c| c.name.clone());
    (item.on_behalf_of, item.on_behalf_of_name) = creator.map(Creator::obo).unwrap_or_default();
}

/// [`attribute_inbox`] for a task.
pub fn attribute_task(task: &mut task::TaskInfo, creator: Option<&Creator>) {
    task.created_by = creator.map(|c| c.r#ref.clone());
    task.created_by_name = creator.and_then(|c| c.name.clone());
    (task.on_behalf_of, task.on_behalf_of_name) = creator.map(Creator::obo).unwrap_or_default();
}

/// Account id → [`Creator`], from the org's auth store and the home
/// org's. Cached; a miss reloads both stores (at most once a second),
/// so a bot made a moment ago is named on its first write.
///
/// A bot account in the home store is resolved with whom it acts for
/// ([`crate::bots::acts_for`]): a personal bot's creator carries its
/// owner. Marking a bot personal or team shows within [`Self::TTL`].
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
        let mut by_id: HashMap<String, Creator> = HashMap::new();
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
        // Bot accounts (only ever in the home store) and whom each acts
        // for: the owner on its key, when the key marks it personal.
        if let Some(home) = &self.home {
            let bots: Vec<(String, uuid::Uuid)> = by_id
                .iter()
                .filter(|(_, c)| creator::is_bot(&c.r#ref))
                .filter_map(|(id, _)| Some((id.clone(), id.parse().ok()?)))
                .collect();
            for (id, bot) in bots {
                let Some(owner) = crate::bots::acts_for(home, bot).await else {
                    continue;
                };
                let person = by_id
                    .get(&owner.to_string())
                    .cloned()
                    .unwrap_or_else(|| Creator::user(&owner.to_string(), None));
                if let Some(c) = by_id.remove(&id) {
                    by_id.insert(id, c.acting_for(person));
                }
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

    // t[verify attribution.on-behalf-of.server-stamped]
    #[test]
    fn whom_a_bot_acted_for_comes_from_the_creator_alone() {
        let cody = Creator::user("c1", Some("Cody".into()));
        let atlas = Creator::bot("b1", Some("Atlas".into())).acting_for(cody.clone());
        let team = Creator::bot("b2", Some("Commander".into()));

        // A personal bot: its owner, whatever the item claims.
        let mut item = inbox_proto::InboxItem::capture("i", "x", "mcp", "2026-10-21T00:00:00Z");
        item.on_behalf_of = Some("user:mallory".into());
        item.on_behalf_of_name = Some("Mallory".into());
        attribute_inbox(&mut item, Some(&atlas));
        assert_eq!(item.created_by.as_deref(), Some("bot:b1"));
        assert_eq!(item.on_behalf_of.as_deref(), Some("user:c1"));
        assert_eq!(item.on_behalf_of_name.as_deref(), Some("Cody"));

        // A team bot, and a person: nobody, not the claimed one.
        for who in [&team, &cody] {
            let mut t = task::capture("y");
            t.on_behalf_of = Some("user:mallory".into());
            t.on_behalf_of_name = Some("Mallory".into());
            attribute_task(&mut t, Some(who));
            assert_eq!(t.on_behalf_of, None, "{who:?}");
            assert_eq!(t.on_behalf_of_name, None);
        }
        // A person never acts "for" someone else.
        assert_eq!(cody.acting_for(team).on_behalf_of, None);
    }
}

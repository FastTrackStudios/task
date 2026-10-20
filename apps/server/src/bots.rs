//! Bots: agents with an account of their own, in the orgs their person
//! chose, holding a key rather than a session.
//!
//! A person runs assistants — Hermes bots, say: Atlas over their own
//! life, a Librarian over every org's notes and wikis. Each needs to
//! reach Task over MCP as *itself*, so what it writes is attributed to
//! it, and to reach only what it was given. So a bot is:
//!
//! - **an account** in this server's home auth store, with an address
//!   under [`BOT_DOMAIN`] and a password nobody is ever told;
//! - **membership rows** for exactly the orgs it was given — the same
//!   fence every cross-org principal passes (`HomeFallbackResolver`, the
//!   MCP account lane), so a bot reaches nothing more;
//! - **an API key** (`ak_…`, architect-auth's), the credential pasted
//!   into the bot's config. Its `permissions_json` carries the bot's
//!   [`Tools`]; its `metadata_json` the person who made it.
//!
//! A person can only give a bot orgs they are in themselves, and a bot
//! is a `member`, never an admin. Revoking deletes the account, which
//! takes its key with it, and its rows.

// t[impl mcp.bots]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AuthState;
use crate::memberships::Memberships;

/// The address space bot accounts live in. `.invalid` is reserved
/// (RFC 2606): no mail can ever reach, or come from, one.
pub const BOT_DOMAIN: &str = "bots.task.invalid";

/// The role a bot holds in each org it is given.
pub const BOT_ROLE: &str = "member";

/// Which of Task's tools a bot is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tools {
    /// Everything the org's plugins offer.
    #[default]
    All,
    /// Notes and wikis — the vault and the wiki lane, read and write —
    /// and the orientation tools. No tasks, calendar, email or money.
    Library,
}

impl Tools {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "all" | "full" => Some(Self::All),
            "library" | "wiki" | "notes" => Some(Self::Library),
            _ => None,
        }
    }

    /// Whether a tool (by its MCP name) is offered under this scope.
    #[must_use]
    pub fn allows(self, tool: &str) -> bool {
        match self {
            Self::All => true,
            Self::Library => {
                const ORIENTATION: &[&str] = &["task_context", "api_reference", "list_orgs"];
                ORIENTATION.contains(&tool)
                    || tool.contains("wiki")
                    || tool.contains("note")
                    || tool.contains("vault")
                    || tool.starts_with("search")
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct KeyPermissions {
    tools: Tools,
}

#[derive(Serialize, Deserialize)]
struct KeyMetadata {
    bot: bool,
    owner: Uuid,
}

/// A bot as listed: who, given what, by whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bot {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub owner: Uuid,
    pub tools: Tools,
    pub orgs: Vec<String>,
}

/// A bot just made, with the one time its key is ever shown.
#[derive(Clone, Debug)]
pub struct NewBot {
    pub bot: Bot,
    pub key: String,
}

/// The bot behind a presented key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BotPrincipal {
    pub user_id: Uuid,
    pub tools: Tools,
}

fn slugify(name: &str) -> String {
    let s: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "bot".to_owned() } else { s }
}

fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Make a bot for `owner`, a member of `orgs`, offered `tools`.
///
/// # Errors
///
/// An org `owner` is not a member of; a name already taken; the auth
/// store or membership table refusing.
pub async fn create(
    home: &AuthState,
    memberships: &Memberships,
    owner: Uuid,
    name: &str,
    orgs: &[String],
    tools: Tools,
) -> eyre::Result<NewBot> {
    if orgs.is_empty() {
        eyre::bail!("a bot needs at least one org");
    }
    let mine: Vec<String> = memberships
        .for_user(owner)
        .await?
        .into_iter()
        .map(|m| m.org_slug)
        .collect();
    for slug in orgs {
        if !mine.contains(slug) {
            eyre::bail!("you are not a member of `{slug}`, so you cannot give it to a bot");
        }
    }
    let email = format!("{}@{BOT_DOMAIN}", slugify(name));
    if home
        .auth
        .find_user_by_email(&email)
        .await
        .map_err(|e| eyre::eyre!("look up {email}: {e:?}"))?
        .is_some()
    {
        eyre::bail!("there is already a bot called `{name}`");
    }
    let session = home
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: email.clone(),
            password: random_secret(),
            name: Some(name.trim().to_owned()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: Some("task bots".into()),
        })
        .await
        .map_err(|e| eyre::eyre!("create the bot's account: {e:?}"))?;
    let bot_id = session.user.id;
    let key = home
        .auth
        .create_api_key(architect_auth::CreateApiKey {
            session_token: session.token.clone(),
            name: Some(name.trim().to_owned()),
            expires_at: None,
            permissions_json: Some(serde_json::to_string(&KeyPermissions { tools })?),
            rate_limit_time_window: None,
            rate_limit_max: None,
            metadata_json: Some(serde_json::to_string(&KeyMetadata { bot: true, owner })?),
        })
        .await
        .map_err(|e| eyre::eyre!("issue the bot's key: {e:?}"))?;
    // The session was only the way to the key; it is not a credential
    // the bot holds.
    let _ = home
        .auth
        .sign_out(architect_auth::SignOut {
            token: session.token,
        })
        .await;
    for slug in orgs {
        memberships.upsert(bot_id, slug, Some(BOT_ROLE)).await?;
    }
    Ok(NewBot {
        bot: Bot {
            id: bot_id,
            name: name.trim().to_owned(),
            email,
            owner,
            tools,
            orgs: orgs.to_vec(),
        },
        key: key.key,
    })
}

/// The bot a key belongs to, if it is a live bot key.
pub async fn resolve(home: &AuthState, token: &str) -> Option<BotPrincipal> {
    if !token.starts_with("ak_") {
        return None;
    }
    let bundle = home
        .auth
        .verify_api_key(architect_auth::VerifyApiKey {
            key: token.to_owned(),
            permission: None,
        })
        .await
        .ok()?;
    if !bundle
        .user
        .email
        .as_deref()
        .is_some_and(|e| e.ends_with(&format!("@{BOT_DOMAIN}")))
    {
        return None;
    }
    let tools = bundle
        .api_key
        .permissions_json
        .as_deref()
        .and_then(|p| serde_json::from_str::<KeyPermissions>(p).ok())
        .map(|p| p.tools)
        .unwrap_or_default();
    Some(BotPrincipal {
        user_id: bundle.user.id,
        tools,
    })
}

/// The bots `owner` made.
pub async fn list(
    home: &AuthState,
    memberships: &Memberships,
    owner: Uuid,
) -> eyre::Result<Vec<Bot>> {
    use architect_auth::AuthStorage as _;
    // Every bot holds at least one row, so the rows name every candidate.
    let mut ids: Vec<Uuid> = memberships
        .all()
        .await?
        .into_iter()
        .map(|(id, ..)| id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut bots = Vec::new();
    for id in ids {
        let Ok(Some(user)) = home.auth.storage.find_user_by_id(id).await else {
            continue;
        };
        let email = user.email.clone().unwrap_or_default();
        if !email.ends_with(&format!("@{BOT_DOMAIN}")) {
            continue;
        }
        let Ok(keys) = home.auth.storage.list_api_keys_by_user_id(id).await else {
            continue;
        };
        let Some((key, meta)) = keys.into_iter().find_map(|k| {
            let meta = k
                .metadata_json
                .as_deref()
                .and_then(|m| serde_json::from_str::<KeyMetadata>(m).ok())
                .filter(|m| m.bot && m.owner == owner)?;
            Some((k, meta))
        }) else {
            continue;
        };
        let tools = key
            .permissions_json
            .as_deref()
            .and_then(|p| serde_json::from_str::<KeyPermissions>(p).ok())
            .map(|p| p.tools)
            .unwrap_or_default();
        let orgs = memberships
            .for_user(id)
            .await?
            .into_iter()
            .map(|m| m.org_slug)
            .collect();
        bots.push(Bot {
            id,
            name: user.name.clone().unwrap_or_else(|| email.clone()),
            email,
            owner: meta.owner,
            tools,
            orgs,
        });
    }
    bots.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(bots)
}

/// Remove a bot `owner` made: its rows, then its account (and with it
/// its key). The bot's past writes keep its name.
pub async fn revoke(
    home: &AuthState,
    memberships: &Memberships,
    owner: Uuid,
    bot: Uuid,
) -> eyre::Result<()> {
    let Some(found) = list(home, memberships, owner)
        .await?
        .into_iter()
        .find(|b| b.id == bot)
    else {
        eyre::bail!("no bot {bot} of yours here");
    };
    for slug in &found.orgs {
        memberships.revoke(found.id, slug).await?;
    }
    home.auth
        .delete_user_local_trusted(found.id)
        .await
        .map_err(|e| eyre::eyre!("delete the bot's account: {e:?}"))?;
    Ok(())
}

/// [`org_proto::BotService`] over this server's home identity: the
/// app's Settings → Bots, on `/server/vox`.
#[derive(Clone, architect::HasDispatcher)]
pub struct BotServiceImpl {
    state: std::sync::Arc<crate::AppState>,
}

impl BotServiceImpl {
    #[must_use]
    pub fn new(state: crate::AppState) -> Self {
        Self {
            state: std::sync::Arc::new(state),
        }
    }

    /// The person behind `token`, and the home identity bots live in.
    fn caller(&self, token: &str) -> Result<(Uuid, crate::HomeIdentity), org_proto::BotError> {
        use org_proto::BotError;
        let home = self
            .state
            .home_identity
            .clone()
            .ok_or_else(|| BotError::Refused("this server has no home org to keep bots in".into()))?;
        let state = self.state.clone();
        let token = token.to_owned();
        let who = tokio::runtime::Handle::current()
            .block_on(async move { crate::central_auth::home_principal(&state, &token).await })
            .ok_or_else(|| BotError::Unauthorized("sign in to manage bots".into()))?;
        Ok((who, home))
    }
}

fn wire(bot: Bot) -> org_proto::BotInfo {
    org_proto::BotInfo {
        id: bot.id.to_string(),
        name: bot.name,
        tools: match bot.tools {
            Tools::All => org_proto::BotTools::All,
            Tools::Library => org_proto::BotTools::Library,
        },
        orgs: bot.orgs,
    }
}

impl org_proto::BotService for BotServiceImpl {
    fn list_bots(&self, session_token: String) -> Result<Vec<org_proto::BotInfo>, org_proto::BotError> {
        let (owner, home) = self.caller(&session_token)?;
        tokio::runtime::Handle::current()
            .block_on(async move { list(&home.auth, &home.memberships, owner).await })
            .map(|bots| bots.into_iter().map(wire).collect())
            .map_err(|e| org_proto::BotError::Internal(format!("{e:#}")))
    }

    fn create_bot(
        &self,
        req: org_proto::CreateBotRequest,
    ) -> Result<org_proto::CreatedBot, org_proto::BotError> {
        let (owner, home) = self.caller(&req.session_token)?;
        let tools = match req.tools {
            org_proto::BotTools::All => Tools::All,
            org_proto::BotTools::Library => Tools::Library,
        };
        let made = tokio::runtime::Handle::current()
            .block_on(async move {
                create(&home.auth, &home.memberships, owner, &req.name, &req.orgs, tools).await
            })
            // What `create` refuses is the request, not the server.
            .map_err(|e| org_proto::BotError::Refused(format!("{e:#}")))?;
        Ok(org_proto::CreatedBot {
            bot: wire(made.bot),
            key: made.key,
        })
    }

    fn revoke_bot(&self, req: org_proto::RevokeBotRequest) -> Result<(), org_proto::BotError> {
        let (owner, home) = self.caller(&req.session_token)?;
        let bot = req
            .bot_id
            .parse::<Uuid>()
            .map_err(|_| org_proto::BotError::Refused("not a bot id".into()))?;
        tokio::runtime::Handle::current()
            .block_on(async move { revoke(&home.auth, &home.memberships, owner, bot).await })
            .map_err(|e| org_proto::BotError::Refused(format!("{e:#}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_bot_sees_notes_and_wikis_only() {
        assert!(Tools::Library.allows("read_wiki_page"));
        assert!(Tools::Library.allows("task_context"));
        assert!(!Tools::Library.allows("create_task"));
        assert!(!Tools::Library.allows("send_email"));
        assert!(Tools::All.allows("create_task"));
    }

    #[test]
    fn names_become_addresses_that_cannot_receive_mail() {
        assert_eq!(slugify("  The Librarian! "), "the-librarian");
        assert_eq!(slugify("???"), "bot");
        assert_eq!(Tools::parse("wiki"), Some(Tools::Library));
    }
}

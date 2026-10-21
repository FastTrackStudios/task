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
//!
//! ## A bot with no orgs
//!
//! A bot may be made with **no** orgs, but only on purpose
//! ([`OrgChoice::None`], `admin bot create --no-orgs`): an empty list
//! by accident is still refused. Such a bot is a live key that reaches
//! nothing — refused on every `/org/{slug}/mcp`, an empty `list_orgs`
//! on the account lane — until an operator grants it an org with
//! `task-server admin grant --org <slug> --principal <bot id>`. That
//! row is `local`, so no issuer sync ever takes it away; the issuer
//! could not grant it in the first place, since a `@`[`BOT_DOMAIN`]
//! account exists only in this server's home store.
//!
//! Because such a bot holds no membership rows, bots are found by
//! **account and key** ([`list`]): a home-store user under
//! [`BOT_DOMAIN`] holding a key whose metadata says `bot` and names its
//! owner. Rows only add the orgs. Scoping is by owner and nothing
//! else: a caller sees, and may revoke, exactly the bots whose key
//! names them as owner, with or without orgs. A zero-org bot is
//! therefore visible to the person (or the `--owner` uuid) that made it
//! and to nobody else; an operator reaches anyone's bots only by naming
//! that owner with `--owner`, which needs the data root anyway.

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
    /// It may watch a live set (`live_observe`, `doc_read`) but not edit
    /// one: a library key is an observer there (spec §7 S1).
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
                // Reads of the notes that don't say `note` / `vault`.
                const NOTES: &[&str] = &["query_base"];
                // Watching a live set, never editing it.
                const LIVE_WATCH: &[&str] = &["live_observe", "doc_read"];
                ORIENTATION.contains(&tool)
                    || NOTES.contains(&tool)
                    || LIVE_WATCH.contains(&tool)
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

/// Which orgs a new bot is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrgChoice {
    /// These, each of which the owner must be a member of. Never empty.
    Some(Vec<String>),
    /// None at all, chosen explicitly: an operator grants orgs later
    /// (`admin grant --principal <bot id>`).
    None,
}

impl OrgChoice {
    /// From the two ways a caller can say it: a list, and a "no orgs"
    /// switch. Exactly one of them must say something.
    ///
    /// # Errors
    ///
    /// Both given, or neither.
    pub fn from_request(orgs: Vec<String>, no_orgs: bool) -> eyre::Result<Self> {
        match (orgs.is_empty(), no_orgs) {
            (true, true) => Ok(Self::None),
            (false, false) => Ok(Self::Some(orgs)),
            (false, true) => eyre::bail!("give a bot orgs or no orgs, not both"),
            (true, false) => eyre::bail!(
                "a bot needs at least one org — or make it with no orgs on purpose \
                 (`--no-orgs`) and grant them later"
            ),
        }
    }

    fn slugs(&self) -> &[String] {
        match self {
            Self::Some(orgs) => orgs,
            Self::None => &[],
        }
    }
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

/// Make a bot for `owner`, a member of `orgs` (or of none, when that
/// was chosen), offered `tools`.
///
/// # Errors
///
/// An empty [`OrgChoice::Some`]; an org `owner` is not a member of; a
/// name already taken; the auth store or membership table refusing.
pub async fn create(
    home: &AuthState,
    memberships: &Memberships,
    owner: Uuid,
    name: &str,
    orgs: &OrgChoice,
    tools: Tools,
) -> eyre::Result<NewBot> {
    if matches!(orgs, OrgChoice::Some(list) if list.is_empty()) {
        eyre::bail!("a bot needs at least one org");
    }
    let orgs = orgs.slugs();
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

/// The bots `owner` made, with the orgs each may reach. An empty org
/// list is a valid answer.
///
/// Found by account and key, not by membership rows: a bot made with
/// no orgs has no rows, and a bot missing from this list would be a
/// live key nobody could see or revoke.
pub async fn list(
    home: &AuthState,
    memberships: &Memberships,
    owner: Uuid,
) -> eyre::Result<Vec<Bot>> {
    use architect_auth::AuthStorage as _;
    const PAGE: usize = 500;
    let suffix = format!("@{BOT_DOMAIN}");
    let mut accounts = Vec::new();
    let mut offset = 0;
    loop {
        let (page, total) = home
            .auth
            .storage
            .list_users(offset, PAGE)
            .await
            .map_err(|e| eyre::eyre!("list the home store's accounts: {e:?}"))?;
        let got = page.len();
        accounts.extend(
            page.into_iter()
                .filter(|u| u.email.as_deref().is_some_and(|e| e.ends_with(&suffix))),
        );
        offset += got;
        if got == 0 || offset >= total {
            break;
        }
    }
    let mut bots = Vec::new();
    for user in accounts {
        let id = user.id;
        let email = user.email.clone().unwrap_or_default();
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
        let home = self.state.home_identity.clone().ok_or_else(|| {
            BotError::Refused("this server has no home org to keep bots in".into())
        })?;
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
    fn list_bots(
        &self,
        session_token: String,
    ) -> Result<Vec<org_proto::BotInfo>, org_proto::BotError> {
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
        let orgs = OrgChoice::from_request(req.orgs, req.no_orgs)
            .map_err(|e| org_proto::BotError::Refused(format!("{e:#}")))?;
        let made = tokio::runtime::Handle::current()
            .block_on(async move {
                create(
                    &home.auth,
                    &home.memberships,
                    owner,
                    &req.name,
                    &orgs,
                    tools,
                )
                .await
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
        assert!(Tools::Library.allows("query_base"));
        assert!(Tools::Library.allows("live_observe"));
        assert!(Tools::Library.allows("doc_read"));
        assert!(
            !Tools::Library.allows("doc_apply"),
            "a library key only observes"
        );
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

    #[test]
    fn no_orgs_is_said_on_purpose_and_never_with_orgs() {
        let one = vec!["acme-audio".to_owned()];
        assert_eq!(
            OrgChoice::from_request(one.clone(), false).unwrap(),
            OrgChoice::Some(one.clone())
        );
        assert_eq!(
            OrgChoice::from_request(Vec::new(), true).unwrap(),
            OrgChoice::None
        );
        assert!(
            OrgChoice::from_request(Vec::new(), false).is_err(),
            "an empty list by accident is still refused"
        );
        assert!(OrgChoice::from_request(one, true).is_err(), "not both");
    }
}

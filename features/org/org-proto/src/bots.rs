//! A person's bots, managed from the app.
//!
//! [`BotService`] runs at `/server/vox` beside [`crate::OrgManagementService`]:
//! bots are accounts in the server's home auth store, so they belong to
//! the server rather than to any one org. Every call carries the caller's
//! token (a home-org session or an issuer access token) and acts for that
//! person only — the bots they made, the orgs they are in. See the server's
//! `bots` module and `docs/spec/agents.md` (`mcp.bots`).

use facet::Facet;
use thiserror::Error;

/// Which of Task's tools a bot is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum BotTools {
    /// Everything the org's plugins offer.
    All,
    /// Notes, wikis and search, and nothing about tasks, calendar,
    /// email or money.
    Library,
}

/// A bot, as its person sees it.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
pub struct BotInfo {
    pub id: String,
    pub name: String,
    pub tools: BotTools,
    /// The org slugs it may reach.
    pub orgs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
pub struct CreateBotRequest {
    pub session_token: String,
    pub name: String,
    /// Orgs the caller is a member of. Empty only with `no_orgs`.
    pub orgs: Vec<String>,
    /// Make the bot with no orgs at all, on purpose — an operator
    /// grants them later (`task-server admin grant --principal <bot
    /// id>`). Refused together with a non-empty `orgs`; an empty
    /// `orgs` without it is refused too.
    pub no_orgs: bool,
    pub tools: BotTools,
}

/// A bot just made, with the only time its key is ever shown.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
pub struct CreatedBot {
    pub bot: BotInfo,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
pub struct RevokeBotRequest {
    pub session_token: String,
    pub bot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum BotError {
    /// The token names nobody on this server.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// The request cannot be honoured as asked (an org the caller is
    /// not in, a name already taken, no orgs).
    #[error("{0}")]
    Refused(String),
    #[error("internal: {0}")]
    Internal(String),
}

/// A person's bots on this server.
#[architect::rpc]
pub trait BotService {
    /// The bots the caller made, with the orgs each may reach (possibly
    /// none).
    fn list_bots(&self, session_token: String) -> Result<Vec<BotInfo>, BotError>;

    /// Make a bot: an account in the orgs given (only ones the caller is
    /// in), offered `tools`, and its key — returned here and never again.
    fn create_bot(&self, req: CreateBotRequest) -> Result<CreatedBot, BotError>;

    /// Remove one of the caller's bots; its key stops working.
    fn revoke_bot(&self, req: RevokeBotRequest) -> Result<(), BotError>;
}

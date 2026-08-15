//! On-disk layout for backends that persist state. Modeled
//! on Hermes's `~/.hermes/webui/` directory tree.
//!
//! ```text
//! <state>/agent/
//! ├── backends.json        ← registered AgentBackend list
//! ├── profiles/<id>/
//! │   ├── config.json      ← Profile manifest
//! │   ├── personalities/   ← per-personality system prompts
//! │   └── secrets.enc      ← API keys (backend-encrypted)
//! ├── projects.json        ← registered Project list
//! ├── sessions/
//! │   └── <session-id>.json
//! ├── messages/
//! │   └── <session-id>/<message-id>.json
//! ├── attachments/
//! │   └── <sha256>          (file content)
//! ├── tools/
//! │   └── <session-id>/<tool-call-id>.json
//! ├── approvals/
//! │   └── <session-id>/<approval-id>.json
//! ├── questions/
//! │   └── <session-id>/<request-id>.json
//! ├── boards/
//! │   ├── <board-id>.json
//! │   ├── cards/<card-id>.json
//! │   ├── links.json
//! │   └── comments/<card-id>.json
//! └── run_journal.sqlite    ← SSE replay journal (crash recovery)
//! ```

pub const AGENT_ROOT: &str = "agent";
pub const BACKENDS_JSON: &str = "backends.json";

pub const PROFILES_DIR: &str = "profiles";
pub const PROFILE_CONFIG_JSON: &str = "config.json";
pub const PERSONALITIES_DIR: &str = "personalities";
pub const SECRETS_ENC: &str = "secrets.enc";

pub const PROJECTS_JSON: &str = "projects.json";
pub const SESSIONS_DIR: &str = "sessions";
pub const MESSAGES_DIR: &str = "messages";
pub const ATTACHMENTS_DIR: &str = "attachments";
pub const TOOLS_DIR: &str = "tools";
pub const APPROVALS_DIR: &str = "approvals";
pub const QUESTIONS_DIR: &str = "questions";

pub const BOARDS_DIR: &str = "boards";
pub const BOARD_CARDS_DIR: &str = "cards";
pub const BOARD_LINKS_JSON: &str = "links.json";
pub const BOARD_COMMENTS_DIR: &str = "comments";

pub const RUN_JOURNAL_SQLITE: &str = "run_journal.sqlite";

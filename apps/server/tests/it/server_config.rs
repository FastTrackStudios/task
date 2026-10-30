//! Golden test for `task_server::config::ServerConfig` (STA-401): every
//! environment variable the server read before the `derive(Config)`
//! move is still read under the same name, into the field the server
//! uses, with the same default.
//!
//! The table below is the contract. Renaming a variable, dropping one
//! or changing a default fails here first; a deployment's environment
//! keeps working only while this passes.

use task_server::config::ServerConfig;

use crate::support;

/// Every variable, in `ServerConfig` field order. Changing a name here
/// is a breaking change for every deployment.
const NAMES: &[&str] = &[
    "TASK_SERVER_BIND",
    "TASK_SERVER_ORG",
    "TASK_SERVER_WEB_DIR",
    "TASK_BUILD_REV",
    "TASK_AUTH_SECRET",
    "TASK_CORS_ALLOWED_ORIGINS",
    "TASK_ENFORCE_PERMISSIONS",
    "TASK_ENFORCE_MEDIA_TOKEN",
    "TASK_SERVER_PUBLIC_URL",
    "TASK_SHARE_PUBLIC_BASE",
    "TASK_SERVER_VAULT_ROOT",
    "TASK_SERVER_WIKI_ROOT",
    "TASK_SERVER_CRDT_ROOT",
    "TASK_SERVER_MAIL_ROOT",
    "TASK_SERVER_AGENT_TASKS_URL",
    "TASK_SERVER_AGENT_RUNNERS_URL",
    "TASK_SERVER_TIMER_URL",
    "TASK_SERVER_THREADS_URL",
    "TASK_SERVER_NOTIFY_URL",
    "TASK_SERVER_PREFS_URL",
    "TASK_SERVER_FINANCE_URL",
    "TASK_WIKI_DOMAINS",
    "TASK_WIKI_CLAIM_TTL_SECS",
    "TASK_WIKI_REPO_SYNC_SECS",
    "TASK_DEVICE_SYNC_SECS",
    "TASK_DEVICE_SYNC_ADOPT",
    "TASK_FILES_COMPACT_OPS",
    "TASK_FORGEJO_BASE_URL",
    "TASK_FORGEJO_TOKEN",
    "FORGEJO_TOKEN",
    "TASK_FORGEJO_BOT_TOKEN",
    "FTS_CODEBERG_ACCESS_TOKEN",
    "TASK_ESV_API_KEY",
    "ESV_API_KEY",
    "TASK_API_BIBLE_KEY",
    "API_BIBLE_KEY",
    "TASK_API_BIBLE_NIV_ID",
    "TASK_LOG_CONSOLE",
];

/// The value the test sets each variable to: its own name, so a field
/// reading the wrong variable shows which one it read.
fn sentinel(name: &str) -> Option<String> {
    Some(format!("{name}=set"))
}

fn clear_all() {
    for name in NAMES {
        // SAFETY: under the binary's one env lock (`support::env_lock`).
        unsafe { std::env::remove_var(name) };
    }
}

#[tokio::test]
async fn every_name_and_default_is_unchanged() {
    let _env = support::env_lock().await;
    clear_all();

    // An empty environment is the defaults, and the defaults are the
    // ones the server had: every override unset, the build rev
    // "unknown", the listener on 0.0.0.0:9090 and share links on
    // 127.0.0.1:3456.
    let empty = ServerConfig::from_env().expect("an empty environment reads");
    assert_eq!(empty, ServerConfig::default());
    assert_eq!(empty.build_rev, "unknown");
    assert_eq!(empty.listen_bind(), "0.0.0.0:9090");
    assert_eq!(empty.share_bind(), "127.0.0.1:3456");
    assert_eq!(empty.forgejo_token(), "");
    assert!(!empty.enforces_permissions());
    assert!(!empty.enforces_media_token());
    assert!(empty.compacts_op_logs());
    assert!(!empty.adopts_offered_roots());

    // Every variable set: each lands in its own field, by name.
    for name in NAMES {
        let value = sentinel(name).expect("sentinel");
        // SAFETY: under the env lock.
        unsafe { std::env::set_var(name, value) };
    }
    let set = ServerConfig::from_env().expect("a full environment reads");
    clear_all();
    let expected = ServerConfig {
        bind: sentinel("TASK_SERVER_BIND"),
        org: sentinel("TASK_SERVER_ORG"),
        web_dir: sentinel("TASK_SERVER_WEB_DIR"),
        build_rev: "TASK_BUILD_REV=set".to_owned(),
        auth_secret: sentinel("TASK_AUTH_SECRET"),
        cors_allowed_origins: sentinel("TASK_CORS_ALLOWED_ORIGINS"),
        enforce_permissions: sentinel("TASK_ENFORCE_PERMISSIONS"),
        enforce_media_token: sentinel("TASK_ENFORCE_MEDIA_TOKEN"),
        public_url: sentinel("TASK_SERVER_PUBLIC_URL"),
        share_public_base: sentinel("TASK_SHARE_PUBLIC_BASE"),
        vault_root: sentinel("TASK_SERVER_VAULT_ROOT"),
        wiki_root: sentinel("TASK_SERVER_WIKI_ROOT"),
        crdt_root: sentinel("TASK_SERVER_CRDT_ROOT"),
        mail_root: sentinel("TASK_SERVER_MAIL_ROOT"),
        agent_tasks_url: sentinel("TASK_SERVER_AGENT_TASKS_URL"),
        agent_runners_url: sentinel("TASK_SERVER_AGENT_RUNNERS_URL"),
        timer_url: sentinel("TASK_SERVER_TIMER_URL"),
        threads_url: sentinel("TASK_SERVER_THREADS_URL"),
        notify_url: sentinel("TASK_SERVER_NOTIFY_URL"),
        prefs_url: sentinel("TASK_SERVER_PREFS_URL"),
        finance_url: sentinel("TASK_SERVER_FINANCE_URL"),
        wiki_domains: sentinel("TASK_WIKI_DOMAINS"),
        wiki_claim_ttl_secs: sentinel("TASK_WIKI_CLAIM_TTL_SECS"),
        wiki_repo_sync_secs: sentinel("TASK_WIKI_REPO_SYNC_SECS"),
        device_sync_secs: sentinel("TASK_DEVICE_SYNC_SECS"),
        device_sync_adopt: sentinel("TASK_DEVICE_SYNC_ADOPT"),
        files_compact_ops: sentinel("TASK_FILES_COMPACT_OPS"),
        forgejo_base_url: sentinel("TASK_FORGEJO_BASE_URL"),
        forgejo_token: sentinel("TASK_FORGEJO_TOKEN"),
        forgejo_token_fallback: sentinel("FORGEJO_TOKEN"),
        forgejo_bot_token: sentinel("TASK_FORGEJO_BOT_TOKEN"),
        codeberg_access_token: sentinel("FTS_CODEBERG_ACCESS_TOKEN"),
        esv_api_key: sentinel("TASK_ESV_API_KEY"),
        esv_api_key_fallback: sentinel("ESV_API_KEY"),
        api_bible_key: sentinel("TASK_API_BIBLE_KEY"),
        api_bible_key_fallback: sentinel("API_BIBLE_KEY"),
        api_bible_niv_id: sentinel("TASK_API_BIBLE_NIV_ID"),
        log_console: sentinel("TASK_LOG_CONSOLE"),
    };
    assert_eq!(set, expected);
}

/// The one reading change the derive brings: a blank value is unset
/// (so the default or the fallback applies), and a value is trimmed.
#[tokio::test]
async fn a_blank_value_reads_as_unset() {
    let _env = support::env_lock().await;
    clear_all();
    // SAFETY: under the env lock.
    unsafe {
        std::env::set_var("TASK_SERVER_VAULT_ROOT", "");
        std::env::set_var("TASK_BUILD_REV", "   ");
        std::env::set_var("TASK_ESV_API_KEY", "");
        std::env::set_var("ESV_API_KEY", " key ");
    }
    let config = ServerConfig::from_env().expect("reads");
    clear_all();
    assert_eq!(config.vault_root, None);
    assert_eq!(config.build_rev, "unknown");
    assert_eq!(config.esv_api_key(), Some("key"));
}

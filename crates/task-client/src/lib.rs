//! **Task, as a library.** Open an org and drive its services — the
//! same typed clients, in-process or over the wire.
//!
//! This crate implements decision 4 of
//! `docs/adr/0004-vault-assets-resources.md`: *Task is embeddable as a
//! library, not only reachable as a server.* The goal behind that
//! decision, stated plainly: **an external app should be able to do
//! exactly what a plugin does.**
//!
//! ## Why this crate is small
//!
//! Almost nothing here is new. Every backend feature in this repo is an
//! `#[architect::rpc]` service, and the macro generates two things: a
//! service trait, which the in-process backend implements (so a plugin
//! calls `GoalBackend` through `GoalService` with no transport at all),
//! and a `GoalServiceClient`, which is what a caller holds over a vox
//! lane. The crucial property — architect's "inject remote vs local,
//! one client" — is that the embedded path is *not a second API*. It is
//! the same vox lane, served by an in-process [`LocalServer`] with no
//! socket. So the client type an external app holds and the client type
//! a plugin holds are already, literally, the same type.
//!
//! What was missing was not a design. It was **visibility**: the four
//! functions that resolve a transport and establish a client lived as
//! private helpers inside `apps/cli`'s *binary* crate, where nothing
//! could reach them. Any external app would have had to reimplement
//! them, and would have got the subtle part — the local-first fallback
//! in [`TaskClient::org`] — wrong, because the subtle part is not
//! obvious. This crate is that extraction, and `apps/cli` now consumes
//! it rather than carrying a copy.
//!
//! ## The shape
//!
//! ```no_run
//! # async fn f() -> Result<(), Box<dyn std::error::Error>> {
//! use project::ProjectServiceClient;
//! use task_client::TaskClient;
//!
//! // Configuration decides the transport. The code below it does not.
//! let task = TaskClient::from_env();
//! let projects: ProjectServiceClient = task.org("acme-audio").await?;
//! for p in projects.list().await? {
//!     println!("{}", p.title);
//! }
//! # Ok(()) }
//! ```
//!
//! `TASK_EMBED=1` runs that against an in-process backend over the
//! local data root. `TASK_VOX_URL=wss://…` runs it against a server.
//! Neither changes a line of it. That is the whole claim, and
//! `examples/open_org.rs` is it as a runnable program.
//!
//! ## Three things a consumer should know
//!
//! **The fallback rule is load-bearing.** [`TaskClient::org_at`]
//! documents it in full; it is the reason "no server running" keeps
//! working on a laptop while an explicit remote target still fails
//! loud. Do not simplify it.
//!
//! **Sessions have a trap, and it is documented in [`session`].** The
//! session file is a routing document; the token lives in a sibling
//! file. Reading that module's header once will save an afternoon.
//!
//! **TLS needs a `CryptoProvider`.** A binary reaching a `wss://`
//! endpoint must install one before the first dial — see
//! [`transport::dial_authenticated`]. This crate deliberately does not
//! install one itself: the choice belongs to the binary, whose
//! dependency graph decides whether there even is an ambiguity.
//!
//! ## A note on the dependency direction
//!
//! `crates/` depending on `apps/` inverts the layout rule, and it does
//! so knowingly. Embedded mode *is* [`task_server::AppState`] — the
//! whole router, hosted in-process — so a crate that offers embedded
//! mode must depend on the server. The honest reading is that
//! `apps/server` has a library half that has outgrown its `apps/`
//! address; splitting it is a larger change than this one and would
//! have obscured the extraction. Recorded here rather than left for
//! someone to discover.
//!
//! [`LocalServer`]: architect::local

pub mod error;
pub mod session;
pub mod transport;

pub use error::{Error, Result};

/// The embedded backend, built at most once per process: a full
/// [`task_server::AppState`] plus the construction [`architect::Scope`]
/// that keeps its in-process vox acceptor tasks alive.
///
/// Process-wide rather than per-[`TaskClient`] on purpose. `AppState`
/// owns the data root's storage registry, its collab documents and its
/// watchers; two of them over the same directory would race each other
/// exactly as two servers would. A consumer that builds several
/// `TaskClient`s — a CLI resolving different orgs, an app with several
/// workspaces — gets one backend and many lanes onto it, which is also
/// what a running server gives them.
struct Embedded {
    state: task_server::AppState,
    scope: std::sync::Arc<architect::Scope>,
}

static EMBEDDED: tokio::sync::OnceCell<Embedded> = tokio::sync::OnceCell::const_new();

/// Lazily build (once) and return the embedded backend. Never booted
/// unless something actually asks for the in-process path, because
/// booting it opens the data root.
async fn embedded() -> Result<&'static Embedded> {
    EMBEDDED
        .get_or_try_init(|| async {
            let scope = architect::Scope::new();
            let state = task_server::AppState::new(None)
                .await
                .map_err(|e| Error::Embedded {
                    what: "backend boot".into(),
                    cause: format!("{e}"),
                })?;
            Ok::<_, Error>(Embedded { state, scope })
        })
        .await
}

/// True when `TASK_EMBED` asks for the in-process backend.
#[must_use]
pub fn embed_env() -> bool {
    std::env::var("TASK_EMBED").is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
}

/// Can `slug` be served in-process? True when the org exists under the
/// local data root — the precondition for the embedded fallback in
/// [`TaskClient::org_at`].
#[must_use]
pub fn org_on_disk(slug: &str) -> bool {
    org_proto::DataRoot::from_env().is_ok_and(|r| r.orgs_dir().join(slug).is_dir())
}

/// How a [`TaskClient`] decides where to go.
///
/// Deliberately a plain data struct with no environment reads of its
/// own: [`TaskClient::from_env`] fills it from the environment, a test
/// or an app with its own settings file fills it directly, and either
/// way the resulting behaviour is a function of these fields alone.
/// The CLI's version of this used process-global `OnceLock`s, which is
/// fine for a binary and unusable in a library.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Explicit server target — the caller's `--server` flag or
    /// `TASK_VOX_URL`. Beats the stored session; `None` lets the
    /// session, then the localhost default, decide.
    pub server: Option<String>,
    /// Explicit server-management target (`TASK_SERVER_VOX_URL`).
    /// Separate from `server` because `/server/vox` is a different
    /// endpoint on the same host, and because the two are configured
    /// independently in practice.
    pub server_vox: Option<String>,
    /// Force the transport rather than inferring it. `Some(true)` is
    /// "always in-process", `Some(false)` is "always dial, never fall
    /// back", `None` reads `TASK_EMBED` and keeps the fallback rule.
    ///
    /// The `Some(false)` case matters for an app that must never
    /// silently open a local data root — a headless service, say, whose
    /// operator would rather see a connection error than a second copy
    /// of the world.
    pub embed: Option<bool>,
    /// Consult the stored session for a server URL and a bearer token.
    /// On by default. Turn it off for an app that manages its own
    /// credentials, or for a test that must not read the developer's
    /// real session file.
    pub use_session: bool,
}

impl Config {
    /// Defaults with sessions enabled — what `Default` would give if
    /// `bool`'s default were the useful one here.
    #[must_use]
    pub fn new() -> Self {
        Self {
            use_session: true,
            ..Self::default()
        }
    }
}

/// A configured way into a Task org.
///
/// Cheap to construct and cheap to clone: it holds configuration, not a
/// connection. Each `org` / `server` call establishes its own lane, so
/// one `TaskClient` can hand out clients for as many services and orgs
/// as a caller wants.
#[derive(Debug, Clone)]
pub struct TaskClient {
    config: Config,
}

impl TaskClient {
    /// Build from an explicit configuration.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Build from the environment, the way the CLI does: `TASK_VOX_URL`
    /// / `TASK_SERVER_VOX_URL` for the target, `TASK_EMBED` for the
    /// transport, the stored session for everything else.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(Config {
            server: non_empty_env("TASK_VOX_URL"),
            server_vox: non_empty_env("TASK_SERVER_VOX_URL"),
            embed: None,
            use_session: true,
        })
    }

    /// Always in-process, over the local data root. No socket, no
    /// session, no server. This is a plugin's position, made available
    /// to a caller outside the process the plugin would have run in.
    #[must_use]
    pub fn embedded() -> Self {
        Self::new(Config {
            embed: Some(true),
            use_session: false,
            ..Config::new()
        })
    }

    /// Always this server, never a fallback. An explicit target is a
    /// statement of intent, and quietly opening a local data root
    /// instead would be the wrong kind of helpful.
    #[must_use]
    pub fn remote(url: impl Into<String>) -> Self {
        Self::new(Config {
            server: Some(url.into()),
            embed: Some(false),
            ..Config::new()
        })
    }

    /// The configuration in force.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Whether calls will be served in-process. Note this answers the
    /// *configured* transport: with `embed: None` a dial may still fall
    /// back to in-process, which is the point of the fallback and the
    /// reason this is not a promise about any individual call.
    #[must_use]
    pub fn is_embedded(&self) -> bool {
        self.config.embed.unwrap_or_else(embed_env)
    }

    /// The resolved vox base — `ws(s)://host[:port]`, no path.
    /// Precedence:
    ///
    /// 1. [`Config::server`] (a `--server` flag or `TASK_VOX_URL`)
    /// 2. the active session's stored server URL — signing in against a
    ///    remote records where, so later calls need nothing but the
    ///    session
    /// 3. the localhost default
    #[must_use]
    pub fn server_base(&self) -> String {
        // Only consult the session file when nothing explicit is set —
        // keeps the hot path off the filesystem.
        let session_url = if self.config.server.is_some() || !self.config.use_session {
            None
        } else {
            session::load()
                .ok()
                .flatten()
                .and_then(|s| s.active_server().map(|e| e.url.clone()))
        };
        transport::pick_server_base(self.config.server.as_deref(), session_url.as_deref())
    }

    /// The per-org vox endpoint this client would dial for `slug`.
    #[must_use]
    pub fn org_vox_url(&self, slug: &str) -> String {
        transport::org_vox_url(&self.server_base(), slug)
    }

    /// HTTP(S) base for the server's plain HTTP routes (`/blobs/*`,
    /// `/.well-known/*`), derived from the resolved vox base.
    #[must_use]
    pub fn http_base(&self) -> String {
        transport::http_base_of(&self.server_base())
    }

    /// Establish a typed service client for one org.
    ///
    /// `C` is any generated `…ServiceClient` — `GoalServiceClient`,
    /// `WikiServiceClient`, `FilesServiceClient`. The same `C` works
    /// over either transport, which is the entire point.
    pub async fn org<C>(&self, slug: &str) -> Result<C>
    where
        C: vox_core::FromVoxLane,
    {
        let url = self.org_vox_url(slug);
        self.org_at(&url).await
    }

    /// Establish a typed client given an already-resolved per-org vox
    /// URL (`…/org/<slug>/vox`). **The choke point every per-org call
    /// goes through**, and the place the transport rule lives.
    ///
    /// Resolution, in order:
    ///
    /// 1. Embedded is configured ([`Config::embed`] = `Some(true)`, or
    ///    `TASK_EMBED` when unset) — serve the slug in-process, always.
    /// 2. Otherwise dial the URL.
    /// 3. The dial failed **and** the target is the localhost default
    ///    (nothing remote was configured — no explicit server, no
    ///    remote session) **and** the org exists under the local data
    ///    root — boot the embedded backend and serve in-process.
    ///
    /// Step 3 is the subtle one and every clause of it is load-bearing.
    /// It is what keeps "no server running" workflows — a timer, a
    /// budget, a wiki on a laptop — working now that every command
    /// talks vox rather than touching disk. The two guards are what
    /// stop it becoming a lie:
    ///
    /// - **only the localhost default.** If a caller named a remote
    ///   server, or is signed into one, then a failure to reach it is
    ///   the answer. Falling back to a local copy of the world would
    ///   silently serve different data than the one they asked for, and
    ///   the moment it succeeded they would stop being able to tell.
    /// - **only if the org is on disk.** In-process mode can only serve
    ///   what the local data root holds. Booting a backend to discover
    ///   it does not host the org turns a clear connection error into a
    ///   confusing not-found one.
    ///
    /// So: a laptop with no server running keeps working, and an
    /// explicit remote target fails loud. Those two sentences are the
    /// specification; the code is the shortest thing that satisfies
    /// both.
    pub async fn org_at<C>(&self, url: &str) -> Result<C>
    where
        C: vox_core::FromVoxLane,
    {
        let slug = transport::slug_of(url);
        if self.is_embedded() {
            let slug = slug.ok_or_else(|| Error::NoSlug {
                url: url.to_owned(),
            })?;
            return self.in_process(slug).await;
        }
        let dial = async {
            if self.config.use_session {
                transport::dial_authenticated(url).await
            } else {
                vox::connect_lane(url).establish().await
            }
        };
        match Box::pin(dial).await {
            Ok(client) => Ok(client),
            Err(e) => {
                // The fallback, guarded exactly as the doc above says.
                if let Some(slug) = slug
                    && self.config.embed.is_none()
                    && url.starts_with(session::DEFAULT_LOCAL_VOX)
                    && org_on_disk(slug)
                {
                    return self.in_process(slug).await;
                }
                Err(Error::Connect {
                    url: url.to_owned(),
                    cause: format!("{e:?}"),
                })
            }
        }
    }

    /// Establish a typed client against the **server-management**
    /// endpoint (`/server/vox` — `OrgManagementService`,
    /// `SnapshotService`): the server-level counterpart of [`org`], with
    /// no per-org slug.
    ///
    /// Returns the client plus a label for the endpoint, because the
    /// callers of this are the ones that tell a person which server
    /// they just acted on, and `(embedded)` is a meaningful answer to
    /// that question.
    ///
    /// [`org`]: Self::org
    pub async fn server<C>(&self) -> Result<(C, String)>
    where
        C: vox_core::FromVoxLane,
    {
        if self.is_embedded() {
            let emb = embedded().await?;
            let client = emb
                .state
                .server_local_server(&emb.scope)
                .establish()
                .await
                .map_err(|e| Error::Embedded {
                    what: "/server/vox establish".into(),
                    cause: format!("{e:?}"),
                })?;
            return Ok((client, "(embedded)".into()));
        }
        let url = self.server_vox_url();
        let client = Box::pin(vox::connect_lane(&url).establish())
            .await
            .map_err(|e| Error::Connect {
                url: url.clone(),
                cause: format!("{e:?}"),
            })?;
        Ok((client, url))
    }

    /// The server-management endpoint this client would dial:
    /// [`Config::server_vox`], else the configured server base mapped
    /// onto `/server/vox`, else the localhost default.
    #[must_use]
    pub fn server_vox_url(&self) -> String {
        if let Some(u) = self.config.server_vox.as_deref().filter(|u| !u.is_empty()) {
            return transport::normalize_server_vox(u);
        }
        if let Some(u) = self.config.server.as_deref().filter(|u| !u.is_empty()) {
            return transport::normalize_server_vox(u);
        }
        transport::DEFAULT_SERVER_VOX.to_owned()
    }

    /// Establish a typed client against the in-process backend for
    /// `slug`, bypassing transport resolution entirely.
    ///
    /// Exposed rather than kept private because it is the one call an
    /// embedding app genuinely wants by name: "open this org, here, now,
    /// and do not think about servers". [`org_at`](Self::org_at) reaches
    /// it via configuration; this reaches it directly.
    pub async fn in_process<C>(&self, slug: &str) -> Result<C>
    where
        C: vox_core::FromVoxLane,
    {
        let emb = embedded().await?;
        emb.state
            .local_server(slug, &emb.scope)
            .ok_or_else(|| Error::NotHosted {
                slug: slug.to_owned(),
            })?
            .establish()
            .await
            .map_err(|e| Error::Embedded {
                what: format!("establish for `{slug}`"),
                cause: format!("{e:?}"),
            })
    }

    /// Look up an org's manifest id from the resolved server's
    /// `/.well-known/task-server.json` — the remote counterpart of
    /// reading `<org>/org.toml` off the local data root.
    ///
    /// Best-effort: `None` on any failure (offline, older server,
    /// unknown slug). Meaningless in embedded mode — the org *is* the
    /// local data root, so the manifest read already answered — and it
    /// returns `None` there rather than pretending otherwise.
    pub async fn remote_org_id(&self, slug: &str) -> Option<uuid::Uuid> {
        if self.is_embedded() {
            return None;
        }
        let url = format!("{}/.well-known/task-server.json", self.http_base());
        let doc: serde_json::Value = reqwest::get(&url).await.ok()?.json().await.ok()?;
        doc.get("orgs")?.as_array()?.iter().find_map(|o| {
            if o.get("slug")?.as_str()? != slug {
                return None;
            }
            o.get("id")?.as_str()?.parse().ok()
        })
    }
}

impl Default for TaskClient {
    fn default() -> Self {
        Self::from_env()
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_remote_never_falls_back() {
        // `remote()` pins `embed: Some(false)`, which is the flag the
        // fallback guard reads. A caller who named a server gets that
        // server or an error, never a local substitute.
        let c = TaskClient::remote("wss://task.starcommand.live");
        assert!(!c.is_embedded());
        assert_eq!(c.config().embed, Some(false));
        assert_eq!(
            c.org_vox_url("codywright"),
            "wss://task.starcommand.live/org/codywright/vox"
        );
        assert_eq!(c.http_base(), "https://task.starcommand.live");
        assert_eq!(c.server_vox_url(), "wss://task.starcommand.live/server/vox");
    }

    #[test]
    fn embedded_client_is_session_free() {
        let c = TaskClient::embedded();
        assert!(c.is_embedded());
        // No session read means no filesystem read, and no chance of a
        // developer's real credentials leaking into an embedded run.
        assert!(!c.config().use_session);
    }

    #[test]
    fn a_bare_config_targets_localhost() {
        let c = TaskClient::new(Config::default());
        assert_eq!(
            c.org_vox_url("acme-audio"),
            "ws://127.0.0.1:18080/org/acme-audio/vox"
        );
        assert_eq!(c.server_vox_url(), transport::DEFAULT_SERVER_VOX);
    }
}

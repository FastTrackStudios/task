//! Accepting an account this server did not issue.
//!
//! Task's identity model is per-org by construction: each org has its own
//! `auth.sqlite`, and "orgs I belong to" is exactly "orgs where my token
//! validates". That is a good model for a self-hosted server and a bad
//! one for a person with five FastTrackStudio apps, because there is no
//! single place an account lives.
//!
//! `fts-auth` is that place. This module is how a Task server accepts a
//! token it did not mint: it asks the issuer.
//!
//! # Introspection, not shared secrets
//!
//! The alternatives were pointing this server at the auth server's
//! database (which couples every Task deployment to one Postgres, and
//! ends self-hosting) or sharing the signing secret (which hands every
//! Task server the ability to mint tokens for every FastTrackStudio
//! account — the blast radius of one compromised box becomes every app).
//! Asking the issuer over HTTPS costs a round trip and keeps the secret
//! where it was.
//!
//! # Off unless configured
//!
//! No `TASK_CENTRAL_AUTH_URL`, no behaviour change: the resolver is not
//! installed and a self-hosted server authenticates exactly as it did.
//! That is the point — self-hosting is not a degraded mode here, it is
//! the default, and central auth is the thing you opt into.
//!
//! # A membership row is still the fence
//!
//! Resolving a token proves *who*, never *where*. A central principal is
//! admitted to an org only when `memberships` has a row for it, the same
//! rule [`crate::permits::HomeFallbackResolver`] applies to home-org
//! principals. Without that, one FastTrackStudio account would reach
//! every org on every server that trusts the issuer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use architect_permissions::{BoxIdentityFuture, IdentityResolver, Principal};

/// How long a resolved answer is reused.
///
/// Every RPC resolves identity, so introspecting each one would put a
/// network round trip in front of every call and make the auth server a
/// hard dependency of every keystroke. A minute is short enough that a
/// revoked session stops working promptly and long enough that a busy
/// socket is not a load generator.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// How long a rejection is remembered.
///
/// Shorter than the positive TTL, and for a different reason: a client
/// with a stale token retries, and without this each retry is a request
/// to the auth server. It must stay short because "this token is bad" is
/// exactly what a *refresh* changes.
const NEGATIVE_TTL: Duration = Duration::from_secs(10);

/// The env var that turns this on — the issuer's base URL, e.g.
/// `https://auth.fasttrackstudio.app`.
pub const CENTRAL_AUTH_URL: &str = "TASK_CENTRAL_AUTH_URL";

/// The configured issuer, if this server has one.
///
/// Server-wide rather than per-org, and read once: an issuer is a
/// property of the deployment, not of an org, and resolving it per org
/// would build one HTTP client and one cache per org for no reason.
/// Reading it once also means the log line below appears once at boot.
#[must_use]
pub fn configured() -> Option<&'static Arc<CentralAuth>> {
    static ISSUER: std::sync::OnceLock<Option<Arc<CentralAuth>>> = std::sync::OnceLock::new();
    ISSUER
        .get_or_init(|| {
            let raw = std::env::var(CENTRAL_AUTH_URL).ok()?;
            let url = raw.trim().trim_end_matches('/').to_owned();
            if url.is_empty() {
                return None;
            }
            tracing::info!(
                issuer = %url,
                "central auth: accepting accounts from this issuer"
            );
            Some(Arc::new(CentralAuth::new(url)))
        })
        .as_ref()
}

/// One issuer, and what this server remembers about its answers.
pub struct CentralAuth {
    base_url: String,
    http: reqwest::Client,
    /// Keyed by token. Holding the token as a map key is the same
    /// exposure as holding it to make the request; it never leaves this
    /// process and is never logged.
    cache: Mutex<HashMap<String, Cached>>,
    /// The most recent token each user presented, for as long as its
    /// cache entry lives. The permissions gate hands a handler only the
    /// principal; a handler that must act at the issuer *as that
    /// person* — reading their linked GitHub token to land a change
    /// they accepted — finds the credential they already presented
    /// here, and nowhere else.
    by_user: Mutex<HashMap<String, Credentials>>,
}

/// The credentials one user has presented lately, by kind.
///
/// A person often holds two at once: the web app's OIDC access token
/// (a JWT, minted by `/oauth2/token`, the only kind the issuer's
/// `/oauth2/linked-token` accepts) and a CLI's plain session token. The
/// last one seen is not the right one to act with — a CLI call must not
/// shadow the web token — so each kind is kept, and the access token is
/// preferred whenever it is still fresh.
#[derive(Default)]
struct Credentials {
    access: Option<(String, Instant)>,
    session: Option<(String, Instant)>,
}

/// How long a presented credential is kept for acting at the issuer. The
/// issuer's access tokens live an hour; a token this old is re-checked by
/// the issuer anyway, so keeping it longer would only add a failed call.
const CREDENTIAL_TTL: Duration = Duration::from_secs(50 * 60);

/// Whether a token is a JWT — three base64url segments — which is what
/// the issuer's OIDC access tokens are, and what its session tokens are
/// not.
fn looks_like_jwt(token: &str) -> bool {
    let mut parts = token.split('.');
    let ok = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
    };
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(h), Some(p), Some(s), None) if ok(h) && ok(p) && ok(s)
    )
}

/// A GitHub account the issuer holds linked to a user, with the token
/// the person granted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedGithub {
    /// GitHub login (`octocat`).
    pub login: String,
    /// Their GitHub access token — used for one push and one pull
    /// request, never stored here.
    pub access_token: String,
}

/// Why the issuer did not hand over a linked GitHub token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkedTokenError {
    /// The person has not linked a GitHub account on the issuer.
    NotLinked,
    /// Task's own grant lacks the `forge:github` scope — the person
    /// signed in before the client requested it, or the issuer does not
    /// grant it to this client. A fresh sign-in fixes the first.
    InsufficientScope,
    /// This server holds no live credential for the user (they came in
    /// on a path that never passed a central token, or it expired).
    NoCredential,
    /// The issuer could not be asked, or answered something else.
    Unavailable(String),
}

impl std::fmt::Display for LinkedTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLinked => write!(f, "no GitHub account is linked to your account"),
            Self::InsufficientScope => write!(
                f,
                "your sign-in did not grant Task access to your linked GitHub account — sign out and back in"
            ),
            Self::NoCredential => write!(f, "no live sign-in credential for this account"),
            Self::Unavailable(why) => write!(f, "the auth server could not be asked: {why}"),
        }
    }
}

struct Cached {
    /// `None` is a remembered rejection, not a missing entry.
    profile: Option<CentralProfile>,
    until: Instant,
}

/// Who the issuer says a token belongs to.
///
/// The id is the part that has to exist — memberships key on it. Email
/// and name are carried so discovery can hand a client its own
/// account without a second trip to the issuer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CentralProfile {
    pub user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// What one issuer endpoint said about a token.
///
/// The distinction that matters is `Declined` vs `Unreachable`: both end
/// as `Principal::Anonymous`, and telling them apart is the difference
/// between "your session expired" and "the auth server is down". Keeping
/// them separate here is what lets the span say which.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Resolved(CentralProfile),
    /// The issuer answered, and did not recognise the token.
    Declined,
    /// We could not ask.
    Unreachable,
}

impl CentralAuth {
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
            cache: Mutex::new(HashMap::new()),
            by_user: Mutex::new(HashMap::new()),
        }
    }

    /// A live credential `user_id` presented lately: their OIDC access
    /// token when there is a fresh one (the kind the issuer acts on),
    /// else their session token.
    #[must_use]
    pub fn token_for(&self, user_id: &str) -> Option<String> {
        let mut map = self.by_user.lock().ok()?;
        let creds = map.get_mut(user_id)?;
        let now = Instant::now();
        for slot in [&mut creds.access, &mut creds.session] {
            if slot.as_ref().is_some_and(|(_, until)| *until <= now) {
                *slot = None;
            }
        }
        let found = creds
            .access
            .as_ref()
            .or(creds.session.as_ref())
            .map(|(t, _)| t.clone());
        if found.is_none() {
            map.remove(user_id);
        }
        found
    }

    /// The GitHub account the issuer holds linked to `user_id`, fetched
    /// with the credential that person presented to this server —
    /// `GET /oauth2/linked-token?provider=github`, bearer their access
    /// token. The issuer answers only when the grant carries
    /// `forge:github`, so a token minted for Task cannot read what the
    /// person never let Task see.
    pub async fn linked_github(&self, user_id: &str) -> Result<LinkedGithub, LinkedTokenError> {
        use architect_telemetry::wide;

        let Some(token) = self.token_for(user_id) else {
            wide::set("auth.linked_github", "no_credential");
            return Err(LinkedTokenError::NoCredential);
        };
        let url = format!("{}/oauth2/linked-token?provider=github", self.base_url);
        let res = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                wide::set("auth.linked_github", "unreachable");
                LinkedTokenError::Unavailable(e.to_string())
            })?;
        let status = res.status();
        let body: serde_json::Value = res.json().await.unwrap_or(serde_json::Value::Null);
        match status.as_u16() {
            200 => {
                let login = body["login"].as_str().unwrap_or_default().to_owned();
                let access_token = body["access_token"].as_str().unwrap_or_default().to_owned();
                if access_token.is_empty() {
                    wide::set("auth.linked_github", "malformed");
                    return Err(LinkedTokenError::Unavailable(
                        "linked-token answered without a token".into(),
                    ));
                }
                wide::set("auth.linked_github", "ok");
                Ok(LinkedGithub {
                    login,
                    access_token,
                })
            }
            404 => {
                wide::set("auth.linked_github", "not_linked");
                Err(LinkedTokenError::NotLinked)
            }
            403 => {
                wide::set("auth.linked_github", "insufficient_scope");
                Err(LinkedTokenError::InsufficientScope)
            }
            401 => {
                wide::set("auth.linked_github", "no_credential");
                Err(LinkedTokenError::NoCredential)
            }
            code => {
                wide::set("auth.linked_github", "error");
                Err(LinkedTokenError::Unavailable(format!(
                    "linked-token answered {code}: {}",
                    body["error"].as_str().unwrap_or_default()
                )))
            }
        }
    }

    /// The issuer's base URL, for discovery to advertise.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.base_url
    }

    /// The user this token belongs to, or `None`.
    ///
    /// `None` covers every failure — rejected, malformed, issuer
    /// unreachable — on purpose. A caller that could tell "no" from
    /// "could not ask" would be tempted to admit on the second, and an
    /// auth server being down must never widen access.
    pub async fn user_for(&self, token: &str) -> Option<String> {
        self.profile_for(token).await.map(|p| p.user_id)
    }

    /// The user this token belongs to, with what the issuer knows of
    /// them, or `None` (same contract as [`Self::user_for`]).
    pub async fn profile_for(&self, token: &str) -> Option<CentralProfile> {
        if let Some(hit) = self.cached(token) {
            return hit;
        }
        let fresh = self.introspect(token).await;
        self.remember(token, fresh.clone());
        fresh
    }

    fn cached(&self, token: &str) -> Option<Option<CentralProfile>> {
        let mut cache = self.cache.lock().ok()?;
        let entry = cache.get(token)?;
        if entry.until <= Instant::now() {
            cache.remove(token);
            return None;
        }
        Some(entry.profile.clone())
    }

    /// Seed the cache, for the e2e test that proves a second lookup does
    /// not reach the network. Not `#[cfg(test)]`: the caller is an
    /// integration test in `tests/`, which compiles against this crate
    /// as a dependency and so never sees its `cfg(test)` items.
    #[doc(hidden)]
    pub fn remember_for_test(&self, token: &str, user_id: Option<String>) {
        self.remember(
            token,
            user_id.map(|user_id| CentralProfile {
                user_id,
                email: None,
                name: None,
            }),
        );
    }

    /// [`Self::remember_for_test`], carrying the email too — for the
    /// personal-org suite, where the email is what the org slug is
    /// named after and so cannot be left out of the fixture.
    #[doc(hidden)]
    pub fn remember_profile_for_test(&self, token: &str, user_id: &str, email: Option<&str>) {
        self.remember(
            token,
            Some(CentralProfile {
                user_id: user_id.to_owned(),
                email: email.map(ToOwned::to_owned),
                name: None,
            }),
        );
    }

    fn remember(&self, token: &str, profile: Option<CentralProfile>) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        let ttl = if profile.is_some() {
            CACHE_TTL
        } else {
            NEGATIVE_TTL
        };
        // Bound it. A server under a token-guessing flood would otherwise
        // grow this map without limit — every distinct bad token is an
        // entry, and the negative TTL alone does not cap the rate.
        if cache.len() > 4096 {
            cache.retain(|_, c| c.until > Instant::now());
            if cache.len() > 4096 {
                cache.clear();
            }
        }
        if let Some(p) = &profile {
            if let Ok(mut by_user) = self.by_user.lock() {
                if by_user.len() > 4096 {
                    let now = Instant::now();
                    by_user.retain(|_, c| {
                        c.access.as_ref().is_some_and(|(_, u)| *u > now)
                            || c.session.as_ref().is_some_and(|(_, u)| *u > now)
                    });
                }
                let entry = by_user.entry(p.user_id.clone()).or_default();
                let kept = (token.to_owned(), Instant::now() + CREDENTIAL_TTL);
                if looks_like_jwt(token) {
                    entry.access = Some(kept);
                } else {
                    entry.session = Some(kept);
                }
            }
        }
        cache.insert(
            token.to_owned(),
            Cached {
                profile,
                until: Instant::now() + ttl,
            },
        );
    }

    /// Ask the issuer who this token belongs to.
    ///
    /// Two endpoints, because a person can arrive holding either of two
    /// different credentials and the server cannot tell them apart by
    /// looking:
    ///
    /// * a **session token**, from Task's own sign-in form posting
    ///   straight to the issuer — validated at `/auth/session`;
    /// * an **OAuth access token**, from the redirect flow, minted by
    ///   `/oauth2/token` in exchange for an authorization code —
    ///   validated at `/oauth2/userinfo`.
    ///
    /// The access token is derived from a session token but is not one,
    /// so `/auth/session` rejects it. Checking only that endpoint is why
    /// a redirect sign-in would look like it succeeded and then land the
    /// person as anonymous.
    ///
    /// Session first: it is the flow that carries a token minted for
    /// Task specifically, and trying it first means the common path
    /// costs one request rather than two.
    async fn introspect(&self, token: &str) -> Option<CentralProfile> {
        use architect_telemetry::wide;

        // `/auth/session` nests the account under `user`; `/oauth2/userinfo`
        // is flat OIDC claims (`sub`, `email`, `name`).
        let session = self.ask(token, "/auth/session", Some("user"), "id").await;
        if let Verdict::Resolved(profile) = session {
            wide::set("auth.central", "session_token");
            return Some(profile);
        }
        match self.ask(token, "/oauth2/userinfo", None, "sub").await {
            Verdict::Resolved(profile) => {
                wide::set("auth.central", "access_token");
                Some(profile)
            }
            // Both endpoints unreachable is an outage, not a bad token,
            // and it is the one case an operator must be paged for: every
            // central sign-in on this server is failing. One line,
            // because denials are alertable and allows are not.
            Verdict::Unreachable if matches!(session, Verdict::Unreachable) => {
                wide::set("auth.central", "issuer_unreachable");
                tracing::warn!(
                    issuer = %self.base_url,
                    "central auth: issuer unreachable — refusing every central token"
                );
                None
            }
            _ => {
                // The issuer answered and did not recognise the token.
                // Ordinary (an expired session), so it rides the wide
                // event only — a log line per rejection is the scatter
                // this pattern exists to delete.
                wide::set("auth.central", "unrecognised");
                None
            }
        }
    }

    /// One introspection request, reading the account out of the reply:
    /// under `nest` when the endpoint wraps it, its id at `id_key`, and
    /// `email`/`name` beside the id when present.
    ///
    /// Returns a [`Verdict`] rather than `Option` so the caller can tell
    /// "the issuer said no" from "we could not ask" — the two are the
    /// same `Principal::Anonymous` downstream, and which one it was is
    /// the difference between "your session expired" and "the auth
    /// server is unreachable".
    async fn ask(&self, token: &str, endpoint: &str, nest: Option<&str>, id_key: &str) -> Verdict {
        let url = format!("{}{endpoint}", self.base_url);
        let res = match self
            .http
            .get(&url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(res) => res,
            Err(_) => return Verdict::Unreachable,
        };
        // `/auth/session` answering 401 is the NORMAL first step for a
        // redirect-flow token, so this is a routine answer, not a fault.
        if !res.status().is_success() {
            return Verdict::Declined;
        }
        let Ok(body) = res.json::<serde_json::Value>().await else {
            return Verdict::Declined;
        };
        let account = match nest {
            Some(key) => match body.get(key) {
                Some(inner) => inner,
                None => return Verdict::Declined,
            },
            None => &body,
        };
        let Some(user_id) = account.get(id_key).and_then(|v| v.as_str()) else {
            return Verdict::Declined;
        };
        let text = |key: &str| {
            account
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Verdict::Resolved(CentralProfile {
            user_id: user_id.to_owned(),
            email: text("email"),
            name: text("name"),
        })
    }
}

/// Who holds this token, as far as the HOME org is concerned.
///
/// The home org is this server's identity authority: the locker, the
/// snapshot lane, storage administration and org creation all ask "is
/// this a home-org account?" before anything else. Each of them used to
/// ask the home org's own auth store and nothing else — so a person
/// signed in through the issuer, whom every org lane admitted, was
/// `invalid session token` to the locker and saw one org in the switcher.
///
/// Same order as [`CentralFallbackResolver`]: the local store first (a
/// token minted here never costs a round trip), then the issuer. And the
/// same fence: an issuer account is admitted only when this server holds
/// at least one membership row for it — knowing who you are is not
/// knowing you belong here.
pub async fn home_principal(state: &crate::AppState, token: &str) -> Option<uuid::Uuid> {
    if token.is_empty() {
        return None;
    }
    let home = state.home_identity.as_ref()?;
    if let Ok(bundle) = home
        .auth
        .auth
        .current_session(architect_auth::commands::CurrentSession {
            token: token.to_owned(),
        })
        .await
    {
        return Some(bundle.user.id);
    }
    let central = configured()?;
    let user_id = central.user_for(token).await?.parse::<uuid::Uuid>().ok()?;
    match home.memberships.for_user(user_id).await {
        Ok(rows) if !rows.is_empty() => Some(user_id),
        Ok(_) => {
            tracing::warn!(
                "central auth: principal has no membership row on this server — \
                 refusing as a home-org account"
            );
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "central auth: membership lookup failed — refusing");
            None
        }
    }
}

/// Who this token belongs to, with no membership fence.
///
/// The one caller is personal-org provisioning, which exists precisely
/// for the principal [`home_principal`] refuses: a valid account holding
/// no membership row anywhere. Asking "who are you" without also asking
/// "do you belong here" is safe only because the single caller grants
/// the asker an org of their own and can reach nothing else — do not
/// reuse it as an authorization check, which is what `home_principal`
/// and `role_for` are for.
///
/// Same order as [`home_principal`]: the local store first, so a token
/// minted here never costs a round trip, then the issuer. The email is
/// a naming hint for the org slug and may be absent; the id is what
/// the membership row keys on.
pub async fn identify_unfenced(
    state: &crate::AppState,
    token: &str,
) -> Option<(uuid::Uuid, Option<String>)> {
    if token.is_empty() {
        return None;
    }
    let home = state.home_identity.as_ref()?;
    if let Ok(bundle) = home
        .auth
        .auth
        .current_session(architect_auth::commands::CurrentSession {
            token: token.to_owned(),
        })
        .await
    {
        return Some((bundle.user.id, bundle.user.email));
    }
    let profile = configured()?.profile_for(token).await?;
    let user_id = profile.user_id.parse::<uuid::Uuid>().ok()?;
    Some((user_id, profile.email))
}

/// Ask the issuer when nothing local knows the token.
///
/// Ordered deliberately: the inner chain (this org's store, then the
/// home org's) answers first, so a server that has both keeps working
/// exactly as before and a token minted here never costs a round trip.
/// Only a token nobody local recognises reaches the network.
pub struct CentralFallbackResolver<R> {
    inner: R,
    central: Arc<CentralAuth>,
    memberships: Arc<crate::memberships::Memberships>,
    slug: String,
}

impl<R> CentralFallbackResolver<R> {
    pub fn new(
        inner: R,
        central: Arc<CentralAuth>,
        memberships: Arc<crate::memberships::Memberships>,
        slug: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            central,
            memberships,
            slug: slug.into(),
        }
    }
}

impl<R: IdentityResolver> IdentityResolver for CentralFallbackResolver<R> {
    fn resolve<'a>(&'a self, bearer_token: Option<&'a str>) -> BoxIdentityFuture<'a> {
        Box::pin(async move {
            use architect_telemetry::wide;

            let local = self.inner.resolve(bearer_token).await;
            if matches!(local, Principal::User { .. }) {
                return local;
            }
            let Some(token) = bearer_token else {
                // No credential at all is not a case to ask about — but
                // it still gets a value, because "the client never sent
                // one" and "the issuer refused it" are the same
                // `Anonymous` downstream, and a field that is merely
                // ABSENT cannot tell them apart.
                wide::set("auth.central", "no_token");
                return Principal::Anonymous;
            };
            let Some(profile) = self.central.profile_for(token).await else {
                return Principal::Anonymous;
            };
            let user_id = profile.user_id;

            // Same fence as the home fallback: knowing who you are is not
            // knowing you belong here.
            let Ok(uuid) = user_id.parse::<uuid::Uuid>() else {
                wide::set("auth.central", "unparsable_user_id");
                return Principal::Anonymous;
            };

            // A principal with no row here may still have been invited
            // by address. Claiming is attempted only on that miss, so an
            // established member never pays for it — and the address is
            // the ISSUER's answer about a token it just validated, never
            // anything the client said, which is the whole security
            // argument for binding a membership to it.
            if matches!(self.memberships.role_for(uuid, &self.slug).await, Ok(None))
                && let Some(email) = profile.email.as_deref()
            {
                match self.memberships.claim_for_email(email, uuid).await {
                    Ok(claimed) if !claimed.is_empty() => {
                        wide::set("auth.invites_claimed", claimed.len() as i64);
                        tracing::info!(
                            principal = %uuid,
                            orgs = %claimed.join(","),
                            "central auth: claimed pending invites on first sign-in"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        // Not fatal: the fence below still decides. A
                        // failed claim denies, it never admits.
                        wide::set("auth.central", "claim_failed");
                        tracing::warn!(error = %e, "central auth: claiming invites failed");
                    }
                }
            }

            match self.memberships.role_for(uuid, &self.slug).await {
                Ok(Some(m)) => {
                    wide::set("auth.central", "member");
                    wide::set(
                        "auth.membership_role",
                        m.role.unwrap_or_else(|| "(member)".into()),
                    );
                    Principal::User { user_id }
                }
                Ok(None) => {
                    // A real account, with no place here. One warn line —
                    // this is the message an operator needs when somebody
                    // says "I signed in and it says I'm not signed in":
                    // they are, to the issuer, and this server has no row
                    // for them and no invite waiting at their address.
                    // `admin invite --org <slug> --email <addr>` writes
                    // one; `admin grant --principal` if the uuid is
                    // already known.
                    wide::set("auth.central", "not_a_member");
                    tracing::warn!(
                        org.slug = self.slug,
                        "central auth: principal has no membership row for this org"
                    );
                    Principal::Anonymous
                }
                Err(e) => {
                    wide::set("auth.central", "lookup_failed");
                    tracing::warn!(
                        org.slug = self.slug,
                        error = %e,
                        "central auth: membership lookup failed — refusing"
                    );
                    Principal::Anonymous
                }
            }
        })
    }
}

/// The claim path, end to end through the resolver.
///
/// The store's own tests prove `claim_for_email` moves rows. These prove
/// the wiring — that the address the claim uses is the ISSUER's, that an
/// invited principal is admitted on its very first request, and that an
/// uninvited one still is not. The fence is the thing most expensive to
/// get wrong, so it is tested where it is actually enforced.
#[cfg(test)]
mod claim_tests {
    use super::{CentralAuth, CentralFallbackResolver};
    use architect_permissions::{BoxIdentityFuture, IdentityResolver, Principal};
    use std::sync::Arc;

    /// A local resolver that recognises nobody, so every request falls
    /// through to the issuer — the case this module exists for.
    struct NobodyLocal;
    impl IdentityResolver for NobodyLocal {
        fn resolve<'a>(&'a self, _: Option<&'a str>) -> BoxIdentityFuture<'a> {
            Box::pin(async { Principal::Anonymous })
        }
    }

    async fn fixture() -> (tempfile::TempDir, Arc<crate::memberships::Memberships>) {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::memberships::Memberships::open(&dir.path().join("m.sqlite"))
            .await
            .unwrap();
        (dir, Arc::new(store))
    }

    fn resolver(
        central: Arc<CentralAuth>,
        memberships: Arc<crate::memberships::Memberships>,
        slug: &str,
    ) -> CentralFallbackResolver<NobodyLocal> {
        CentralFallbackResolver::new(NobodyLocal, central, memberships, slug)
    }

    /// One issuer holding one account, cached so nothing reaches out.
    fn issuer_holding(principal: uuid::Uuid, email: Option<&str>) -> Arc<CentralAuth> {
        let central = Arc::new(CentralAuth::new("https://auth.example.app"));
        central.remember_profile_for_test("tok", &principal.to_string(), email);
        central
    }

    #[tokio::test]
    async fn an_invited_address_is_admitted_on_its_first_request() {
        let (_d, memberships) = fixture().await;
        memberships
            .invite("cody@example.app", "cbu", Some("admin"))
            .await
            .unwrap();

        let principal = uuid::Uuid::new_v4();
        let central = issuer_holding(principal, Some("cody@example.app"));

        let who = resolver(central, Arc::clone(&memberships), "cbu")
            .resolve(Some("tok"))
            .await;
        assert!(
            matches!(&who, Principal::User { user_id } if user_id == &principal.to_string()),
            "an invite must admit on the first request, not the second: {who:?}"
        );
        assert_eq!(
            memberships
                .role_for(principal, "cbu")
                .await
                .unwrap()
                .unwrap()
                .role,
            Some("admin".into()),
            "and it must carry the invited role"
        );
    }

    /// The fence, unchanged. A real account at the issuer with no invite
    /// and no row reaches nothing — this is the property that keeps one
    /// FastTrackStudio account from being a key to every org.
    #[tokio::test]
    async fn a_real_account_with_no_invite_is_still_refused() {
        let (_d, memberships) = fixture().await;
        memberships
            .invite("someone-else@example.app", "cbu", None)
            .await
            .unwrap();

        let central = issuer_holding(uuid::Uuid::new_v4(), Some("mallory@example.app"));
        let who = resolver(central, memberships, "cbu")
            .resolve(Some("tok"))
            .await;
        assert!(matches!(who, Principal::Anonymous), "got {who:?}");
    }

    /// An invite to one org is not an invite to the server.
    #[tokio::test]
    async fn an_invite_admits_only_the_org_it_names() {
        let (_d, memberships) = fixture().await;
        memberships
            .invite("cody@example.app", "cbu", None)
            .await
            .unwrap();

        let central = issuer_holding(uuid::Uuid::new_v4(), Some("cody@example.app"));

        // The `tombrooksmusic` lane claims the invite (it is keyed by
        // address, not by org) and must still refuse, because the row it
        // wrote is for `cbu`.
        let who = resolver(
            Arc::clone(&central),
            Arc::clone(&memberships),
            "tombrooksmusic",
        )
        .resolve(Some("tok"))
        .await;
        assert!(matches!(who, Principal::Anonymous), "got {who:?}");

        // And the org it does name still works afterwards.
        let who = resolver(central, memberships, "cbu")
            .resolve(Some("tok"))
            .await;
        assert!(matches!(who, Principal::User { .. }), "got {who:?}");
    }

    /// A profile the issuer reported no address for authenticates and
    /// claims nothing — it must not match an invite by accident.
    #[tokio::test]
    async fn an_identity_with_no_address_claims_nothing() {
        let (_d, memberships) = fixture().await;
        memberships
            .invite("cody@example.app", "cbu", None)
            .await
            .unwrap();

        let central = issuer_holding(uuid::Uuid::new_v4(), None);
        let who = resolver(central, Arc::clone(&memberships), "cbu")
            .resolve(Some("tok"))
            .await;
        assert!(matches!(who, Principal::Anonymous), "got {who:?}");
        assert_eq!(memberships.pending().await.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::{CACHE_TTL, CentralAuth, CentralProfile, NEGATIVE_TTL, looks_like_jwt};

    /// A person's web access token is what the issuer acts on; a CLI's
    /// session token presented afterwards must not shadow it.
    #[test]
    fn the_access_token_is_preferred_over_a_later_session_token() {
        let central = CentralAuth::new("https://issuer.test");
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1LTEifQ.c2ln";
        assert!(looks_like_jwt(jwt));
        assert!(!looks_like_jwt("plain-session-token-with-no-dots"));
        central.remember_for_test(jwt, Some("u-1".into()));
        central.remember_for_test("cli-session-token", Some("u-1".into()));
        assert_eq!(central.token_for("u-1").as_deref(), Some(jwt));
        // Only a session token: it is what there is.
        central.remember_for_test("other-session", Some("u-2".into()));
        assert_eq!(central.token_for("u-2").as_deref(), Some("other-session"));
        // A rejection remembers nothing for the user.
        central.remember_for_test("bad", None);
        assert_eq!(central.token_for("nobody"), None);
    }

    /// A rejection is remembered for a shorter time than an acceptance,
    /// because a refresh is exactly the thing that changes it.
    #[test]
    fn a_rejection_is_forgotten_sooner_than_an_acceptance() {
        assert!(NEGATIVE_TTL < CACHE_TTL);
    }

    #[test]
    fn a_configured_issuer_loses_its_trailing_slash() {
        // The URL is joined with `/auth/session`; a trailing slash would
        // make that `//auth/session`, which some gateways route
        // differently and none route better.
        let auth = CentralAuth::new("https://auth.example.app");
        assert_eq!(auth.base_url, "https://auth.example.app");
    }

    /// The cache must answer from memory, including for a rejection —
    /// otherwise a client retrying a stale token is a load generator
    /// pointed at the auth server.
    #[test]
    fn a_remembered_answer_is_reused() {
        let auth = CentralAuth::new("https://auth.example.app");
        assert!(auth.cached("tok").is_none(), "nothing remembered yet");

        let profile = CentralProfile {
            user_id: "user-1".into(),
            email: Some("one@example.app".into()),
            name: None,
        };
        auth.remember("tok", Some(profile.clone()));
        assert_eq!(auth.cached("tok"), Some(Some(profile)));

        auth.remember("bad", None);
        assert_eq!(
            auth.cached("bad"),
            Some(None),
            "a rejection is an answer, not a missing entry"
        );
    }

    /// Unbounded growth here is a denial-of-service: every distinct bad
    /// token would be an entry, and the negative TTL caps how long one
    /// lives, not how many arrive.
    #[test]
    fn the_cache_does_not_grow_without_limit() {
        let auth = CentralAuth::new("https://auth.example.app");
        for i in 0..5000 {
            auth.remember(&format!("token-{i}"), None);
        }
        let len = auth.cache.lock().expect("cache").len();
        assert!(len <= 4096, "cache grew to {len}");
    }
}

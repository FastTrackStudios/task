//! Signing in by being sent to the issuer and coming back.
//!
//! Task has two ways to reach a central account, and they are not
//! redundant:
//!
//! * **The form.** Task's own sign-in screen posts an email and password
//!   straight to the issuer and holds the session token it returns. One
//!   app, one login, no round trip through a browser redirect.
//! * **The redirect, here.** The browser goes to the issuer, signs in
//!   *there*, and comes back with an authorization code. Task never sees
//!   the password, and because the session lives at the issuer, signing
//!   in once covers Session, Signal, Keyflow and Ignition too. That is
//!   what makes it single sign-on rather than five separate logins.
//!
//! # This module is only the glue
//!
//! PKCE, the URL shapes and the response shapes live in
//! [`auth_client::oidc`], in architect, because every FastTrackStudio
//! app needs exactly the same thing and five copies of a challenge
//! calculation is five chances to get it subtly wrong. What is left here
//! is the part that genuinely is Task's: where to park a verifier, how
//! to leave the page, and how to make an HTTP request.

use auth_client::oidc::OidcError;
// Only the browser paths build URLs and bodies; native reaches the
// issuer through the credential form instead.
#[cfg(target_arch = "wasm32")]
use auth_client::oidc::{self, Pkce};

pub use auth_client::oidc::UserInfo;

/// Where the verifier waits while the browser is away at the issuer.
///
/// `sessionStorage`, not `localStorage`: it is meaningful for one
/// attempt in one tab, and `localStorage` would leave it readable by
/// every later page load in the origin.
#[cfg(target_arch = "wasm32")]
const VERIFIER_KEY: &str = "task.auth.pkce.verifier";
#[cfg(target_arch = "wasm32")]
const STATE_KEY: &str = "task.auth.pkce.state";
/// The issuer this attempt was started against.
///
/// Parked rather than looked up on return, because coming back from the
/// issuer is a FRESH page load: org discovery has not fetched
/// `/.well-known` yet, so the issuer registry is still empty and
/// `central_auth::issuer()` answers `None`. The redemption then fails
/// with "this server issues its own accounts" — a message about
/// self-hosting, on a server that had just redirected us to its issuer.
///
/// It is also the more correct value: a code must be redeemed at the
/// issuer that minted it, not at whatever the app has since discovered.
#[cfg(target_arch = "wasm32")]
const ISSUER_KEY: &str = "task.auth.pkce.issuer";

/// The OIDC client id Task is registered under at the issuer.
///
/// Matches the `oidcClients` entry in the auth server's deployment; a
/// mismatch is refused at `/oauth2/authorize` before anything else.
pub const CLIENT_ID: &str = "task";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// No issuer advertised — this server mints its own accounts.
    NoIssuer,
    /// The browser API we need is missing (or we are not in a browser).
    NoBrowser,
    /// We never started a sign-in in this tab, so there is no verifier.
    NoAttemptInProgress,
    /// The issuer refused, or answered with something unusable.
    Exchange(String),
}

impl From<OidcError> for LoginError {
    fn from(e: OidcError) -> Self {
        Self::Exchange(e.to_string())
    }
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoIssuer => write!(f, "this server issues its own accounts"),
            Self::NoBrowser => write!(f, "no browser context"),
            Self::NoAttemptInProgress => {
                write!(f, "no sign-in was started in this tab — start again")
            }
            Self::Exchange(why) => write!(f, "the issuer refused the sign-in: {why}"),
        }
    }
}

// ── Starting ─────────────────────────────────────────────────────────

/// Send the browser to the issuer to sign in.
///
/// Returns only on failure: on success the page is navigating away.
#[cfg(target_arch = "wasm32")]
pub fn begin(redirect_uri: &str) -> Result<(), LoginError> {
    let issuer = task_ui_core::central_auth::issuer().ok_or(LoginError::NoIssuer)?;
    let pkce = Pkce::from_entropy(random_bytes::<32>()?, random_bytes::<16>()?);

    // Stored BEFORE navigating: once `assign` runs this context is on
    // its way out, and a verifier that did not reach storage first is a
    // sign-in that can never complete.
    let storage = session_storage().ok_or(LoginError::NoBrowser)?;
    storage
        .set_item(VERIFIER_KEY, pkce.verifier())
        .map_err(|_| LoginError::NoBrowser)?;
    storage
        .set_item(STATE_KEY, pkce.state())
        .map_err(|_| LoginError::NoBrowser)?;
    storage
        .set_item(ISSUER_KEY, &issuer)
        .map_err(|_| LoginError::NoBrowser)?;

    let url = oidc::authorize_url(
        &issuer,
        CLIENT_ID,
        redirect_uri,
        &pkce,
        oidc::DEFAULT_SCOPE,
    );
    web_sys::window()
        .ok_or(LoginError::NoBrowser)?
        .location()
        .assign(&url)
        .map_err(|_| LoginError::NoBrowser)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn begin(_redirect_uri: &str) -> Result<(), LoginError> {
    // Native builds sign in through the form, which needs no browser to
    // leave and come back.
    Err(LoginError::NoBrowser)
}

// ── Finishing ────────────────────────────────────────────────────────

/// What a completed redemption yields.
///
/// Carries the issuer as well as the token so nothing downstream has to
/// look it up again — the lookup is exactly what races with discovery on
/// the way back in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redeemed {
    pub token: String,
    pub issuer: String,
}

/// Redeem the authorization code for a token Task can present.
///
/// The token that comes back is an OAuth **access token**, not a session
/// token — the server introspects it at `/oauth2/userinfo` rather than
/// `/auth/session`. Both are accepted; see `central_auth` on the server.
#[cfg(target_arch = "wasm32")]
pub async fn complete(
    redirect_uri: &str,
    code: &str,
    state: &str,
) -> Result<Redeemed, LoginError> {
    let storage = session_storage().ok_or(LoginError::NoBrowser)?;

    let parked = |key: &str| {
        storage
            .get_item(key)
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
            .ok_or(LoginError::NoAttemptInProgress)
    };
    // The issuer comes from the attempt, not from discovery — see
    // ISSUER_KEY. Read before the clear below.
    let issuer = parked(ISSUER_KEY)?;
    let pkce = Pkce::resume(parked(VERIFIER_KEY)?, parked(STATE_KEY)?);
    pkce.check_state(state)?;

    // Cleared before the exchange, not after: an authorization code is
    // single-use, so a verifier left behind past this point can only be
    // replayed, never legitimately reused.
    let _ = storage.remove_item(VERIFIER_KEY);
    let _ = storage.remove_item(STATE_KEY);
    let _ = storage.remove_item(ISSUER_KEY);

    let body = oidc::token_request_body(CLIENT_ID, redirect_uri, code, &pkce);
    let text = post_form(
        &format!("{}/oauth2/token", issuer.trim_end_matches('/')),
        &body,
    )
    .await?;
    Ok(Redeemed {
        token: oidc::access_token_from(&text)?,
        issuer,
    })
}

/// Ask the issuer who a token belongs to.
///
/// The redirect returns a token and nothing else, but Task needs a name
/// and address to show a signed-in person.
#[cfg(target_arch = "wasm32")]
#[allow(clippy::missing_errors_doc)]
pub async fn user_info(issuer: &str, token: &str) -> Result<UserInfo, LoginError> {
    let text = get_with_bearer(
        &format!("{}/oauth2/userinfo", issuer.trim_end_matches('/')),
        token,
    )
    .await?;
    Ok(oidc::user_from(&text)?)
}

/// Native builds never run the redirect flow, but `auth.rs` calls this
/// from code compiled for every target.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::unused_async, clippy::missing_errors_doc)]
pub async fn user_info(_issuer: &str, _token: &str) -> Result<UserInfo, LoginError> {
    Err(LoginError::NoBrowser)
}

// ── HTTP, the way this crate already does it on wasm ─────────────────
//
// `window.fetch`, not reqwest: reqwest is a NATIVE-only dependency here
// (see the note on it in Cargo.toml — `orgs::fetch_orgs` uses it for
// desktop and iOS, where there is no `window`). Reaching for it in a
// wasm-gated block compiles everywhere except the one target that runs
// this code.

#[cfg(target_arch = "wasm32")]
async fn post_form(url: &str, body: &str) -> Result<String, LoginError> {
    let headers = web_sys::Headers::new().map_err(js)?;
    headers
        .set("content-type", "application/x-www-form-urlencoded")
        .map_err(js)?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_headers(&headers);
    init.set_body(&wasm_bindgen::JsValue::from_str(body));
    send(web_sys::Request::new_with_str_and_init(url, &init).map_err(js)?).await
}

#[cfg(target_arch = "wasm32")]
async fn get_with_bearer(url: &str, token: &str) -> Result<String, LoginError> {
    let headers = web_sys::Headers::new().map_err(js)?;
    headers
        .set("authorization", &format!("Bearer {token}"))
        .map_err(js)?;
    let init = web_sys::RequestInit::new();
    init.set_headers(&headers);
    send(web_sys::Request::new_with_str_and_init(url, &init).map_err(js)?).await
}

/// Send, and keep the body on failure — an issuer's refusal explains
/// itself there, and dropping it leaves only "it didn't work".
#[cfg(target_arch = "wasm32")]
async fn send(request: web_sys::Request) -> Result<String, LoginError> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let win = web_sys::window().ok_or(LoginError::NoBrowser)?;
    let value = JsFuture::from(win.fetch_with_request(&request))
        .await
        .map_err(js)?;
    let response: web_sys::Response = value
        .dyn_into()
        .map_err(|_| LoginError::Exchange("fetch returned a non-Response".to_owned()))?;
    let status = response.status();
    let text = JsFuture::from(response.text().map_err(js)?)
        .await
        .map_err(js)?
        .as_string()
        .unwrap_or_default();
    if (200..300).contains(&status) {
        Ok(text)
    } else {
        Err(LoginError::Exchange(format!("{status}: {text}")))
    }
}

#[cfg(target_arch = "wasm32")]
fn js(e: wasm_bindgen::JsValue) -> LoginError {
    LoginError::Exchange(format!("{e:?}"))
}

// ── Browser bits ─────────────────────────────────────────────────────

/// PKCE needs unguessable bytes. `Math.random` is not that — it is
/// seeded per-context and predictable, and a guessable verifier is not a
/// weaker PKCE but no PKCE at all.
#[cfg(target_arch = "wasm32")]
fn random_bytes<const N: usize>() -> Result<[u8; N], LoginError> {
    let crypto = web_sys::window()
        .ok_or(LoginError::NoBrowser)?
        .crypto()
        .map_err(|_| LoginError::NoBrowser)?;
    let mut buf = [0u8; N];
    crypto
        .get_random_values_with_u8_array(&mut buf)
        .map_err(|_| LoginError::NoBrowser)?;
    Ok(buf)
}

#[cfg(target_arch = "wasm32")]
fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.session_storage().ok().flatten())
}

/// The callback URL to send as `redirect_uri`.
///
/// Built from the page's own origin so a deployment does not have to be
/// told where it lives — but it must still match a `redirect_uris` entry
/// registered for this client at the issuer, exactly, or authorize
/// refuses before the person ever sees a login page.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn redirect_uri() -> Option<String> {
    let origin = web_sys::window()?.location().origin().ok()?;
    Some(format!("{origin}/auth/callback"))
}

#[cfg(test)]
mod tests {
    use super::{CLIENT_ID, LoginError};
    use auth_client::oidc::{self, Pkce};

    /// The client id is what the issuer matches against its registered
    /// `oidcClients`; a rename here is a sign-in that fails at authorize
    /// with no indication which side is wrong.
    #[test]
    fn task_is_registered_under_this_client_id() {
        assert_eq!(CLIENT_ID, "task");
        let url = oidc::authorize_url(
            "https://auth.fasttrackstudio.app",
            CLIENT_ID,
            "https://task.fasttrackstudio.app/auth/callback",
            &Pkce::from_entropy([1u8; 32], [2u8; 16]),
            oidc::DEFAULT_SCOPE,
        );
        assert!(url.contains("client_id=task"));
    }

    /// A state mismatch has to surface as a refusal, not as a mystery:
    /// it is the check standing between a person and a forged callback.
    #[test]
    fn a_state_mismatch_reads_as_a_refusal() {
        let err: LoginError = Pkce::from_entropy([1u8; 32], [2u8; 16])
            .check_state("not-the-state")
            .unwrap_err()
            .into();
        assert!(
            err.to_string().contains("state did not match"),
            "got {err}"
        );
    }
}

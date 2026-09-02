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

use auth_client::oidc::{self, OidcError};
// The browser paths build URLs and bodies here; native does the same in
// `native`, through the operating system's authentication session.
#[cfg(target_arch = "wasm32")]
use auth_client::oidc::Pkce;

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

    let url = oidc::authorize_url(&issuer, CLIENT_ID, redirect_uri, &pkce, oidc::DEFAULT_SCOPE);
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
pub async fn complete(redirect_uri: &str, code: &str, state: &str) -> Result<Redeemed, LoginError> {
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
/// Native: the same `/oauth2/userinfo` GET over reqwest. Needed because a
/// token redeemed by the native redirect ([`native::sign_in`]) goes
/// through the same adoption path as the browser's.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::missing_errors_doc)]
pub async fn user_info(issuer: &str, token: &str) -> Result<UserInfo, LoginError> {
    let response = reqwest::Client::new()
        .get(format!("{}/oauth2/userinfo", issuer.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| LoginError::Exchange(e.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| LoginError::Exchange(e.to_string()))?;
    if !status.is_success() {
        return Err(LoginError::Exchange(format!("{status}: {text}")));
    }
    Ok(oidc::user_from(&text)?)
}

/// The redirect on a device that is not a browser.
///
/// A native app has no page to navigate away from and no `/auth/callback`
/// route the issuer could send it to. What it has is the operating
/// system's authentication session — `ASWebAuthenticationSession` on
/// iOS — which opens the issuer in a system browser sheet and hands the
/// callback URL straight back to the app on a custom scheme. That sheet
/// shares Safari's cookies, so signing in to one FastTrackStudio app
/// signs in to all of them on the device: the point of the redirect.
///
/// The platform half is a [`native::BrowserSession`] the app installs at
/// boot; everything that is not platform — PKCE, the callback's shape,
/// the token exchange — lives here so five apps share one copy.
#[cfg(not(target_arch = "wasm32"))]
pub mod native {
    use std::sync::{Arc, OnceLock};

    use auth_client::oidc::{self, Pkce};

    use super::{CLIENT_ID, LoginError, Redeemed};

    /// What the operating system provides: open a URL for sign-in and
    /// deliver the callback URL it was redirected to.
    pub trait BrowserSession: Send + Sync + 'static {
        /// The URL scheme the callback arrives on — the app's bundle id
        /// on iOS (`app.fasttrackstudio.task`).
        fn callback_scheme(&self) -> String;

        /// Open `url` in a system authentication session. `done` is
        /// called exactly once with the full callback URL, or with why
        /// there is none (the person cancelled, the system refused).
        fn authenticate(
            &self,
            url: String,
            callback_scheme: String,
            done: Box<dyn FnOnce(Result<String, String>) + Send + 'static>,
        );
    }

    static BROWSER: OnceLock<Arc<dyn BrowserSession>> = OnceLock::new();

    /// Install the platform's session. Once per process; a second call
    /// is ignored.
    pub fn install(browser: Arc<dyn BrowserSession>) {
        let _ = BROWSER.set(browser);
    }

    /// Whether this build can sign in through the system browser.
    #[must_use]
    pub fn available() -> bool {
        BROWSER.get().is_some()
    }

    /// The redirect the issuer must have registered for this app:
    /// `<scheme>://auth/callback`.
    #[must_use]
    pub fn redirect_uri() -> Option<String> {
        BROWSER
            .get()
            .map(|b| format!("{}://auth/callback", b.callback_scheme()))
    }

    /// `code` and `state` from a callback URL, in that order.
    ///
    /// Written by hand rather than through a URL crate because the shape
    /// is fixed by us on both ends and the scheme is not one a general
    /// parser has opinions about. Percent-decoding is applied because the
    /// issuer encodes the values it puts in the query.
    pub fn parse_callback(url: &str) -> Result<(String, String), LoginError> {
        let query = url.split_once('?').map(|(_, q)| q).unwrap_or_default();
        let query = query.split('#').next().unwrap_or_default();
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = percent_decode(v);
            match k {
                "code" => code = Some(v),
                "state" => state = Some(v),
                "error" => error = Some(v),
                _ => {}
            }
        }
        if let Some(error) = error {
            return Err(LoginError::Exchange(error));
        }
        match (code, state) {
            (Some(c), Some(s)) if !c.is_empty() => Ok((c, s)),
            _ => Err(LoginError::NoAttemptInProgress),
        }
    }

    fn percent_decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'%' if i + 2 < bytes.len() => {
                    let hex = &s[i + 1..i + 3];
                    match u8::from_str_radix(hex, 16) {
                        Ok(b) => {
                            out.push(b);
                            i += 3;
                        }
                        Err(_) => {
                            out.push(b'%');
                            i += 1;
                        }
                    }
                }
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn entropy<const N: usize>() -> [u8; N] {
        // Two v4 UUIDs are 32 bytes of OS randomness, which is what the
        // verifier wants; the state takes the first 16.
        let mut buf = [0u8; N];
        let mut filled = 0;
        while filled < N {
            let chunk = *uuid::Uuid::new_v4().as_bytes();
            let take = (N - filled).min(chunk.len());
            buf[filled..filled + take].copy_from_slice(&chunk[..take]);
            filled += take;
        }
        buf
    }

    /// Sign in through the installed [`BrowserSession`]: send the person
    /// to the issuer, take the callback, redeem the code. Returns the
    /// token and the issuer it came from, ready for
    /// `AuthCtx::adopt_central_token`.
    ///
    /// # Errors
    ///
    /// [`LoginError::NoBrowser`] when no session is installed or the
    /// server advertises no issuer; [`LoginError::Exchange`] with the
    /// system's or the issuer's own words otherwise.
    pub async fn sign_in() -> Result<Redeemed, LoginError> {
        let browser = BROWSER.get().cloned().ok_or(LoginError::NoBrowser)?;
        let issuer = task_ui_core::central_auth::issuer().ok_or(LoginError::NoIssuer)?;
        let redirect_uri = redirect_uri().ok_or(LoginError::NoBrowser)?;
        let pkce = Pkce::from_entropy(entropy::<32>(), entropy::<16>());
        let url = oidc::authorize_url(
            &issuer,
            CLIENT_ID,
            &redirect_uri,
            &pkce,
            oidc::DEFAULT_SCOPE,
        );

        let (tx, rx) = futures_channel::oneshot::channel::<Result<String, String>>();
        let tx = std::sync::Mutex::new(Some(tx));
        browser.authenticate(
            url,
            browser.callback_scheme(),
            Box::new(move |outcome| {
                if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
                    let _ = tx.send(outcome);
                }
            }),
        );
        let callback = rx
            .await
            .map_err(|_| LoginError::Exchange("the sign-in sheet went away".to_owned()))?
            .map_err(LoginError::Exchange)?;

        let (code, state) = parse_callback(&callback)?;
        pkce.check_state(&state)?;

        let body = oidc::token_request_body(CLIENT_ID, &redirect_uri, &code, &pkce);
        let response = reqwest::Client::new()
            .post(format!("{}/oauth2/token", issuer.trim_end_matches('/')))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| LoginError::Exchange(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| LoginError::Exchange(e.to_string()))?;
        if !status.is_success() {
            return Err(LoginError::Exchange(format!("{status}: {text}")));
        }
        Ok(Redeemed {
            token: oidc::access_token_from(&text)?,
            issuer,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_callback_yields_its_code_and_state() {
            let (code, state) = parse_callback(
                "app.fasttrackstudio.task://auth/callback?code=abc%2F1&state=xyz&extra=1",
            )
            .unwrap();
            assert_eq!(code, "abc/1");
            assert_eq!(state, "xyz");
        }

        #[test]
        fn a_refusal_is_the_issuers_word() {
            let err =
                parse_callback("app.fasttrackstudio.task://auth/callback?error=access_denied")
                    .unwrap_err();
            assert!(
                matches!(err, LoginError::Exchange(ref m) if m == "access_denied"),
                "{err:?}"
            );
        }

        #[test]
        fn a_callback_without_a_code_is_not_a_sign_in() {
            assert!(matches!(
                parse_callback("app.fasttrackstudio.task://auth/callback"),
                Err(LoginError::NoAttemptInProgress)
            ));
        }

        #[test]
        fn nothing_installed_means_no_browser() {
            assert!(!available() || redirect_uri().is_some());
        }
    }
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
        assert!(err.to_string().contains("state did not match"), "got {err}");
    }
}

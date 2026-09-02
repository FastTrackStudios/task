//! Where the issuer sends the browser back.
//!
//! A page nobody navigates to on purpose. It exists for the few hundred
//! milliseconds between `auth.fasttrackstudio.app` handing back an
//! authorization code and Task holding a token, and its whole job is to
//! redeem the one for the other and get out of the way.
//!
//! It still needs to render something, because that exchange is a
//! network round trip and a blank screen during it reads as a hang. And
//! it needs to render *failure* properly: this is the end of a journey
//! that started on another origin, so "it didn't work" has to be legible
//! here or it is invisible everywhere.

use dioxus::prelude::*;

use crate::auth::AuthCtx;
use crate::central_login::{self, LoginError, Redeemed};
use crate::routes::Route;

#[component]
pub fn AuthCallbackView(code: String, state: String, error: String) -> Element {
    let auth = use_context::<AuthCtx>();
    let nav = use_navigator();
    let mut failure = use_signal(|| Option::<String>::None);

    // Runs once. `use_resource` keyed on the code means a re-render
    // cannot redeem it twice — authorization codes are single-use, and
    // the second attempt fails in a way that would overwrite a
    // successful sign-in with an error.
    let _exchange = use_resource({
        let code = code.clone();
        let state = state.clone();
        let error = error.clone();
        move || {
            let code = code.clone();
            let state = state.clone();
            let error = error.clone();
            async move {
                // OAuth reports a refusal by redirecting here with
                // `error=` and no code. Surfacing the issuer's own word
                // for it matters: `access_denied` (you cancelled) and
                // `invalid_client` (Task is misregistered) need very
                // different responses.
                if !error.is_empty() {
                    failure.set(Some(format!("The issuer refused the sign-in: {error}")));
                    return;
                }
                if code.is_empty() {
                    failure.set(Some(
                        "This page was opened without a sign-in in progress.".to_owned(),
                    ));
                    return;
                }

                match exchange(&code, &state).await {
                    Ok(redeemed) => {
                        // Fire-and-forget into the root coroutine, so
                        // navigating away immediately cannot cancel it.
                        // The issuer rides along because discovery may
                        // not have resolved on this fresh page load.
                        auth.adopt_central_token(redeemed.tokens, redeemed.issuer);
                        nav.replace(Route::HomeRoute {});
                    }
                    Err(e) => failure.set(Some(e.to_string())),
                }
            }
        }
    });

    rsx! {
        div { class: "flex min-h-[60vh] items-center justify-center p-6",
            div { class: "w-full max-w-sm text-center",
                if let Some(message) = failure() {
                    h1 { class: "text-lg font-semibold", "Sign-in didn't complete" }
                    p { class: "mt-2 text-sm text-muted-foreground", "{message}" }
                    Link {
                        to: Route::HomeRoute {},
                        class: "mt-4 inline-block text-sm underline",
                        "Back to Task"
                    }
                } else {
                    p { class: "text-sm text-muted-foreground", "Finishing sign-in…" }
                }
            }
        }
    }
}

/// Redeem the code. Split out so the component body stays about what is
/// shown, and because the two builds differ only here.
#[cfg(target_arch = "wasm32")]
async fn exchange(code: &str, state: &str) -> Result<Redeemed, LoginError> {
    let redirect_uri = central_login::redirect_uri().ok_or(LoginError::NoBrowser)?;
    central_login::complete(&redirect_uri, code, state).await
}

/// Native builds never reach this route — the redirect flow needs a
/// browser to leave and come back — but the page still has to compile
/// for the desktop and mobile targets that share this crate.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::unused_async)]
async fn exchange(_code: &str, _state: &str) -> Result<Redeemed, LoginError> {
    let _ = central_login::CLIENT_ID;
    Err(LoginError::NoBrowser)
}

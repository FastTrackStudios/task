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
//!
//! # The ordering this page owns
//!
//! Coming back from the issuer is a FRESH page load, and this page is
//! the only thing standing between that load and the workspace. So it
//! is where the ordering guarantee lives:
//!
//! **Nothing that talks to an org is mounted until the redeemed token is
//! the app's identity.**
//!
//! Two halves, and both are needed:
//!
//! 1. This route sits OUTSIDE `#[layout(AppShell)]` (see `routes.rs`),
//!    so redeeming the code mounts no vault explorer, no presence
//!    publisher and no store-backed list hook — nothing that fans
//!    `project/list` out across the orgs.
//! 2. It leaves for [`Route::HomeRoute`] only once
//!    [`AuthCtx::active`] has actually resolved, NOT the moment the
//!    token is handed to the auth service. `adopt_central_token` is
//!    fire-and-forget: it queues an action on the root coroutine and
//!    returns, and the work behind it is a `userinfo` round trip. The
//!    old code navigated on that return, so the shell mounted while the
//!    app was still signed out, every org lane was dialled anonymously
//!    — a lane presents its identity once, at the WebSocket upgrade —
//!    and the workspace came up on
//!    `permission denied: anonymous is not a member (project/list)`.
//!    A plain reload cured it, because by then the token was in storage
//!    before anything dialled. That was the whole bug.
//!
//! Waiting on `active` is enough because of the order inside
//! `run_central_token_sign_in`: it publishes the session token (which
//! also drops every socket opened under the previous identity) and only
//! *then* writes `active`. So "active is Some" implies "`vox_session::bearer()`
//! is this session's token", and every lane dialled from here on
//! presents it.

use dioxus::prelude::*;

use crate::auth::AuthCtx;
use crate::central_login::{self, LoginError, Redeemed};
use crate::routes::Route;

#[component]
pub fn AuthCallbackView(code: String, state: String, error: String) -> Element {
    let auth = use_context::<AuthCtx>();
    let nav = use_navigator();
    let mut failure = use_signal(|| Option::<String>::None);
    // Has the redeemed token been handed to the auth service? From here
    // on the page is waiting on that service, not on the issuer, and
    // the two are told apart by nothing else: both look like
    // "Finishing sign-in…".
    let mut adopted = use_signal(|| false);

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
                        //
                        // And then we WAIT. Navigating on this line is
                        // what mounted the workspace over a session that
                        // did not exist yet — see the module docs.
                        auth.adopt_central_token(redeemed.tokens, redeemed.issuer);
                        adopted.set(true);
                    }
                    Err(e) => failure.set(Some(e.to_string())),
                }
            }
        }
    });

    // The handover. Reading all four signals unconditionally is what
    // subscribes this effect to each of them, so the page re-decides
    // whenever the auth service moves — a conditional read would leave
    // it asleep on exactly the transition it is waiting for.
    let active = auth.active;
    let busy = auth.busy;
    let auth_error = auth.error;
    use_effect(move || {
        let step = step(
            adopted(),
            active.read().is_some(),
            busy(),
            auth_error.read().is_some(),
        );
        match step {
            Step::Wait => {}
            Step::GoHome => {
                nav.replace(Route::HomeRoute {});
            }
            Step::Failed => {
                let message = auth_error
                    .read()
                    .clone()
                    .unwrap_or_else(|| "the sign-in did not complete".to_owned());
                failure.set(Some(message));
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

/// What the page does next, once the code has been redeemed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Hold the page — and with it the entire workspace, which cannot
    /// mount behind a route that lives outside the shell.
    Wait,
    /// The session resolved. The shell may mount, and every lane it
    /// dials will present this session's token.
    GoHome,
    /// The auth service finished without producing a session. Say so
    /// here: this is the end of a journey that started on another
    /// origin, and there is nowhere else for the news to appear.
    Failed,
}

/// The handover decision, as a function of four facts rather than of
/// four `use_effect` bodies fighting over one navigator.
///
/// The rules, and why each one is the way round it is:
///
/// - **Nothing until `adopted`.** Before that the page is still talking
///   to the issuer; the exchange reports its own failures directly.
/// - **`signed_in` beats `busy`.** `run_central_token_sign_in` writes
///   `active` *before* it lowers `busy` (there is a locker pull and a
///   link push in between), so waiting for `busy` to clear would sit on
///   a resolved session for two more round trips.
/// - **A failure is only a failure once the service is idle.** `error`
///   can still be holding the *previous* attempt's message while this
///   one is in flight, and an errored-but-busy state means "working on
///   it", not "gave up".
/// - **`adopted && !signed_in && !busy && !errored` waits.** That is the
///   gap between queueing the action and the coroutine picking it up:
///   the service has not started, so there is nothing to report yet.
///   Treating it as failure would flash "Sign-in didn't complete" over
///   a sign-in that was about to succeed.
pub(crate) fn step(adopted: bool, signed_in: bool, busy: bool, errored: bool) -> Step {
    if !adopted {
        return Step::Wait;
    }
    if signed_in {
        return Step::GoHome;
    }
    if !busy && errored {
        return Step::Failed;
    }
    Step::Wait
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

#[cfg(test)]
mod tests {
    use super::{Step, step};

    /// THE regression. The token is handed to the auth service and the
    /// page must not move: `adopt_central_token` only queues the work,
    /// and leaving on that return mounted the workspace signed out —
    /// every org lane dialled anonymously, and
    /// "permission denied: anonymous is not a member (project/list)"
    /// on the first load after every issuer sign-in.
    #[test]
    fn the_page_holds_until_the_session_actually_resolves() {
        // Queued, coroutine not yet awake: no session, not busy, no error.
        assert_eq!(step(true, false, false, false), Step::Wait);
        // The service picked it up and is doing the userinfo round trip.
        assert_eq!(step(true, false, true, false), Step::Wait);
        // `active` landed. NOW the shell may mount.
        assert_eq!(step(true, true, true, false), Step::GoHome);
    }

    /// `run_central_token_sign_in` writes `active` before it lowers
    /// `busy` — a locker pull and a link push happen in between — so a
    /// resolved session must not be held back by `busy`.
    #[test]
    fn a_resolved_session_does_not_wait_for_busy_to_clear() {
        assert_eq!(step(true, true, true, false), Step::GoHome);
        assert_eq!(step(true, true, false, false), Step::GoHome);
        // Even with a stale error from a previous attempt on the signal.
        assert_eq!(step(true, true, true, true), Step::GoHome);
    }

    /// Nothing happens before the code is redeemed — the exchange
    /// reports its own failures, and the auth service has been told
    /// nothing yet.
    #[test]
    fn nothing_happens_before_the_token_is_adopted() {
        for signed_in in [false, true] {
            for busy in [false, true] {
                for errored in [false, true] {
                    assert_eq!(
                        step(false, signed_in, busy, errored),
                        Step::Wait,
                        "adopted=false must never act (signed_in={signed_in}, \
                         busy={busy}, errored={errored})"
                    );
                }
            }
        }
    }

    /// A refusal is only a refusal once the service is idle: an error
    /// still on the signal while a sign-in is in flight is the previous
    /// attempt's, and must not end this one.
    #[test]
    fn a_failure_is_reported_only_once_the_service_is_idle() {
        assert_eq!(step(true, false, true, true), Step::Wait, "still working");
        assert_eq!(step(true, false, false, true), Step::Failed);
    }
}

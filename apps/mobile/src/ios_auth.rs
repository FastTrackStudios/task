//! Central sign-in on iOS: the issuer in a system browser sheet.
//!
//! `ui::central_login::native` owns everything that is not platform —
//! PKCE, the authorize URL, redeeming the code. What iOS contributes is
//! `ASWebAuthenticationSession`: a Safari-backed sheet that opens the
//! issuer, shares Safari's cookies (so one sign-in covers Session,
//! Signal, Keyflow and Ignition on the same device), and calls back
//! with the redirect URL once the issuer sends the browser to our
//! scheme. No URL-handling plumbing in the app delegate is needed; the
//! session intercepts its own callback scheme.
//!
//! The scheme is the bundle id (`app.fasttrackstudio.task`), declared in
//! Info.plist by `ios/deploy-testflight.sh` and registered at the issuer
//! as `app.fasttrackstudio.task://auth/callback` (fts-auth.nix).
//!
//! Off iOS this module is a no-op: `init` installs nothing, and the login
//! screen keeps offering the credential form alone.

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    // `AnyThread` for allocating the session, `MainThreadOnly` for the
    // context provider class the protocol makes main-thread-only.
    use objc2::{AnyThread as _, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_authentication_services::{
        ASPresentationAnchor, ASWebAuthenticationPresentationContextProviding,
        ASWebAuthenticationSession,
    };
    use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString, NSURL};
    use objc2_ui_kit::UIApplication;
    use ui::central_login::native::BrowserSession;

    /// The URL scheme the callback arrives on. Must equal the bundle id
    /// (`[bundle] identifier` in Dioxus.toml) — the plist declares that
    /// exact scheme and the issuer has that exact redirect registered.
    const SCHEME: &str = "app.fasttrackstudio.task";

    define_class!(
        /// Tells the session which window to present its sheet over.
        /// Required on iOS 13+; without it `start` fails with
        /// "presentation context invalid".
        #[unsafe(super(NSObject))]
        // The protocol is main-thread-only (it hands out UI), so the class
        // conforming to it must be too.
        #[thread_kind = MainThreadOnly]
        #[name = "TaskAuthPresentationContext"]
        struct PresentationContext;

        unsafe impl NSObjectProtocol for PresentationContext {}

        unsafe impl ASWebAuthenticationPresentationContextProviding for PresentationContext {
            // `method_id`: the method hands back a `Retained`, which is
            // what objc2's `define_class!` requires for that return type.
            #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
            // `keyWindow`/`windows` are deprecated in favour of scenes;
            // this app has one scene and one window, and the deprecated
            // pair is the one that needs no scene bookkeeping.
            #[allow(deprecated)]
            fn anchor(
                &self,
                _session: &ASWebAuthenticationSession,
            ) -> Retained<ASPresentationAnchor> {
                // This delegate method is invoked on the main thread.
                let mtm =
                    MainThreadMarker::new().expect("presentation anchor asked off the main thread");
                let app = UIApplication::sharedApplication(mtm);
                let window = app
                    .keyWindow()
                    .or_else(|| app.windows().firstObject())
                    .expect("an application window to present the sign-in over");
                // `ASPresentationAnchor` is `UIWindow` on iOS; the bindings
                // spell the alias as `NSObject`, which a `UIWindow` is.
                unsafe { Retained::cast_unchecked(window) }
            }
        }
    );

    impl PresentationContext {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    /// The most recent session and its context provider, kept alive at
    /// least until the next sign-in — a released
    /// `ASWebAuthenticationSession` dismisses itself mid-flow.
    struct Live {
        _session: Retained<ASWebAuthenticationSession>,
        _context: Retained<PresentationContext>,
    }

    // SAFETY: the retained objects are only touched from the main thread;
    // the mutex exists to hand the pair between the start and completion
    // call sites, both of which run there.
    unsafe impl Send for Live {}

    static LIVE: Mutex<Option<Live>> = Mutex::new(None);

    pub struct WebAuthSession;

    impl BrowserSession for WebAuthSession {
        fn callback_scheme(&self) -> String {
            SCHEME.to_owned()
        }

        fn authenticate(
            &self,
            url: String,
            callback_scheme: String,
            done: Box<dyn FnOnce(Result<String, String>) + Send + 'static>,
        ) {
            // The UI event that asked runs on the main thread, which is
            // where UIKit wants the sheet presented from.
            let Some(mtm) = MainThreadMarker::new() else {
                done(Err("sign-in must start on the main thread".to_owned()));
                return;
            };
            let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(&url)) else {
                done(Err("the authorize URL did not parse".to_owned()));
                return;
            };
            let scheme = NSString::from_str(&callback_scheme);

            let done = Arc::new(Mutex::new(Some(done)));
            let handler = RcBlock::new(move |callback: *mut NSURL, error: *mut NSError| {
                let outcome = if !callback.is_null() {
                    // SAFETY: non-null, handed to us for the duration of
                    // the call by the framework.
                    let url = unsafe { &*callback };
                    url.absoluteString()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "the callback URL had no string form".to_owned())
                } else if !error.is_null() {
                    let error = unsafe { &*error };
                    // Code 1 is ASWebAuthenticationSessionErrorCanceledLogin:
                    // the person dismissed the sheet. Say so plainly.
                    if error.code() == 1 {
                        Err("sign-in cancelled".to_owned())
                    } else {
                        Err(error.localizedDescription().to_string())
                    }
                } else {
                    Err("the sign-in ended without a result".to_owned())
                };
                if let Some(done) = done.lock().ok().and_then(|mut d| d.take()) {
                    done(outcome);
                }
                // Deliberately NOT releasing `LIVE` here: this block is
                // retained by the session it would release, and freeing
                // the code that is running is not a thing to do from
                // inside it. The next `authenticate` replaces the pair.
            });

            // `RcBlock` derefs to the block the framework wants a pointer
            // to; the session copies/retains it, so `handler` may drop at
            // the end of this call.
            let handler_ptr = std::ptr::from_ref(&*handler).cast_mut();
            // The undeprecated initializer takes an `ASWebAuthenticationSessionCallback`
            // (iOS 17.4+); the scheme form runs on every iOS this app
            // supports and is what the issuer's registration matches.
            #[allow(deprecated)]
            let session = unsafe {
                ASWebAuthenticationSession::initWithURL_callbackURLScheme_completionHandler(
                    ASWebAuthenticationSession::alloc(),
                    &ns_url,
                    Some(&scheme),
                    handler_ptr,
                )
            };
            let context = PresentationContext::new(mtm);
            unsafe {
                session.setPresentationContextProvider(Some(ProtocolObject::from_ref(&*context)));
                // Share Safari's cookies: that is what turns a sign-in at
                // the issuer into single sign-on across the apps.
                session.setPrefersEphemeralWebBrowserSession(false);
            }
            // The handler block must outlive the session; the session
            // retains the block it was given, so `handler` may drop here.
            let started = unsafe { session.start() };
            if started {
                if let Ok(mut live) = LIVE.lock() {
                    *live = Some(Live {
                        _session: session,
                        _context: context,
                    });
                }
            } else {
                tracing::warn!("ASWebAuthenticationSession refused to start");
            }
        }
    }
}

/// Install the iOS browser session into the UI's central-login hook.
pub fn init() {
    #[cfg(target_os = "ios")]
    {
        ui::central_login::native::install(std::sync::Arc::new(imp::WebAuthSession));
    }
}

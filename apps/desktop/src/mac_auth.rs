//! Central sign-in on macOS: the issuer in a system browser sheet.
//!
//! The desktop twin of `apps/mobile/src/ios_auth.rs`. `ui::central_login::native`
//! owns everything that is not platform (PKCE, the authorize URL,
//! redeeming the code); macOS contributes `ASWebAuthenticationSession`,
//! the same Safari-backed sheet iOS uses (AppKit since macOS 10.15),
//! presented over the app's key window. It intercepts its own callback
//! scheme, so the app registers no URL handler.
//!
//! The scheme is the iOS app's (`app.fasttrackstudio.task`): it is the
//! redirect the issuer has registered for this client
//! (`app.fasttrackstudio.task://auth/callback`), and on macOS the
//! session catches it without the app declaring the scheme.
//!
//! Off macOS `init` installs nothing and the sign-in screen keeps the
//! credential form alone.

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{AnyThread as _, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::NSApplication;
    use objc2_authentication_services::{
        ASPresentationAnchor, ASWebAuthenticationPresentationContextProviding,
        ASWebAuthenticationSession,
    };
    use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString, NSURL};
    use ui::central_login::native::BrowserSession;

    const SCHEME: &str = "app.fasttrackstudio.task";

    define_class!(
        /// Which window the session's sheet attaches to.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "TaskDesktopAuthPresentationContext"]
        struct PresentationContext;

        unsafe impl NSObjectProtocol for PresentationContext {}

        unsafe impl ASWebAuthenticationPresentationContextProviding for PresentationContext {
            #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
            fn anchor(
                &self,
                _session: &ASWebAuthenticationSession,
            ) -> Retained<ASPresentationAnchor> {
                let mtm =
                    MainThreadMarker::new().expect("presentation anchor asked off the main thread");
                let app = NSApplication::sharedApplication(mtm);
                let window = app
                    .keyWindow()
                    .or_else(|| app.mainWindow())
                    .or_else(|| app.windows().firstObject())
                    .expect("an application window to present the sign-in over");
                // `ASPresentationAnchor` is `NSWindow` on macOS; the
                // bindings spell the alias as `NSObject`, which it is.
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

    /// The most recent session and its provider, kept alive until the
    /// next sign-in — a released session dismisses itself mid-flow.
    struct Live {
        _session: Retained<ASWebAuthenticationSession>,
        _context: Retained<PresentationContext>,
    }

    // SAFETY: only touched on the main thread; the mutex hands the pair
    // between the start and completion call sites, both there.
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
                    // SAFETY: non-null, live for the call.
                    let url = unsafe { &*callback };
                    url.absoluteString()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "the callback URL had no string form".to_owned())
                } else if !error.is_null() {
                    let error = unsafe { &*error };
                    // 1 = ASWebAuthenticationSessionErrorCanceledLogin.
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
            });
            let handler_ptr = std::ptr::from_ref(&*handler).cast_mut();
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
                // Share Safari's cookies: one sign-in, every FTS app.
                session.setPrefersEphemeralWebBrowserSession(false);
            }
            if unsafe { session.start() } {
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

/// Install the macOS browser session into the UI's central-login hook.
pub fn init() {
    #[cfg(target_os = "macos")]
    {
        ui::central_login::native::install(std::sync::Arc::new(imp::WebAuthSession));
    }
}

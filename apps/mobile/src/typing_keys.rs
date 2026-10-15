//! The keyboard, for the typing test (`ui::typing_test`).
//!
//! The iOS keyboard types into a web view by calling `insertText:` (a
//! character, or `"\n"` for Return) and `deleteBackward` on the focused
//! text-input responder — WebKit's `WKContentView` — which turns each
//! call into the page's `beforeinput`/`input` events, exactly as for a
//! tap on a key. The typing test types through those same two calls, on
//! the app's own web view, so what it finds is what a person typing into
//! a note finds.
//!
//! Registered only for a typing-test launch (`TASK_TYPING_TEST=1`), and
//! compiled only into debug builds — `UIKeyboardImpl` is private, and a
//! release (TestFlight) build carries no trace of it. [`init`] is a no-op
//! elsewhere.

#[cfg(all(target_os = "ios", debug_assertions))]
mod ios {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{MainThreadMarker, Message as _};
    use objc2::{msg_send, sel};
    use objc2_foundation::{NSArray, NSString};
    use objc2_ui_kit::{UIApplication, UIView};

    /// The web view's text-input responder, if it holds the keyboard.
    #[allow(deprecated)] // `windows`: one scene, one window (see ios_auth).
    fn responder() -> Option<Retained<UIView>> {
        let mtm = MainThreadMarker::new()?;
        let app = UIApplication::sharedApplication(mtm);
        for window in app.windows().iter() {
            if let Some(v) = find(&window) {
                // A page's `focus()` gives the element focus but, without
                // a tap, iOS starts no input session: make the content view
                // the responder, as the tap would have.
                if !v.isFirstResponder() {
                    v.becomeFirstResponder();
                }
                return v.isFirstResponder().then_some(v);
            }
        }
        None
    }

    fn find(view: &UIView) -> Option<Retained<UIView>> {
        if view.class().name() == c"WKContentView" {
            return Some(view.retain());
        }
        let subviews: Retained<NSArray<UIView>> = view.subviews();
        subviews.iter().find_map(|v| find(&v))
    }

    /// The key's text, as the keyboard hands it to the web view once the
    /// page has let the key's `keydown` through (the test dispatches that
    /// first, as WebKit does — see `ui::typing_test`).
    pub fn type_text(text: &str) -> bool {
        let Some(view) = responder() else {
            return false;
        };
        let ns = NSString::from_str(text);
        // SAFETY: `WKContentView` adopts `UIKeyInput`; `insertText:` takes
        // an `NSString *` and returns nothing.
        unsafe {
            let obj: &AnyObject = &view;
            let _: () = msg_send![obj, insertText: &*ns];
        }
        true
    }

    pub fn backspace() -> bool {
        let Some(view) = responder() else {
            return false;
        };
        // SAFETY: `UIKeyInput`'s `deleteBackward` takes and returns nothing.
        unsafe {
            let obj: &AnyObject = &view;
            if obj.class().responds_to(sel!(deleteBackward)) {
                let _: () = msg_send![obj, deleteBackward];
            }
        }
        true
    }

    // ── focus without a tap ─────────────────────────────────────────
    //
    // iOS starts an input session — the keyboard, with keys that reach the
    // page — only for a focus it believes the user made. The test focuses
    // the note with the page's own `focus()` (it cannot tap), so in a test
    // launch WebKit is told every focus is the user's: the override
    // Cordova and Capacitor apps ship as `KeyboardDisplayRequiresUserAction
    // = false`, on the method of that name for this iOS.

    use std::sync::OnceLock;

    use objc2::runtime::{Bool, Imp, Sel};

    static ORIGINAL: OnceLock<usize> = OnceLock::new();

    type ElementDidFocus = unsafe extern "C-unwind" fn(
        *mut AnyObject,
        Sel,
        *mut std::ffi::c_void,
        Bool,
        Bool,
        u64,
        *mut AnyObject,
    );

    unsafe extern "C-unwind" fn element_did_focus(
        this: *mut AnyObject,
        cmd: Sel,
        element: *mut std::ffi::c_void,
        _user_is_interacting: Bool,
        blur_previous: Bool,
        activity_changes: u64,
        user_object: *mut AnyObject,
    ) {
        let Some(&imp) = ORIGINAL.get() else { return };
        // SAFETY: `imp` is the method's own implementation, saved when it
        // was replaced; the arguments are passed through unchanged but one.
        unsafe {
            let original: ElementDidFocus = std::mem::transmute(imp);
            original(
                this,
                cmd,
                element,
                Bool::YES,
                blur_previous,
                activity_changes,
                user_object,
            );
        }
    }

    /// Let a page's `focus()` start the input session, as a tap would.
    pub fn allow_programmatic_focus() {
        let Some(cls) = AnyClass::get(c"WKContentView") else {
            return;
        };
        for m in cls.instance_methods().iter() {
            let name = m.name().name().to_bytes();
            // `self`, `_cmd` and the five arguments.
            if name.starts_with(b"_elementDidFocus:userIsInteracting:") && m.arguments_count() == 7
            {
                // SAFETY: the replacement has the method's signature and
                // forwards to the original it replaces.
                unsafe {
                    let replacement: Imp =
                        std::mem::transmute::<ElementDidFocus, Imp>(element_did_focus);
                    let original = m.set_implementation(replacement);
                    let _ = ORIGINAL.set(original as usize);
                }
                tracing::info!(
                    selector = %String::from_utf8_lossy(name),
                    "typing test: page focus starts the keyboard"
                );
                return;
            }
        }
        tracing::warn!("typing test: no _elementDidFocus: on WKContentView; focus needs a tap");
    }

    /// Which way keys are being pressed, for the report.
    pub fn via() -> &'static str {
        "keydown to the page, then the web view's insertText:/deleteBackward"
    }
}

/// Register the keyboard with the typing test, when this launch is one.
pub fn init() {
    #[cfg(all(target_os = "ios", debug_assertions))]
    if ui::typing_test::requested() {
        ios::allow_programmatic_focus();
        ui::typing_test::set_keyboard(ui::typing_test::Keyboard {
            type_text: ios::type_text,
            backspace: ios::backspace,
            via: ios::via,
        });
        tracing::info!(
            via = ios::via(),
            "typing test: keys are pressed through the iOS keyboard"
        );
    }
}

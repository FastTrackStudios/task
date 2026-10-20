//! No `⌃ ⌄ ✓` bar over the keyboard.
//!
//! A web view puts its own input accessory bar — previous field, next
//! field, Done — on every text field it focuses. The app draws its own
//! bar above the keyboard (the markdown toolbar, with its own
//! hide-keyboard button: `ui::doc_editor::KeyboardToolbar`), so the
//! system one is a second bar stacked under it. Obsidian's iOS app,
//! also a web view, removes it the same way: WebKit's content view
//! (`WKContentView`, the responder that owns the keyboard) is made to
//! answer `inputAccessoryView` with nil.
//!
//! The method is added on that class alone (`class_replaceMethod`), so
//! no other responder in the app changes. The same module lets a page's own
//! `focus()` raise the keyboard (WebKit otherwise wants a tap first),
//! so capture opened from the Action button or a widget is ready to type. Everything is
//! `cfg(target_os = "ios")`; [`init`] is a no-op elsewhere.

#[cfg(target_os = "ios")]
mod ios {
    use objc2::ffi::class_replaceMethod;
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::sel;

    /// `-[WKContentView inputAccessoryView]`: none.
    unsafe extern "C-unwind" fn no_accessory(_this: *mut AnyObject, _cmd: Sel) -> *mut AnyObject {
        std::ptr::null_mut()
    }

    pub fn init() {
        let Some(cls) = AnyClass::get(c"WKContentView") else {
            tracing::warn!("keyboard bar: WKContentView not found; the system bar stays");
            return;
        };
        // SAFETY: the implementation matches the selector's signature
        // (`- (UIView *)inputAccessoryView`, type encoding `@@:`), and
        // returning nil is what the property's contract allows.
        unsafe {
            let imp: Imp = std::mem::transmute::<
                unsafe extern "C-unwind" fn(*mut AnyObject, Sel) -> *mut AnyObject,
                Imp,
            >(no_accessory);
            class_replaceMethod(
                std::ptr::from_ref(cls).cast_mut(),
                sel!(inputAccessoryView),
                imp,
                c"@@:".as_ptr(),
            );
        }
        tracing::info!("keyboard bar: the web view's accessory bar is off");
        allow_programmatic_focus();
    }

    use std::sync::OnceLock;

    use objc2::runtime::Bool;

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

    /// `-[WKContentView _elementDidFocus:userIsInteracting:…]`, told the
    /// person is interacting — so a page's `focus()` raises the keyboard.
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
        // was replaced; the arguments pass through unchanged but one.
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

    /// Let the page's own `focus()` start the keyboard, as a tap does —
    /// Cordova's `KeyboardDisplayRequiresUserAction = false`. Capture
    /// opened from the Action button or a widget arrives with no tap,
    /// and WebKit otherwise focuses the field without a keyboard.
    fn allow_programmatic_focus() {
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
                tracing::info!("keyboard: a page's focus() raises the keyboard");
                return;
            }
        }
        tracing::warn!("keyboard: no _elementDidFocus: on WKContentView; focus needs a tap");
    }
}

/// Remove the web view's accessory bar (iOS); no-op elsewhere.
pub fn init() {
    #[cfg(target_os = "ios")]
    ios::init();
}

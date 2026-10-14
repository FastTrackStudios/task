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
//! no other responder in the app changes. Everything is
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
    }
}

/// Remove the web view's accessory bar (iOS); no-op elsewhere.
pub fn init() {
    #[cfg(target_os = "ios")]
    ios::init();
}

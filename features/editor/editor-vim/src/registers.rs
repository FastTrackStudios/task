//! Registers: 26 named (`a..z`), unnamed (`"`), and system
//! clipboard (`+`/`*`) gated behind a callback.
//!
//! vim ref: codemirror-vim/src/vim.js (`RegisterController`)

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegisterKey {
    Named(char), // 'a'..'z'
    Unnamed,     // "
    SystemPlus,  // +
    SystemStar,  // *
}

impl RegisterKey {
    #[must_use]
    pub fn from_char(ch: char) -> Option<Self> {
        if ch.is_ascii_lowercase() {
            Some(Self::Named(ch))
        } else if ch == '"' {
            Some(Self::Unnamed)
        } else if ch == '+' {
            Some(Self::SystemPlus)
        } else if ch == '*' {
            Some(Self::SystemStar)
        } else {
            None
        }
    }
}

/// Stored register contents. `linewise` matters for paste
/// position; v1 records it but the paste path doesn't yet
/// differentiate.
#[derive(Clone, Debug, Default)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

/// Callback for the system clipboard. The host (e.g. the Dioxus
/// view) injects this so `editor-vim` stays DOM-free.
pub type Clipboard = Box<dyn Fn(ClipboardOp) -> Option<String> + Send + Sync>;

pub enum ClipboardOp<'a> {
    Read,
    Write(&'a str),
}

#[derive(Default)]
pub struct Registers {
    map: HashMap<RegisterKey, Register>,
    clipboard: Option<Clipboard>,
}

impl std::fmt::Debug for Registers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registers")
            .field("map", &self.map)
            .field("clipboard", &self.clipboard.is_some())
            .finish()
    }
}

impl Clone for Registers {
    /// Clipboard callback isn't cloneable in general (`Box<dyn Fn>`
    /// is not `Clone`); cloning a `Registers` drops it. The host
    /// re-injects on rehydrate. Fine for v1 — `VimState::clone()`
    /// is for snapshotting in tests / undo, not for live state.
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            clipboard: None,
        }
    }
}

impl Registers {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_clipboard(clipboard: Clipboard) -> Self {
        Self {
            map: HashMap::new(),
            clipboard: Some(clipboard),
        }
    }

    pub fn set_clipboard(&mut self, clipboard: Clipboard) {
        self.clipboard = Some(clipboard);
    }

    /// Write `text` to the unnamed register and, if `key` is
    /// `Some`, also to the named one. System-clipboard targets
    /// go through the injected callback (or no-op if missing).
    pub fn write(&mut self, key: Option<RegisterKey>, text: &str) {
        self.write_unnamed(text);
        if let Some(k) = key {
            match k {
                RegisterKey::SystemPlus | RegisterKey::SystemStar => {
                    if let Some(cb) = &self.clipboard {
                        let _ = cb(ClipboardOp::Write(text));
                    }
                }
                _ => {
                    self.map.insert(
                        k,
                        Register {
                            text: text.to_string(),
                            linewise: text.ends_with('\n'),
                        },
                    );
                }
            }
        }
    }

    pub fn write_unnamed(&mut self, text: &str) {
        self.map.insert(
            RegisterKey::Unnamed,
            Register {
                text: text.to_string(),
                linewise: text.ends_with('\n'),
            },
        );
    }

    /// Read the full [`Register`] entry (text + linewise flag),
    /// defaulting to the unnamed register. Clipboard reads come
    /// back charwise unless the payload ends in `\n`.
    #[must_use]
    pub fn read_full(&self, key: Option<RegisterKey>) -> Option<Register> {
        let key = key.unwrap_or(RegisterKey::Unnamed);
        match key {
            RegisterKey::SystemPlus | RegisterKey::SystemStar => self
                .clipboard
                .as_ref()
                .and_then(|cb| cb(ClipboardOp::Read))
                .map(|text| Register {
                    linewise: text.ends_with('\n'),
                    text,
                }),
            _ => self.map.get(&key).cloned(),
        }
    }

    /// Read from `key`, defaulting to the unnamed register.
    #[must_use]
    pub fn read(&self, key: Option<RegisterKey>) -> Option<String> {
        let key = key.unwrap_or(RegisterKey::Unnamed);
        match key {
            RegisterKey::SystemPlus | RegisterKey::SystemStar => {
                self.clipboard.as_ref().and_then(|cb| cb(ClipboardOp::Read))
            }
            _ => self.map.get(&key).map(|r| r.text.clone()),
        }
    }
}

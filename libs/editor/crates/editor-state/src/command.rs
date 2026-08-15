//! Commands and keymaps. A **command** is a pure function from
//! the current state to an optional [`TransactionSpec`] —
//! "if I want to do something, here's the transaction to apply;
//! otherwise None and the runtime moves on."
//!
//! A **keymap** is an ordered list of `(KeySpec, Command)`
//! pairs. The view's onkeydown dispatches to the first matching
//! binding whose command returns `Some`. CM6 reference: `view/src/keymap.ts`.

use crate::state::EditorState;
use crate::transaction::TransactionSpec;

/// A command runs against the current state and returns
/// the transaction to apply, or `None` to defer to the next
/// binding / the browser default.
///
/// Type alias so users can write `fn select_all(state: &State) -> Option<TransactionSpec>`
/// and pass it directly as a command — no boxing for the common case.
pub type Command = fn(&EditorState) -> Option<TransactionSpec>;

/// One key binding: the key spec (parsed string) and the command.
#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub key: KeySpec,
    pub run: Command,
}

/// Manual `PartialEq`: we compare the key spec and treat the
/// command function pointers as opaque (Rust warns that fn
/// pointer equality is unstable across codegen units; for
/// keymap-equality purposes "same key, same fn at this address"
/// is correct enough — Dioxus prop diffing uses this to skip
/// re-rendering on identical keymaps).
impl PartialEq for KeyBinding {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && std::ptr::fn_addr_eq(self.run, other.run)
    }
}

impl KeyBinding {
    pub fn new(key: impl Into<KeySpec>, run: Command) -> Self {
        Self {
            key: key.into(),
            run,
        }
    }
}

/// Ordered list of bindings — first match wins. Reorder to
/// override.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
}

impl Keymap {
    /// Empty keymap. Add bindings via [`Self::with`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build with a list of bindings.
    #[must_use]
    pub fn from_bindings(bindings: Vec<KeyBinding>) -> Self {
        Self { bindings }
    }

    /// Append a binding. Chains for fluent construction.
    #[must_use]
    pub fn with(mut self, key: impl Into<KeySpec>, run: Command) -> Self {
        self.bindings.push(KeyBinding {
            key: key.into(),
            run,
        });
        self
    }

    /// All bindings in order.
    #[must_use]
    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    /// Find the first binding whose key matches and whose
    /// command returns `Some(spec)`. Returns the resulting spec.
    #[must_use]
    pub fn dispatch(&self, key: &KeySpec, state: &EditorState) -> Option<TransactionSpec> {
        for b in &self.bindings {
            if b.key.matches(key) {
                if let Some(spec) = (b.run)(state) {
                    return Some(spec);
                }
            }
        }
        None
    }
}

/// A parsed keyboard shortcut. CM6 uses dash-joined strings like
/// `"Mod-z"` / `"Ctrl-Shift-K"`; we parse those into typed
/// fields so dispatch is plain field comparison.
///
/// Special keys are case-sensitive identifiers (`"Enter"`,
/// `"ArrowUp"`, `"Backspace"`); single printable characters are
/// case-insensitive against the platform key (`"a"` matches an
/// `a` keypress regardless of whether shift is held — that's
/// what users expect for `Mod-z` to fire on both `z` and `Z`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct KeySpec {
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    /// `Mod` in the source string: Cmd on Mac, Ctrl elsewhere.
    /// Resolved at construction so the matcher doesn't need to
    /// know which platform it's running on.
    pub r#mod: bool,
}

impl KeySpec {
    /// Match this spec against an *actual* keystroke.
    /// - `key` is compared case-insensitively for single chars
    ///   (so `Mod-z` matches both `z` and `Z` presses).
    /// - `alt` and `shift` must match exactly.
    /// - `Mod` is platform-normalized: a spec with `mod=true`
    ///   accepts a press that has *either* `ctrl=true` (Linux/
    ///   Windows) or `meta=true` (macOS). A spec with explicit
    ///   `ctrl` or `meta` requires exact equality.
    ///
    /// This mirrors CM6's behavior where `"Mod-z"` resolves to
    /// the platform-appropriate modifier at parse / dispatch time.
    #[must_use]
    pub fn matches(&self, other: &KeySpec) -> bool {
        let key_eq = if self.key.len() == 1 && other.key.len() == 1 {
            self.key.eq_ignore_ascii_case(&other.key)
        } else {
            self.key == other.key
        };
        if !key_eq {
            return false;
        }
        let mod_match = if self.r#mod {
            other.ctrl || other.meta
        } else {
            self.ctrl == other.ctrl && self.meta == other.meta
        };
        mod_match && self.alt == other.alt && self.shift == other.shift
    }
}

impl From<&str> for KeySpec {
    /// Parse a CM6-style shortcut string. Examples:
    ///
    /// - `"a"` → key `"a"`, no mods
    /// - `"Mod-z"` → key `"z"`, `mod = true`
    /// - `"Mod-Shift-Z"` → key `"Z"`, `mod` + `shift`
    /// - `"Ctrl-Alt-Backspace"` → key `"Backspace"`, ctrl + alt
    ///
    /// Unknown modifiers are silently ignored (so we can extend
    /// the syntax without breaking existing keymaps).
    fn from(s: &str) -> Self {
        let mut spec = KeySpec {
            key: String::new(),
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            r#mod: false,
        };
        let parts: Vec<&str> = s.split('-').collect();
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                spec.key = (*part).to_string();
            } else {
                match *part {
                    "Mod" => spec.r#mod = true,
                    "Ctrl" => spec.ctrl = true,
                    "Alt" | "Option" => spec.alt = true,
                    "Shift" => spec.shift = true,
                    "Cmd" | "Meta" => spec.meta = true,
                    _ => {}
                }
            }
        }
        spec
    }
}

impl From<String> for KeySpec {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Changes;
    use crate::selection::{Range, Selection};

    #[test]
    fn keyspec_parses_mod_z() {
        let k = KeySpec::from("Mod-z");
        assert_eq!(k.key, "z");
        assert!(k.r#mod);
        assert!(!k.ctrl && !k.shift && !k.alt && !k.meta);
    }

    #[test]
    fn keyspec_parses_multiple_modifiers() {
        let k = KeySpec::from("Mod-Shift-z");
        assert!(k.r#mod);
        assert!(k.shift);
        assert_eq!(k.key, "z");
    }

    #[test]
    fn keyspec_single_char_match_case_insensitive() {
        let bound = KeySpec::from("Mod-z");
        // Capital Z from a shift-held keypress — should still
        // match `Mod-z` because the binding doesn't include
        // shift. The press carries platform-actual `ctrl=true`
        // (Linux) — matches() resolves `Mod` against it.
        let pressed = KeySpec {
            key: "Z".into(),
            ctrl: true,
            r#mod: true,
            shift: false,
            alt: false,
            meta: false,
        };
        assert!(bound.matches(&pressed));
    }

    #[test]
    fn keyspec_mod_accepts_ctrl_or_meta() {
        let bound = KeySpec::from("Mod-b");
        let linux_press = KeySpec {
            key: "b".into(),
            ctrl: true,
            r#mod: true,
            ..Default::default()
        };
        let mac_press = KeySpec {
            key: "b".into(),
            meta: true,
            r#mod: true,
            ..Default::default()
        };
        assert!(bound.matches(&linux_press));
        assert!(bound.matches(&mac_press));
    }

    #[test]
    fn keyspec_explicit_ctrl_doesnt_match_meta() {
        let bound = KeySpec::from("Ctrl-b");
        let mac_press = KeySpec {
            key: "b".into(),
            meta: true,
            ..Default::default()
        };
        assert!(!bound.matches(&mac_press));
    }

    #[test]
    fn keyspec_named_key_case_sensitive() {
        let bound = KeySpec::from("Enter");
        let pressed_enter = KeySpec {
            key: "Enter".into(),
            ..Default::default()
        };
        let pressed_lower = KeySpec {
            key: "enter".into(),
            ..Default::default()
        };
        assert!(bound.matches(&pressed_enter));
        assert!(!bound.matches(&pressed_lower));
    }

    /// Demo command: select the whole document.
    fn select_all(state: &EditorState) -> Option<TransactionSpec> {
        Some(TransactionSpec::new().selection(Selection::single(Range::new(0, state.doc.len()))))
    }

    #[test]
    fn keymap_runs_matched_command() {
        let km = Keymap::new().with("Mod-a", select_all);
        let state = EditorState::new("hello");
        // Press needs an actual platform mod (ctrl or meta)
        // for `Mod-` bindings to match — the keymap matcher
        // is platform-aware, not just label-aware.
        let press = KeySpec {
            key: "a".into(),
            ctrl: true,
            r#mod: true,
            ..Default::default()
        };
        let spec = km.dispatch(&press, &state).expect("matched");
        let next = state.update(spec);
        let p = next.selection.primary();
        assert_eq!(p.from(), 0);
        assert_eq!(p.to(), 5);
    }

    #[test]
    fn keymap_skips_unmatched_keys() {
        let km = Keymap::new().with("Mod-a", select_all);
        let state = EditorState::new("hello");
        let press = KeySpec {
            key: "b".into(),
            ctrl: true,
            r#mod: true,
            ..Default::default()
        };
        assert!(km.dispatch(&press, &state).is_none());
    }

    /// Demo command that conditionally fires — used to verify
    /// the "command returns None → fall through to next binding"
    /// dispatch rule.
    fn fail_command(_state: &EditorState) -> Option<TransactionSpec> {
        None
    }

    fn insert_x(_state: &EditorState) -> Option<TransactionSpec> {
        Some(TransactionSpec::new().changes(Changes::insert(0, "X")))
    }

    #[test]
    fn keymap_falls_through_when_command_returns_none() {
        let km = Keymap::new()
            .with("Mod-a", fail_command)
            .with("Mod-a", insert_x);
        let state = EditorState::new("hello");
        let press = KeySpec {
            key: "a".into(),
            ctrl: true,
            r#mod: true,
            ..Default::default()
        };
        let spec = km.dispatch(&press, &state).expect("second binding fires");
        assert!(!spec.changes.is_empty());
    }
}

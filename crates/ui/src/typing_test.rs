//! Typing into a note on iOS, for real — the typing test.
//!
//! A launch with `TASK_TYPING_TEST=1` (the runner,
//! `apps/mobile/ios/typing-test.sh`, sets it) runs every case below in
//! the app itself: for each one it creates a note, opens it in the note
//! view, and types the case's keys through the iOS keyboard's own input
//! calls on the app's web view (`insertText:` / `deleteBackward` on
//! `WKContentView`, registered by the iOS crate as a [`Keyboard`]). Then
//! it checks three things:
//!
//! - **text** — the note's body is what those keys should make, the
//!   editor's own help included (a list continues on Return, a bracket
//!   pairs);
//! - **caret** — the editor's caret, the DOM's selection and the caret on
//!   screen all sit where the text says (after Return the caret is on the
//!   new line, not left behind);
//! - **render** — the construct renders as itself (a heading is `.md-h1`,
//!   a task has its checkbox, …).
//!
//! Progress and the report land in `$HOME/Documents/typing-test/` in the
//! app's container, where the runner reads them and takes a screenshot of
//! each case as it finishes. The notes it made are deleted at the end.

use std::sync::OnceLock;
use std::time::Duration;

use dioxus::prelude::*;
use serde::Serialize;

/// The iOS keyboard's two input calls, on the focused web view.
#[derive(Clone, Copy)]
pub struct Keyboard {
    /// `insertText:` — a character, or `"\n"` for Return. `false` when
    /// nothing holds the keyboard.
    pub type_text: fn(&str) -> bool,
    /// `deleteBackward`.
    pub backspace: fn() -> bool,
    /// Which input path is pressing keys, for the report.
    pub via: fn() -> &'static str,
}

static KEYBOARD: OnceLock<Keyboard> = OnceLock::new();

/// Whether this launch asked for the typing test.
pub fn requested() -> bool {
    std::env::var("TASK_TYPING_TEST").is_ok_and(|v| v == "1")
}

/// The platform's keyboard (the iOS crate registers it).
pub fn set_keyboard(k: Keyboard) {
    let _ = KEYBOARD.set(k);
}

/// Whether the test runs: asked for, and a keyboard to type with.
pub fn enabled() -> bool {
    requested() && KEYBOARD.get().is_some()
}

/// A trace launch (`TASK_TYPING_TRACE=1`): nothing is typed for you —
/// type yourself, and every key, input and selection event the note's
/// editor sees is written to `typing-test/trace.json` with the DOM's caret
/// and the editor's caret at that moment. For finding out what a real
/// keyboard does that the test's keys do not.
pub fn trace_requested() -> bool {
    std::env::var("TASK_TYPING_TRACE").is_ok_and(|v| v == "1")
}

const TRACE_JS: &str = r#"
if (!window.__taskTrace) {
  window.__taskTrace = [];
  const t0 = performance.now();
  const editor = () => document.querySelector('.note-body [data-editor-id]');
  const off = () => {
    const el = editor(), s = window.getSelection();
    if (!el || !s || !s.rangeCount || !el.contains(s.focusNode)) return null;
    const n = s.focusNode, tile = (n.nodeType === 3 ? n.parentElement : n).closest('[data-tile-pos]');
    return tile ? Number(tile.dataset.tilePos) + (n.nodeType === 3 ? s.focusOffset : 0) : null;
  };
  const rec = (e) => {
    const note = window.__taskNote || {};
    window.__taskTrace.push({
      t: Math.round(performance.now() - t0), type: e.type,
      key: e.key, inputType: e.inputType, data: e.data,
      trusted: e.isTrusted, prevented: e.defaultPrevented,
      dom: off(), head: note.head, body: note.body,
    });
  };
  for (const ty of ['keydown', 'keyup', 'beforeinput', 'input', 'compositionstart', 'compositionend']) {
    document.addEventListener(ty, rec, true);
    // After every handler: did the page cancel it?
    document.addEventListener(ty, (e) => { const l = window.__taskTrace[window.__taskTrace.length - 1]; if (l && l.type === e.type) l.preventedAfter = e.defaultPrevented; }, false);
  }
  document.addEventListener('selectionchange', rec, true);
}
const out = window.__taskTrace; window.__taskTrace = [];
return out;
"#;

/// Record the editor's events while someone types. Mount in the shell.
#[component]
pub fn TypingTraceRecorder() -> Element {
    use_hook(|| {
        spawn(async move {
            let mut all: Vec<serde_json::Value> = Vec::new();
            loop {
                sleep(400).await;
                if let Some(serde_json::Value::Array(batch)) = eval_json(TRACE_JS).await
                    && !batch.is_empty()
                {
                    all.extend(batch);
                    write_json("trace.json", &all);
                }
            }
        });
    });
    rsx! {}
}

// ── the cases ───────────────────────────────────────────────────────

/// One key, as a person presses it.
#[derive(Clone, Copy, Debug)]
pub enum Press {
    /// Typed one character at a time.
    Text(&'static str),
    Return,
    Backspace,
    /// Move the caret this many characters left (a person taps there; the
    /// keyboard has no arrows).
    Left(u32),
}
use Press::{Backspace, Left, Return, Text};

pub struct Case {
    pub name: &'static str,
    pub keys: &'static [Press],
    /// The body the keys should make.
    pub text: &'static str,
    /// Where the caret ends, in bytes; `None` = the end of `text`.
    pub head: Option<usize>,
    /// Selectors that must match inside the note's editor.
    pub render: &'static [&'static str],
}

const fn case(name: &'static str, keys: &'static [Press], text: &'static str) -> Case {
    Case {
        name,
        keys,
        text,
        head: None,
        render: &[],
    }
}

/// Every construct the editor shows, typed.
pub const CASES: &[Case] = &[
    case("plain text", &[Text("Hello world")], "Hello world"),
    case(
        "return moves the caret to the new line",
        &[Text("one"), Return, Text("two"), Return, Text("three")],
        "one\ntwo\nthree",
    ),
    case(
        "return twice leaves a blank line",
        &[Text("first"), Return, Return, Text("third")],
        "first\n\nthird",
    ),
    case(
        "return at the end of the text",
        &[Text("line"), Return],
        "line\n",
    ),
    Case {
        head: Some(4),
        ..case(
            "return mid-line splits it and the caret follows",
            &[Text("abcd"), Left(2), Return, Text("X")],
            "ab\nXcd",
        )
    },
    Case {
        render: &[".md-h1", ".md-h2", ".md-h3"],
        ..case(
            "headings",
            &[
                Text("# Title"),
                Return,
                Text("## Section"),
                Return,
                Text("### Sub"),
                Return,
                Text("body"),
            ],
            "# Title\n## Section\n### Sub\nbody",
        )
    },
    Case {
        render: &[".md-bold", ".md-italic", ".md-strike", ".md-highlight"],
        ..case(
            "bold italic strike highlight",
            &[Text("**bold** *it* ~~gone~~ ==hi== end")],
            "**bold** *it* ~~gone~~ ==hi== end",
        )
    },
    Case {
        render: &[".md-code"],
        ..case("inline code", &[Text("use `code` here")], "use `code` here")
    },
    Case {
        render: &[".md-list-item"],
        ..case(
            "bullet list continues and ends on an empty item",
            &[
                Text("- one"),
                Return,
                Text("two"),
                Return,
                Return,
                Text("after"),
            ],
            "- one\n- two\nafter",
        )
    },
    Case {
        render: &[".md-list-item"],
        ..case(
            "numbered list counts on",
            &[
                Text("1. first"),
                Return,
                Text("second"),
                Return,
                Text("third"),
            ],
            "1. first\n2. second\n3. third",
        )
    },
    Case {
        render: &[".md-task-checkbox"],
        ..case(
            "task list continues unchecked",
            &[Text("- [ ] buy strings"), Return, Text("tune")],
            "- [ ] buy strings\n- [ ] tune",
        )
    },
    Case {
        render: &[".md-blockquote"],
        ..case("blockquote", &[Text("> quoted words")], "> quoted words")
    },
    Case {
        render: &[".md-callout"],
        ..case(
            "callout continues its quote on return",
            &[Text("> [!note] Heads up"), Return, Text("the body")],
            "> [!note] Heads up\n> the body",
        )
    },
    Case {
        render: &[".md-wikilink"],
        ..case(
            "wikilink",
            &[Text("see [[Setlist]] now")],
            "see [[Setlist]] now",
        )
    },
    Case {
        render: &[".md-link"],
        ..case(
            "markdown link",
            &[Text("a [site](https://example.com) b")],
            "a [site](https://example.com) b",
        )
    },
    Case {
        render: &[".md-tag"],
        ..case(
            "tag",
            &[Text("tagged #mixing today")],
            "tagged #mixing today",
        )
    },
    Case {
        render: &[".md-math-inline, .md-math-widget-inline"],
        ..case(
            "inline math",
            &[Text("energy $E = m c^2$ ok")],
            "energy $E = m c^2$ ok",
        )
    },
    Case {
        render: &[".md-hr"],
        ..case(
            "rule",
            &[
                Text("above"),
                Return,
                Return,
                Text("---"),
                Return,
                Text("below"),
            ],
            "above\n\n---\nbelow",
        )
    },
    Case {
        render: &[".md-table"],
        ..case(
            // A table shows its source while the caret is in it (Obsidian's
            // way) and renders once the caret moves on.
            "table renders once the caret leaves it",
            &[
                Text("| a | b |"),
                Return,
                Text("|---|---|"),
                Return,
                Text("| 1 | 2 |"),
                Return,
                Return,
                Text("after"),
            ],
            "| a | b |\n|---|---|\n| 1 | 2 |\n\nafter",
        )
    },
    Case {
        render: &[".md-code-block"],
        ..case(
            "code block",
            &[Text("```"), Return, Text("let x = 1;"), Return, Text("```")],
            "```\nlet x = 1;\n```",
        )
    },
    Case {
        head: Some(2),
        ..case(
            "typing in the middle of a line",
            &[Text("ac"), Left(1), Text("b")],
            "abc",
        )
    },
    case(
        "backspace deletes and return-backspace rejoins",
        &[Text("abcx"), Backspace, Return, Backspace, Text("d")],
        "abcd",
    ),
];

// ── the run ─────────────────────────────────────────────────────────

/// What one case found.
#[derive(Serialize, Clone)]
pub struct Outcome {
    pub name: String,
    /// The input path that typed it.
    pub via: String,
    pub passed: bool,
    pub failures: Vec<String>,
    pub text: String,
    pub head: Option<usize>,
    pub dom_head: Option<usize>,
}

#[derive(Serialize)]
struct Progress<'a> {
    /// The case just finished (0-based) — the runner screenshots it.
    case: usize,
    total: usize,
    name: &'a str,
    done: bool,
}

fn out_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home)
        .join("Documents")
        .join("typing-test")
}

fn write_json(file: &str, value: &impl Serialize) {
    let dir = out_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(s) = serde_json::to_string_pretty(value) {
        let _ = std::fs::write(dir.join(file), s);
    }
}

async fn sleep(ms: u64) {
    architect::sleep(Duration::from_millis(ms)).await;
}

/// Evaluate `js` (which `return`s a JSON-able value) and read it back.
async fn eval_json(js: &str) -> Option<serde_json::Value> {
    let mut ev = document::eval(&format!("dioxus.send((() => {{ {js} }})());"));
    ev.recv::<serde_json::Value>().await.ok()
}

/// The note's editor, its text and caret as the editor holds them (the
/// note view's test mirror, `window.__taskNote`), the DOM's caret, and
/// where the caret is drawn against the line it should be on.
const PROBE: &str = r#"
const el = document.querySelector('.note-body [data-editor-id]');
const note = window.__taskNote || null;
if (!el || !note) return null;
function docOffset(node, off) {
  if (!node || !el.contains(node)) return null;
  if (node.nodeType === 3) {
    const tile = node.parentElement && node.parentElement.closest('[data-tile-pos]');
    if (!tile) return null;
    let before = 0;
    const walk = document.createTreeWalker(tile, NodeFilter.SHOW_TEXT);
    for (let t = walk.nextNode(); t && t !== node; t = walk.nextNode()) before += t.nodeValue.length;
    return Number(tile.dataset.tilePos) + before + off;
  }
  const own = node.closest && node.closest('[data-tile-pos]');
  if (own && (off === 0 || node.childNodes.length === 0)) return Number(own.dataset.tilePos);
  const kids = Array.from(node.childNodes);
  for (let i = Math.min(off, kids.length) - 1; i >= 0; i--) {
    const k = kids[i];
    const t = k.dataset && k.dataset.tilePos !== undefined ? k : (k.querySelector && k.querySelector('[data-tile-pos]'));
    if (t) return Number(t.dataset.tilePos) + Number(t.dataset.tileLen || 0);
  }
  return own ? Number(own.dataset.tilePos) : null;
}
const s = window.getSelection();
let domHead = null, caretY = null;
if (s && s.rangeCount) {
  domHead = docOffset(s.focusNode, s.focusOffset);
  const r = s.getRangeAt(0).cloneRange();
  r.collapse(false);
  let rect = r.getClientRects()[0];
  if (!rect || rect.height === 0) {
    const n = s.focusNode.nodeType === 1 ? s.focusNode : s.focusNode.parentElement;
    rect = n.getBoundingClientRect();
  }
  caretY = rect.top + rect.height / 2;
}
const lines = Array.from(el.querySelectorAll('div.cm-line[data-tile-pos]')).map((t) => {
  const b = t.getBoundingClientRect();
  return { pos: Number(t.dataset.tilePos), len: Number(t.dataset.tileLen || 0), top: b.top, bottom: b.bottom };
});
const focused = el === document.activeElement || el.contains(document.activeElement);
return { text: note.body, head: note.head, path: note.path, domHead, caretY, lines, focused };
"#;

#[derive(serde::Deserialize, Debug, Clone)]
struct Line {
    pos: usize,
    len: usize,
    top: f64,
    bottom: f64,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct Probe {
    text: String,
    head: usize,
    path: String,
    #[serde(rename = "domHead")]
    dom_head: Option<usize>,
    #[serde(rename = "caretY")]
    caret_y: Option<f64>,
    lines: Vec<Line>,
    focused: bool,
}

async fn probe() -> Option<Probe> {
    serde_json::from_value(eval_json(PROBE).await?).ok()
}

/// Read once the editor stops changing.
async fn settled() -> Option<Probe> {
    let mut last: Option<(String, usize, Option<usize>)> = None;
    let mut same = 0;
    for _ in 0..50 {
        let p = probe().await?;
        let key = (p.text.clone(), p.head, p.dom_head);
        if last.as_ref() == Some(&key) {
            same += 1;
            if same >= 3 {
                return Some(p);
            }
        } else {
            same = 0;
            last = Some(key);
        }
        sleep(120).await;
    }
    probe().await
}

/// Dispatch a key event (`keydown` / `keyup`) on the focused element;
/// whether no handler cancelled it.
async fn key_event(kind: &str, key: &str) -> bool {
    let k = serde_json::to_string(key).unwrap_or_default();
    let js = format!(
        "const t = document.activeElement || document.body; \
         return t.dispatchEvent(new KeyboardEvent('{kind}', \
           {{ key: {k}, bubbles: true, cancelable: true, composed: true }}));"
    );
    eval_json(&js)
        .await
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// One key, in WebKit's order for a key on either keyboard: `keydown`;
/// then, unless a handler cancelled it, the key's input (`input`); then
/// `keyup` — after the input, which is where a stale selection report
/// can overtake the editor's own edit.
async fn key(name: &str, input: impl FnOnce() -> bool) -> Result<(), String> {
    if !key_event("keydown", name).await {
        // A handler took the key: no input; the key-up follows at once.
        key_event("keyup", name).await;
        return Ok(());
    }
    // On a phone the `keyup` reaches the page before anything the
    // editor's Rust side sends back about the input (Rust runs outside
    // the page and answers asynchronously), which is where a stale
    // selection report overtakes the editor's own edit. Make that order
    // certain rather than likely: queue the `keyup` from inside the
    // input's own `beforeinput` turn (a microtask, after every handler),
    // so no reply can come first.
    let k = serde_json::to_string(name).unwrap_or_default();
    let _ = eval_json(&format!(
        "const k = {k}; \
         window.addEventListener('beforeinput', () => queueMicrotask(() => {{ \
           const t = document.activeElement || document.body; \
           t.dispatchEvent(new KeyboardEvent('keyup', {{ key: k, bubbles: true, cancelable: true, composed: true }})); \
         }}), {{ once: true }}); \
         return true;"
    ))
    .await;
    if !input() {
        return Err("nothing holds the keyboard".into());
    }
    Ok(())
}

async fn press(kb: Keyboard, key_press: Press) -> Result<(), String> {
    match key_press {
        Text(s) => {
            for ch in s.chars() {
                let ch = ch.to_string();
                key(&ch, || (kb.type_text)(&ch)).await?;
                sleep(45).await;
            }
        }
        Return => {
            key("Enter", || (kb.type_text)("\n")).await?;
            sleep(120).await;
        }
        Backspace => {
            key("Backspace", || (kb.backspace)()).await?;
            sleep(80).await;
        }
        Left(n) => {
            // A tap n characters back: the selection moves, the
            // keyboard stays.
            let _ = eval_json(&format!(
                "const s = window.getSelection(); for (let i = 0; i < {n}; i++) s.modify('move', 'backward', 'character'); return true;"
            ))
            .await;
            sleep(250).await;
        }
    }
    Ok(())
}

fn check(c: &Case, p: &Probe) -> Vec<String> {
    let want = c.head.unwrap_or(c.text.len());
    let mut f = Vec::new();
    if p.text != c.text {
        f.push(format!("text: got {:?}, want {:?}", p.text, c.text));
    }
    if p.head != want {
        f.push(format!("editor caret: got {}, want {want}", p.head));
    }
    if p.dom_head != Some(want) {
        f.push(format!("DOM caret: got {:?}, want {want}", p.dom_head));
    }
    if !p.focused {
        f.push("the editor lost focus (the keyboard went away)".into());
    }
    // The caret is drawn on the line that holds `want`.
    let line_of = |pos: usize| p.lines.iter().rposition(|l| l.pos <= pos);
    if let (Some(y), Some(i)) = (p.caret_y, line_of(want)) {
        let l = &p.lines[i];
        if y < l.top - 2.0 || y > l.bottom + 2.0 {
            let drawn = p
                .lines
                .iter()
                .position(|l| y >= l.top - 2.0 && y <= l.bottom + 2.0);
            f.push(format!(
                "caret drawn on line {}, not line {} (where the text puts it)",
                drawn.map_or("?".to_owned(), |d| (d + 1).to_string()),
                i + 1
            ));
        }
        let _ = l.len;
    }
    f
}

async fn run_case(
    i: usize,
    c: &Case,
    kb: Keyboard,
    slug: &str,
    nav: dioxus::router::Navigator,
) -> Result<Outcome, String> {
    let path = format!("Typing test {:02}.md", i + 1);
    let client = crate::vox_clients::vault_client(slug).await?;
    let _ = client
        .delete_file(
            crate::document_session::VAULT_ID.to_owned(),
            path.clone(),
            vault_proto::IfMatch::Force,
        )
        .await;
    crate::pages::vault::rpc::create_new_file(
        slug.to_owned(),
        crate::document_session::VAULT_ID.to_owned(),
        path.clone(),
    )
    .await?;
    nav.push(crate::routes::Route::VaultRoute {
        path: path.clone(),
        org: String::new(),
    });

    // The note view has the note, empty.
    let mut ready = false;
    for _ in 0..80 {
        if let Some(p) = probe().await
            && p.path == path
            && p.text.is_empty()
        {
            ready = true;
            break;
        }
        sleep(150).await;
    }
    if !ready {
        return Err("the note never opened".into());
    }
    // Focus the body, caret at its start: a fresh note may take the caret
    // on its own (the shell's focus rule allows it).
    let _ = eval_json(
        "const e = document.querySelector('.note-body [data-editor-id]'); e.focus(); \
         const r = document.createRange(); r.selectNodeContents(e); r.collapse(true); \
         const s = window.getSelection(); s.removeAllRanges(); s.addRange(r); return true;",
    )
    .await;
    sleep(600).await;

    for &k in c.keys {
        press(kb, k).await?;
    }
    let p = settled().await.ok_or("the editor could not be read")?;
    let mut failures = check(c, &p);
    for sel in c.render {
        let js = format!(
            "return document.querySelector('.note-body [data-editor-id]').querySelectorAll({}).length;",
            serde_json::to_string(sel).unwrap_or_default()
        );
        let n = eval_json(&js).await.and_then(|v| v.as_u64()).unwrap_or(0);
        if n == 0 {
            failures.push(format!("render: nothing matches {sel}"));
        }
    }
    Ok(Outcome {
        name: c.name.to_owned(),
        via: (kb.via)().to_owned(),
        passed: failures.is_empty(),
        failures,
        text: p.text,
        head: Some(p.head),
        dom_head: p.dom_head,
    })
}

/// Runs the typing test once, when this launch is one. Mount in the shell.
#[component]
pub fn TypingTestRunner() -> Element {
    let nav = use_navigator();
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    use_hook(move || {
        let Some(&kb) = KEYBOARD.get() else { return };
        spawn(async move {
            let _ = std::fs::remove_dir_all(out_dir());
            // Let the node boot and the org list arrive.
            let mut slug = String::new();
            for _ in 0..100 {
                slug = crate::orgs::active_slug(&selection.peek(), &org_list.peek());
                if !slug.is_empty() {
                    break;
                }
                sleep(200).await;
            }
            sleep(1500).await;
            let total = CASES.len();
            let mut outcomes = Vec::new();
            for (i, c) in CASES.iter().enumerate() {
                tracing::info!(case = c.name, "typing test: case");
                let outcome = match run_case(i, c, kb, &slug, nav).await {
                    Ok(o) => o,
                    Err(e) => Outcome {
                        name: c.name.to_owned(),
                        via: (kb.via)().to_owned(),
                        passed: false,
                        failures: vec![format!("error: {e}")],
                        text: String::new(),
                        head: None,
                        dom_head: None,
                    },
                };
                outcomes.push(outcome);
                write_json(
                    "progress.json",
                    &Progress {
                        case: i,
                        total,
                        name: c.name,
                        done: false,
                    },
                );
                // Room for the runner's screenshot of this case.
                sleep(1500).await;
            }
            // Leave nothing behind.
            if let Ok(client) = crate::vox_clients::vault_client(&slug).await {
                for i in 0..total {
                    let _ = client
                        .delete_file(
                            crate::document_session::VAULT_ID.to_owned(),
                            format!("Typing test {:02}.md", i + 1),
                            vault_proto::IfMatch::Force,
                        )
                        .await;
                }
            }
            let passed = outcomes.iter().filter(|o| o.passed).count();
            tracing::info!(passed, total, "typing test: done");
            write_json("report.json", &outcomes);
            write_json(
                "progress.json",
                &Progress {
                    case: total,
                    total,
                    name: "",
                    done: true,
                },
            );
        });
    });
    rsx! {}
}

/// The note view's mirror for the test: its path, body and caret, kept on
/// `window.__taskNote` while a typing test runs.
pub fn publish_note(path: &str, body: &str, head: usize) {
    if !enabled() && !trace_requested() {
        return;
    }
    let js = format!(
        "window.__taskNote = {{ path: {}, body: {}, head: {head} }};",
        serde_json::to_string(path).unwrap_or_default(),
        serde_json::to_string(body).unwrap_or_default(),
    );
    let _ = document::eval(&js);
}

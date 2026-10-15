# Documents Spec

A task's description and a vault note are the same kind of thing: a
markdown document with properties beside it. They are written in one
editor and their properties wear one look, on a desk and on a phone, so
moving between an issue and a note never changes how writing feels.

The surfaces: `crates/ui/src/doc_editor.rs` (the editor, its slash
catalog, its keymap and its typography), `crates/ui/src/pages/task_detail.rs`
(an issue's description), `crates/ui/src/pages/note_view.rs` (a vault
note), `crates/ui/src/pages/note_properties.rs` (a note's properties) and
`features/task/task-ui/src/views/properties.rs` (the shared property row
and chip).

The model is Linear's issue page — title, then the body, then the
properties in a column on a desk and a chip row under the title on a
phone — with one deliberate difference: Linear hides its markdown, and
Task keeps it live. The marks stay in the text, dimmed, so a document is
a plain file anywhere else.

---

## The editor

### One editor for tasks and notes

t[doc.editor.one]
A task's description is edited with the same markdown editor as a vault
note — `editor::Editor` with its `/` palette, the document keymap and the
document typography — not a plain text field. The same component serves
the desk and the phone. The seeded *Master review with the label* task
opens on a description with headings, a checklist, a quote, a code block
and a folded section.

---

### The body only

t[doc.editor.body-only]
The editor holds a task's body and never its frontmatter: properties are
the task service's to write, and are changed through the property chips.
A body commits as a task update (the whole record, `details` changed)
1.5 seconds after typing stops, on blur, and when the page goes, against
the store's current row so a property changed meanwhile is not written
back over. What the editor wrote is what comes back, byte for byte — no
blank first line gained per save. A change arriving from elsewhere is
taken into the buffer only when no local edit is outstanding; otherwise
the local text is written over it on the next commit (last writer wins,
the task service's own rule for a record).

---

### The slash menu

t[doc.editor.slash]
`/` at the start of a line or after a space opens the document palette.
It lists Linear's groups first, in Linear's order — **Text**: Heading 1,
Heading 2, Heading 3, Bulleted list, Numbered list, Checklist;
**Insert**: Image, Attach files, Code block, Diagram, Collapsible section,
Blockquote, Callout, Divider, Table — each with its shortcut on the
right, then the markdown extras a wiki page needs (study blocks, callout
kinds, links, math). Nothing is listed twice; arrows move, Enter picks,
Escape closes. Every insert writes plain markdown.

---

### The shortcuts

t[doc.editor.shortcuts]
The shortcuts the menu shows work in the editor: ⌘⌥1–3 headings, ⌘⇧8
bullets, ⌘⇧9 numbers, ⌘⇧7 checklist, ⌘⇧\\ code block, ⌘⇧6 collapsible
section, ⌘⇧U attach, ⌥⇧. blockquote — under every key a browser may
report for the chord (a Mac reports ⌘⌥1 as `¡`).

---

### The typography

t[doc.editor.typography]
Both editors read at Linear's measures: body 15/24 at weight 450 with
−0.1px tracking (16px on a touch screen, so iOS never zooms); H1 22/29.6,
H2 19/28, H3 16/24, all weight 600; a blank line between blocks is a
16px gap rather than a full row; titles 24/32 weight 600. The slash menu
is a 12px-radius popover of 13px medium rows with the shortcut muted on
the right.

---

## Properties

### One look for every record's properties

t[doc.properties.shared]
A note's properties use the task's property system: on a desk, a column
of rows with the name muted on the left and a chip-like value on the
right (the shared `PropertyRow`); on a phone, a row of chips under the
title (the shared chip classes), which opens the full panel in place.
Editing a note's property still rewrites only that property's bytes in
its frontmatter.

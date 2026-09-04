# SSG Spec

Publishing a vault — a directory of markdown notes with frontmatter and
`[[wikilink]]` cross-references — as a static site.

The behaviour these rules exist to replace: four sites each grew their own
build script and their own renderer for the same job, and drifted into three
different answers. Keyflow rendered its guide through the *editor* in read-only
mode, so reading a paragraph meant first loading the editor, its state machine,
its decoration pipeline and a WebGL2 chart surface. Signal shipped raw markdown
and parsed it in the browser on every visit. Ignition alone rendered at build
time.

Editing a vault is the Task app's job. This feature only reads.

---

## Rendering

### A page is finished before it is served

t[ssg.render.finished]
What a reader receives is complete HTML: prose, resolved cross-references and
expanded fences, produced at build time. No markdown parser, no vault data and
no rendering code is shipped to the browser in order to display a page that has
not changed since the build.

---

### A cross-reference resolves or fails the build

t[ssg.render.links]
A `[[wikilink]]` naming a page in the same vault becomes an ordinary link to
that page's URL. One naming no page fails the build, reporting every broken
reference with the note it appears in. A build may opt into a warning instead,
but never silently emits a link that leads nowhere.

---

### Wikilink syntax inside code is text

t[ssg.render.code-verbatim]
`[[…]]` inside a fenced block or a code span is left exactly as written. A note
documenting the syntax renders what the author typed.

---

### A site substitutes its own fence rendering

t[ssg.render.fences]
A fenced block's markup can be supplied by the site rather than the renderer,
chosen by the fence's info string, and is produced at build time like the rest
of the page. A site whose fences are charts, diagrams or scores ships their
rendered output, not a renderer.

---

### Metadata does not reach the reader

t[ssg.render.metadata]
Frontmatter and a note's trailing navigation footer are not part of its prose.
Navigation is drawn from the vault's own ordering, so it renders once.

---

## Ordering and structure

### Reading order is declared, and reproducible

t[ssg.order.reading]
Pages are ordered by their frontmatter `order:`, ties broken by slug, and a note
without one sorts last. Two builds of the same vault produce byte-identical
output regardless of the order the filesystem lists the directory in.

---

### The graph is derived, not authored

t[ssg.order.backlinks]
Backlinks, chapter navigation, stage grouping and the link graph are computed
from the notes. An author writes a cross-reference once, in one direction.

---

## Build

### An empty vault fails the build

t[ssg.build.non-empty]
A vault directory that is missing, unreadable, or holds no markdown fails the
build with its path. Shipping an empty guide is never the result of a
successful build.

---

### Editing a note rebuilds the site

t[ssg.build.rerun]
Every note read is declared to cargo, individually. Editing, adding or deleting
one triggers a rebuild of the site that publishes it.

---

## Output

### A pre-rendered page is complete before its bundle arrives

t[ssg.output.prerendered]
A pre-rendered route's HTML carries the finished page — prose, resolved
cross-references, expanded fences — so a browser paints it without executing
anything. The wasm bundle then hydrates it into the running app. Static
generation is Dioxus's own (`dx build --ssg` against a `static_routes` server
function); nothing here reimplements it.

---

### Pre-rendered components are pure

t[ssg.output.hydratable]
A component that renders into a pre-rendered route is a function of `&'static`
data alone — no hooks, no state, no I/O. The client's first render is therefore
identical to the server's, which is the condition hydration requires; a
component that cannot promise that belongs on a route that is not pre-rendered.

---

### The vault enumerates its own routes

t[ssg.output.routes]
A vault can list every URL it publishes, and that list is what the site returns
from its `static_routes` server function. A parameterised route is absent from
the router's own `static_routes()` — only the vault knows its slugs — so
supplying them is what makes the generation *partial*: those paths are
pre-rendered, every other route in the app is untouched.

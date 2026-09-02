//! The seam an app plugs into Task through.
//!
//! Task's core — the vault, files, orgs, auth, sync — is the platform.
//! A **plugin app** is a domain on top of it: Cooking, Session,
//! Keyflow, Signal. It brings its own screens and its own vocabulary,
//! and it keeps its data in Task's: markdown notes in the vault, File
//! Roots on disk. That is the trade the whole design turns on — an app
//! gets the file management, sync, sharing and version history for
//! free, and in exchange it stores nothing only it can read.
//!
//! # Why a registry and not a route variant
//!
//! [`crate::nav`] already records why a feature crate cannot name a
//! route: `Route` is one enum in the shell, and a crate outside it
//! cannot add a variant. That module solved the outbound half — a
//! feature linking *out* of itself, via href builders handed down as
//! context.
//!
//! This is the inbound half. The shell keeps one catch-all route,
//! `/app/<id>/<rest>`, and dispatches it here; a plugin supplies a
//! function from its own sub-path to an [`Element`] and never names a
//! route at all. So the routing stays typed where the shell owns it and
//! stringly-typed exactly at the boundary an external crate reaches.
//!
//! # The dependency direction
//!
//! A plugin depends on this crate. This crate depends on no plugin, and
//! the *only* place that names every plugin is the app binary that
//! registers them:
//!
//! ```ignore
//! // apps/desktop/src/main.rs — the composition root, and the one
//! // crate that knows the full list.
//! task_ui_core::plugin::register(cooking_task_plugin::APP);
//! task_ui_core::plugin::register(session_task_plugin::APP);
//! ```
//!
//! Cargo cares about crate cycles, not repo cycles, so a plugin repo
//! that also depends on Task's server API is fine. What must never
//! happen is registration leaking down here or into `task-ui` — the
//! moment this crate names a plugin, the extension point is gone and
//! it is just more coupling with extra steps.
//!
//! # One Dioxus
//!
//! Widgets crossing a crate boundary means every plugin must compile
//! against the byte-identical `dioxus` and `architect-ui`; a skew makes
//! [`Element`] a different type and nothing composes, with an error
//! that never mentions versions. So plugins use [`dioxus`] and
//! [`architect_ui`] *re-exported from here* rather than declaring their
//! own — then there is one version by construction and skew cannot
//! happen.

use dioxus::prelude::*;

/// The component library, re-exported for the same reason.
pub use architect_ui;
/// Dioxus, for plugins to use instead of their own dependency — see the
/// module docs on why that matters.
pub use dioxus;
/// The note-widget vocabulary, re-exported for the same reason — a
/// `WidgetSpec` built against a different version is a different type.
pub use task_widgets;

/// One screen an app contributes to Task's navigation.
#[derive(Clone, Copy)]
pub struct PluginNav {
    /// What the tab says.
    pub label: &'static str,
    /// Its icon. A function rather than an `Element` because an
    /// `Element` cannot be built outside a render.
    pub icon: fn() -> Element,
    /// The app's own path for this screen — `""` is its front page,
    /// `"setlists"` a section within it. Never a Task route: the shell
    /// turns this into `/app/<id>/<path>`.
    pub path: &'static str,
    /// Ask for a slot in the icon rail — the narrow always-visible
    /// strip, not the full sidebar.
    ///
    /// The rail is deliberately short: a dozen destinations somebody
    /// reaches constantly, not everything they have. So this is opt-in
    /// and most screens should leave it `false` — an app with three
    /// nav entries that asks for three rail slots has made the rail
    /// worse for everybody, including itself.
    ///
    /// It is a request, not a guarantee: the shell owns the rail and
    /// may still not have room.
    pub rail: bool,
}

/// Where a claimed link goes — a screen inside the claiming app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    /// The app's own path, as [`PluginApp::view`] will receive it.
    pub path: String,
    /// Its query, empty when there is none.
    pub query: String,
}

impl LinkTarget {
    /// The app's front page, with a deep link in the query — the common
    /// case: a reference, a song, a passage.
    #[must_use]
    pub fn query(query: impl Into<String>) -> Self {
        Self {
            path: String::new(),
            query: query.into(),
        }
    }

    /// A specific screen with no parameters.
    #[must_use]
    pub fn path(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            query: String::new(),
        }
    }

    /// The app's front page with one parameter, encoded.
    ///
    /// Use this rather than `format!("k={v}")` for anything a person
    /// typed. `John 3:16-20@ESV` is a perfectly ordinary value and it
    /// contains a space; pasted into a URL raw, the space ends the URL
    /// and the passage arrives as `John`.
    #[must_use]
    pub fn param(key: &str, value: &str) -> Self {
        Self::query(format!("{key}={}", encode(value)))
    }
}

/// A link to one of an app's **own** screens.
///
/// Apps with more than one screen navigate between them, and until
/// this existed they could not: [`LinkTarget`] describes where a
/// *claimed link* goes and is handed back to the shell to route, which
/// is no help to an app that just wants an `href` for a button. The
/// shape is the shell's (`/app/<id>/<path>`), so the shell should be
/// the one that knows it — but the app is the one that needs to write
/// it, so it lives here, in the crate they both depend on.
///
/// `path` is the app's own, exactly as [`PluginApp::view`] will
/// receive it. `query` is the app's own query string — already in
/// `k=v&k=v` form, with **each value** [`encode`]d by the app if it
/// could contain a `&` or an `=`.
///
/// [`pack`] carries the whole thing to the app unchanged, so the URL
/// is not what needs the escaping — [`query_param`] is. It splits on
/// `&`, and cannot tell the app's separator from one inside a value.
/// For the common single-parameter case use [`href_param`], which
/// cannot get that wrong.
///
/// ```
/// # use task_plugin_ui::href;
/// assert_eq!(href("mealplan", "shopping", ""), "/app/mealplan/shopping");
/// ```
#[must_use]
pub fn href(app: &str, path: &str, query: &str) -> String {
    let mut out = format!("/app/{app}");
    if !path.is_empty() {
        out.push('/');
        out.push_str(path);
    }
    if !query.is_empty() {
        out.push_str("?q=");
        out.push_str(&pack(query));
    }
    out
}

/// An app's query, as the single opaque `q` the shell's route carries.
///
/// Base64url, and **not** percent-encoding, for a reason worth writing
/// down because the obvious choice is wrong. The router decodes `q`
/// when it parses a URL and re-serialises the route without encoding it
/// again, so one round trip through the address bar turns a
/// percent-escaped `%26` into a literal `&` — which then ends the
/// parameter, and every value after the first silently disappears. It
/// looks like the app was handed a one-parameter link.
///
/// Base64url has no `%`, no `&` and no `=` (padding is omitted), so
/// there is nothing left for a decode to reinterpret and the round trip
/// is lossless however many times it happens.
#[must_use]
pub fn pack(query: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(query.as_bytes())
}

/// Undo [`pack`]. A `q` that is not valid base64url comes back as
/// itself, so a hand-typed `/app/mealplan?q=dish=Bolognese` still
/// works — a debugging convenience that costs nothing, since a packed
/// query never looks like a readable one.
#[must_use]
pub fn unpack(q: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(q.as_bytes())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| decode(q))
}

/// A link to one of an app's own screens carrying one parameter.
///
/// The common case, and the one that cannot be got wrong: the value is
/// encoded before it becomes part of a query, so a `&` or an `=` in it
/// — a recipe called `Ragu & Chips`, a path with a query-ish name — is
/// a character rather than a separator.
///
/// ```
/// # use task_plugin_ui::{href_param, query_param, unpack};
/// let url = href_param("mealplan", "recipe/read", "path", "Ragu & Chips.cook");
/// let q = unpack(url.split_once("?q=").unwrap().1);
/// assert_eq!(query_param(&q, "path").as_deref(), Some("Ragu & Chips.cook"));
/// ```
#[must_use]
pub fn href_param(app: &str, path: &str, key: &str, value: &str) -> String {
    href(app, path, &format!("{key}={}", encode(value)))
}

/// Percent-encode everything that is not unreserved.
///
/// Deliberately conservative: it is never wrong to encode a character
/// that did not need it, and the set of characters that *do* need it
/// varies by where in a URL you are. Plugins should not have to know
/// that.
#[must_use]
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Read one parameter out of a query string, decoded.
///
/// The shell hands an app its query verbatim — it has no idea what the
/// keys mean, which is what keeps it from having to. This is the other
/// half: the app names its key and gets its value back the way it was
/// written.
///
/// Returns `None` for a key that is absent, and `Some("")` for one
/// present and empty, because those are different questions.
#[must_use]
pub fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| decode(v))
}

/// Undo [`encode`], and `+` for a space — forms send that, and a query
/// that came back through one should still read.
///
/// A malformed escape is left as written rather than dropped. Losing
/// characters silently is worse than showing a stray `%`.
#[must_use]
pub fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How strongly an app claims a link.
///
/// The distinction is about the *text*, not the app, and only the app
/// can judge it. `John 3:16` is a scripture reference and nothing else —
/// a note of that name would be somebody trying to write about the
/// verse, not to replace it. `Washed` is a song and also a perfectly
/// ordinary thing to call a note.
///
/// So an app says which it is, per text, and the shell orders
/// accordingly. Making this a property of the whole app would force
/// scripture to choose between shadowing every note and never resolving
/// a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// Only when the vault has no page of that name. The safe kind, and
    /// the right one whenever a person could reasonably mean something
    /// else by the words.
    IfUnknown(LinkTarget),
    /// Always, even over a vault page. For text that has exactly one
    /// meaning — a chapter-and-verse reference, an ISBN, a timestamp —
    /// where a page of the same name is a note *about* the thing rather
    /// than a replacement for it.
    Always(LinkTarget),
}

impl Claim {
    /// Where it goes, whichever kind it is.
    #[must_use]
    pub fn target(&self) -> &LinkTarget {
        match self {
            Self::IfUnknown(t) | Self::Always(t) => t,
        }
    }

    /// Does this beat a vault page of the same name?
    #[must_use]
    pub fn beats_a_page(&self) -> bool {
        matches!(self, Self::Always(_))
    }
}

/// An app registered into Task.
#[derive(Clone, Copy)]
pub struct PluginApp {
    /// Matches the `task-plugin` catalog id, which is what an org's
    /// manifest turns on and off. An app whose id is not enabled for
    /// the active org contributes nothing — it is not merely hidden,
    /// its screens do not resolve.
    pub id: &'static str,
    /// What this build of the app calls itself — `env!("CARGO_PKG_VERSION")`
    /// in nearly every case.
    ///
    /// Apps release on their own schedules, from their own repositories,
    /// so "which Session is this?" is a real question with no other way
    /// to answer it: two machines can be running the same Task and
    /// different apps, and the difference is invisible without this.
    ///
    /// It is **not** a compatibility gate, and there is deliberately no
    /// runtime check against it. A plugin is linked into the binary, so
    /// a version of the SDK it cannot work with is a compile error, not
    /// a thing to discover at startup — a check here could only ever
    /// re-report what the compiler already refused. What the version is
    /// for is being *told*: in the settings list, in a log line, and in
    /// whatever an app stamps on the data it writes.
    pub version: &'static str,
    /// The screens it puts in the navigation. May be empty: an app can
    /// be reachable only from another app's link, or from a file.
    pub nav: &'static [PluginNav],
    /// Render one of its screens.
    ///
    /// `path` is whatever followed `/app/<id>/`, empty for the front
    /// page. `query` is the raw query string, empty when there is none —
    /// a deep link like a scripture reference or a note path arrives
    /// there, and the app parses it, because only the app knows what
    /// its own parameters mean.
    ///
    /// Returning `None` means "not one of mine" and the shell shows its
    /// own not-found rather than the app pretending to have a page.
    pub view: fn(path: &str, query: &str) -> Option<Element>,
    /// What this app puts in the docked side panel.
    ///
    /// The panel is the strip that stays open beside whatever page you
    /// are on — the place for something you refer to *while* doing
    /// something else. The shell owns the dock itself (the toggle, the
    /// width, the drag handle, remembering both); an app fills it.
    ///
    /// This exists because a panel is the one surface where "the app is
    /// off" was not previously expressible. A screen that does not
    /// resolve is obvious, and a nav tab that is not there is obvious.
    /// A dock rendered by the shell has no such state — an org with the
    /// app disabled got the panel anyway, empty, with no way to tell
    /// that from it being broken.
    ///
    /// Rendered only for enabled apps, so turning the app off empties
    /// the dock, which is the honest answer.
    pub panel: Option<fn() -> Element>,
    /// Which **files** this app opens.
    ///
    /// Task's vault is files, and not all of them are notes. A `.cook`
    /// recipe shown in the note editor is raw cooklang; it wants the
    /// recipe reader. The shell knew that, by extension, in three
    /// different places — a base row, a file list, the schedule
    /// overlay — and each was the shell holding a fact about an app.
    ///
    /// Now the app says so. Given a vault-relative path, return where
    /// it opens, or `None` to leave it to the vault. Keep it to files
    /// this app genuinely owns the reading of: claiming `.md` would
    /// take the vault away from everybody.
    pub claim_file: Option<fn(path: &str) -> Option<LinkTarget>>,
    /// Install this app's contexts at the app root — its
    /// `task_stores::stores!` `provide_stores()` in nearly every case.
    ///
    /// It has to happen at the *root*, not on the app's own screen,
    /// and that is the whole reason this is a contribution rather than
    /// something the app does for itself. A store provided when a page
    /// mounts is thrown away when that page unmounts, taking the cache
    /// and any in-flight optimistic write with it; and an app's rows
    /// are wanted in places its screens are not — a note widget, a
    /// search result, another app's page.
    ///
    /// Called once per launch, for every registered app, whether or not
    /// anybody visits it. Keep it to providing context: it runs during
    /// the root render, so work here is on the path to first paint for
    /// people who do not use this app at all. Subscriptions are fine —
    /// the store driver starts them lazily and heals them itself.
    pub provide: Option<fn()>,
    /// How this app's notes render *inside the editor*.
    ///
    /// A screen is the small half of what an app contributes. The
    /// larger half is what a note of its kind looks like when somebody
    /// opens it: a song note that is a player, a setlist that is a
    /// queue, a recipe that is a method with its own timers. A
    /// [`task_widgets::WidgetSpec`] claims notes by type or frontmatter,
    /// renders inline, handles its own link clicks, and can take over
    /// the body in fullscreen.
    ///
    /// This is what keeps markdown the substrate rather than a storage
    /// format nobody reads. The note stays a note — openable in any
    /// editor, syncable, diffable — and the app supplies the way to
    /// *look* at it.
    ///
    /// A function rather than a list because a `WidgetSpec` holds
    /// closures and cannot be a `const`.
    pub widgets: Option<fn() -> Vec<task_widgets::WidgetSpec>>,
    /// Claim a wikilink that matches no vault page.
    ///
    /// A note writes `[[John 3:16]]` or `[[Washed]]`, and if the vault
    /// has no such page, whichever app recognises the text takes it: the
    /// scripture reader opens the passage, the player opens the song.
    /// That is how one app links into another's material without either
    /// knowing the other exists — the note says what it means and the
    /// registry finds who understands it.
    ///
    /// The app returns a [`Claim`] saying how strongly it means it —
    /// see there for why that judgement belongs to the app and not to
    /// the shell.
    pub claim_link: Option<fn(text: &str) -> Option<Claim>>,
    /// Claim a link scheme this app's own widgets emit —
    /// `scripture-open:`, `song-play:`.
    ///
    /// Separate from [`Self::claim_link`] because the resolution order
    /// is different: a scheme is unambiguous and is claimed *before* the
    /// vault is consulted, where a wikilink is claimed only after.
    pub claim_href: Option<fn(href: &str) -> Option<LinkTarget>>,
    /// Code fences this app renders — ```` ```kf ```` and the like.
    ///
    /// Separate from [`Self::widgets`] because the editor's fence
    /// registry is its own seam, added so `editor-state` could render a
    /// chart without depending on a music engraver. Same principle one
    /// level down: the editor knows there are fences, not what any of
    /// them mean.
    pub fences: Option<fn()>,
}

impl PluginApp {
    /// What this app should stamp on anything it writes —
    /// `mealplan@0.4.1`.
    ///
    /// The convention is an `app:` key in a note's frontmatter, and it
    /// is what makes a version worth having. A note is markdown that
    /// outlives every build that touched it: a recipe written by
    /// `mealplan@0.2` will be opened by `mealplan@1.0` years later, and
    /// the only way that version can know what shape to expect — or
    /// whether to migrate it — is if the note says who wrote it.
    ///
    /// The same reason a project records who made it. Provenance is
    /// cheap at the moment of writing and impossible to reconstruct
    /// afterwards.
    #[must_use]
    pub fn stamp(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }
}

/// Everything registered so far.
///
/// A `RwLock` rather than a `OnceLock<Vec<_>>` because registration
/// happens in `main` before the first render but there is no single
/// call that could take the whole list — each plugin registers itself,
/// and a build with different features registers a different set.
static REGISTRY: std::sync::RwLock<Vec<PluginApp>> = std::sync::RwLock::new(Vec::new());

// ─────────────────────────────────────────────────────────────────────
// Apps offering each other things
// ─────────────────────────────────────────────────────────────────────
//
// Everything above is an app talking to the *shell*. This is the other
// conversation: bookings wanting to raise an invoice, which is
// finance's business and not something the shell should broker.
//
// The rule that shapes it is that neither app may require the other.
// Bookings has to work with finance turned off, and finance has to
// build in a version of Task that has no bookings — so a crate
// dependency between them is out, and so is anything that fails when
// the other end is missing.
//
// What is left is: a **contract crate** holds the type, both apps
// depend on that and not on each other, the offering app registers a
// value, and the asking app gets an `Option`. The type is the key —
// there is no name to spell wrong, no registry of strings to keep in
// sync, and a contract that changes shape is a compile error in both
// apps rather than a lookup that silently stops matching.

type Offers = std::collections::HashMap<std::any::TypeId, (&'static str, ArcAny)>;
type ArcAny = std::sync::Arc<dyn std::any::Any + Send + Sync>;

static OFFERS: std::sync::RwLock<Option<Offers>> = std::sync::RwLock::new(None);

/// Offer something to the other apps.
///
/// Call from the offering app's `provide`, or from the composition
/// root. `T` comes from a contract crate both ends depend on:
///
/// ```ignore
/// // in finance's `provide`
/// task_plugin_ui::offer("finance", finance_contract::Billing { bill });
///
/// // in bookings, wherever the button would go
/// if let Some(billing) = task_plugin_ui::offered::<Billing>(|id| enabled.contains(id)) {
///     // render "Invoice this" → (billing.bill)(work)
/// }
/// ```
///
/// One offer per type wins: offering the same `T` twice replaces the
/// first, on the same reasoning as registering an app id twice.
pub fn offer<T: std::any::Any + Send + Sync>(app: &'static str, value: T) {
    OFFERS
        .write()
        .expect("plugin offers poisoned")
        .get_or_insert_with(Default::default)
        .insert(
            std::any::TypeId::of::<T>(),
            (app, std::sync::Arc::new(value)),
        );
}

/// What some app offers of this type, if any app enabled here does.
///
/// `None` is the ordinary case, not an error: the other app is not in
/// this build, or is turned off for this org. A caller that gets `None`
/// should simply not show that affordance — an integration is a thing
/// an app *gains*, never a thing it needs.
///
/// Filtered by `enabled` for the same reason screens and claims are: an
/// org that turned finance off should not be offered a way into it.
#[must_use]
pub fn offered<T: std::any::Any + Send + Sync>(
    enabled: impl Fn(&str) -> bool,
) -> Option<std::sync::Arc<T>> {
    let guard = OFFERS.read().expect("plugin offers poisoned");
    let (app, value) = guard.as_ref()?.get(&std::any::TypeId::of::<T>())?;
    if !enabled(app) {
        return None;
    }
    value.clone().downcast::<T>().ok()
}

/// Which app offered this type, enabled or not — for a settings screen
/// explaining why an integration is not showing.
#[must_use]
pub fn offered_by<T: std::any::Any + Send + Sync>() -> Option<&'static str> {
    let guard = OFFERS.read().expect("plugin offers poisoned");
    guard
        .as_ref()?
        .get(&std::any::TypeId::of::<T>())
        .map(|(a, _)| *a)
}

/// Register an app. Call from the composition root, before launch.
///
/// Registering the same id twice replaces the first: a build that
/// somehow lists an app in two places should behave like the one that
/// meant it, not show two identical tabs.
pub fn register(app: PluginApp) {
    let mut registry = REGISTRY.write().expect("plugin registry poisoned");
    match registry.iter_mut().find(|a| a.id == app.id) {
        Some(existing) => *existing = app,
        None => registry.push(app),
    }
}

/// The contribution contract this SDK speaks.
///
/// Bumped when a contribution *kind* is added or changes shape, so a
/// log line can say what a build was capable of. Not enforced at
/// runtime, for the reason in [`PluginApp::version`]: plugins link in,
/// and the compiler settles compatibility before anything runs.
pub const CONTRACT: u32 = 1;

/// Every registered app, in registration order.
#[must_use]
pub fn registered() -> Vec<PluginApp> {
    REGISTRY.read().expect("plugin registry poisoned").clone()
}

/// What is installed, for a settings surface or a support question.
///
/// `(id, version)` per registered app, in registration order.
#[must_use]
pub fn installed() -> Vec<(&'static str, &'static str)> {
    REGISTRY
        .read()
        .expect("plugin registry poisoned")
        .iter()
        .map(|a| (a.id, a.version))
        .collect()
}

/// Which app claims this wikilink, if any.
///
/// `enabled` decides which apps may answer, so an app an org turned off
/// does not silently capture links — the note falls back to being a
/// missing page, which is the truth for that org.
///
/// First claim wins, in registration order. Two apps claiming one text
/// is a wiring decision the composition root already made by ordering
/// them; resolving it here would hide that.
#[must_use]
pub fn claim_link(text: &str, enabled: impl Fn(&str) -> bool) -> Option<(&'static str, Claim)> {
    let registry = REGISTRY.read().expect("plugin registry poisoned");
    let claims = || registry.iter().filter(|a| enabled(a.id));
    // An `Always` claim wins over an `IfUnknown` one regardless of
    // registration order: it is a statement that the text has one
    // meaning, and the order two apps happened to be registered in is
    // no reason to hand it to the weaker claim.
    claims()
        .find_map(|a| {
            a.claim_link
                .and_then(|c| c(text))
                .filter(Claim::beats_a_page)
                .map(|c| (a.id, c))
        })
        .or_else(|| claims().find_map(|a| a.claim_link.and_then(|c| c(text)).map(|c| (a.id, c))))
}

/// Which app claims this href scheme, if any. See [`claim_link`].
#[must_use]
pub fn claim_href(
    href: &str,
    enabled: impl Fn(&str) -> bool,
) -> Option<(&'static str, LinkTarget)> {
    REGISTRY
        .read()
        .expect("plugin registry poisoned")
        .iter()
        .filter(|a| enabled(a.id))
        .find_map(|a| a.claim_href.and_then(|c| c(href)).map(|t| (a.id, t)))
}

/// Which app opens this file, if any.
///
/// Ask before falling back to the vault: a file nobody claims is a
/// note, which is the right default and the common case.
#[must_use]
pub fn claim_file(
    path: &str,
    enabled: impl Fn(&str) -> bool,
) -> Option<(&'static str, LinkTarget)> {
    REGISTRY
        .read()
        .expect("plugin registry poisoned")
        .iter()
        .filter(|a| enabled(a.id))
        .find_map(|a| a.claim_file.and_then(|c| c(path)).map(|t| (a.id, t)))
}

/// The app with this id, if one registered.
#[must_use]
pub fn find(id: &str) -> Option<PluginApp> {
    REGISTRY
        .read()
        .expect("plugin registry poisoned")
        .iter()
        .find(|a| a.id == id)
        .copied()
}

// ─────────────────────────────────────────────────────────────────────
// Lazy screens — the wasm-split boundary
// ─────────────────────────────────────────────────────────────────────

/// What a screen is asked for, owned so it can cross a chunk boundary.
///
/// [`PluginApp::view`] takes `&str`s because it is called synchronously
/// from the shell's route. A screen that lives in its own wasm chunk
/// (see [`lazy_view!`]) is called *after* that chunk has downloaded, by
/// which point the borrowed strings are gone — so the arguments are
/// copied into this and handed over by value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ViewArgs {
    /// What followed `/app/<id>/`, empty for the front page.
    pub path: String,
    /// The app's raw query, empty when there is none.
    pub query: String,
}

/// Put an app's screens in their own wasm chunk, downloaded on first
/// visit.
///
/// The web bundle is one binary, and every app in it pays its weight
/// on the first load whether or not anybody opens it: notation fonts,
/// a PDF engine, an IMAP stack. `dx build --wasm-split` cuts the binary
/// along `wasm_split` boundaries and this macro puts one around an
/// app's screens, so the shell downloads only itself and the chunks it
/// actually navigates to. Only the *screens* move — an app's nav
/// entries, widgets, fences and store providers are registered at
/// startup and stay in the main chunk, because the shell consults them
/// before anything is clicked.
///
/// Use it as the body of [`PluginApp::view`]:
///
/// ```ignore
/// fn view(path: &str, query: &str) -> Option<Element> {
///     task_plugin_ui::lazy_view!("scripture", screen, path, query)
/// }
///
/// fn screen(path: &str, query: &str) -> Option<Element> {
///     match path { "" => Some(rsx! { ScriptureView {} }), _ => None }
/// }
/// ```
///
/// `"scripture"` names the chunk (`module_N_scripture.wasm` in the
/// bundle) and must be unique across apps. `screen` is the ordinary
/// synchronous view function; everything it reaches that the shell
/// does not is what ends up in the chunk.
///
/// Outside a split build — desktop, mobile, `dx serve` without
/// `--wasm-split` — this is a plain call to `screen`. The
/// `wasm-split` cargo feature on this crate is what switches it, and
/// the web app turns that on only for its split release build.
#[cfg(all(target_arch = "wasm32", feature = "wasm-split"))]
#[macro_export]
macro_rules! lazy_view {
    ($module:literal, $screen:ident, $path:expr, $query:expr) => {{
        fn __lazy_screen_adapter(
            args: $crate::ViewArgs,
        ) -> Option<$crate::dioxus::prelude::Element> {
            $screen(&args.path, &args.query)
        }
        static __LOADER: $crate::lazy::Loader = {
            use $crate::dioxus::wasm_split;
            wasm_split::lazy_loader!(
                extern $module fn __lazy_screen_adapter(
                    args: $crate::ViewArgs,
                ) -> Option<$crate::dioxus::prelude::Element>
            )
        };
        $crate::lazy::lazy_screen($crate::lazy::LazyView(&__LOADER), $path, $query)
    }};
}

/// See the documentation on the `wasm-split` variant of this macro.
///
/// This is the arm every non-split build gets: a direct call, no
/// loader, no suspense — the app behaves exactly as if it had written
/// `screen(path, query)` itself.
#[cfg(not(all(target_arch = "wasm32", feature = "wasm-split")))]
#[macro_export]
macro_rules! lazy_view {
    ($module:literal, $screen:ident, $path:expr, $query:expr) => {
        $screen($path, $query)
    };
}

/// The runtime half of [`lazy_view!`]: the loader type the macro's
/// `static` has, and the component that suspends on it.
#[cfg(all(target_arch = "wasm32", feature = "wasm-split"))]
#[doc(hidden)]
pub mod lazy {
    use super::ViewArgs;
    use dioxus::prelude::*;

    /// A screen function living in a not-yet-downloaded chunk.
    pub type Loader = dioxus::wasm_split::LazyLoader<ViewArgs, Option<Element>>;

    /// A `'static` loader as a prop. Two views are the same view when
    /// they are the same static.
    #[derive(Clone, Copy)]
    pub struct LazyView(pub &'static Loader);

    impl PartialEq for LazyView {
        fn eq(&self, other: &Self) -> bool {
            std::ptr::eq(self.0, other.0)
        }
    }

    /// What `lazy_view!` returns to the shell: a screen that suspends
    /// until its chunk is here, then renders whatever the app's own
    /// view function says.
    pub fn lazy_screen(view: LazyView, path: &str, query: &str) -> Option<Element> {
        let path = path.to_string();
        let query = query.to_string();
        Some(rsx! {
            SuspenseBoundary {
                fallback: |_| rsx! { LazyFallback {} },
                LazyScreen { view, path, query }
            }
        })
    }

    #[component]
    fn LazyScreen(view: LazyView, path: String, query: String) -> Element {
        let loader = view.0;
        let loaded = use_resource(move || async move { loader.load().await }).suspend()?;
        if !*loaded.read() {
            return rsx! {
                LazyNotice {
                    title: "This screen could not be loaded",
                    detail: "Its code did not download. Check the connection and reload.",
                }
            };
        }
        match loader.call(ViewArgs { path, query }) {
            Ok(Some(view)) => view,
            // The same answer the shell gives for an eager app that
            // returns `None` — a path the app does not recognise.
            Ok(None) => rsx! {
                LazyNotice {
                    title: "No such screen",
                    detail: "This app has no page at that address.",
                }
            },
            Err(_) => rsx! {
                LazyNotice {
                    title: "This screen could not be loaded",
                    detail: "Its code did not download. Check the connection and reload.",
                }
            },
        }
    }

    /// Shown while the chunk downloads — a first visit only; the
    /// browser caches it afterwards.
    #[component]
    fn LazyFallback() -> Element {
        rsx! {
            div { class: "flex h-full min-h-40 items-center justify-center text-sm text-muted-foreground",
                "Loading…"
            }
        }
    }

    #[component]
    fn LazyNotice(title: &'static str, detail: &'static str) -> Element {
        rsx! {
            div { class: "flex h-full min-h-40 flex-col items-center justify-center gap-1 p-6 text-center",
                p { class: "text-base font-medium", "{title}" }
                p { class: "text-sm text-muted-foreground", "{detail}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nowhere(_path: &str, _query: &str) -> Option<Element> {
        None
    }

    /// Registering twice under one id replaces rather than duplicates —
    /// two identical tabs is a worse answer than either one.
    #[test]
    fn registering_an_id_twice_keeps_the_last() {
        register(PluginApp {
            id: "test-dup",
            version: "0.0.0-test",
            nav: &[],
            view: nowhere,
            panel: None,
            claim_file: None,
            provide: None,
            widgets: None,
            fences: None,
            claim_link: None,
            claim_href: None,
        });
        register(PluginApp {
            id: "test-dup",
            version: "0.0.0-test",
            nav: &[],
            view: nowhere,
            panel: None,
            claim_file: None,
            provide: None,
            widgets: None,
            fences: None,
            claim_link: None,
            claim_href: None,
        });
        assert_eq!(
            registered().iter().filter(|a| a.id == "test-dup").count(),
            1
        );
    }

    /// An id nobody registered is absent, not empty — the shell needs
    /// to tell "this app is not installed" from "this app has no
    /// screens".
    #[test]
    fn an_unregistered_id_is_absent() {
        assert!(find("test-never-registered").is_none());
    }

    fn claims_verses(text: &str) -> Option<Claim> {
        text.starts_with("John ")
            .then(|| Claim::Always(LinkTarget::query(text)))
    }

    /// An app an org turned off must not capture links. The note then
    /// reads as a missing page, which is the truth for that org — an
    /// app nobody enabled has no business deciding where a link goes.
    #[test]
    fn a_disabled_app_claims_nothing() {
        register(PluginApp {
            id: "test-verses",
            version: "0.0.0-test",
            nav: &[],
            view: nowhere,
            panel: None,
            claim_file: None,
            provide: None,
            widgets: None,
            fences: None,
            claim_link: Some(claims_verses),
            claim_href: None,
        });

        let on = claim_link("John 3:16", |_| true);
        assert_eq!(
            on.map(|(id, c)| (id, c.target().query.clone())),
            Some(("test-verses", "John 3:16".to_string()))
        );

        assert!(
            claim_link("John 3:16", |id| id != "test-verses").is_none(),
            "an app that is off must not take the link"
        );
    }

    /// The stamp is what a note carries so a later build knows what
    /// wrote it.
    #[test]
    fn an_app_stamps_its_id_and_version() {
        let app = PluginApp {
            id: "test-stamp",
            version: "1.2.3",
            nav: &[],
            view: nowhere,
            panel: None,
            claim_file: None,
            provide: None,
            widgets: None,
            fences: None,
            claim_link: None,
            claim_href: None,
        };
        assert_eq!(app.stamp(), "test-stamp@1.2.3");
    }

    /// Text no app recognises is nobody's. The shell falls back to the
    /// vault, which is what a wikilink means by default.
    #[test]
    fn unclaimed_text_belongs_to_the_vault() {
        assert!(claim_link("Groceries for Tuesday", |_| true).is_none());
    }

    fn claims_songs(text: &str) -> Option<Claim> {
        (text == "Washed").then(|| Claim::IfUnknown(LinkTarget::query("song=Washed")))
    }

    /// A claim that says it means it beats one that defers, whatever
    /// order the two apps were registered in. The order two apps
    /// happened to be listed in is no reason to hand a reference to the
    /// weaker claim.
    #[test]
    fn a_certain_claim_beats_a_deferring_one() {
        // The deferring app registers FIRST, so first-wins would give
        // it the link.
        register(PluginApp {
            id: "test-songs",
            version: "0.0.0-test",
            nav: &[],
            view: nowhere,
            panel: None,
            claim_file: None,
            provide: None,
            widgets: None,
            fences: None,
            claim_link: Some(claims_songs),
            claim_href: None,
        });
        register(PluginApp {
            id: "test-both",
            version: "0.0.0-test",
            nav: &[],
            view: nowhere,
            panel: None,
            claim_file: None,
            provide: None,
            widgets: None,
            fences: None,
            claim_link: Some(|text| {
                (text == "Washed").then(|| Claim::Always(LinkTarget::query("verse=Washed")))
            }),
            claim_href: None,
        });

        let (id, claim) = claim_link("Washed", |_| true).expect("somebody claims it");
        assert_eq!(id, "test-both");
        assert!(claim.beats_a_page());
    }

    /// The case the encoding exists for. A reference has a space in it,
    /// and a raw one truncates the passage to its book.
    #[test]
    fn a_reference_survives_the_round_trip() {
        let there = LinkTarget::param("reference", "John 3:16-20@ESV");
        assert!(!there.query.contains(' '), "a raw space ends the URL");
        assert_eq!(
            query_param(&there.query, "reference").as_deref(),
            Some("John 3:16-20@ESV")
        );
    }

    #[test]
    fn an_absent_key_and_an_empty_one_are_different_answers() {
        assert_eq!(query_param("dish=", "dish").as_deref(), Some(""));
        assert_eq!(query_param("dish=", "reference"), None);
    }

    #[test]
    fn a_later_key_does_not_shadow_the_one_asked_for() {
        let query = "reference=John%203%3A16&tx=ESV";
        assert_eq!(query_param(query, "tx").as_deref(), Some("ESV"));
        assert_eq!(
            query_param(query, "reference").as_deref(),
            Some("John 3:16")
        );
    }

    /// Dropping characters silently would turn a bad link into a
    /// *plausible* one, which is the harder bug to see.
    #[test]
    fn a_malformed_escape_keeps_its_characters() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("a%zz"), "a%zz");
    }

    /// The bug this packing exists for, caught in a browser and not by
    /// a type: the router decodes `q` when it parses a URL and
    /// re-serialises the route without encoding it again. One trip
    /// through the address bar turned a percent-escaped `%26` into a
    /// literal `&`, which ended the parameter — and every value after
    /// the first vanished, looking exactly like a one-parameter link.
    #[test]
    fn a_packed_query_survives_being_decoded_again() {
        let q = "bill=30-minute consult&client=Sam Reeve&minutes=30";
        let packed = pack(q);
        assert!(
            !packed.contains('&'),
            "an ampersand would end the parameter"
        );
        assert!(
            !packed.contains('%'),
            "a percent is something to reinterpret"
        );
        assert!(!packed.contains('='), "padding would need escaping too");

        // However many times something decodes it, it is still itself.
        assert_eq!(unpack(&packed), q);
        assert_eq!(unpack(&decode(&packed)), q);
        assert_eq!(unpack(&decode(&decode(&packed))), q);

        let out = unpack(&packed);
        assert_eq!(query_param(&out, "client").as_deref(), Some("Sam Reeve"));
        assert_eq!(query_param(&out, "minutes").as_deref(), Some("30"));
    }

    /// A hand-typed URL still works, which is worth the fallback: a
    /// packed query never looks like a readable one, so there is
    /// nothing to confuse.
    #[test]
    fn an_unpacked_query_is_read_as_written() {
        assert_eq!(unpack("dish=Bolognese"), "dish=Bolognese");
    }

    // ── apps offering each other things ─────────────────────────────

    /// The shape of a real contract: finance says where to go to bill
    /// something, bookings renders a button to there.
    struct Billing {
        bill: fn(&str) -> String,
    }

    /// A second, unrelated contract — proves the type is the key.
    struct Transcribing;

    #[test]
    fn an_app_finds_what_another_offers() {
        offer(
            "test-finance",
            Billing {
                bill: |what| format!("bill:{what}"),
            },
        );
        let billing = offered::<Billing>(|_| true).expect("finance offers it");
        assert_eq!((billing.bill)("booking-7"), "bill:booking-7");
    }

    /// The case the whole mechanism exists for. An app must work with
    /// the other end turned off — and get `None`, not a panic and not
    /// a stale value.
    #[test]
    fn an_offer_from_a_disabled_app_is_not_available() {
        offer(
            "test-finance",
            Billing {
                bill: |_| String::new(),
            },
        );
        assert!(offered::<Billing>(|id| id != "test-finance").is_none());
    }

    #[test]
    fn a_contract_nobody_offers_is_simply_absent() {
        assert!(offered::<Transcribing>(|_| true).is_none());
        assert!(offered_by::<Transcribing>().is_none());
    }

    /// Who offered it is answerable even when it is not available, so
    /// a settings screen can say *why* an integration is missing.
    #[test]
    fn the_offering_app_is_nameable_even_when_off() {
        offer(
            "test-finance",
            Billing {
                bill: |_| String::new(),
            },
        );
        assert_eq!(offered_by::<Billing>(), Some("test-finance"));
    }

    /// An app linking to its own screens — the same URL the shell
    /// builds for a claimed link, so a button and a wikilink land in
    /// exactly the same place.
    #[test]
    fn an_app_links_to_its_own_screens() {
        assert_eq!(href("mealplan", "", ""), "/app/mealplan");
        assert_eq!(href("mealplan", "shopping", ""), "/app/mealplan/shopping");
        // The query is packed, so the URL says nothing about its
        // shape — only that it round-trips.
        let url = href("mealplan", "recipe/read", "path=Cookbook/Ragu.cook");
        assert!(url.starts_with("/app/mealplan/recipe/read?q="), "{url}");
        let q = url.split_once("?q=").expect("has a query").1;
        assert_eq!(unpack(q), "path=Cookbook/Ragu.cook");
    }

    /// The round trip that matters: what the app wrote is what the
    /// app gets back — including the ampersand that would otherwise
    /// read as the end of the parameter.
    #[test]
    fn a_self_link_survives_the_shell() {
        let url = href_param(
            "mealplan",
            "recipe/read",
            "path",
            "Cookbook/Ragu & Chips.cook",
        );
        let q = url.split_once("?q=").expect("has a query").1;
        assert_eq!(
            query_param(&unpack(q), "path").as_deref(),
            Some("Cookbook/Ragu & Chips.cook")
        );
    }

    #[test]
    fn a_form_sends_a_space_as_a_plus() {
        assert_eq!(
            query_param("reference=John+3:16", "reference").as_deref(),
            Some("John 3:16")
        );
    }
}

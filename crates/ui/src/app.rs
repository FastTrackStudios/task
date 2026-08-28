//! Top-level `App` component — platform launchers mount this.
//!
//! Wires the theme provider + per-org overrides, then mounts the
//! router. Per-route layout lives in
//! [`crate::shell::app_shell::AppShell`].

use std::collections::HashMap;

use architect_ui::prelude::*;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection, fetch_orgs, home_slug};
use crate::routes::Route;
use crate::theming::{OrgThemeOverrides, ProjectThemeOverrides, state_from_preset_name};

/// App-wide mobile baseline, injected once at the root. Plain
/// (unlayered) CSS so it outranks Tailwind's `@layer utilities` on the
/// cascade regardless of specificity — that's what lets the `font-size`
/// rule beat per-input `text-sm` classes.
///
/// - **iOS zoom:** Mobile Safari zooms the viewport when a focused
///   control's font is < 16px. Every form here uses `text-sm` (14px),
///   so tapping any field jumped the zoom. Floor controls at 16px on
///   phones; desktop keeps the denser look (the rule is `max-width`
///   gated).
/// - **Sideways bleed:** `overflow-x: clip` on the root stops a stray
///   wide child from scrolling the whole page horizontally. `clip`
///   (not `hidden`) doesn't create a scroll container, so it leaves the
///   shell's sticky header / fixed tab bar intact.
/// - **Touch polish:** kill the grey tap-flash; momentum-scroll the
///   horizontal scrollers (kanban board, chip rows) and contain their
///   overscroll so a swipe doesn't bounce the page.
const MOBILE_BASELINE_CSS: &str = "\
html,body{overflow-x:clip;-webkit-text-size-adjust:100%}\
*{-webkit-tap-highlight-color:transparent}\
.overflow-x-auto,.overflow-x-scroll{-webkit-overflow-scrolling:touch;overscroll-behavior-x:contain}\
@media (max-width:640px){\
input:not([type=checkbox]):not([type=radio]):not([type=range]),select,textarea{font-size:16px!important}\
}";

#[component]
pub fn App() -> Element {
    // Share-guest entry (issue #272): a `/share-review?org=…&token=…`
    // boot skips the ENTIRE shell — no auth restore, no org discovery,
    // no router — and mounts the guest review page over the scoped
    // vox lane. The branch is decided once per page load (a share URL
    // is a full navigation), so the hook order below stays consistent
    // for this component's lifetime.
    if let Some(session) = crate::guest_share::detect() {
        return rsx! {
            crate::guest_share::GuestShell { session }
        };
    }

    // Stage 4b-1: the in-browser session engine is no longer seeded with the
    // demo setlist at app boot. The `type: setlist` page now builds a
    // per-setlist engine on demand via `session_engine::build_for_setlist`
    // (seeded from that setlist's real songs) and mounts the event bridge.
    // The demo `session_engine::bootstrap()` remains available as a
    // self-contained fallback but is intentionally not called here.

    // App-wide architect registries (notifications + reactivity) and
    // the shared per-org connection: one `Connection<vox::Caller>` for
    // the active org (the selected one, or home under "All"),
    // re-established when the switcher moves. Atom-pattern hooks build
    // typed clients from this caller; multi-org fan-out keeps using
    // `vox_clients::caller_for` per slug (same cached sockets).
    //
    // Note: the connect closure reads the selection + org list
    // signals synchronously — that's what re-triggers it (see
    // `use_connect_reactive`'s dependency contract).
    // Multi-org data selection: which org(s) the data views load from.
    // Defaults to `All` (every hosted org); the org switcher drives it.
    let org_selection: Signal<OrgSelection> =
        use_context_provider(|| Signal::new(OrgSelection::All));

    // Multi-server registry (add/select servers by URL). Provided for the
    // Servers UI + auth (which writes tokens back into the active entry).
    let server_registry = use_context_provider(crate::server_registry::ServerRegistry::new);
    // Seed the process-global active-server holder synchronously from the
    // persisted active entry, so the very first org discovery + connection
    // dial the right server (before any effect has run).
    crate::vox_session::set_active_server(server_registry.active_entry().map(|e| {
        crate::vox_session::ActiveServer {
            url: e.server_url,
            token: e.session_token,
        }
    }));
    // Keep the holder in lock-step with the active selection / entries.
    use_effect(move || {
        crate::vox_session::set_active_server(server_registry.active_entry().map(|e| {
            crate::vox_session::ActiveServer {
                url: e.server_url,
                token: e.session_token,
            }
        }));
    });

    // Hosted org list, discovered from the server's well-known endpoint
    // and published for the switcher + data fetchers.
    let mut org_list: Signal<Vec<OrgMeta>> = use_context_provider(|| Signal::new(Vec::new()));
    // The session token, as a REACTIVE signal — provided here, before
    // discovery, and written by `auth::publish_session_token`.
    //
    // Discovery is what tags which orgs are ours (#109 criterion 6), and
    // it can only do that if it carries the token. But discovery runs
    // FIRST: sign-in needs the home slug, so the org list has to resolve
    // before auth can restore a session. That ordering means the boot
    // fetch is necessarily anonymous, every org comes back `member: null`
    // ("not asked"), and the switcher correctly falls back to showing all
    // of them — permanently, because nothing re-ran discovery.
    //
    // Caught by watching the live network log: exactly ONE well-known
    // request per page load, before the session existed. So the token is
    // a dependency of the resource below, and resolving a session
    // re-discovers with it.
    let session_token: Signal<Option<String>> =
        use_context_provider(|| Signal::new(None::<String>));
    // Re-discover orgs whenever the active server OR the session changes
    // (reading both inside the resource closure registers them as
    // dependencies).
    let orgs_res = use_resource(move || {
        let _active = server_registry.active_id();
        let _session = session_token();
        async move { fetch_orgs().await }
    });
    // Pull the identity locker's per-org tokens once the session
    // resolves. Auth stores are per-org, so the ambient token makes you
    // a member of exactly ONE org — without these, the switcher can
    // only ever show that one, whatever the well-known doc lists.
    let links_res = use_resource(move || {
        let session = session_token();
        async move {
            // INSTRUMENTED at every stage. The first cut swallowed all
            // three failure modes into one silent `Err`, and the server
            // logs showed the browser never issuing a `list_links` call
            // at all — so "which stage" is the whole question.
            tracing::info!(
                has_session = session.is_some(),
                has_bearer = task_ui_core::vox_session::bearer().is_some(),
                "locker: resolving links"
            );
            let client: identity_proto::IdentityServiceClient =
                match task_ui_core::vox_clients::establish_server(None).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "locker: establish /server/vox FAILED");
                        return Err(e);
                    }
                };
            let Some(token) = task_ui_core::vox_session::bearer() else {
                tracing::warn!("locker: no bearer at call time — skipping list_links");
                return Err("not signed in".to_string());
            };
            match client.list_links(token).await {
                Ok(links) => {
                    tracing::info!(count = links.len(), "locker: list_links OK");
                    Ok(links)
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "locker: list_links FAILED");
                    Err(format!("list links: {e:?}"))
                }
            }
        }
    });
    use_effect(move || {
        // A server with no identity locker (not a home server) answers
        // an error here; that is a normal deployment, not a fault, so
        // the ambient token keeps being used — but it is LOGGED, not
        // swallowed. Silently ignoring the error arm is what hid this
        // failure through three wrong fixes.
        if let Some(Err(e)) = &*links_res.read_unchecked() {
            tracing::warn!(error = %e, "locker: no linked orgs (switcher stays single-org)");
        }
        if let Some(Ok(links)) = &*links_res.read_unchecked() {
            tracing::info!(links = links.len(), "locker: applying linked orgs");
            let map: std::collections::HashMap<String, String> = links
                .iter()
                .filter_map(|l| l.token.clone().map(|t| (l.remote_slug.clone(), t)))
                .collect();
            let slugs: Vec<String> = map.keys().cloned().collect();
            // Membership FIRST, and it must not sit downstream of the
            // connection teardown below — see why that ordering matters.
            //
            // Membership has to land ON the org list, not in a global:
            // `org_list` is the Signal every consumer reads, so patching
            // it here is what actually re-renders the switcher. A static
            // holder would be invisible to Dioxus — the list would stay
            // stale until something unrelated happened to re-render.
            mark_linked_orgs(org_list, &slugs);
            if task_ui_core::vox_session::set_linked_tokens(map) {
                // A socket presents its identity once, at establish, so
                // connections opened under the old (single-token) view
                // have to go before the new credentials can be used.
                //
                // DEFERRED, and that is load-bearing. This effect runs
                // synchronously inside the WebSocket message callback
                // that delivered `list_links` — tearing the cache down
                // there drops the very socket whose callback is on the
                // stack, and wasm-bindgen throws "closure invoked
                // recursively or after being dropped". That exception
                // aborted the rest of this effect, which is why the
                // switcher stayed at one org even once membership was
                // computed correctly. Spawning defers the teardown past
                // the callback's return.
                spawn(async move {
                    task_ui_core::vox_clients::drop_cached_connections();
                });
            }
        }
    });

    // Surface the discovery outcome so the Servers UI can show *why* it
    // didn't resolve (native fetch failures are otherwise invisible).
    let mut discovery_err = use_context_provider(|| crate::orgs::DiscoveryError(Signal::new(None)));
    use_effect(move || match &*orgs_res.read_unchecked() {
        Some(Ok(list)) => {
            if *org_list.peek() != *list {
                org_list.set(list.clone());
                // Discovery answers `member` for the ONE token it
                // presented, so a fresh list forgets every linked org.
                // Re-apply, or a re-discovery (a session change, a
                // server switch) silently collapses the switcher back
                // to a single org.
                mark_linked_orgs(org_list, &task_ui_core::vox_session::linked_slugs());
            }
            if discovery_err.0.peek().is_some() {
                discovery_err.0.set(None);
            }
        }
        Some(Err(e)) => {
            if discovery_err.0.peek().as_deref() != Some(e.as_str()) {
                discovery_err.0.set(Some(e.clone()));
            }
        }
        None => {}
    });

    let org_overrides: OrgThemeOverrides = use_context_provider(|| OrgThemeOverrides {
        map: Signal::new(HashMap::<String, String>::new()),
        mode: Signal::new(ThemeMode::Dark),
    });

    let _project_overrides: ProjectThemeOverrides =
        use_context_provider(|| ProjectThemeOverrides {
            map: Signal::new(HashMap::<Uuid, String>::new()),
        });

    // Theme follows the active org (the selected one, or home under
    // "All"), keyed by slug in the overrides map. Until discovery
    // resolves, the default preset holds.
    let theme_org_slug = use_memo(move || match &*org_selection.read() {
        OrgSelection::One(slug) => slug.clone(),
        OrgSelection::All => home_slug(&org_list.read()),
    });

    let mut theme_state = use_signal(|| state_from_preset_name("", ThemeMode::Dark));

    use_effect(move || {
        let slug = theme_org_slug();
        let resolved_name: String = org_overrides
            .map
            .read()
            .get(&slug)
            .cloned()
            .unwrap_or_default();
        // Rebuild through state_from_preset_name so the Obsidian
        // token overlay survives (set_preset would discard it).
        let mode = theme_state.peek().mode;
        theme_state.set(state_from_preset_name(&resolved_name, mode));
    });

    use_effect(move || {
        let mode = *org_overrides.mode.read();
        if theme_state.peek().mode != mode {
            theme_state.write().set_mode(mode);
        }
    });

    // Supervised: the established caller is watched for death
    // (`Caller::closed()` resolves when the org socket drops — server
    // restart, network blip) and the connection re-establishes under
    // exponential backoff with full jitter (floor 250ms, cap 10s). The
    // `Connection::generation` bump on each re-establish is what tells
    // hooks/caches downstream to invalidate; `caller_for`'s root cache
    // additionally self-validates via `is_connected()` (see
    // `vox_clients`), so the reconnect lands on a fresh socket.
    let _conn: architect::Connection<vox_core::Caller> = architect::use_app_supervised(
        move || {
            let slug = match &*org_selection.read() {
                OrgSelection::One(slug) => slug.clone(),
                OrgSelection::All => home_slug(&org_list.read()),
            };
            async move {
                if slug.is_empty() {
                    // Discovery hasn't resolved yet — stay Connecting;
                    // the org-list signal write re-runs this closure.
                    return Err("awaiting org discovery".to_owned());
                }
                crate::vox_clients::caller_for(&slug).await
            }
        },
        |caller: vox_core::Caller| async move { caller.closed().await },
    );

    // The per-feature optimistic stores (architect-atom `Store`s) every
    // route page's `use_<entity>_list` / `use_<entity>_mutations` hooks
    // read. Provided after `use_app_supervised` so mutations find the
    // notifications + reactivity registries it installed above.
    crate::stores::provide_stores();

    // Web auth: the active-account context + boot restore (validate
    // the persisted session, or auto sign-in as Guest). Needs the
    // org-list signal above (auth talks to the home org's endpoint);
    // must precede the router so pages/presence can read the account.
    crate::auth::provide_auth();
    // Watch config publisher: mirror the active {server, org, session}
    // to the paired Apple Watch via the platform sink (a no-op unless
    // the iOS shell registered one). After provide_auth — it reads the
    // active-account context.
    crate::watch_sync::use_watch_config_publisher();
    // Pair this machine with the org for file sync: the local agent's
    // endpoint id to the org, the org's back to the agent, on the
    // authority of the sign-in this app already holds. Once per launch,
    // native only, and it prompts nobody — see the module docs.
    crate::device_pairing::use_device_pairing();
    // Per-user prefs (server-backed) — after auth, which it watches.
    crate::prefs::provide_prefs();
    // Theme persistence: seed the org-theme overrides above from prefs
    // on boot, mirror picker changes back (localStorage + server).
    crate::theming::use_theme_prefs_sync();

    // The ONE derived-presence poll (agent sessions + open timers).
    // Every roster surface shares it; when each consumer owned its own,
    // the two mounted on the desktop shell ran duplicate 60s polls and
    // list_sessions accounted for ~390 of the ~400 reads per hour.
    crate::presence::use_provide_derived_presence();

    // ── Note-widget registry ──────────────────────────────────
    // The ONE site where widget provider crates are named. Everything
    // else in the shell (note_view, collab decorations, link clicks)
    // goes through the `task_widgets::WidgetRegistry` context and never
    // learns who provided a widget. Registration is explicit — no
    // linker magic — so this list IS the app's widget roster.
    let widget_registry = use_context_provider(|| {
        // Same spirit as the widget roster: the editor renders ```kf
        // fences through a registry, not a dependency (editor-state must
        // stay below the notation domain), so the app names the renderer
        // explicitly. Without this, charts in notes show as source.
        task_player_ui::register_chart_fences();

        let registry = task_widgets::WidgetRegistry::new();
        registry.register(task_player_ui::widgets());
        registry.register(task_note_tabs::widgets());
        registry.register(files_ui::widgets());
        registry
    });
    // Keep the registry pointed at the ACTIVE org's enabled plugin set
    // (specs of disabled plugins stop matching/decorating). Registration
    // stays as-is — the filter is read-time, so an org switch retunes
    // the same registry.
    use_effect(move || {
        widget_registry.set_plugin_set(crate::orgs::active_plugin_set(
            &org_selection.read(),
            &org_list.read(),
        ));
    });

    // Org-wide presence: join the active org's `DocPresence` channel
    // over the shared connection above (the hook reads the
    // `Connection<vox::Caller>` context, so this must come AFTER
    // `use_app_reactive`) and provide the identity/status contexts the
    // publisher, roster, and picker consume. The publisher itself
    // lives in the shell (`PresencePublisher`) — it needs the router.
    crate::presence::provide_org_presence();

    // NOTE: the app-wide keyboard shortcut engine
    // (`crate::shortcuts::use_app_shortcuts`) cannot mount here — it
    // dispatches into the palette/chrome contexts and the router's
    // `Navigator`, all of which only exist under the `AppShell` layout
    // inside the `Router` below. It mounts from the shell's app-long
    // `CommandPalette` instead (see `crate::shortcuts`).
    rsx! {
        style { dangerous_inner_html: MOBILE_BASELINE_CSS }
        ThemeProvider { state: theme_state,
            // App-wide toast host: any component under the router can
            // `use_toast()` to surface the result of a mutation (a named
            // version, a restored file, a resolved divergence — issue
            // #267 is the first consumer).
            ToastProvider {
                div { class: "min-h-screen bg-background text-foreground",
                    // Unauthenticated visitors land on sign-in rather than on
                    // a shell full of empty panels (#109 criterion 5). The
                    // gate is presentation only — the server is what actually
                    // refuses data.
                    crate::auth::SignInGate {
                        Router::<Route> {}
                    }
                }
            }
        }
    }
}

/// Mark every org we hold a locker credential for as one of ours.
///
/// Discovery can only answer `member` for the single token it presented,
/// so on a server hosting several orgs it reports `false` for every org
/// that didn't issue that token — including ones this account is a full
/// member of. Holding a working credential is the same claim, so this
/// folds the locker's answer into the list every consumer already reads.
///
/// Writing it onto the `org_list` Signal (rather than a static holder)
/// is the part that matters: Dioxus re-renders on Signal writes, and a
/// global would leave the switcher showing a stale single org until
/// something unrelated forced a re-render.
fn mark_linked_orgs(mut org_list: Signal<Vec<crate::orgs::OrgMeta>>, linked: &[String]) {
    if linked.is_empty() {
        return;
    }
    let mut list = org_list.peek().clone();
    let mut changed = false;
    for org in &mut list {
        if linked.iter().any(|s| s == &org.slug) && org.member != Some(true) {
            org.member = Some(true);
            changed = true;
        }
    }
    // Only write on a real change — an unconditional set would loop the
    // effect that reads this list.
    if changed {
        org_list.set(list);
    }
}

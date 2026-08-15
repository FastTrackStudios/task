//! Two-tier theming for Task: organization → project.
//!
//! Tier 1 — **organization theme**. The `App` root re-renders an
//! `architect_ui::ThemeProvider` keyed on the active org. The preset name is
//! resolved from `OrgThemeOverrides` first (user-picked, in-memory),
//! then falls back to the org's static `theme_preset` field in
//! `crate::data`.
//!
//! Tier 2 — **project override**. When a project route wants to deviate
//! from the org theme it wraps its content in `ProjectThemeScope`,
//! which renders an `architect_ui::ThemeScope` if the project has an entry in
//! `ProjectThemeOverrides`. If not, it returns children unchanged so
//! the org theme shows through.
//!
//! Both override stores are in-memory `Signal<HashMap<...>>` provided
//! at `App` root via `use_context_provider`. The **home-org entry +
//! mode** persist per-user through `UserPrefs` (`theme_preset` /
//! `theme_mode` — see [`use_theme_prefs_sync`]): boot seeds the
//! overrides from prefs once loaded; any picker change routes back
//! through `PrefsCtx::update` (localStorage + server). Other orgs'
//! entries stay in-memory.
//!
//! FUTURE: persist `ProjectThemeOverrides` to localStorage or to a
//! per-project setting on the Project entity.

use std::collections::HashMap;

use dioxus::prelude::*;
use architect_ui::prelude::*;
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection, home_slug};
use crate::prefs::PrefsCtx;

/// User-picked preset for an organization. Keyed by `Organization::id`
/// (which is currently `&'static str` in `crate::data`). Missing entry
/// means "use the org's static default".
///
/// `mode` carries the current light/dark choice. It's intentionally
/// global (not per-org) — users want one consistent mode across the app
/// even when they switch organizations. The popover's `ThemeSwitcher`
/// writes to this through `OrgSwitcher`'s bridge effect; the App-level
/// effect reads it and sets `theme_state.mode` so `ThemeProvider`
/// re-renders the CSS variables for the right palette.
#[derive(Clone, Copy)]
pub struct OrgThemeOverrides {
    pub map: Signal<HashMap<String, String>>,
    pub mode: Signal<ThemeMode>,
}

/// Per-project preset name override. `None` (i.e. missing entry) means
/// "inherit the org theme".
#[derive(Clone, Copy)]
pub struct ProjectThemeOverrides {
    pub map: Signal<HashMap<Uuid, String>>,
}

/// Read the per-project override context. Call from inside the project
/// route or any of its descendants.
#[must_use]
pub fn use_project_theme_overrides() -> ProjectThemeOverrides {
    use_context::<ProjectThemeOverrides>()
}

/// Read the per-org override context.
#[must_use]
pub fn use_org_theme_overrides() -> OrgThemeOverrides {
    use_context::<OrgThemeOverrides>()
}

/// Build a `ThemeState` for a preset name + mode. Falls back to the
/// architect-ui default preset if the name doesn't match anything.
/// The Obsidian dark palette (Cody, 2026-07-03) — kept in lockstep
/// with the static override sheet at `apps/task/fts-theme.css`, which
/// layers these same tokens over the canonical design-token sheet at
/// `libs/architect-ui/architect-ui/assets/fts-theme.css`. The runtime
/// `ThemeProvider` writes tokens onto `.fts-theme-root`, which beats
/// the stylesheet's `:root` block for everything inside the app — so
/// the palette must be applied HERE, not only in the CSS file.
/// NOTE: bare token keys — `ThemeStyle::css_variables` prepends the
/// `--` itself.
const OBSIDIAN_DARK: &[(&str, &str)] = &[
    ("background", "#1e1e1e"),
    ("foreground", "#dadada"),
    ("card", "#161616"),
    ("card-foreground", "#dadada"),
    ("popover", "#1e1e1e"),
    ("popover-foreground", "#dadada"),
    ("primary", "#7f6df2"),
    ("primary-foreground", "#fbfbfb"),
    ("sidebar", "#161616"),
];

pub fn state_from_preset_name(name: &str, mode: ThemeMode) -> ThemeState {
    // Unset/unknown preset ⇒ the FastTrackStudio brand theme (the product
    // default); the architect-ui "default" preset only appears when explicitly
    // chosen (and keeps its Obsidian token overlay below).
    let preset = theme_preset(name)
        .or_else(|| theme_preset("fasttrackstudio"))
        .unwrap_or_else(default_theme_preset);
    // Org/project presets keep their own colors; only the default
    // preset gets the Obsidian skin.
    let is_default = preset.name == "default";
    let mut state = ThemeState::new(preset, mode);
    if is_default {
        for (key, value) in OBSIDIAN_DARK {
            state.set_token(ThemeMode::Dark, *key, *value);
        }
    }
    state
}

/// `""`/unknown ⇒ `None` — an unset pref must not force a mode.
fn mode_from_str(s: &str) -> Option<ThemeMode> {
    match s {
        "light" => Some(ThemeMode::Light),
        "dark" => Some(ThemeMode::Dark),
        _ => None,
    }
}

/// Bridge a `ThemeSwitcher`'s `Signal<ThemeState>` to the active org's
/// entry in [`OrgThemeOverrides`] — the shared state logic behind the
/// org switcher's Palette popover, the rail theme button, and the
/// settings page's Appearance section. Needs the `OrgSelection` /
/// `Vec<OrgMeta>` / `OrgThemeOverrides` contexts (App root).
///
/// Two guarded effect pairs keep switcher and overrides converged
/// without ping-pong (each writes only on an observed difference, and
/// peeks what it writes):
/// - overrides → state: an external write (another picker instance, the
///   prefs boot seed) rebuilds this switcher's state, so a stale picker
///   never clobbers it back. Rebuilds only on preset-name/mode change —
///   local token tweaks (radius/spacing/font) survive.
/// - state → overrides: the picked preset name lands in `map[slug]`,
///   the picked mode in the global `mode`.
#[must_use]
pub fn use_org_theme_switcher_state() -> Signal<ThemeState> {
    let mut org_overrides = use_org_theme_overrides();
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // Theme edits apply to the active org (the selected one, or home
    // under "All"), keyed by slug in the overrides map.
    let active_slug = use_memo(move || match &*selection.read() {
        OrgSelection::One(slug) => slug.clone(),
        OrgSelection::All => home_slug(&org_list.read()),
    });

    let mut switcher_state = use_signal(|| {
        let name = org_overrides
            .map
            .read()
            .get(&active_slug())
            .cloned()
            .unwrap_or_default();
        let mode = *org_overrides.mode.read();
        state_from_preset_name(&name, mode)
    });

    // overrides → state. Ordered before the reverse bridge so on any
    // simultaneous first run the external value wins.
    use_effect(move || {
        let name = org_overrides
            .map
            .read()
            .get(&active_slug())
            .cloned()
            .unwrap_or_default();
        let mode = *org_overrides.mode.read();
        // Resolve through the preset table so ""/unknown names compare
        // as "default" — what `state_from_preset_name` yields.
        let resolved = theme_preset(&name).unwrap_or_else(default_theme_preset).name;
        let differs = {
            let state = switcher_state.peek();
            state.preset != resolved || state.mode != mode
        };
        if differs {
            switcher_state.set(state_from_preset_name(&name, mode));
        }
    });

    // state → overrides (preset name).
    use_effect(move || {
        let name = switcher_state.read().preset.clone();
        let slug = active_slug();
        // Guard only — peek, so this effect doesn't subscribe to the
        // map it writes (the `prev != name` check keeps it stable, but
        // a subscription would still re-run it on every other slug's
        // theme change).
        let prev = org_overrides.map.peek().get(&slug).cloned();
        if prev.as_deref() != Some(name.as_str()) {
            let mut m = org_overrides.map.write();
            m.insert(slug, name);
        }
    });

    // state → overrides (mode — intentionally global, see the struct
    // doc above).
    let mut org_mode = org_overrides.mode;
    use_effect(move || {
        let mode = switcher_state.read().mode;
        if *org_mode.peek() != mode {
            org_mode.set(mode);
        }
    });

    switcher_state
}

/// Persist the home-org theme choice through `UserPrefs`. Call once at
/// the `App` root, after [`crate::prefs::provide_prefs`].
///
/// - **boot → seed**: once prefs carry a real user (post-load) and a
///   non-empty `theme_preset`/`theme_mode`, the values land in the
///   overrides (home-org entry + global mode). Runs again on server
///   reconcile / account switch; guarded writes keep it idempotent.
/// - **change → save**: any picker writes the overrides (see
///   [`use_org_theme_switcher_state`]); this effect mirrors the
///   home-org entry + mode into prefs via `PrefsCtx::update`
///   (optimistic signal → localStorage → server). It *peeks* prefs
///   (no ping-pong) and normalizes never-persisted prefs (`""`) to the
///   app defaults (`"default"` preset, dark mode) so boot noise —
///   pickers inserting "default" at mount — never upserts a row.
pub fn use_theme_prefs_sync() {
    let org_overrides = use_org_theme_overrides();
    let prefs_ctx = use_context::<PrefsCtx>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let home = use_memo(move || home_slug(&org_list.read()));

    // boot → seed (subscribes prefs + home; peeks the overrides).
    use_effect(move || {
        let prefs = prefs_ctx.prefs.read().clone();
        let slug = home();
        if prefs.user_id.is_nil() || slug.is_empty() {
            return; // prefs not loaded / discovery pending
        }
        if !prefs.theme_preset.is_empty() {
            let mut map = org_overrides.map;
            let prev = map.peek().get(&slug).cloned();
            if prev.as_deref() != Some(prefs.theme_preset.as_str()) {
                map.write().insert(slug, prefs.theme_preset.clone());
            }
        }
        if let Some(mode) = mode_from_str(&prefs.theme_mode) {
            let mut org_mode = org_overrides.mode;
            if *org_mode.peek() != mode {
                org_mode.set(mode);
            }
        }
    });

    // change → save (subscribes the overrides + home; peeks prefs).
    use_effect(move || {
        let slug = home();
        let name = org_overrides.map.read().get(&slug).cloned();
        let mode = *org_overrides.mode.read();
        let (user_id, prefs_name, prefs_mode) = {
            let prefs = prefs_ctx.prefs.peek();
            (
                prefs.user_id,
                prefs.theme_preset.clone(),
                prefs.theme_mode.clone(),
            )
        };
        if user_id.is_nil() || slug.is_empty() {
            return;
        }
        let Some(name) = name else { return };
        let mode_str = mode.as_str();
        // "" ⇒ the app defaults, so a never-persisted account isn't
        // upserted just because a picker mounted.
        let prefs_name = if prefs_name.is_empty() {
            "default"
        } else {
            prefs_name.as_str()
        };
        let prefs_mode = if prefs_mode.is_empty() {
            ThemeMode::Dark.as_str()
        } else {
            prefs_mode.as_str()
        };
        if prefs_name != name || prefs_mode != mode_str {
            prefs_ctx.update(|p| {
                p.theme_preset = name;
                p.theme_mode = mode_str.to_string();
            });
        }
    });
}

/// Wraps `children` in a `ThemeScope` when the project has an override,
/// otherwise just renders `children`. `ThemeScope` reads the parent
/// `ThemeContext` to inherit the current mode (light/dark), so the
/// override only swaps the color tokens.
#[component]
pub fn ProjectThemeScope(project_id: Uuid, children: Element) -> Element {
    let overrides = use_project_theme_overrides();
    let name_opt = overrides.map.read().get(&project_id).cloned();

    match name_opt {
        Some(name) => {
            let preset = theme_preset(&name).unwrap_or_else(default_theme_preset);
            rsx! {
                ThemeScope { styles: preset.styles, {children} }
            }
        }
        None => rsx! { {children} },
    }
}

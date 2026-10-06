//! Appearance — how the app looks, in words a person would use.
//!
//! The theme preset and light/dark mode were already choosable, but
//! the rest was raw CSS typed into text boxes (`0.625rem`, a font stack)
//! and nothing previewed what a preset looked like. This module is the
//! readable layer over the same machinery:
//!
//! - **Mode** — Light, Dark, or System (follows the device, live).
//! - **Theme** — every preset as a small card drawn in its own colours.
//! - **Text size**, **Density**, **Corners**, **Contrast**, **Font** —
//!   a few named steps each, applied on top of whatever preset is
//!   picked, so changing theme never loses them.
//!
//! Preset and mode keep syncing through the account (`UserPrefs`, see
//! [`crate::theming`]). The rest is per device — a phone and a 27"
//! monitor rarely want the same text size — and lives in
//! `localStorage` under [`STORAGE_KEY`].

use architect_ui::prelude::*;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Where the per-device settings are kept.
const STORAGE_KEY: &str = "task.appearance";

/// The whole interface scales with the root font size (every size is
/// `rem`-based), so this is the honest "make everything bigger" knob.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
    Larger,
}

impl TextSize {
    const ALL: [Self; 4] = [Self::Small, Self::Default, Self::Large, Self::Larger];
    fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
            Self::Larger => "Larger",
        }
    }
    fn root_px(self) -> u8 {
        match self {
            Self::Small => 15,
            Self::Default => 16,
            Self::Large => 18,
            Self::Larger => 20,
        }
    }
}

/// How much air between things: Tailwind's `--spacing` unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Density {
    Compact,
    #[default]
    Comfortable,
    Roomy,
}

impl Density {
    const ALL: [Self; 3] = [Self::Compact, Self::Comfortable, Self::Roomy];
    fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Comfortable => "Comfortable",
            Self::Roomy => "Roomy",
        }
    }
    /// `None` = the theme's own.
    fn spacing(self) -> Option<&'static str> {
        match self {
            Self::Compact => Some("0.22rem"),
            Self::Comfortable => None,
            Self::Roomy => Some("0.29rem"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corners {
    #[default]
    Theme,
    Square,
    Soft,
    Round,
}

impl Corners {
    const ALL: [Self; 4] = [Self::Theme, Self::Square, Self::Soft, Self::Round];
    fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme's",
            Self::Square => "Square",
            Self::Soft => "Soft",
            Self::Round => "Round",
        }
    }
    fn radius(self) -> Option<&'static str> {
        match self {
            Self::Theme => None,
            Self::Square => Some("0.125rem"),
            Self::Soft => Some("0.5rem"),
            Self::Round => Some("0.9rem"),
        }
    }
}

/// High contrast lifts secondary text and borders toward the
/// foreground — the faint grey that is the first thing to disappear on
/// a bright screen or for tired eyes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Contrast {
    #[default]
    Standard,
    High,
}

impl Contrast {
    const ALL: [Self; 2] = [Self::Standard, Self::High];
    fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::High => "High",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Font {
    #[default]
    Theme,
    System,
    Inter,
    Serif,
    Mono,
}

impl Font {
    const ALL: [Self; 5] = [
        Self::Theme,
        Self::System,
        Self::Inter,
        Self::Serif,
        Self::Mono,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme's",
            Self::System => "System",
            Self::Inter => "Inter",
            Self::Serif => "Serif",
            Self::Mono => "Mono",
        }
    }
    fn stack(self) -> Option<&'static str> {
        match self {
            Self::Theme => None,
            Self::System => Some(
                "system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", sans-serif",
            ),
            Self::Inter => Some("Inter, ui-sans-serif, system-ui, sans-serif"),
            Self::Serif => Some("\"Iowan Old Style\", \"Palatino Linotype\", Georgia, serif"),
            Self::Mono => Some("ui-monospace, \"JetBrains Mono\", \"SF Mono\", Menlo, monospace"),
        }
    }
}

/// Everything per device. `follow_system` is the "System" mode choice;
/// Light/Dark themselves live with the preset in `UserPrefs`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub follow_system: bool,
    pub text: TextSize,
    pub density: Density,
    pub corners: Corners,
    pub contrast: Contrast,
    pub font: Font,
}

impl Appearance {
    /// The CSS custom properties this appearance sets over the theme —
    /// empty for anything left at the theme's own value.
    #[must_use]
    pub fn css_vars(&self) -> String {
        let mut css = String::new();
        if let Some(r) = self.corners.radius() {
            // The same derivation the theme provider applies to its own
            // radius, so every `rounded-*` step moves together.
            css.push_str(&format!(
                "--radius:{r};--radius-sm:calc({r} - 4px);--radius-md:calc({r} - 2px);\
                 --radius-lg:{r};--radius-xl:calc({r} + 4px);--radius-2xl:calc({r} + 8px);\
                 --radius-3xl:calc({r} + 12px);"
            ));
        }
        if let Some(s) = self.density.spacing() {
            css.push_str(&format!("--spacing:{s};"));
        }
        if let Some(f) = self.font.stack() {
            css.push_str(&format!("--font-sans:{f};--default-font-family:{f};"));
        }
        if self.contrast == Contrast::High {
            css.push_str(
                "--muted-foreground:color-mix(in oklab, var(--foreground) 82%, var(--background));\
                 --border:color-mix(in oklab, var(--foreground) 30%, var(--background));\
                 --input:color-mix(in oklab, var(--foreground) 34%, var(--background));",
            );
        }
        css
    }
}

/// The appearance signal, provided once at the app root.
#[derive(Clone, Copy)]
pub struct AppearanceCtx(pub Signal<Appearance>);

#[must_use]
pub fn use_appearance() -> Signal<Appearance> {
    use_context::<AppearanceCtx>().0
}

/// Provide the appearance signal, load it from this device, and keep
/// it saved and applied. Call once at the app root, inside the
/// `ThemeProvider` (System mode writes the provider's mode).
pub fn provide_appearance() -> Signal<Appearance> {
    let mut appearance =
        use_context_provider(|| AppearanceCtx(Signal::new(Appearance::default()))).0;
    let mut loaded = use_signal(|| false);

    // Load once.
    use_hook(move || {
        spawn(async move {
            let stored = dioxus::document::eval(&format!(
                "return localStorage.getItem('{STORAGE_KEY}') || '';"
            ))
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .and_then(|s| serde_json::from_str::<Appearance>(&s).ok());
            if let Some(a) = stored {
                appearance.set(a);
            }
            loaded.set(true);
        });
    });

    // Save on change (after the load, so defaults never overwrite).
    use_effect(move || {
        let a = *appearance.read();
        if !loaded() {
            return;
        }
        if let Ok(json) = serde_json::to_string(&a) {
            let json = json.replace('\\', "\\\\").replace('\'', "\\'");
            let _ = dioxus::document::eval(&format!(
                "localStorage.setItem('{STORAGE_KEY}', '{json}');"
            ));
        }
    });

    // Text size scales the root, so it applies to the whole document.
    use_effect(move || {
        let px = appearance.read().text.root_px();
        let _ = dioxus::document::eval(&format!(
            "document.documentElement.style.fontSize = '{px}px';"
        ));
    });

    // System mode: follow the device's light/dark, live.
    let overrides = crate::theming::use_org_theme_overrides();
    use_effect(move || {
        if !appearance.read().follow_system {
            return;
        }
        let mut mode = overrides.mode;
        spawn(async move {
            let mut watch = dioxus::document::eval(
                "const q = window.matchMedia('(prefers-color-scheme: dark)');\
                 dioxus.send(q.matches);\
                 q.addEventListener('change', e => dioxus.send(e.matches));\
                 await new Promise(() => {});",
            );
            while let Ok(dark) = watch.recv::<bool>().await {
                if !appearance.peek().follow_system {
                    break;
                }
                let next = if dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                };
                if *mode.peek() != next {
                    mode.set(next);
                }
            }
        });
    });

    appearance
}

/// Applies the per-device appearance over the theme. Wrap the app body
/// in it, inside the `ThemeProvider`: CSS custom properties set here
/// override the provider's for everything below.
#[component]
pub fn AppearanceScope(children: Element) -> Element {
    let appearance = use_appearance();
    let vars = appearance.read().css_vars();
    rsx! {
        div { class: "contents", style: "{vars}", {children} }
    }
}

// ── the panel ───────────────────────────────────────────────────────

/// Segmented choice row.
#[component]
fn Choice<T: Copy + PartialEq + 'static>(
    label: &'static str,
    options: Vec<(T, &'static str)>,
    value: T,
    on_pick: EventHandler<T>,
    /// Per-option `font-family`, so each font names itself in its own
    /// face. Empty = the panel's.
    #[props(default)]
    fonts: Vec<&'static str>,
) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1.5",
            span { class: "text-xs font-medium text-muted-foreground", "{label}" }
            div { class: "flex flex-wrap gap-1",
                for (i, (opt, name)) in options.into_iter().enumerate() {
                    {
                        let active = opt == value;
                        let cls = if active {
                            "rounded-md border border-primary bg-primary/15 px-2.5 py-1.5 text-xs font-medium text-foreground"
                        } else {
                            "rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
                        };
                        let style = fonts
                            .get(i)
                            .filter(|stack| !stack.is_empty())
                            .map(|stack| format!("font-family:{stack}"))
                            .unwrap_or_default();
                        rsx! {
                            button {
                                key: "{name}",
                                r#type: "button",
                                class: "{cls}",
                                style: "{style}",
                                aria_pressed: active.to_string(),
                                onclick: move |_| on_pick.call(opt),
                                "{name}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One preset drawn in its own colours: page, a card, a line of text,
/// a muted line, and the accent — enough to tell themes apart at a
/// glance, which a list of names never did.
#[component]
fn PresetCard(
    preset: ThemePreset,
    mode: ThemeMode,
    active: bool,
    on_pick: EventHandler<String>,
) -> Element {
    let style = preset.styles.active(mode);
    let get = |k: &str, fallback: &str| style.get(k).unwrap_or(fallback).to_owned();
    let bg = get("background", "#111");
    let card = get("card", &bg);
    let fg = get("foreground", "#eee");
    let muted = get("muted-foreground", &fg);
    let primary = get("primary", &fg);
    let border = get("border", &muted);
    let ring = if active {
        "ring-2 ring-primary"
    } else {
        "ring-1 ring-border hover:ring-primary/50"
    };
    let name = preset.name.clone();
    rsx! {
        button {
            r#type: "button",
            class: "flex flex-col gap-1 rounded-lg p-1 text-left transition-shadow {ring}",
            aria_pressed: active.to_string(),
            onclick: move |_| on_pick.call(name.clone()),
            div {
                class: "flex h-14 flex-col gap-1 rounded-md p-1.5",
                style: "background:{bg};border:1px solid {border}",
                div {
                    class: "flex flex-1 flex-col justify-center gap-1 rounded px-1.5",
                    style: "background:{card}",
                    div { class: "h-1.5 w-3/4 rounded-full", style: "background:{fg}" }
                    div { class: "h-1 w-1/2 rounded-full", style: "background:{muted}" }
                }
                div { class: "h-1.5 w-1/3 rounded-full", style: "background:{primary}" }
            }
            span { class: "truncate px-0.5 text-[11px] text-foreground", "{preset.label}" }
        }
    }
}

/// The appearance panel — the popover in the rail and the Settings
/// section are both this.
#[component]
pub fn AppearancePanel(state: Signal<ThemeState>, #[props(default)] class: String) -> Element {
    let mut state = state;
    let mut appearance = use_appearance();
    let a = *appearance.read();
    let mode = state.read().mode;
    let current = state.read().preset.clone();

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Light,
        Dark,
        System,
    }
    let mode_choice = if a.follow_system {
        Mode::System
    } else if mode.is_dark() {
        Mode::Dark
    } else {
        Mode::Light
    };

    rsx! {
        div { class: "flex flex-col gap-4 {class}",
            Choice {
                label: "Mode",
                options: vec![(Mode::Light, "Light"), (Mode::Dark, "Dark"), (Mode::System, "System")],
                value: mode_choice,
                on_pick: move |m: Mode| {
                    appearance.write().follow_system = m == Mode::System;
                    match m {
                        Mode::Light => state.write().set_mode(ThemeMode::Light),
                        Mode::Dark => state.write().set_mode(ThemeMode::Dark),
                        // The provide_appearance watcher sets it from the device.
                        Mode::System => {}
                    }
                },
            }
            div { class: "flex flex-col gap-1.5",
                span { class: "text-xs font-medium text-muted-foreground", "Theme" }
                div { class: "grid grid-cols-3 gap-1.5",
                    for preset in theme_presets() {
                        PresetCard {
                            key: "{preset.name}",
                            active: preset.name == current,
                            preset: preset.clone(),
                            mode,
                            on_pick: move |name: String| {
                                if let Some(p) = theme_preset(&name) {
                                    state.write().set_preset(p);
                                }
                            },
                        }
                    }
                }
            }
            Choice {
                label: "Text size",
                options: TextSize::ALL.iter().map(|t| (*t, t.label())).collect(),
                value: a.text,
                on_pick: move |t| appearance.write().text = t,
            }
            Choice {
                label: "Density",
                options: Density::ALL.iter().map(|t| (*t, t.label())).collect(),
                value: a.density,
                on_pick: move |t| appearance.write().density = t,
            }
            Choice {
                label: "Corners",
                options: Corners::ALL.iter().map(|t| (*t, t.label())).collect(),
                value: a.corners,
                on_pick: move |t| appearance.write().corners = t,
            }
            Choice {
                label: "Contrast",
                options: Contrast::ALL.iter().map(|t| (*t, t.label())).collect(),
                value: a.contrast,
                on_pick: move |t| appearance.write().contrast = t,
            }
            Choice {
                label: "Font",
                options: Font::ALL.iter().map(|t| (*t, t.label())).collect(),
                value: a.font,
                on_pick: move |t| appearance.write().font = t,
                fonts: Font::ALL.iter().map(|f| f.stack().unwrap_or("")).collect::<Vec<_>>(),
            }
            button {
                r#type: "button",
                class: "self-start text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline",
                onclick: move |_| {
                    appearance.set(Appearance::default());
                    if let Some(p) = theme_preset("default") {
                        state.write().set_preset(p);
                    }
                    state.write().set_mode(ThemeMode::Dark);
                },
                "Reset to defaults"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_the_theme_alone() {
        assert_eq!(Appearance::default().css_vars(), "");
    }

    #[test]
    fn choices_become_the_theme_variables_they_name() {
        let a = Appearance {
            corners: Corners::Square,
            density: Density::Roomy,
            contrast: Contrast::High,
            font: Font::Serif,
            ..Appearance::default()
        };
        let css = a.css_vars();
        assert!(css.contains("--radius:0.125rem;"));
        assert!(css.contains("--radius-xl:calc(0.125rem + 4px);"));
        assert!(css.contains("--spacing:0.29rem;"));
        assert!(css.contains("--muted-foreground:color-mix("));
        assert!(css.contains("--font-sans:\"Iowan Old Style\""));
    }

    /// Stored settings from an older build (a field missing) still load.
    #[test]
    fn a_partial_stored_appearance_loads_with_defaults() {
        let a: Appearance = serde_json::from_str(r#"{"text":"Large"}"#).unwrap();
        assert_eq!(a.text, TextSize::Large);
        assert_eq!(a.density, Density::Comfortable);
    }
}

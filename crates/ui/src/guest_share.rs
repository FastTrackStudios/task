//! The share-guest entry (issue #272): `/share-review?org=…&token=…`
//! boots the REAL app in guest mode — no account, no sign-in gate, no
//! shell chrome. Every org connection dials the token-scoped guest
//! lane (`/org/{slug}/share/{token}/vox`), which speaks the identical
//! wire contract, so the review player and comment thread mount
//! unchanged: playback, timecode seeking, commenting, drawing.
//!
//! Detection happens once at boot from `window.location` (a share URL
//! is a full page load, never an in-app navigation), BEFORE the app
//! root's providers — the guest path never runs auth restore, org
//! discovery, presence, or the router.

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::theming::state_from_preset_name;

/// What the share URL carries. The review's own scope (root, file) is
/// deliberately NOT in the URL — the guest lane's `list_reviews`
/// answers with exactly its one review.
#[derive(Clone, Debug, PartialEq)]
pub struct GuestShareSession {
    pub org: String,
    pub token: String,
    pub pw: Option<String>,
    /// The server origin the landing page linked from — a fresh guest
    /// has no server registry, and same-origin only holds when app and
    /// server share an origin (prod).
    pub server: Option<String>,
}

/// Parse the boot URL. `None` on native and on every non-share URL —
/// the app boots normally.
pub fn detect() -> Option<GuestShareSession> {
    #[cfg(target_arch = "wasm32")]
    {
        let location = web_sys::window()?.location();
        if location.pathname().ok()? != "/share-review" {
            return None;
        }
        let search = location.search().ok()?;
        let mut org = None;
        let mut token = None;
        let mut pw = None;
        let mut server = None;
        for pair in search.trim_start_matches('?').split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = js_sys::decode_uri_component(value)
                .ok()
                .map(|v| String::from(v))
                .unwrap_or_else(|| value.to_string());
            match key {
                "org" => org = Some(value),
                "token" => token = Some(value),
                "pw" => pw = Some(value).filter(|p| !p.is_empty()),
                "server" => server = Some(value).filter(|s| !s.is_empty()),
                _ => {}
            }
        }
        let (org, token) = (org?, token?);
        if org.is_empty() || token.is_empty() {
            return None;
        }
        Some(GuestShareSession {
            org,
            token,
            pw,
            server,
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// The whole guest app: a themed page with the review player — nothing
/// else of the shell exists on this lane.
#[component]
pub fn GuestShell(session: GuestShareSession) -> Element {
    // Enter guest mode before any connection is established: from here
    // on, every `establish_for(org)` dials the share lane.
    {
        let session = session.clone();
        use_hook(move || {
            // The landing told us where the server lives; without it,
            // same-origin (the prod deployment's shape) applies.
            if let Some(server) = &session.server {
                let ws = server
                    .replacen("https://", "wss://", 1)
                    .replacen("http://", "ws://", 1);
                crate::vox_session::set_active_server(Some(crate::vox_session::ActiveServer {
                    url: ws,
                    token: None,
                }));
            }
            crate::vox_session::set_guest_share(Some(crate::vox_session::GuestShare {
                org: session.org.clone(),
                token: session.token.clone(),
                pw: session.pw.clone(),
            }));
        });
    }
    let theme_state = use_signal(|| state_from_preset_name("", ThemeMode::Dark));

    let org_for_review = session.org.clone();
    let review = use_resource(move || {
        let org = org_for_review.clone();
        async move { files_ui::review::guest_scoped_review(&org).await }
    });

    let org = session.org.clone();
    rsx! {
        ThemeProvider { state: theme_state,
            ToastProvider {
                div { class: "h-screen w-screen overflow-hidden bg-background text-foreground",
                    {match &*review.read_unchecked() {
                        None => rsx! {
                            div { class: "flex h-full items-center justify-center",
                                Text { variant: TextVariant::Muted, "Opening the review…" }
                            }
                        },
                        Some(Err(e)) => rsx! {
                            div { class: "flex h-full items-center justify-center p-6",
                                div { class: "w-full max-w-md rounded-lg border border-border/40 bg-card/40 p-6 flex flex-col gap-2",
                                    Heading { level: HeadingLevel::H2, "This review link didn't open" }
                                    Text { variant: TextVariant::Muted, "{e}" }
                                    Text { variant: TextVariant::Muted, class: "text-xs",
                                        "The link may be disabled, expired, or password-protected — reopen it from the page you were given."
                                    }
                                }
                            }
                        },
                        // The full review experience owns the viewport —
                        // the guest badge and attribution note ride its
                        // top bar.
                        Some(Ok(info)) => rsx! {
                            files_ui::review::ReviewScreen {
                                org: org.clone(),
                                root_id: info.root_id,
                                path: info.file_path.clone(),
                            }
                        },
                    }}
                }
            }
        }
    }
}

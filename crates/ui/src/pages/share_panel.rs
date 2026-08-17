//! The note Share panel (right-sidebar tab) — mint, track, and manage
//! share links for the focused note. Samply-style: every link ever
//! created is listed here with its capability, and can be disabled
//! (reversibly) or deleted after the fact — changes are retroactive on
//! the server.

use dioxus::prelude::*;

use crate::vox_clients::share_client;
use share_proto::{NewShareLink, ShareLinkInfo, ShareTarget};

/// Copy `text` to the clipboard (browser only; ignored on native).
fn copy_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(win) = web_sys::window() {
            let _ = win.navigator().clipboard().write_text(text);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = text;
}

#[component]
pub fn SharePanel(slug: String, path: Option<String>) -> Element {
    let Some(path) = path else {
        return rsx! {
            div { class: "p-4 text-sm text-muted-foreground",
                "Select a note to share it."
            }
        };
    };

    // Bump to refetch after a mutation.
    let refresh = use_signal(|| 0u32);
    let slug2 = slug.clone();
    let path2 = path.clone();
    let links = use_resource(use_reactive!(|(slug2, path2)| {
        let _ = refresh();
        async move {
            let client = share_client(&slug2).await?;
            client
                .links_for_target(ShareTarget::Note {
                    path: path2.clone(),
                })
                .await
                .map_err(|e| format!("{e:?}"))
        }
    }));

    let create = use_callback({
        let slug = slug.clone();
        let path = path.clone();
        let mut refresh = refresh;
        move |()| {
            let slug = slug.clone();
            let path = path.clone();
            spawn(async move {
                match share_client(&slug).await {
                    Ok(client) => {
                        if let Err(e) = client
                            .create_link(
                                ShareTarget::Note { path },
                                NewShareLink {
                                    label: "share link".into(),
                                    capabilities: None,
                                    password: None,
                                    expires_unix: None,
                                },
                            )
                            .await
                        {
                            tracing::warn!("share: create_link failed: {e:?}");
                        }
                    }
                    Err(e) => tracing::warn!("share: client: {e}"),
                }
                refresh += 1;
            });
        }
    });

    let set_disabled = use_callback({
        let slug = slug.clone();
        let mut refresh = refresh;
        move |(token, disabled): (String, bool)| {
            let slug = slug.clone();
            spawn(async move {
                if let Ok(client) = share_client(&slug).await {
                    if let Err(e) = client.set_link_disabled(token, disabled).await {
                        tracing::warn!("share: set_link_disabled failed: {e:?}");
                    }
                }
                refresh += 1;
            });
        }
    });

    let delete = use_callback({
        let slug = slug.clone();
        let mut refresh = refresh;
        move |token: String| {
            let slug = slug.clone();
            spawn(async move {
                if let Ok(client) = share_client(&slug).await {
                    if let Err(e) = client.delete_link(token).await {
                        tracing::warn!("share: delete_link failed: {e:?}");
                    }
                }
                refresh += 1;
            });
        }
    });

    rsx! {
        div { class: "flex flex-col gap-3 p-3",
            div { class: "flex items-center justify-between",
                span { class: "text-xs font-semibold uppercase tracking-wider text-muted-foreground",
                    "Links"
                }
                button {
                    class: "rounded-md bg-primary px-2.5 py-1 text-xs font-semibold text-primary-foreground hover:opacity-90",
                    onclick: move |_| create.call(()),
                    "+ New link"
                }
            }
            match &*links.read_unchecked() {
                None => rsx! {
                    span { class: "text-sm text-muted-foreground", "Loading…" }
                },
                Some(Err(e)) => rsx! {
                    crate::states::InlineError {
                        message: e.clone(),
                        label: "Sharing".to_string(),
                    }
                },
                Some(Ok(list)) if list.is_empty() => rsx! {
                    div { class: "rounded-md border border-dashed border-border p-4 text-center text-sm text-muted-foreground",
                        "No links yet. Mint one to share this note — anyone with the link can open it; disable or delete it here any time."
                    }
                },
                Some(Ok(list)) => rsx! {
                    for link in list.clone() {
                        ShareLinkRow {
                            link,
                            on_toggle: set_disabled,
                            on_delete: delete,
                        }
                    }
                },
            }
        }
    }
}

#[component]
fn ShareLinkRow(
    link: ShareLinkInfo,
    on_toggle: Callback<(String, bool)>,
    on_delete: Callback<String>,
) -> Element {
    let mut copied = use_signal(|| false);
    let url = link.url.clone();
    let token_t = link.token.clone();
    let token_d = link.token.clone();
    let disabled = link.disabled;
    rsx! {
        div {
            class: if disabled {
                "flex flex-col gap-1.5 rounded-md border border-border bg-muted/40 p-2.5 opacity-60"
            } else {
                "flex flex-col gap-1.5 rounded-md border border-border bg-muted/40 p-2.5"
            },
            div { class: "flex items-center justify-between gap-2",
                span { class: "truncate text-sm font-medium text-foreground", "{link.label}" }
                span { class: "shrink-0 rounded-full border border-border px-2 py-0.5 text-[10px] uppercase tracking-wider text-muted-foreground",
                    if disabled {
                        "disabled"
                    } else if link.capabilities.comment {
                        "comment"
                    } else {
                        "view"
                    }
                }
            }
            div { class: "truncate rounded bg-background px-2 py-1 font-mono text-[11px] text-muted-foreground",
                "{link.url}"
            }
            div { class: "flex items-center gap-2",
                button {
                    class: "rounded px-2 py-0.5 text-xs text-foreground hover:bg-accent",
                    onclick: move |_| {
                        copy_to_clipboard(&url);
                        copied.set(true);
                    },
                    if copied() { "Copied ✓" } else { "Copy" }
                }
                button {
                    class: "rounded px-2 py-0.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                    onclick: move |_| on_toggle.call((token_t.clone(), !disabled)),
                    if disabled { "Enable" } else { "Disable" }
                }
                button {
                    class: "ml-auto rounded px-2 py-0.5 text-xs text-destructive hover:bg-accent",
                    onclick: move |_| on_delete.call(token_d.clone()),
                    "Delete"
                }
            }
        }
    }
}

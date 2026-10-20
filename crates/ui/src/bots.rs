//! Settings → Bots: the agents a person runs (Hermes bots, say), each
//! with an account of its own in the orgs it was given, and a key.
//!
//! Bots live on a server — the always-on one their MCP client reaches —
//! so this panel works against one of the person's servers (never this
//! device's own node, which only listens on loopback), over
//! `BotService` on its `/server/vox`. The key is shown once, on the
//! screen that made it, with the lines a Hermes profile needs.

use dioxus::prelude::*;
use org_proto::{BotInfo, BotServiceClient, BotTools, CreateBotRequest, RevokeBotRequest};
use task_ui_core::sheet::{SheetGroup, SheetSection, SheetSelect, SheetToggle};

use crate::server_registry::{ServerEntry, ServerRegistry};

/// `https://host` for a server entry's vox URL.
fn http_base(server_url: &str) -> String {
    let (scheme, rest) = server_url.split_once("://").unwrap_or(("wss", server_url));
    let host = rest.split('/').next().unwrap_or(rest);
    let scheme = if scheme == "ws" { "http" } else { "https" };
    format!("{scheme}://{host}")
}

fn token_for(entry: &ServerEntry) -> Option<String> {
    entry
        .session_token
        .clone()
        .or_else(task_ui_core::vox_session::bearer)
}

async fn client(entry: &ServerEntry) -> Result<BotServiceClient, String> {
    crate::vox_clients::establish_server::<BotServiceClient>(Some(&entry.server_url)).await
}

fn tools_label(t: BotTools) -> &'static str {
    match t {
        BotTools::All => "Every tool",
        BotTools::Library => "Notes and wikis",
    }
}

/// The Hermes profile lines for a key just made.
fn hermes_snippet(base: &str, key: &str) -> String {
    format!(
        "# ~/.hermes/profiles/<bot>/.env\nTASK_TOKEN={key}\n\n# ~/.hermes/profiles/<bot>/config.yaml\nmcp_servers:\n  task:\n    url: \"{base}/mcp\"\n    headers:\n      Authorization: \"Bearer ${{TASK_TOKEN}}\"\n"
    )
}

#[component]
pub fn BotsPanel() -> Element {
    let registry = use_context::<ServerRegistry>();
    let hosts: Vec<ServerEntry> = registry
        .list()
        .into_iter()
        .filter(|s| !task_ui_core::node::is_local(&s.server_url))
        .collect();
    let mut chosen = use_signal(|| None::<String>);
    let Some(default) = hosts
        .iter()
        .find(|h| Some(h.id) == registry.active_id())
        .or_else(|| hosts.first())
        .cloned()
    else {
        return rsx! {
            p { class: "px-2 text-[15px] text-muted-foreground",
                "Bots live on a server that is always on — your own Task server. Add it under Account, then make bots for it here."
            }
        };
    };
    let entry = chosen()
        .and_then(|id| hosts.iter().find(|h| h.id.to_string() == id).cloned())
        .unwrap_or(default);

    rsx! {
        if hosts.len() > 1 {
            SheetGroup {
                SheetSelect {
                    label: "Server".to_owned(),
                    value: entry.id.to_string(),
                    options: hosts.iter().map(|h| (h.id.to_string(), h.label.clone())).collect(),
                    on_change: move |id| chosen.set(Some(id)),
                }
            }
        }
        ServerBots { key: "{entry.id}", entry }
    }
}

#[component]
fn ServerBots(entry: ServerEntry) -> Element {
    let mut epoch = use_signal(|| 0u32);
    let e = entry.clone();
    let bots = use_resource(move || {
        let e = e.clone();
        let _ = epoch();
        async move {
            let token = token_for(&e).ok_or("sign in to this server first")?;
            client(&e)
                .await?
                .list_bots(token)
                .await
                .map_err(|err| format!("{err:?}"))
        }
    });
    let mut made = use_signal(|| None::<(String, String)>);
    let base = http_base(&entry.server_url);

    rsx! {
        div { "data-testid": "bots-panel",
            SheetSection { label: format!("Bots on {}", entry.label) }
            match &*bots.read() {
                None => rsx! { p { class: "px-2 pb-3 text-[13px] text-muted-foreground", "Loading…" } },
                Some(Err(e)) => rsx! { p { class: "px-2 pb-3 text-[13px] text-destructive", "{e}" } },
                Some(Ok(list)) if list.is_empty() => rsx! {
                    p { class: "px-2 pb-3 text-[15px] text-muted-foreground",
                        "No bots yet. A bot gets its own account in the orgs you give it, and a key for its MCP client."
                    }
                },
                Some(Ok(list)) => rsx! {
                    SheetGroup {
                        for bot in list.iter().cloned() {
                            BotRow {
                                key: "{bot.id}",
                                bot,
                                entry: entry.clone(),
                                on_revoked: move |()| epoch += 1,
                            }
                        }
                    }
                },
            }
            if let Some((name, key)) = made() {
                KeyCard { name, snippet: hermes_snippet(&base, &key), on_done: move |()| made.set(None) }
            } else {
                NewBot {
                    entry: entry.clone(),
                    on_made: move |(name, key)| {
                        made.set(Some((name, key)));
                        epoch += 1;
                    },
                }
            }
        }
    }
}

#[component]
fn BotRow(bot: BotInfo, entry: ServerEntry, on_revoked: EventHandler<()>) -> Element {
    let mut busy = use_signal(|| false);
    let orgs = bot.orgs.join(", ");
    let id = bot.id.clone();
    rsx! {
        div { class: "flex min-h-[60px] items-center gap-3 px-4 py-2",
            div { class: "flex min-w-0 flex-1 flex-col",
                span { class: "truncate text-[16px] text-foreground", "{bot.name}" }
                span { class: "truncate text-[13px] text-muted-foreground", "{tools_label(bot.tools)} · {orgs}" }
            }
            button {
                r#type: "button",
                class: "rounded-full px-3 py-1.5 text-[14px] text-destructive active:bg-destructive/10 disabled:opacity-50",
                disabled: busy(),
                onclick: move |_| {
                    let entry = entry.clone();
                    let id = id.clone();
                    busy.set(true);
                    spawn(async move {
                        let done = async {
                            let token = token_for(&entry).ok_or("not signed in")?;
                            client(&entry)
                                .await?
                                .revoke_bot(RevokeBotRequest { session_token: token, bot_id: id })
                                .await
                                .map_err(|e| format!("{e:?}"))
                        }
                        .await;
                        busy.set(false);
                        match done {
                            Ok(()) => on_revoked.call(()),
                            Err(e) => tracing::warn!("bots: revoke failed — {e}"),
                        }
                    });
                },
                "Revoke"
            }
        }
    }
}

#[component]
fn NewBot(entry: ServerEntry, on_made: EventHandler<(String, String)>) -> Element {
    let mut name = use_signal(String::new);
    let mut library = use_signal(|| false);
    let mut picked = use_signal(Vec::<String>::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    // The orgs to offer: the ones the app knows this account is in. The
    // server checks again — a bot gets only orgs its person is in.
    let orgs: Vec<String> = use_context::<Signal<Vec<task_ui_core::orgs::OrgMeta>>>()
        .read()
        .iter()
        .filter(|o| o.member != Some(false))
        .map(|o| o.slug.clone())
        .collect();

    let can_make = !name().trim().is_empty() && !picked().is_empty() && !busy();
    rsx! {
        SheetSection { label: "New bot".to_owned() }
        SheetGroup {
            div { class: "flex min-h-[54px] items-center px-4",
                input {
                    class: "w-full bg-transparent text-[16px] text-foreground outline-none placeholder:text-muted-foreground",
                    placeholder: "Name — Atlas, Librarian…",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            SheetToggle {
                label: "Notes and wikis only".to_owned(),
                on: library(),
                on_toggle: move |on| library.set(on),
            }
        }
        if !orgs.is_empty() {
            SheetSection { label: "It may reach".to_owned() }
            SheetGroup {
                for slug in orgs {
                    SheetToggle {
                        key: "{slug}",
                        label: slug.clone(),
                        on: picked().contains(&slug),
                        on_toggle: move |on: bool| {
                            let mut p = picked.write();
                            p.retain(|s| *s != slug);
                            if on {
                                p.push(slug.clone());
                            }
                        },
                    }
                }
            }
        }
        if let Some(e) = error() {
            p { class: "px-2 pb-2 text-[13px] text-destructive", "{e}" }
        }
        button {
            r#type: "button",
            class: "mb-3 w-full rounded-[14px] bg-primary py-3 text-[16px] font-semibold text-primary-foreground disabled:opacity-40",
            disabled: !can_make,
            onclick: move |_| {
                let entry = entry.clone();
                let req_name = name.peek().trim().to_owned();
                let orgs = picked.peek().clone();
                let tools = if library() { BotTools::Library } else { BotTools::All };
                busy.set(true);
                error.set(None);
                spawn(async move {
                    let made = async {
                        let token = token_for(&entry).ok_or("sign in to this server first")?;
                        client(&entry)
                            .await?
                            .create_bot(CreateBotRequest {
                                session_token: token,
                                name: req_name.clone(),
                                orgs,
                                tools,
                            })
                            .await
                            .map_err(|e| format!("{e:?}"))
                    }
                    .await;
                    busy.set(false);
                    match made {
                        Ok(created) => {
                            name.set(String::new());
                            picked.set(Vec::new());
                            on_made.call((created.bot.name, created.key));
                        }
                        Err(e) => error.set(Some(e)),
                    }
                });
            },
            "Make bot"
        }
    }
}

/// The key, once: copy it into the bot's Hermes profile now.
#[component]
fn KeyCard(name: String, snippet: String, on_done: EventHandler<()>) -> Element {
    let copy = snippet.clone();
    rsx! {
        div { class: "mb-3 rounded-[20px] bg-foreground/[0.05] p-4", "data-testid": "bot-key",
            div { class: "pb-1 text-[16px] font-semibold text-foreground", "{name} is ready" }
            p { class: "pb-3 text-[13px] text-muted-foreground",
                "This is the only time its key is shown. Put these in the bot's Hermes profile."
            }
            pre { class: "select-all overflow-x-auto whitespace-pre rounded-xl bg-background p-3 font-mono text-[12px] text-foreground",
                "{snippet}"
            }
            div { class: "flex gap-2 pt-3",
                button {
                    r#type: "button",
                    class: "flex-1 rounded-[14px] bg-foreground/[0.08] py-2.5 text-[15px] text-foreground",
                    onclick: move |_| {
                        let js = format!("navigator.clipboard.writeText({})", serde_json::to_string(&copy).unwrap_or_default());
                        document::eval(&js);
                    },
                    "Copy"
                }
                button {
                    r#type: "button",
                    class: "flex-1 rounded-[14px] bg-primary py-2.5 text-[15px] font-semibold text-primary-foreground",
                    onclick: move |_| on_done.call(()),
                    "Done"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_snippet_points_hermes_at_the_servers_mcp() {
        assert_eq!(super::http_base("wss://task.example.com/vox"), "https://task.example.com");
        assert_eq!(super::http_base("ws://127.0.0.1:9090/vox"), "http://127.0.0.1:9090");
        let s = super::hermes_snippet("https://task.example.com", "ak_x");
        assert!(s.contains("TASK_TOKEN=ak_x"));
        assert!(s.contains("url: \"https://task.example.com/mcp\""));
        assert!(s.contains("Bearer ${TASK_TOKEN}"));
    }
}

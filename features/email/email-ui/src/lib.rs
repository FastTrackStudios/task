//! `/email` — mail over the org's `EmailSync` + `EmailProduct`
//! services.
//!
//! v1 product surface: account chips, the account's recent
//! `INBOX` envelopes, a minimal compose (new message + reply)
//! that *stages* drafts into the outbox, and the outbox panel
//! where staged mail is approved or cancelled — the
//! human-in-the-loop gate. Everything re-reads on the one
//! `EmailChange` stream (changes-only contract): mailbox events
//! re-list envelopes, `OutboxChanged` re-lists the outbox.
//!
//! The backend is a Maildir-backed `EmailSync` impl; an org with
//! no configured mailbox returns an empty account list, which
//! renders as an empty state rather than an error. Sending
//! requires the account's `account.json` to configure an SMTP
//! `submit` endpoint — a staged entry on an account without one
//! surfaces the delivery error in the outbox panel.

mod offline;

use architect_ui::prelude::*;
use dioxus::prelude::*;
use email_proto::{Account, Addr, Draft, Envelope, OutboxEntry, OutboxStatus};

use task_ui_core::feeds;
use task_ui_core::orgs::{OrgMeta, OrgSelection};

/// What the compose form opens with. `None` reply fields = a
/// fresh message.
#[derive(Clone, PartialEq)]
struct ComposeSeed {
    to: String,
    subject: String,
    in_reply_to: Option<String>,
}

#[component]
pub fn EmailView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we read mail from (first selected, or home).
    let slug = use_memo(move || {
        task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    // Which account is selected (its id). `None` until accounts load;
    // we default to the first account once they arrive.
    let mut selected_account = use_signal(|| None::<String>);
    // Open compose form, if any.
    let mut composing = use_signal(|| None::<ComposeSeed>);

    let accounts = use_resource(move || async move {
        let Some(s) = slug() else {
            return Ok(Vec::new());
        };
        // Cached on success, served on failure — same contract as the
        // listings. Without this the offline path never starts: no
        // accounts means nothing selected, which means the cached
        // envelopes are never asked for.
        match fetch_email_accounts(&s).await {
            Ok(list) => {
                offline::put_accounts(&s, &list);
                Ok(list)
            }
            Err(err) => offline::get_accounts(&s).ok_or(err),
        }
    });

    // Settle on a default account once the list loads (first one).
    // Accounts gate everything below — nothing is selected, so nothing
    // is listed, until they resolve. Painting the cached list while the
    // live call is in flight is what actually makes an offline reload
    // instant; caching the envelopes alone still waited on this.
    let account_list: Vec<Account> = match &*accounts.read() {
        Some(Ok(list)) => list.clone(),
        Some(Err(_)) => Vec::new(),
        None => slug()
            .and_then(|s| offline::get_accounts(&s))
            .unwrap_or_default(),
    };
    use_effect(move || {
        if selected_account.peek().is_none() {
            if let Some(Ok(list)) = &*accounts.read() {
                if let Some(first) = list.first() {
                    selected_account.set(Some(first.id.0.clone()));
                }
            }
        }
    });

    // Every mailbox on the account, for the folder rail. Restarted by
    // `FoldersChanged` alongside the mailbox events below.
    let folders = use_resource(move || async move {
        match (slug(), selected_account()) {
            (Some(s), Some(acct)) => fetch_email_folders(&s, &acct).await,
            _ => Ok(Vec::new()),
        }
    });
    let folder_list: Vec<email_proto::Folder> = match &*folders.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };

    // Which mailbox is open. `INBOX` is the safe default: every
    // backend has it, including ones that report no folder list at all.
    let mut selected_folder = use_signal(|| "INBOX".to_owned());
    // Which message is open in the reading pane, by message-id.
    let mut open_message = use_signal(|| None::<String>);

    // Changing account invalidates both — the folder ids and
    // message-ids belong to the account that is going away.
    use_effect(move || {
        let _ = selected_account();
        selected_folder.set("INBOX".to_owned());
        open_message.set(None);
    });
    // Likewise, a message open in one folder isn't in the next.
    use_effect(move || {
        let _ = selected_folder();
        open_message.set(None);
    });

    let envelopes = use_resource(move || async move {
        match (slug(), selected_account()) {
            (Some(s), Some(acct)) => fetch_email_envelopes(&s, &acct, &selected_folder(), 50).await,
            _ => Ok(Vec::new()),
        }
    });

    // The open message's full body. `None` selection resolves to
    // `Ok(None)` so the pane renders its placeholder rather than an
    // error.
    let message = use_resource(move || async move {
        match (slug(), selected_account(), open_message()) {
            (Some(s), Some(acct), Some(id)) => fetch_email_message(&s, &acct, &id).await.map(Some),
            _ => Ok(None),
        }
    });

    // Outbox entries for the selected account, newest first.
    let outbox = use_resource(move || async move {
        match (slug(), selected_account()) {
            (Some(s), Some(acct)) => fetch_email_outbox(&s, &acct).await,
            _ => Ok(Vec::new()),
        }
    });

    // Triage derivations (urgency / tags) for the listed
    // envelopes. Reads the envelopes resource, so it re-fetches
    // whenever the list does; `DerivationsUpdated` restarts it
    // directly.
    let derivs = use_resource(move || async move {
        let ids: Vec<String> = match &*envelopes.read() {
            Some(Ok(list)) => list.iter().map(|e| e.message_id.clone()).collect(),
            _ => Vec::new(),
        };
        match (slug(), selected_account()) {
            (Some(s), Some(acct)) if !ids.is_empty() => {
                fetch_email_derivations(&s, &acct, ids).await
            }
            _ => Ok(Vec::new()),
        }
    });

    // ── Live changes ──────────────────────────────────────────
    // One `EmailChange` stream carries mailbox AND outbox events
    // (shared hub server-side). Events name what changed, not the
    // new value — a hit for the selected account re-reads the
    // touched list.
    architect::use_stream(
        move |tx| {
            let slug = slug();
            async move {
                let Some(slug) = slug else {
                    return false;
                };
                let Ok(client) = task_ui_core::vox_clients::establish_for::<
                    email_proto::EmailSyncStreamClient,
                >(&slug)
                .await
                else {
                    return false;
                };
                client.changes(tx).await.is_ok()
            }
        },
        move |change: email_proto::EmailChange| {
            let mut envelopes = envelopes;
            let mut outbox = outbox;
            let mut derivs = derivs;
            let mut folders = folders;
            let mut message = message;
            let mut open_message = open_message;
            if selected_account.peek().as_deref() != Some(change.account.as_str()) {
                return;
            }
            match change.event {
                email_proto::EmailEvent::OutboxChanged { .. } => outbox.restart(),
                email_proto::EmailEvent::DerivationsUpdated { .. } => derivs.restart(),
                // The rail carries per-folder unread counts, so a new
                // message or a flag flip changes it too — not just an
                // explicit folder-list change.
                email_proto::EmailEvent::FolderListChanged => {
                    folders.restart();
                    envelopes.restart();
                }
                // A flag change on the message we are reading (ours or
                // another client's) has to reach the open pane.
                email_proto::EmailEvent::FlagsChanged { ref message_id, .. } => {
                    if open_message.peek().as_deref() == Some(message_id.as_str()) {
                        message.restart();
                    }
                    folders.restart();
                    envelopes.restart();
                }
                // Whatever we were reading is no longer here.
                email_proto::EmailEvent::Moved { ref message_id, .. }
                | email_proto::EmailEvent::Deleted { ref message_id } => {
                    if open_message.peek().as_deref() == Some(message_id.as_str()) {
                        open_message.set(None);
                    }
                    folders.restart();
                    envelopes.restart();
                }
                _ => {
                    folders.restart();
                    envelopes.restart();
                }
            }
        },
    );

    let accounts_err = match &*accounts.read() {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    };
    let current = selected_account();
    let current_address = account_list
        .iter()
        .find(|a| Some(a.id.0.as_str()) == current.as_deref())
        .map(|a| a.address.clone());
    // While the live call is still in flight, show the cached listing
    // rather than a blank pane.
    //
    // `fetch_email_envelopes` already falls back to the cache on
    // failure, but only *after* the call gives up — and a dead server
    // takes several seconds to give up, which reads as a hang. Painting
    // the cache immediately and letting the live answer replace it
    // makes the mailbox appear at once, online or off. (Fixing the
    // underlying delay means fail-fast in the shared connection path,
    // which every service in the app uses — not worth the blast radius
    // for this.)
    let (rows, rows_err): (Vec<Envelope>, Option<String>) = match &*envelopes.read() {
        Some(Ok(list)) => (list.clone(), None),
        Some(Err(e)) => (Vec::new(), Some(e.clone())),
        None => (
            match (slug(), current.clone()) {
                (Some(s), Some(acct)) => {
                    offline::get_envelopes(&s, &acct, &selected_folder()).unwrap_or_default()
                }
                _ => Vec::new(),
            },
            None,
        ),
    };
    let outbox_rows: Vec<OutboxEntry> = match &*outbox.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    // Same for the reading pane: a body we have read before paints
    // immediately instead of after the failed call times out.
    let (open_msg, msg_err): (Option<email_proto::Message>, Option<String>) = match &*message.read()
    {
        Some(Ok(m)) => (m.clone(), None),
        Some(Err(e)) => (None, Some(e.clone())),
        None => (
            match (slug(), current.clone(), open_message()) {
                (Some(s), Some(acct), Some(id)) => offline::get_message(&s, &acct, &id),
                _ => None,
            },
            None,
        ),
    };
    // Only a *cold* open shows the spinner — with a cached body already
    // painted, "Opening…" would replace readable content with a
    // placeholder.
    let msg_loading = open_message().is_some() && message.read().is_none() && open_msg.is_none();
    // Destination folders for the two filing actions. Role first, then
    // a name match, so plain-Maildir backends that report no roles
    // still get working buttons (and none at all if the folder is
    // genuinely absent — the button hides rather than erroring).
    let archive_folder = folder_for_role(&folder_list, email_proto::FolderRole::Archive, "Archive");
    let trash_folder = folder_for_role(&folder_list, email_proto::FolderRole::Trash, "Trash");
    // message_id → (urgency, tags) from the derivation cache.
    let deriv_map: std::collections::HashMap<String, (Option<u8>, Vec<String>)> = {
        let mut map = std::collections::HashMap::new();
        if let Some(Ok(rows)) = &*derivs.read() {
            for d in rows {
                let entry = map
                    .entry(d.message_id.clone())
                    .or_insert((None, Vec::new()));
                match d.kind {
                    email_proto::DerivationKind::Urgency => entry.0 = d.urgency(),
                    email_proto::DerivationKind::Tags => {
                        entry.1 = d.tags().into_iter().map(str::to_string).collect();
                    }
                }
            }
        }
        map
    };
    let loading = envelopes.read().is_none() && current.is_some();

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-5 p-4 sm:p-6 lg:p-8",
            div { class: "flex items-baseline justify-between gap-3",
                Heading { level: HeadingLevel::H1, "Email" }
                if current.is_some() {
                    Button {
                        size: ButtonSize::Small,
                        on_click: move |_| {
                            composing
                                .set(
                                    Some(ComposeSeed {
                                        to: String::new(),
                                        subject: String::new(),
                                        in_reply_to: None,
                                    }),
                                )
                        },
                        "New message"
                    }
                }
            }
            Text {
                variant: TextVariant::Muted,
                class: "text-sm -mt-2",
                "Synced mail for the selected org. Compose stages into the outbox; approval sends.",
            }

            if let Some(err) = accounts_err {
                div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive",
                    "Couldn't load accounts: {err}"
                }
            }

            // ── Account picker ─────────────────────────────────────
            if account_list.is_empty() {
                div { class: "rounded-lg border border-dashed border-border px-4 py-10 text-center",
                    Text {
                        variant: TextVariant::Muted,
                        "No mail accounts configured for this org yet.",
                    }
                }
            } else {
                div { class: "flex flex-wrap gap-2",
                    for acct in account_list.iter().cloned() {
                        AccountChip {
                            key: "{acct.id.0}",
                            id: acct.id.0.clone(),
                            label: if acct.name.is_empty() { acct.address.clone() } else { acct.name.clone() },
                            selected: current.as_deref() == Some(acct.id.0.as_str()),
                            on_select: move |id: String| selected_account.set(Some(id)),
                        }
                    }
                }
            }

            // ── Compose ────────────────────────────────────────────
            if let (Some(seed), Some(slug_now), Some(acct), Some(from)) = (
                composing(),
                slug(),
                current.clone(),
                current_address.clone(),
            ) {
                ComposeForm {
                    // Keyed on the seed so switching between "new"
                    // and a specific reply remounts the form (its
                    // field signals initialize from the seed).
                    key: "{seed.in_reply_to:?}|{seed.to}",
                    slug: slug_now,
                    account: acct,
                    from,
                    seed_to: seed.to.clone(),
                    seed_subject: seed.subject.clone(),
                    in_reply_to: seed.in_reply_to.clone(),
                    on_done: move |_| composing.set(None),
                }
            }

            // ── Outbox ─────────────────────────────────────────────
            if !outbox_rows.is_empty() {
                OutboxPanel {
                    slug: slug().unwrap_or_default(),
                    account: current.clone().unwrap_or_default(),
                    entries: outbox_rows,
                }
            }

            // ── Mail: folders | messages | reader ──────────────────
            if !account_list.is_empty() {
                // Mailbox switcher. Deliberately buttons in the page,
                // not a second vertical rail — the app already has one
                // (the shell's icon rail), and stacking another beside
                // it eats width the reading pane wants.
                div { class: "flex flex-wrap items-center gap-1.5",
                    FolderRail {
                        folders: folder_list
                            .iter()
                            .map(|f| (f.id.clone(), folder_label(f), f.unread_count))
                            .collect::<Vec<_>>(),
                        selected: selected_folder(),
                        on_select: move |id: String| selected_folder.set(id),
                    }
                }

                div { class: "grid gap-4 xl:grid-cols-[24rem_minmax(0,1fr)]",
                    // Message list.
                    div { class: "flex min-w-0 flex-col gap-1.5",
                        if let Some(err) = rows_err {
                            div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive",
                                "Couldn't load messages: {err}"
                            }
                        } else if loading {
                            Text { variant: TextVariant::Muted, class: "text-sm", "Loading messages…" }
                        } else if rows.is_empty() {
                            div { class: "rounded-lg border border-dashed border-border px-4 py-10 text-center",
                                Text { variant: TextVariant::Muted, "Nothing in this folder." }
                            }
                        } else {
                            for env in rows {
                                EnvelopeRow {
                                    key: "{env.message_id}",
                                    from: sender_label(&env),
                                    subject: if env.subject.is_empty() { "(no subject)".to_owned() } else { env.subject.clone() },
                                    snippet: env.snippet.clone().filter(|s| !s.is_empty()),
                                    date: format_date(env.date_ms),
                                    unread: is_unread(&env),
                                    flagged: is_flagged(&env),
                                    selected: open_message().as_deref() == Some(env.message_id.as_str()),
                                    urgency: deriv_map.get(&env.message_id).and_then(|(u, _)| *u),
                                    tags: deriv_map.get(&env.message_id).map(|(_, t)| t.clone()).unwrap_or_default(),
                                    on_open: {
                                        // Opening marks it read, the way every
                                        // mail client does. Fire-and-forget: the
                                        // `FlagsChanged` event re-reads the list,
                                        // so a failure just leaves it bold.
                                        let id = env.message_id.clone();
                                        let was_unread = is_unread(&env);
                                        let slug_now = slug();
                                        let acct = current.clone();
                                        move |_| {
                                            open_message.set(Some(id.clone()));
                                            if was_unread {
                                                if let (Some(s), Some(a)) = (slug_now.clone(), acct.clone()) {
                                                    let id = id.clone();
                                                    spawn(async move {
                                                        let _ = set_email_flags(
                                                                &s,
                                                                &a,
                                                                &id,
                                                                vec![FLAG_SEEN.to_owned()],
                                                                Vec::new(),
                                                            )
                                                            .await;
                                                    });
                                                }
                                            }
                                        }
                                    },
                                    on_reply: {
                                        let sender = env.from.first().map(|a| a.email.clone()).unwrap_or_default();
                                        let subject = reply_subject(&env.subject);
                                        let message_id = env.message_id.clone();
                                        move |_| {
                                            composing
                                                .set(
                                                    Some(ComposeSeed {
                                                        to: sender.clone(),
                                                        subject: subject.clone(),
                                                        in_reply_to: Some(message_id.clone()),
                                                    }),
                                                )
                                        }
                                    },
                                }
                            }
                        }
                    }

                    // Reading pane. Spans the full width below the list
                    // until xl, where it becomes the third column.
                    div { class: "min-w-0",
                        if let Some(err) = msg_err {
                            div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive",
                                "Couldn't open message: {err}"
                            }
                        } else if msg_loading {
                            Text { variant: TextVariant::Muted, class: "text-sm", "Opening…" }
                        } else if let Some(msg) = open_msg {
                            MessageReader {
                                slug: slug().unwrap_or_default(),
                                account: current.clone().unwrap_or_default(),
                                id: msg.envelope.message_id.clone(),
                                subject: msg.envelope.subject.clone(),
                                sender: sender_label(&msg.envelope),
                                date: format_date(msg.envelope.date_ms),
                                to_line: msg
                                    .envelope
                                    .to
                                    .iter()
                                    .map(|a| a.email.clone())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                body: message_body(&msg),
                                flagged: is_flagged(&msg.envelope),
                                attachments: msg
                                    .attachments
                                    .iter()
                                    .map(|a| {
                                        (
                                            a.filename.clone().unwrap_or_else(|| a.part.clone()),
                                            a.mime.clone(),
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                                archive_folder: archive_folder.clone(),
                                trash_folder: trash_folder.clone(),
                                on_close: move |_| open_message.set(None),
                                on_reply: {
                                    let sender = msg
                                        .envelope
                                        .from
                                        .first()
                                        .map(|a| a.email.clone())
                                        .unwrap_or_default();
                                    let subject = reply_subject(&msg.envelope.subject);
                                    let message_id = msg.envelope.message_id.clone();
                                    move |_| {
                                        composing
                                            .set(
                                                Some(ComposeSeed {
                                                    to: sender.clone(),
                                                    subject: subject.clone(),
                                                    in_reply_to: Some(message_id.clone()),
                                                }),
                                            )
                                    }
                                },
                            }
                        } else {
                            div { class: "hidden rounded-lg border border-dashed border-border px-4 py-16 text-center xl:block",
                                Text { variant: TextVariant::Muted, "Select a message to read it." }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One selectable account chip. Takes primitive props (the proto
/// `Account` doesn't impl `PartialEq`, which Dioxus props require).
#[component]
fn AccountChip(
    id: String,
    label: String,
    selected: bool,
    on_select: EventHandler<String>,
) -> Element {
    let cls = if selected {
        "rounded-full border border-primary bg-primary/10 px-3 py-1 text-sm text-foreground"
    } else {
        "rounded-full border border-border bg-card/40 px-3 py-1 text-sm text-muted-foreground hover:text-foreground"
    };
    rsx! {
        button {
            class: "{cls}",
            onclick: move |_| on_select.call(id.clone()),
            "{label}"
        }
    }
}

/// One message summary row: sender, subject, date, triage chips,
/// reply action. Primitive props for the same reason as
/// [`AccountChip`].
#[component]
#[allow(clippy::too_many_arguments)]
fn EnvelopeRow(
    from: String,
    subject: String,
    snippet: Option<String>,
    date: String,
    unread: bool,
    flagged: bool,
    selected: bool,
    urgency: Option<u8>,
    tags: Vec<String>,
    on_open: EventHandler<()>,
    on_reply: EventHandler<()>,
) -> Element {
    let weight = if unread { "font-semibold" } else { "" };
    let row_cls = if selected {
        "border-primary/60 bg-accent"
    } else {
        "border-border bg-card/40 hover:bg-accent/40"
    };
    // Chips stay quiet for the boring cases: urgency 0 and the
    // `other` tag render nothing.
    let urgency_chip = urgency.filter(|u| *u > 0).map(|u| {
        let cls = match u {
            1 => "border-border text-muted-foreground",
            2 => "border-amber-500/50 text-amber-600 dark:text-amber-400",
            _ => "border-destructive/60 text-destructive",
        };
        (format!("!{u}"), cls)
    });
    let shown_tags: Vec<String> = tags.into_iter().filter(|t| t != "other").collect();

    rsx! {
        // The whole row opens the message; Reply is a nested button, so
        // it stops propagation rather than doing both.
        div {
            // Stable test hooks, same convention as the rest of the
            // app (dioxus-test's `by_testid` / Playwright's
            // `getByTestId` both resolve `[data-testid]`).
            "data-testid": "email-row",
            class: "group flex cursor-pointer flex-col gap-0.5 rounded-lg border px-3 py-2 {row_cls}",
            onclick: move |_| on_open.call(()),
            div { class: "flex min-w-0 items-baseline gap-2",
                if unread {
                    span { class: "h-1.5 w-1.5 shrink-0 rounded-full bg-primary" }
                }
                span { class: "min-w-0 flex-1 truncate text-sm {weight} text-foreground", "{from}" }
                if flagged {
                    span { class: "shrink-0 text-xs text-amber-500", "★" }
                }
                span { class: "shrink-0 text-[11px] text-muted-foreground", "{date}" }
            }
            div { class: "flex min-w-0 items-baseline gap-1.5",
                if let Some((label, cls)) = urgency_chip {
                    span { class: "shrink-0 rounded-full border px-1.5 text-[10px] font-semibold {cls}",
                        "{label}"
                    }
                }
                span { class: "truncate text-sm {weight} text-foreground", "{subject}" }
                for tag in shown_tags {
                    span {
                        key: "{tag}",
                        class: "shrink-0 rounded-full border border-border px-1.5 text-[10px] text-muted-foreground",
                        "{tag}"
                    }
                }
            }
            div { class: "flex min-w-0 items-baseline gap-2",
                if let Some(snippet) = snippet.as_ref().filter(|s| !s.is_empty()) {
                    span { class: "min-w-0 flex-1 truncate text-xs text-muted-foreground", "{snippet}" }
                }
                button {
                    class: "shrink-0 text-[11px] text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100",
                    onclick: move |e: Event<MouseData>| {
                        e.stop_propagation();
                        on_reply.call(());
                    },
                    "Reply"
                }
            }
        }
    }
}

/// Minimal compose: to / subject / body. "Send" stages the draft
/// AND approves it in one go (the poller delivers moments later);
/// "Stage" leaves it pending in the outbox for review — the shape
/// agent-drafted mail always takes.
#[component]
fn ComposeForm(
    slug: String,
    account: String,
    from: String,
    seed_to: String,
    seed_subject: String,
    in_reply_to: Option<String>,
    on_done: EventHandler<()>,
) -> Element {
    let to = use_signal(|| seed_to.clone());
    let subject = use_signal(|| seed_subject.clone());
    let body = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let title = if in_reply_to.is_some() {
        "Reply"
    } else {
        "New message"
    };

    let submit = move |approve_now: bool| {
        let slug = slug.clone();
        let account = account.clone();
        let from = from.clone();
        let in_reply_to = in_reply_to.clone();
        spawn(async move {
            let recipients = parse_addr_list(&to.peek());
            if recipients.is_empty() {
                error.set(Some("Add at least one recipient.".into()));
                return;
            }
            busy.set(true);
            error.set(None);
            let draft = Draft {
                from: Addr {
                    name: None,
                    email: from,
                },
                to: recipients,
                cc: vec![],
                bcc: vec![],
                subject: subject.peek().clone(),
                body_text: body.peek().clone(),
                body_html: None,
                in_reply_to: in_reply_to.clone(),
                references: in_reply_to.clone().into_iter().collect(),
                attachments: vec![],
            };
            let result = stage_email_draft(&slug, &account, draft, approve_now).await;
            busy.set(false);
            match result {
                Ok(()) => on_done.call(()),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    rsx! {
        div { class: "flex flex-col gap-2 rounded-lg border border-border bg-card/60 p-3",
            div { class: "flex items-center justify-between",
                Text { class: "text-sm font-medium", "{title}" }
                button {
                    class: "text-xs text-muted-foreground hover:text-foreground",
                    onclick: move |_| on_done.call(()),
                    "Close"
                }
            }
            Input { value: to, placeholder: "To (comma-separated)" }
            Input { value: subject, placeholder: "Subject" }
            Textarea { value: body, placeholder: "Write…", rows: 6 }
            if let Some(err) = error() {
                div { class: "rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1 text-xs text-destructive",
                    "{err}"
                }
            }
            div { class: "flex items-center justify-end gap-2",
                Button {
                    variant: ButtonVariant::Outline,
                    size: ButtonSize::Small,
                    disabled: busy(),
                    on_click: {
                        let submit = submit.clone();
                        move |_| submit(false)
                    },
                    "Stage for approval"
                }
                Button {
                    size: ButtonSize::Small,
                    disabled: busy(),
                    on_click: {
                        let submit = submit.clone();
                        move |_| submit(true)
                    },
                    "Send"
                }
            }
        }
    }
}

/// The outbox: staged sends with their status and the
/// approve / cancel gates. Rendered whenever the account has
/// entries (terminal ones included, so outcomes stay visible).
#[component]
fn OutboxPanel(slug: String, account: String, entries: Vec<OutboxEntry>) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1.5",
            SectionHeader { label: "Outbox".to_string() }
            for entry in entries {
                OutboxRow {
                    key: "{entry.id}",
                    slug: slug.clone(),
                    account: account.clone(),
                    id: entry.id,
                    status: entry.status,
                    subject: if entry.draft.subject.is_empty() { "(no subject)".to_owned() } else { entry.draft.subject.clone() },
                    to: entry.draft.to.iter().map(|a| a.email.clone()).collect::<Vec<_>>().join(", "),
                    origin: entry.origin.clone(),
                    error: entry.last_error.clone(),
                    retries: entry.retries,
                }
            }
        }
    }
}

/// One outbox entry. The approve / cancel buttons show only in
/// the states where the transition is legal; the stream's
/// `OutboxChanged` events keep the row fresh.
#[component]
#[allow(clippy::too_many_arguments)]
fn OutboxRow(
    slug: String,
    account: String,
    id: u64,
    status: OutboxStatus,
    subject: String,
    to: String,
    origin: String,
    error: Option<String>,
    retries: u32,
) -> Element {
    let mut busy = use_signal(|| false);
    let (badge, badge_variant) = status_badge(status);
    let approvable = matches!(status, OutboxStatus::PendingApproval | OutboxStatus::Failed);
    let cancellable = matches!(
        status,
        OutboxStatus::Draft
            | OutboxStatus::PendingApproval
            | OutboxStatus::Approved
            | OutboxStatus::Failed
    );
    let from_agent = origin != "user";

    let act = move |approve: bool| {
        let slug = slug.clone();
        let account = account.clone();
        spawn(async move {
            busy.set(true);
            // Errors surface via the refreshed list (the row's
            // status simply won't change); keep the panel dumb.
            let _ = outbox_action(&slug, &account, id, approve).await;
            busy.set(false);
        });
    };

    rsx! {
        div { class: "flex items-baseline gap-3 rounded-lg border border-border bg-card/40 px-3 py-2",
            Badge { variant: badge_variant, "{badge}" }
            div { class: "flex min-w-0 flex-1 flex-col",
                span { class: "truncate text-sm text-foreground", "{subject}" }
                span { class: "truncate text-xs text-muted-foreground",
                    "to {to}"
                    if from_agent {
                        " · staged by {origin}"
                    }
                    if retries > 0 {
                        " · {retries} attempts"
                    }
                }
                if let Some(err) = error.as_ref() {
                    span { class: "truncate text-xs text-destructive", "{err}" }
                }
            }
            if approvable {
                Button {
                    size: ButtonSize::Small,
                    disabled: busy(),
                    on_click: {
                        let act = act.clone();
                        move |_| act(true)
                    },
                    if status == OutboxStatus::Failed { "Retry" } else { "Approve" }
                }
            }
            if cancellable {
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    disabled: busy(),
                    on_click: {
                        let act = act.clone();
                        move |_| act(false)
                    },
                    "Cancel"
                }
            }
        }
    }
}

fn status_badge(status: OutboxStatus) -> (&'static str, BadgeVariant) {
    match status {
        OutboxStatus::Draft => ("draft", BadgeVariant::Outline),
        OutboxStatus::PendingApproval => ("pending", BadgeVariant::Secondary),
        OutboxStatus::Approved => ("approved", BadgeVariant::Default),
        OutboxStatus::Sending => ("sending", BadgeVariant::Default),
        OutboxStatus::Sent => ("sent", BadgeVariant::Outline),
        OutboxStatus::Failed => ("failed", BadgeVariant::Destructive),
        OutboxStatus::Cancelled => ("cancelled", BadgeVariant::Outline),
    }
}

/// Display name for an envelope's first sender: their name if present,
/// else their email, else a placeholder.
fn sender_label(env: &Envelope) -> String {
    env.from.first().map_or_else(
        || "(unknown sender)".to_owned(),
        |a| {
            a.name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| a.email.clone())
        },
    )
}

/// `Re:`-prefix a subject exactly once.
fn reply_subject(subject: &str) -> String {
    if subject.trim_start().to_ascii_lowercase().starts_with("re:") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

/// Comma/space-separated address list → `Addr`s (bare emails,
/// display names come later).
fn parse_addr_list(raw: &str) -> Vec<Addr> {
    raw.split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|email| Addr {
            name: None,
            email: email.to_string(),
        })
        .collect()
}

/// Format a unix-ms timestamp as a short local date. Falls back to an
/// empty string for the zero/sentinel value so undated envelopes don't
/// render "1970".
fn format_date(date_ms: i64) -> String {
    if date_ms <= 0 {
        return String::new();
    }
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(date_ms)
        .single()
        .map(|dt| dt.format("%b %-d").to_string())
        .unwrap_or_default()
}

/// The account's mailboxes as an inline row of buttons, with unread
/// counts — Inbox / Archive / Sent / …
///
/// Deliberately not a second vertical rail: the app shell already owns
/// the left rail, and a mail column beside it would spend width the
/// reading pane needs. Falls back to a single synthetic INBOX entry
/// when the backend reports no folders at all (plain Maildir trees
/// often don't), so the row never renders empty next to a list that
/// clearly has mail in it.
#[component]
fn FolderRail(
    /// `(id, label, unread)` per mailbox. Primitive props: the proto
    /// `Folder` doesn't impl `PartialEq`, which Dioxus props require
    /// (same reason `AccountChip` takes primitives).
    folders: Vec<(String, String, Option<u32>)>,
    selected: String,
    on_select: EventHandler<String>,
) -> Element {
    let mut shown = folders;
    if shown.is_empty() {
        shown.push(("INBOX".to_owned(), "Inbox".to_owned(), None));
    }

    rsx! {
        nav { class: "flex flex-wrap items-center gap-1.5",
            for (id, label, unread) in shown {
                {
                    let is_sel = id == selected;
                    let cls = if is_sel {
                        "border-primary/60 bg-accent text-foreground"
                    } else {
                        "border-border text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                    };
                    let pick = id.clone();
                    rsx! {
                        button {
                            key: "{id}",
                            "data-testid": "email-folder",
                            "data-folder": "{id}",
                            r#type: "button",
                            class: "flex shrink-0 items-center gap-1.5 rounded-full border px-3 py-1 text-sm {cls}",
                            onclick: move |_| on_select.call(pick.clone()),
                            span { class: "truncate", "{label}" }
                            if let Some(n) = unread.filter(|n| *n > 0) {
                                span { class: "shrink-0 rounded-full bg-primary/15 px-1.5 text-[10px] font-semibold tabular-nums text-primary",
                                    "{n}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The reading pane: headers, body, attachment list, and the filing
/// actions.
///
/// Body preference is text over HTML. The HTML alternative is rendered
/// as its stripped text rather than injected as markup — a mail body is
/// hostile input, and `dangerous_inner_html` on it would be an XSS sink
/// aimed straight at the app's own origin. Rich HTML rendering needs a
/// sandboxed iframe + a sanitizer pass; until then, readable-and-safe
/// beats pretty-and-exploitable.
#[component]
#[allow(clippy::too_many_arguments)]
fn MessageReader(
    slug: String,
    account: String,
    /// Message-id of the open message.
    id: String,
    subject: String,
    sender: String,
    date: String,
    to_line: String,
    body: String,
    flagged: bool,
    /// `(filename, mime)` per part — metadata only.
    attachments: Vec<(String, String)>,
    archive_folder: Option<String>,
    trash_folder: Option<String>,
    on_close: EventHandler<()>,
    on_reply: EventHandler<()>,
) -> Element {
    let mut busy = use_signal(|| false);
    let mut action_err = use_signal(|| None::<String>);

    // One shape for every action: run it, surface the error inline,
    // and let the `EmailChange` stream refresh the lists.
    let run =
        move |fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>,
              close_after: bool| {
            spawn(async move {
                busy.set(true);
                action_err.set(None);
                match fut.await {
                    Ok(()) => {
                        if close_after {
                            on_close.call(());
                        }
                    }
                    Err(e) => action_err.set(Some(e)),
                }
                busy.set(false);
            });
        };

    rsx! {
        article {
            "data-testid": "email-reader",
            class: "flex min-w-0 flex-col gap-3 rounded-lg border border-border bg-card/40 p-4",
            div { class: "flex items-start justify-between gap-3",
                div { class: "flex min-w-0 flex-col gap-0.5",
                    Heading { level: HeadingLevel::H2, class: "text-base",
                        if subject.is_empty() { "(no subject)" } else { "{subject}" }
                    }
                    Text { variant: TextVariant::Muted, class: "text-xs", "{sender} · {date}" }
                    if !to_line.is_empty() {
                        Text { variant: TextVariant::Muted, class: "text-xs", "to {to_line}" }
                    }
                }
                button {
                    r#type: "button",
                    class: "shrink-0 text-xs text-muted-foreground hover:text-foreground",
                    onclick: move |_| on_close.call(()),
                    "Close"
                }
            }

            // ── Actions ────────────────────────────────────────
            div { class: "flex flex-wrap items-center gap-2",
                Button {
                    size: ButtonSize::Small,
                    on_click: move |_| on_reply.call(()),
                    "Reply"
                }
                {
                    let (s, a, m) = (slug.clone(), account.clone(), id.clone());
                    rsx! {
                        Button {
                            size: ButtonSize::Small,
                            variant: ButtonVariant::Outline,
                            disabled: busy(),
                            on_click: move |_| {
                                let (s, a, m) = (s.clone(), a.clone(), m.clone());
                                let (add, remove) = if flagged {
                                    (Vec::new(), vec![FLAG_FLAGGED.to_owned()])
                                } else {
                                    (vec![FLAG_FLAGGED.to_owned()], Vec::new())
                                };
                                run(
                                    Box::pin(async move {
                                        set_email_flags(&s, &a, &m, add, remove).await
                                    }),
                                    false,
                                );
                            },
                            if flagged { "Unstar" } else { "Star" }
                        }
                    }
                }
                {
                    let (s, a, m) = (slug.clone(), account.clone(), id.clone());
                    rsx! {
                        Button {
                            size: ButtonSize::Small,
                            variant: ButtonVariant::Outline,
                            disabled: busy(),
                            on_click: move |_| {
                                let (s, a, m) = (s.clone(), a.clone(), m.clone());
                                run(
                                    Box::pin(async move {
                                        set_email_flags(
                                                &s,
                                                &a,
                                                &m,
                                                Vec::new(),
                                                vec![FLAG_SEEN.to_owned()],
                                            )
                                            .await
                                    }),
                                    true,
                                );
                            },
                            "Mark unread"
                        }
                    }
                }
                if let Some(dest) = archive_folder.clone() {
                    {
                        let (s, a, m) = (slug.clone(), account.clone(), id.clone());
                        rsx! {
                            Button {
                                size: ButtonSize::Small,
                                variant: ButtonVariant::Outline,
                                disabled: busy(),
                                on_click: move |_| {
                                    let (s, a, m, d) = (s.clone(), a.clone(), m.clone(), dest.clone());
                                    run(
                                        Box::pin(async move {
                                            move_email_message(&s, &a, &m, &d).await
                                        }),
                                        true,
                                    );
                                },
                                "Archive"
                            }
                        }
                    }
                }
                {
                    // Prefer moving to Trash (recoverable) over a hard
                    // delete; fall back to delete when the account has
                    // no trash folder.
                    let (s, a, m) = (slug.clone(), account.clone(), id.clone());
                    let trash = trash_folder.clone();
                    rsx! {
                        Button {
                            size: ButtonSize::Small,
                            variant: ButtonVariant::Outline,
                            disabled: busy(),
                            on_click: move |_| {
                                let (s, a, m, t) = (s.clone(), a.clone(), m.clone(), trash.clone());
                                run(
                                    Box::pin(async move {
                                        match t {
                                            Some(dest) => move_email_message(&s, &a, &m, &dest).await,
                                            None => delete_email_message(&s, &a, &m).await,
                                        }
                                    }),
                                    true,
                                );
                            },
                            if trash_folder.is_some() { "Trash" } else { "Delete" }
                        }
                    }
                }
            }

            if let Some(err) = action_err() {
                div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive",
                    "{err}"
                }
            }

            // ── Body ───────────────────────────────────────────
            pre {
                "data-testid": "email-body",
                class: "max-h-[32rem] overflow-auto whitespace-pre-wrap break-words text-sm text-foreground",
                "{body}"
            }

            if !attachments.is_empty() {
                div { class: "flex flex-col gap-1 border-t border-border pt-2",
                    Text { variant: TextVariant::Muted, class: "text-xs font-semibold uppercase tracking-wide",
                        "Attachments"
                    }
                    for (filename , mime) in attachments.iter() {
                        div { key: "{filename}", class: "flex items-baseline gap-2 text-xs",
                            span { class: "truncate text-foreground", "{filename}" }
                            span { class: "shrink-0 text-muted-foreground", "{mime}" }
                        }
                    }
                }
            }
        }
    }
}

/// Body text for the reader: the plain-text alternative when present,
/// otherwise the HTML one reduced to text. Never returns empty — an
/// empty pane reads as a bug.
fn message_body(msg: &email_proto::Message) -> String {
    msg.body_text
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| msg.body_html.as_deref().map(strip_html))
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "(no body)".to_owned())
}

/// A folder's user-visible label: the last hierarchy segment, so
/// `INBOX.Lists.rust` reads as "rust" in the rail.
fn folder_label(f: &email_proto::Folder) -> String {
    let name = if f.name.is_empty() { &f.id } else { &f.name };
    if f.delimiter.is_empty() {
        return name.clone();
    }
    // Trim a trailing delimiter first: `rsplit` on "a/b/" yields an
    // empty first segment, which would otherwise fall all the way back
    // to the full path.
    let trimmed = name.trim_end_matches(&f.delimiter);
    trimmed
        .rsplit(&f.delimiter)
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(name)
        .to_owned()
}

/// Very small HTML → text reduction for the body fallback: drop tags,
/// decode the handful of entities that actually show up, collapse
/// blank runs. Deliberately not a parser — it feeds a `pre`, and the
/// output is never treated as markup.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut skip_until: Option<&str> = None;
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = skip_until {
            if lower[i..].starts_with(end) {
                i += end.len();
                skip_until = None;
            } else {
                i += 1;
            }
            continue;
        }
        let c = bytes[i] as char;
        if c == '<' {
            if lower[i..].starts_with("<script") {
                skip_until = Some("</script>");
                continue;
            }
            if lower[i..].starts_with("<style") {
                skip_until = Some("</style>");
                continue;
            }
            // Block-ish tags become line breaks so paragraphs survive.
            if lower[i..].starts_with("<br")
                || lower[i..].starts_with("</p")
                || lower[i..].starts_with("</div")
                || lower[i..].starts_with("</tr")
            {
                out.push('\n');
            }
            in_tag = true;
            i += 1;
            continue;
        }
        if c == '>' {
            in_tag = false;
            i += 1;
            continue;
        }
        if !in_tag {
            out.push(c);
        }
        i += 1;
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    // Collapse 3+ newlines to 2.
    let mut collapsed = String::with_capacity(out.len());
    let mut blanks = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        collapsed.push_str(line.trim_end());
        collapsed.push('\n');
    }
    collapsed.trim().to_owned()
}

// ── data ────────────────────────────────────────────────────────────
//
// This slice's RPCs live with the page that calls them, not in the
// shell's `feeds` module — that is the point of the split. `feeds!` and
// the fan-out helpers come from `task-ui-core`; see its `feeds` module
// for the shape.

feeds! {
    email_proto::EmailSyncClient {
        /// Every mail account the org's `EmailSync` backend serves. An org
        /// with no configured mailbox returns an empty list (operational but
        /// unconfigured) — the `/email` page renders that as an empty state
        /// rather than an error.
        fetch_email_accounts() -> Vec<email_proto::Account>
            = accounts() as "list accounts";
    }
}

/// Recent envelopes (header summaries) for one account's `folder`,
/// newest first. `count` caps the slice. Returns an empty list for an
/// empty mailbox; surfaces backend errors verbatim so the page can show
/// them inline.
pub async fn fetch_email_envelopes(
    slug: &str,
    account: &str,
    folder: &str,
    count: u32,
) -> Result<Vec<email_proto::Envelope>, String> {
    // Offline read path: a live answer always wins and refreshes the
    // cache; only a *failed* call falls back to it, so an online client
    // can never be served stale mail.
    let fetched = async {
        let client =
            task_ui_core::vox_clients::establish_for::<email_proto::EmailSyncClient>(slug).await?;
        client
            .fetch_envelopes(
                account.to_owned(),
                folder.to_owned(),
                email_proto::SeqRange::Recent(count),
            )
            .await
            .map_err(|e| format!("{slug}: fetch envelopes: {e:?}"))
    }
    .await;

    let mut envelopes = match fetched {
        Ok(list) => {
            offline::put_envelopes(slug, account, folder, &list);
            list
        }
        Err(err) => match offline::get_envelopes(slug, account, folder) {
            Some(cached) => cached,
            None => return Err(err),
        },
    };
    // Newest first — the backend's `Recent` ordering isn't guaranteed
    // across implementations, so sort defensively on the date.
    envelopes.sort_by(|a, b| b.date_ms.cmp(&a.date_ms));
    Ok(envelopes)
}

/// Cached triage derivations (urgency / tags) for the given
/// message-ids. Messages the background pass hasn't reached yet
/// simply have no rows.
pub async fn fetch_email_derivations(
    slug: &str,
    account: &str,
    ids: Vec<String>,
) -> Result<Vec<email_proto::Derivation>, String> {
    let client =
        task_ui_core::vox_clients::establish_for::<email_proto::EmailProductClient>(slug).await?;
    client
        .derivations(account.to_owned(), ids)
        .await
        .map_err(|e| format!("{slug}: derivations: {e:?}"))
}

/// The account's outbox, newest first (terminal entries included).
pub async fn fetch_email_outbox(slug: &str, account: &str) -> Result<Vec<OutboxEntry>, String> {
    let client =
        task_ui_core::vox_clients::establish_for::<email_proto::EmailProductClient>(slug).await?;
    client
        .list_outbox(account.to_owned())
        .await
        .map_err(|e| format!("{slug}: list outbox: {e:?}"))
}

/// Stage a draft into the outbox; when `approve_now`, immediately
/// approve it too (the user pressing "Send" is the approval).
pub async fn stage_email_draft(
    slug: &str,
    account: &str,
    draft: Draft,
    approve_now: bool,
) -> Result<(), String> {
    let client =
        task_ui_core::vox_clients::establish_for::<email_proto::EmailProductClient>(slug).await?;
    let entry = client
        .submit_draft(account.to_owned(), draft, "user".to_owned())
        .await
        .map_err(|e| format!("{slug}: stage draft: {e:?}"))?;
    if approve_now {
        client
            .approve(account.to_owned(), entry.id)
            .await
            .map_err(|e| format!("{slug}: approve: {e:?}"))?;
    }
    Ok(())
}

/// Approve (`true`) or cancel (`false`) one outbox entry.
pub async fn outbox_action(
    slug: &str,
    account: &str,
    id: u64,
    approve: bool,
) -> Result<(), String> {
    let client =
        task_ui_core::vox_clients::establish_for::<email_proto::EmailProductClient>(slug).await?;
    if approve {
        client
            .approve(account.to_owned(), id)
            .await
            .map_err(|e| format!("{slug}: approve: {e:?}"))?;
    } else {
        client
            .cancel(account.to_owned(), id)
            .await
            .map_err(|e| format!("{slug}: cancel: {e:?}"))?;
    }
    Ok(())
}

// ── reading + organizing ────────────────────────────────────────────
//
// `EmailSync` has carried these five since the slice was written; the
// page just never called them, so mail could be listed but not read,
// filed, flagged or deleted. Same shape as the helpers above: resolve
// the org's client, call, stringify the error for inline display.

/// Every mailbox on the account, for the folder rail. An account whose
/// backend exposes no folders returns an empty list — the page falls
/// back to `INBOX`, which every backend has.
pub async fn fetch_email_folders(
    slug: &str,
    account: &str,
) -> Result<Vec<email_proto::Folder>, String> {
    let client =
        task_ui_core::vox_clients::establish_for::<email_proto::EmailSyncClient>(slug).await?;
    client
        .list_folders(account.to_owned())
        .await
        .map_err(|e| format!("{slug}: list folders: {e:?}"))
}

/// One message in full — headers, both body alternatives, and
/// attachment *metadata* (bytes come separately via `fetch_attachment`,
/// which the reader does not need until someone clicks one).
pub async fn fetch_email_message(
    slug: &str,
    account: &str,
    message_id: &str,
) -> Result<email_proto::Message, String> {
    let fetched = async {
        let client =
            task_ui_core::vox_clients::establish_for::<email_proto::EmailSyncClient>(slug).await?;
        client
            .fetch_message(account.to_owned(), message_id.to_owned())
            .await
            .map_err(|e| format!("{slug}: fetch message: {e:?}"))
    }
    .await;

    match fetched {
        Ok(message) => {
            // Remember what was actually opened — that, not the whole
            // mailbox, is the realistic offline reading set.
            offline::put_message(slug, account, &message);
            Ok(message)
        }
        Err(err) => offline::get_message(slug, account, message_id).ok_or(err),
    }
}

/// Add/remove flags on one message. Both lists may be empty — the
/// backend treats that as a no-op rather than an error.
pub async fn set_email_flags(
    slug: &str,
    account: &str,
    message_id: &str,
    add: Vec<String>,
    remove: Vec<String>,
) -> Result<(), String> {
    let client =
        task_ui_core::vox_clients::establish_for::<email_proto::EmailSyncClient>(slug).await?;
    client
        .set_flags(
            account.to_owned(),
            message_id.to_owned(),
            email_proto::FlagDelta { add, remove },
        )
        .await
        .map_err(|e| format!("{slug}: set flags: {e:?}"))
}

/// File a message into another folder on the same account.
pub async fn move_email_message(
    slug: &str,
    account: &str,
    message_id: &str,
    dest_folder: &str,
) -> Result<(), String> {
    let client =
        task_ui_core::vox_clients::establish_for::<email_proto::EmailSyncClient>(slug).await?;
    client
        .move_message(
            account.to_owned(),
            message_id.to_owned(),
            dest_folder.to_owned(),
        )
        .await
        .map_err(|e| format!("{slug}: move message: {e:?}"))
}

/// Delete a message. Idempotent per the proto contract, so a
/// double-click on Delete is harmless.
pub async fn delete_email_message(
    slug: &str,
    account: &str,
    message_id: &str,
) -> Result<(), String> {
    let client =
        task_ui_core::vox_clients::establish_for::<email_proto::EmailSyncClient>(slug).await?;
    client
        .delete_message(account.to_owned(), message_id.to_owned())
        .await
        .map_err(|e| format!("{slug}: delete message: {e:?}"))
}

/// The IMAP "seen" keyword. Backends differ on whether they return the
/// backslash form, so reads must tolerate both — see [`is_unread`].
pub const FLAG_SEEN: &str = "\\Seen";
/// The IMAP "flagged"/starred keyword.
pub const FLAG_FLAGGED: &str = "\\Flagged";

/// Does this flag list contain `flag`, whichever spelling the backend
/// happens to use?
///
/// Three are in play at once: IMAP's `\Seen`, the bare word `Seen`,
/// and — because the maildir backend reports the raw filename letters —
/// a single `S`. Matching only the IMAP spelling silently mis-renders
/// every maildir mailbox (everything looks unread, nothing looks
/// starred), which is exactly what happened before this existed.
fn has_flag(flags: &[String], word: &str, letter: &str) -> bool {
    flags
        .iter()
        .any(|f| f.trim_start_matches('\\') == word || f == letter)
}

/// True when the envelope carries no seen-flag in any spelling.
pub fn is_unread(env: &Envelope) -> bool {
    !has_flag(&env.flags, "Seen", "S")
}

/// True when the envelope is starred, in any spelling.
pub fn is_flagged(env: &Envelope) -> bool {
    has_flag(&env.flags, "Flagged", "F")
}

/// Pick the folder id matching `role`, if the backend labelled one.
/// Falls back to a case-insensitive name match so backends that report
/// no roles (plain Maildir) still get working Archive / Trash actions.
pub fn folder_for_role(
    folders: &[email_proto::Folder],
    role: email_proto::FolderRole,
    fallback_name: &str,
) -> Option<String> {
    folders
        .iter()
        .find(|f| f.role.as_ref() == Some(&role))
        .or_else(|| {
            folders
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(fallback_name))
        })
        .map(|f| f.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str, delim: &str) -> email_proto::Folder {
        email_proto::Folder {
            id: id.to_owned(),
            name: name.to_owned(),
            delimiter: delim.to_owned(),
            role: None,
            message_count: None,
            unread_count: None,
        }
    }

    #[test]
    fn folder_label_shows_the_last_hierarchy_segment() {
        assert_eq!(folder_label(&folder("INBOX", "INBOX", ".")), "INBOX");
        assert_eq!(
            folder_label(&folder("INBOX.Lists.rust", "INBOX.Lists.rust", ".")),
            "rust"
        );
        // Slash-delimited backends, and a trailing delimiter.
        assert_eq!(folder_label(&folder("a/b", "a/b", "/")), "b");
        assert_eq!(folder_label(&folder("a/b/", "a/b/", "/")), "b");
        // No delimiter reported: the name is the label, untouched.
        assert_eq!(folder_label(&folder("a.b", "a.b", "")), "a.b");
        // Empty name falls back to the id, so the rail never renders a
        // blank, unclickable-looking row.
        assert_eq!(folder_label(&folder("Archive", "", ".")), "Archive");
    }

    #[test]
    fn folder_for_role_prefers_the_role_then_the_name() {
        let mut archive = folder("Archive", "Archive", ".");
        let inbox = folder("INBOX", "INBOX", ".");
        // Name match when the backend reports no roles (plain Maildir).
        let by_name = vec![inbox.clone(), archive.clone()];
        assert_eq!(
            folder_for_role(&by_name, email_proto::FolderRole::Archive, "Archive"),
            Some("Archive".to_owned())
        );
        // Role wins over a same-named folder elsewhere in the list.
        archive.id = "ARC".to_owned();
        archive.name = "Stashed".to_owned();
        archive.role = Some(email_proto::FolderRole::Archive);
        let by_role = vec![inbox.clone(), archive];
        assert_eq!(
            folder_for_role(&by_role, email_proto::FolderRole::Archive, "Archive"),
            Some("ARC".to_owned())
        );
        // Genuinely absent → None, so the button hides instead of
        // moving mail somewhere arbitrary.
        assert_eq!(
            folder_for_role(&[inbox], email_proto::FolderRole::Archive, "Archive"),
            None
        );
    }

    fn env_with(flags: &[&str]) -> Envelope {
        Envelope {
            message_id: "m".into(),
            thread_id: None,
            folder: "INBOX".into(),
            subject: String::new(),
            from: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            date_ms: 0,
            flags: flags.iter().map(|s| (*s).to_owned()).collect(),
            size: 0,
            has_attachments: false,
            snippet: None,
        }
    }

    #[test]
    fn flag_reads_accept_every_backend_spelling() {
        // IMAP (`\\Seen`), the bare word, and the maildir filename
        // letter all mean the same thing. The maildir backend reports
        // the letter, so missing that case makes every maildir message
        // render as unread and never starred.
        for seen in [r"\Seen", "Seen", "S"] {
            assert!(!is_unread(&env_with(&[seen])), "{seen} should read as seen");
        }
        for star in [r"\Flagged", "Flagged", "F"] {
            assert!(
                is_flagged(&env_with(&[star])),
                "{star} should read as starred"
            );
        }
        assert!(is_unread(&env_with(&[])));
        assert!(!is_flagged(&env_with(&["S"])));
        // A real maildir pair: seen + flagged.
        let both = env_with(&["F", "S"]);
        assert!(!is_unread(&both));
        assert!(is_flagged(&both));
    }

    #[test]
    fn strip_html_drops_markup_and_keeps_the_text() {
        assert_eq!(strip_html("<p>Hello <b>there</b></p>"), "Hello there");
        // Entities that actually show up in mail.
        assert_eq!(strip_html("a&nbsp;b &amp; c &lt;d&gt;"), "a b & c <d>");
    }

    #[test]
    fn strip_html_never_emits_script_or_style_bodies() {
        // The reader renders this into a `pre`, never as markup — but
        // script/style CONTENT is not display text either, and leaking
        // it would dump JS source into the reading pane.
        let got =
            strip_html("<style>.x{color:red}</style><p>Body</p><script>alert('xss')</script>");
        assert_eq!(got, "Body");
        assert!(!got.contains("alert"), "{got}");
        assert!(!got.contains("color:red"), "{got}");
    }

    #[test]
    fn strip_html_turns_block_ends_into_line_breaks() {
        let got = strip_html("<p>one</p><p>two</p>");
        assert_eq!(got, "one\ntwo");
        assert_eq!(strip_html("a<br>b"), "a\nb");
    }

    #[test]
    fn message_body_prefers_text_then_html_then_a_placeholder() {
        let env = email_proto::Envelope {
            message_id: "m1".into(),
            thread_id: None,
            folder: "INBOX".into(),
            subject: "s".into(),
            from: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            date_ms: 0,
            flags: Vec::new(),
            size: 0,
            has_attachments: false,
            snippet: None,
        };
        let mk = |text: Option<&str>, html: Option<&str>| email_proto::Message {
            envelope: env.clone(),
            headers_raw: String::new(),
            body_text: text.map(str::to_owned),
            body_html: html.map(str::to_owned),
            attachments: Vec::new(),
            references: Vec::new(),
        };
        assert_eq!(
            message_body(&mk(Some("plain"), Some("<p>rich</p>"))),
            "plain"
        );
        assert_eq!(message_body(&mk(None, Some("<p>rich</p>"))), "rich");
        // A whitespace-only text part must not win over real HTML.
        assert_eq!(message_body(&mk(Some("   "), Some("<p>rich</p>"))), "rich");
        assert_eq!(message_body(&mk(None, None)), "(no body)");
    }
}

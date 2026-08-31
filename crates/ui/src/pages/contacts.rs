//! `/contacts` — the people ledger: a vault-backed directory that
//! unifies who you work with and who you bill.
//!
//! A searchable, group-filterable list of **person-chips** (a
//! deterministic-initials avatar + name + a source badge) backed by the
//! shared optimistic contact store ([`crate::stores`]). Clicking a chip
//! opens a detail drawer with every email/phone, the postal address,
//! groups, notes, and edit/delete. A quick-add form authors a new
//! contact; the **Sync accounts** panel manages CardDAV addressbooks and
//! runs a one-way pull per account.
//!
//! The [`PersonChip`] here is the shared signature the ledger renders
//! identically wherever a person appears — `/invoices` (bill-to) and
//! `/members` (the roster) both reach for it.

use architect::Id;
use architect_ui::prelude::*;
use chrono::Utc;
use dioxus::prelude::*;

use contacts_proto::{CardDavAccount, CardDavProvider, Contact, ContactSource};

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

/// Shared input styling — theme tokens only, matches the rest of Task.
const INPUT_CLS: &str = "w-full rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50 placeholder:text-muted-foreground";

// ── Shared signature: the person-chip ────────────────────────────────

/// The provenance badge for a contact's `source` — the one visual tell
/// that separates hand-authored people from synced ones.
pub fn source_badge(source: &str) -> (StatusBadgeVariant, &'static str) {
    match source {
        ContactSource::NEXTCLOUD => (StatusBadgeVariant::Success, "Nextcloud"),
        ContactSource::ICLOUD => (StatusBadgeVariant::Warning, "iCloud"),
        ContactSource::CARDDAV => (StatusBadgeVariant::Neutral, "CardDAV"),
        _ => (StatusBadgeVariant::Neutral, "Manual"),
    }
}

/// A person, rendered the same way everywhere: initials avatar (color
/// derived from the name/email), the display name, an optional subtitle
/// (email · org), and an optional badge (source or role).
pub use task_ui_core::avatar::PersonChip;

/// The name the ledger shows for a contact — `full_name`, falling back
/// to the organization, then a placeholder.
pub fn display_name(c: &Contact) -> String {
    if !c.full_name.trim().is_empty() {
        c.full_name.clone()
    } else if let Some(org) = c.organization.as_deref().filter(|o| !o.trim().is_empty()) {
        org.to_string()
    } else {
        "Unnamed contact".to_string()
    }
}

/// The one-line subtitle under a chip: primary email, then org.
fn chip_subtitle(c: &Contact) -> String {
    match (c.primary_email(), c.organization.as_deref()) {
        (Some(e), Some(o)) if !o.trim().is_empty() => format!("{e} · {o}"),
        (Some(e), _) => e.to_string(),
        (None, Some(o)) if !o.trim().is_empty() => o.to_string(),
        _ => String::new(),
    }
}

// ── The directory ────────────────────────────────────────────────────

#[component]
pub fn ContactsView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we author into (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let result = stores::use_contact_list();
    let store = stores::use_contact_store();

    let mut search = use_signal(String::new);
    let mut group_filter: Signal<Option<String>> = use_signal(|| None);
    let mut selected: Signal<Option<String>> = use_signal(|| None);
    let mut adding = use_signal(|| false);

    let all_rows: Vec<(Id<String>, Contact)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();
    let first_load = result.is_waiting() && result.value().is_none();

    // Active (non-archived) contacts, name-sorted.
    let mut active: Vec<(Id<String>, Contact)> = all_rows
        .iter()
        .filter(|(_, c)| !c.archived)
        .cloned()
        .collect();
    active.sort_by(|a, b| {
        display_name(&a.1)
            .to_lowercase()
            .cmp(&display_name(&b.1).to_lowercase())
    });

    // Group chips — every category across active contacts.
    let mut groups: Vec<String> = active
        .iter()
        .flat_map(|(_, c)| c.group_list().into_iter().map(str::to_string))
        .collect();
    groups.sort();
    groups.dedup();

    // Filter: search over name/email/org + the active group chip.
    let q = search().trim().to_lowercase();
    let active_group = group_filter();
    let rows: Vec<(Id<String>, Contact)> = active
        .iter()
        .filter(|(_, c)| {
            let hay = format!(
                "{} {} {}",
                c.full_name,
                c.emails,
                c.organization.clone().unwrap_or_default()
            )
            .to_lowercase();
            let matches_q = q.is_empty() || hay.contains(&q);
            let matches_group = match active_group.as_deref() {
                None => true,
                Some(g) => c.group_list().contains(&g),
            };
            matches_q && matches_group
        })
        .cloned()
        .collect();

    let selected_contact = selected().and_then(|id| {
        all_rows
            .iter()
            .find(|(_, c)| c.id == id)
            .map(|(_, c)| c.clone())
    });

    let total_active = active.len();
    let shown = rows.len();

    rsx! {
        div { class: "mx-auto flex w-full max-w-3xl flex-col gap-5 p-4 pb-14 sm:p-6 md:pb-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "People ledger"
                }
                div { class: "flex items-end justify-between gap-3",
                    Heading { level: HeadingLevel::H1, class: "tracking-tight", "Contacts" }
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: move |_| adding.toggle(),
                        if adding() { "Close" } else { "Add contact" }
                    }
                }
                Text { variant: TextVariant::Muted, class: "text-sm",
                    "Everyone you work with and bill — one directory, synced from your addressbooks."
                }
            }

            // ── Add contact ────────────────────────────────────────
            if adding() {
                AddContactForm {
                    slug,
                    on_added: move |_| adding.set(false),
                }
            }

            // ── Search ─────────────────────────────────────────────
            input {
                class: "{INPUT_CLS}",
                r#type: "search",
                placeholder: "Search by name, email, or organization…",
                value: "{search}",
                oninput: move |e| search.set(e.value()),
            }

            // ── Group filter chips ─────────────────────────────────
            if !groups.is_empty() {
                div { class: "flex flex-wrap items-center gap-1.5",
                    GroupChip {
                        label: "All".to_string(),
                        active: active_group.is_none(),
                        on_pick: move |_| group_filter.set(None),
                    }
                    for g in groups.clone() {
                        GroupChip {
                            key: "{g}",
                            label: g.clone(),
                            active: active_group.as_deref() == Some(g.as_str()),
                            on_pick: {
                                let g = g.clone();
                                move |_| group_filter.set(Some(g.clone()))
                            },
                        }
                    }
                }
            }

            // ── The directory ──────────────────────────────────────
            if first_load {
                crate::states::LoadingState {}
            } else if rows.is_empty() {
                if let Some(err) = load_err {
                    crate::states::ErrorState {
                        title: "Couldn't load contacts",
                        message: err,
                        on_retry: move |()| store.reload(),
                    }
                } else if total_active == 0 {
                    EmptyState {
                        message: "No contacts yet. Add one above, or sync an addressbook below.".to_string(),
                    }
                } else {
                    EmptyState { message: "No contacts match your filters.".to_string() }
                }
            } else {
                Card { class: "overflow-hidden".to_string(),
                    TableContainer {
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { class: "text-[0.7rem] uppercase tracking-wider text-muted-foreground".to_string(),
                                        "Person"
                                    }
                                    TableHead { class: "hidden text-[0.7rem] uppercase tracking-wider text-muted-foreground sm:table-cell".to_string(),
                                        "Phone"
                                    }
                                    TableHead { class: "w-10 text-right".to_string(), "" }
                                }
                            }
                            TableBody {
                                for (id , c) in rows {
                                    ContactRow {
                                        key: "{id}",
                                        pending: id.is_temp(),
                                        contact: c.clone(),
                                        on_open: move |id| selected.set(Some(id)),
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "px-1 text-xs text-muted-foreground",
                    "Showing {shown} of {total_active}"
                }
            }

            // ── Sync accounts ──────────────────────────────────────
            SyncAccounts { slug, on_synced: move |_| store.reload() }
        }

        // ── Detail drawer ──────────────────────────────────────────
        if let Some(c) = selected_contact {
            ContactDrawer {
                key: "{c.id}",
                contact: c,
                slug,
                on_close: move |_| selected.set(None),
            }
        }
    }
}

/// A group filter chip.
#[component]
fn GroupChip(label: String, active: bool, on_pick: EventHandler<()>) -> Element {
    let cls = if active {
        "rounded-full border border-primary bg-primary/15 px-2.5 py-1 text-xs text-foreground"
    } else {
        "rounded-full border border-border bg-card/40 px-2.5 py-1 text-xs text-muted-foreground hover:text-foreground"
    };
    rsx! {
        button { r#type: "button", class: "{cls}", onclick: move |_| on_pick.call(()), "{label}" }
    }
}

/// One directory row — a clickable person-chip + primary phone.
#[component]
fn ContactRow(contact: Contact, pending: bool, on_open: EventHandler<String>) -> Element {
    let (variant, badge) = source_badge(&contact.source);
    let name = display_name(&contact);
    let email = contact.primary_email().unwrap_or_default().to_string();
    let subtitle = chip_subtitle(&contact);
    let phone = contact.primary_phone().unwrap_or_default().to_string();
    let id = contact.id.clone();
    let row_cls = if pending { "opacity-60" } else { "" };

    rsx! {
        TableRow { class: row_cls.to_string(),
            TableCell {
                button {
                    r#type: "button",
                    class: "flex w-full items-center text-left",
                    onclick: move |_| on_open.call(id.clone()),
                    PersonChip {
                        name,
                        email,
                        subtitle: Some(subtitle),
                        badge_label: Some(badge.to_string()),
                        badge_variant: variant,
                    }
                }
            }
            TableCell { class: "hidden text-sm text-muted-foreground sm:table-cell".to_string(),
                if phone.is_empty() {
                    span { class: "text-muted-foreground/50", "—" }
                } else {
                    "{phone}"
                }
            }
            TableCell { class: "text-right text-muted-foreground".to_string(), "›" }
        }
    }
}

// ── Add contact ──────────────────────────────────────────────────────

#[component]
fn AddContactForm(slug: Memo<Option<String>>, on_added: EventHandler<()>) -> Element {
    let muts = stores::use_contact_mutations();
    let mut full_name = use_signal(String::new);
    let mut email = use_signal(String::new);
    let mut phone = use_signal(String::new);
    let mut org = use_signal(String::new);

    let mut submit = move || {
        let name = full_name.peek().trim().to_string();
        if name.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let id = uuid::Uuid::new_v4().to_string();
        let mut contact = Contact::create(id, name, Utc::now().to_rfc3339());
        let e = email.peek().trim().to_string();
        if !e.is_empty() {
            contact.emails = e;
        }
        let p = phone.peek().trim().to_string();
        if !p.is_empty() {
            contact.phones = p;
        }
        let o = org.peek().trim().to_string();
        if !o.is_empty() {
            contact.organization = Some(o);
        }
        full_name.set(String::new());
        email.set(String::new());
        phone.set(String::new());
        org.set(String::new());
        muts.create(s, contact);
        on_added.call(());
    };

    rsx! {
        Card { class: "p-4".to_string(),
            div { class: "flex flex-col gap-3",
                span { class: "text-sm font-medium text-foreground", "New contact" }
                Field {
                    FieldLabel { required: true, "Full name" }
                    input {
                        class: "{INPUT_CLS}",
                        placeholder: "Ada Lovelace",
                        value: "{full_name}",
                        oninput: move |e| full_name.set(e.value()),
                        onkeydown: move |e| if e.key() == Key::Enter { submit(); },
                    }
                }
                div { class: "grid gap-3 sm:grid-cols-2",
                    Field {
                        FieldLabel { "Email" }
                        input {
                            class: "{INPUT_CLS}",
                            r#type: "email",
                            placeholder: "ada@example.com",
                            value: "{email}",
                            oninput: move |e| email.set(e.value()),
                        }
                    }
                    Field {
                        FieldLabel { "Phone" }
                        input {
                            class: "{INPUT_CLS}",
                            placeholder: "+1 555 0100",
                            value: "{phone}",
                            oninput: move |e| phone.set(e.value()),
                        }
                    }
                }
                Field {
                    FieldLabel { "Organization" }
                    input {
                        class: "{INPUT_CLS}",
                        placeholder: "Analytical Engines Ltd.",
                        value: "{org}",
                        oninput: move |e| org.set(e.value()),
                    }
                }
                div { class: "flex justify-end",
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: move |_| submit(),
                        "Save contact"
                    }
                }
            }
        }
    }
}

// ── Detail drawer ────────────────────────────────────────────────────

/// The full record for one contact, plus inline edit + delete. Keyed by
/// id so switching contacts remounts with fresh edit state.
#[component]
fn ContactDrawer(
    contact: Contact,
    slug: Memo<Option<String>>,
    on_close: EventHandler<()>,
) -> Element {
    let muts = stores::use_contact_mutations();
    let mut editing = use_signal(|| false);

    // Edit buffers, seeded from the record.
    let mut full_name = use_signal(|| contact.full_name.clone());
    let mut org = use_signal(|| contact.organization.clone().unwrap_or_default());
    let mut title = use_signal(|| contact.title.clone().unwrap_or_default());
    let mut emails = use_signal(|| contact.emails.clone());
    let mut phones = use_signal(|| contact.phones.clone());
    let mut address = use_signal(|| contact.address.clone().unwrap_or_default());
    let mut groups = use_signal(|| contact.groups.clone());
    let mut notes = use_signal(|| contact.notes.clone().unwrap_or_default());

    let record = use_signal(|| contact.clone());
    let (variant, badge) = source_badge(&record.read().source);
    let is_synced = record.read().is_synced();

    let save = move |_| {
        let Some(s) = slug() else { return };
        let mut next = record.peek().clone();
        next.full_name = full_name.peek().trim().to_string();
        let opt = |v: String| {
            let t = v.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        };
        next.organization = opt(org.peek().clone());
        next.title = opt(title.peek().clone());
        next.emails = emails.peek().trim().to_string();
        next.phones = phones.peek().trim().to_string();
        next.address = opt(address.peek().clone());
        next.groups = groups.peek().trim().to_string();
        next.notes = opt(notes.peek().clone());
        next.updated = Some(Utc::now().to_rfc3339());
        muts.save(s, next);
        editing.set(false);
    };

    let delete = move |_| {
        let Some(s) = slug() else { return };
        muts.delete(s, record.peek().id.clone());
        on_close.call(());
    };

    let snap = record.read();
    let name = display_name(&snap);
    let avatar_email = snap.primary_email().unwrap_or_default().to_string();
    let email_list: Vec<String> = snap.email_list().into_iter().map(str::to_string).collect();
    let phone_list: Vec<String> = snap.phone_list().into_iter().map(str::to_string).collect();
    let group_list: Vec<String> = snap.group_list().into_iter().map(str::to_string).collect();
    let addr = snap.address.clone();
    let note_text = snap.notes.clone();
    let title_text = snap.title.clone();
    let org_text = snap.organization.clone();
    drop(snap);

    rsx! {
        Drawer {
            open: true,
            side: DrawerSide::Right,
            class: "w-full max-w-md overflow-y-auto".to_string(),
            on_close: move |_| on_close.call(()),

            DrawerHeader { class: "p-0".to_string(),
                div { class: "flex items-start justify-between gap-3",
                    PersonChip {
                        name: name.clone(),
                        email: avatar_email,
                        subtitle: org_text.clone(),
                        badge_label: Some(badge.to_string()),
                        badge_variant: variant,
                        size: 44,
                    }
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        on_click: move |_| on_close.call(()),
                        "Close"
                    }
                }
                if let Some(t) = title_text.clone() {
                    if !t.trim().is_empty() {
                        Text { variant: TextVariant::Muted, class: "mt-1 text-sm", "{t}" }
                    }
                }
            }

            div { class: "mt-4 flex flex-col gap-4",
                if editing() {
                    // ── Edit form ──────────────────────────────────
                    Field {
                        FieldLabel { required: true, "Full name" }
                        input { class: "{INPUT_CLS}", value: "{full_name}", oninput: move |e| full_name.set(e.value()) }
                    }
                    div { class: "grid gap-3 sm:grid-cols-2",
                        Field {
                            FieldLabel { "Organization" }
                            input { class: "{INPUT_CLS}", value: "{org}", oninput: move |e| org.set(e.value()) }
                        }
                        Field {
                            FieldLabel { "Title" }
                            input { class: "{INPUT_CLS}", value: "{title}", oninput: move |e| title.set(e.value()) }
                        }
                    }
                    Field {
                        FieldLabel { "Emails" }
                        FieldDescription { "One per line — the first is primary." }
                        textarea { class: "{INPUT_CLS} min-h-16", value: "{emails}", oninput: move |e| emails.set(e.value()) }
                    }
                    Field {
                        FieldLabel { "Phones" }
                        textarea { class: "{INPUT_CLS} min-h-16", value: "{phones}", oninput: move |e| phones.set(e.value()) }
                    }
                    Field {
                        FieldLabel { "Address" }
                        textarea { class: "{INPUT_CLS} min-h-16", value: "{address}", oninput: move |e| address.set(e.value()) }
                    }
                    Field {
                        FieldLabel { "Groups" }
                        FieldDescription { "One per line." }
                        textarea { class: "{INPUT_CLS} min-h-16", value: "{groups}", oninput: move |e| groups.set(e.value()) }
                    }
                    Field {
                        FieldLabel { "Notes" }
                        textarea { class: "{INPUT_CLS} min-h-20", value: "{notes}", oninput: move |e| notes.set(e.value()) }
                    }
                    div { class: "flex justify-end gap-2",
                        Button { variant: ButtonVariant::Ghost, size: ButtonSize::Small, on_click: move |_| editing.set(false), "Cancel" }
                        Button { variant: ButtonVariant::Primary, size: ButtonSize::Small, on_click: save, "Save" }
                    }
                } else {
                    // ── Read view ──────────────────────────────────
                    if !email_list.is_empty() {
                        DetailBlock { label: "Email".to_string(),
                            for e in email_list.clone() {
                                a { key: "{e}", class: "block truncate text-sm text-primary hover:underline", href: "mailto:{e}", "{e}" }
                            }
                        }
                    }
                    if !phone_list.is_empty() {
                        DetailBlock { label: "Phone".to_string(),
                            for p in phone_list.clone() {
                                span { key: "{p}", class: "block text-sm text-foreground", "{p}" }
                            }
                        }
                    }
                    if let Some(a) = addr.clone() {
                        if !a.trim().is_empty() {
                            DetailBlock { label: "Address".to_string(),
                                span { class: "block whitespace-pre-wrap text-sm text-foreground", "{a}" }
                            }
                        }
                    }
                    if !group_list.is_empty() {
                        DetailBlock { label: "Groups".to_string(),
                            div { class: "flex flex-wrap gap-1.5",
                                for g in group_list.clone() {
                                    span { key: "{g}", class: "rounded-full border border-border bg-card/40 px-2 py-0.5 text-xs text-muted-foreground", "{g}" }
                                }
                            }
                        }
                    }
                    if let Some(n) = note_text.clone() {
                        if !n.trim().is_empty() {
                            DetailBlock { label: "Notes".to_string(),
                                span { class: "block whitespace-pre-wrap text-sm text-foreground", "{n}" }
                            }
                        }
                    }

                    if is_synced {
                        Text { variant: TextVariant::Muted, class: "text-xs",
                            "Synced from CardDAV — edits here may be overwritten on the next pull."
                        }
                    }

                    div { class: "flex justify-between gap-2 border-t border-border pt-4",
                        Button { variant: ButtonVariant::Destructive, size: ButtonSize::Small, on_click: delete, "Delete" }
                        Button { variant: ButtonVariant::Secondary, size: ButtonSize::Small, on_click: move |_| editing.set(true), "Edit" }
                    }
                }
            }
        }
    }
}

/// A labelled detail block in the drawer read view.
#[component]
fn DetailBlock(label: String, children: Element) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1",
            span { class: "text-[0.7rem] font-semibold uppercase tracking-wider text-muted-foreground", "{label}" }
            {children}
        }
    }
}

// ── Sync accounts ────────────────────────────────────────────────────

#[component]
fn SyncAccounts(slug: Memo<Option<String>>, on_synced: EventHandler<()>) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut adding = use_signal(|| false);
    // A transient banner for the latest sync report / error (stands in
    // for a toast — the app mounts no ToastProvider).
    let mut banner: Signal<Option<(bool, String)>> = use_signal(|| None);

    let accounts = use_resource(move || {
        let _ = reload();
        async move {
            let Some(s) = slug() else {
                return Ok::<Vec<CardDavAccount>, String>(Vec::new());
            };
            crate::feeds::fetch_carddav_accounts(&s).await
        }
    });

    let list = accounts.read().clone().unwrap_or(Ok(Vec::new()));

    rsx! {
        div { class: "mt-2 flex flex-col gap-3",
            div { class: "flex items-center justify-between gap-3",
                div { class: "flex flex-col",
                    Heading { level: HeadingLevel::H3, "Sync accounts" }
                    Text { variant: TextVariant::Muted, class: "text-sm",
                        "Pull contacts from a Nextcloud, iCloud, or generic CardDAV addressbook."
                    }
                }
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Small,
                    on_click: move |_| adding.toggle(),
                    if adding() { "Close" } else { "Add account" }
                }
            }

            if let Some((ok, msg)) = banner() {
                div {
                    class: if ok {
                        "rounded-lg border border-green-500/30 bg-green-500/10 px-3 py-2 text-sm text-green-500"
                    } else {
                        "rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-sm text-red-500"
                    },
                    "{msg}"
                }
            }

            if adding() {
                AddAccountForm {
                    slug,
                    on_saved: move |_| {
                        adding.set(false);
                        reload += 1;
                    },
                }
            }

            {match list {
                Ok(accounts) => {
                    if accounts.is_empty() {
                        rsx! {
                            EmptyState { message: "No sync accounts. Add one to import contacts.".to_string() }
                        }
                    } else {
                        rsx! {
                            Card { class: "overflow-hidden".to_string(),
                                for account in accounts {
                                    AccountRow {
                                        key: "{account.id}",
                                        account: account.clone(),
                                        slug,
                                        on_change: move |_| reload += 1,
                                        on_report: move |(ok, msg): (bool, String)| {
                                            banner.set(Some((ok, msg)));
                                            reload += 1;
                                            on_synced.call(());
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => rsx! {
                    crate::states::ErrorState {
                        title: "Couldn't load sync accounts",
                        message: e,
                        on_retry: move |()| reload += 1,
                    }
                },
            }}
        }
    }
}

/// The provider badge for a sync account.
fn provider_badge(provider: &str) -> (StatusBadgeVariant, &'static str) {
    match provider {
        CardDavProvider::NEXTCLOUD => (StatusBadgeVariant::Success, "Nextcloud"),
        CardDavProvider::ICLOUD => (StatusBadgeVariant::Warning, "iCloud"),
        _ => (StatusBadgeVariant::Neutral, "CardDAV"),
    }
}

#[component]
fn AccountRow(
    account: CardDavAccount,
    slug: Memo<Option<String>>,
    on_change: EventHandler<()>,
    on_report: EventHandler<(bool, String)>,
) -> Element {
    let mut syncing = use_signal(|| false);
    let (variant, provider) = provider_badge(&account.provider);
    let id = account.id.clone();
    let label = account.label.clone();
    let username = account.username.clone();
    let last_sync = account.last_sync.clone();

    let del_id = id.clone();
    let delete = move |_| {
        let Some(s) = slug() else { return };
        let del_id = del_id.clone();
        spawn(async move {
            let _ = crate::feeds::delete_carddav_account(&s, &del_id).await;
            on_change.call(());
        });
    };

    let sync_id = id.clone();
    let sync = move |_| {
        let Some(s) = slug() else { return };
        let sync_id = sync_id.clone();
        syncing.set(true);
        spawn(async move {
            let report = crate::feeds::sync_carddav_account(&s, &sync_id).await;
            syncing.set(false);
            match report {
                Ok(r) => on_report.call((true, r.message)),
                Err(e) => on_report.call((false, e)),
            }
        });
    };

    rsx! {
        div { class: "flex items-center justify-between gap-3 border-b border-border px-3 py-3 last:border-b-0",
            div { class: "flex min-w-0 flex-col gap-1",
                div { class: "flex items-center gap-2",
                    span { class: "truncate text-sm font-medium text-foreground", "{label}" }
                    StatusBadge { variant, label: provider.to_string(), class: "px-1.5 py-0 text-[10px]".to_string() }
                }
                span { class: "truncate text-xs text-muted-foreground", "{username}" }
                if let Some(ls) = last_sync.clone() {
                    span { class: "text-[11px] text-muted-foreground", "Last synced {ls}" }
                } else {
                    span { class: "text-[11px] text-muted-foreground", "Never synced" }
                }
            }
            div { class: "flex shrink-0 items-center gap-1",
                Button {
                    variant: ButtonVariant::Primary,
                    size: ButtonSize::Small,
                    disabled: syncing(),
                    on_click: sync,
                    if syncing() { "Syncing…" } else { "Sync" }
                }
                Button { variant: ButtonVariant::Ghost, size: ButtonSize::Small, on_click: delete, "Remove" }
            }
        }
    }
}

#[component]
fn AddAccountForm(slug: Memo<Option<String>>, on_saved: EventHandler<()>) -> Element {
    let mut label = use_signal(String::new);
    let provider = use_signal(|| CardDavProvider::NEXTCLOUD.to_string());
    let mut server_url = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut saving = use_signal(|| false);

    let submit = move |_| {
        let l = label.peek().trim().to_string();
        if l.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let mut account = CardDavAccount::create(
            uuid::Uuid::new_v4().to_string(),
            l,
            provider.peek().clone(),
            Utc::now().to_rfc3339(),
        );
        account.server_url = server_url.peek().trim().to_string();
        account.username = username.peek().trim().to_string();
        account.password = password.peek().clone();
        saving.set(true);
        spawn(async move {
            let _ = crate::feeds::upsert_carddav_account(&s, account).await;
            saving.set(false);
            label.set(String::new());
            server_url.set(String::new());
            username.set(String::new());
            password.set(String::new());
            on_saved.call(());
        });
    };

    rsx! {
        Card { class: "p-4".to_string(),
            div { class: "flex flex-col gap-3",
                div { class: "grid gap-3 sm:grid-cols-2",
                    Field {
                        FieldLabel { required: true, "Label" }
                        input { class: "{INPUT_CLS}", placeholder: "Personal iCloud", value: "{label}", oninput: move |e| label.set(e.value()) }
                    }
                    Field {
                        FieldLabel { "Provider" }
                        Select {
                            value: provider,
                            placeholder: "Provider".to_string(),
                            SelectContent {
                                SelectItem { value: CardDavProvider::NEXTCLOUD.to_string(), index: 0, "Nextcloud" }
                                SelectItem { value: CardDavProvider::ICLOUD.to_string(), index: 1, "iCloud" }
                                SelectItem { value: CardDavProvider::GENERIC.to_string(), index: 2, "Generic" }
                            }
                        }
                    }
                }
                Field {
                    FieldLabel { "Server URL" }
                    FieldDescription { "Leave blank for iCloud — it uses the standard endpoint." }
                    input { class: "{INPUT_CLS}", placeholder: "https://cloud.example.com", value: "{server_url}", oninput: move |e| server_url.set(e.value()) }
                }
                div { class: "grid gap-3 sm:grid-cols-2",
                    Field {
                        FieldLabel { "Username" }
                        input { class: "{INPUT_CLS}", placeholder: "you@example.com", value: "{username}", oninput: move |e| username.set(e.value()) }
                    }
                    Field {
                        FieldLabel { "App password" }
                        input { class: "{INPUT_CLS}", r#type: "password", placeholder: "••••••••", value: "{password}", oninput: move |e| password.set(e.value()) }
                    }
                }
                div { class: "flex justify-end",
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        disabled: saving(),
                        on_click: submit,
                        if saving() { "Saving…" } else { "Add account" }
                    }
                }
            }
        }
    }
}

//! `/members` — the org's people + their billing rates.
//!
//! Lists every member of the active org (via `AuthService::list_org_members`)
//! and their org-level hourly rate (cascade level 3). The rate is
//! editable inline and upserted through `TimerService::set_org_member_rate`,
//! so several people can bill at different rates.
//!
//! Members render as the ledger's shared **person-chip** (initials
//! avatar + name + role badge), the same shape used on `/contacts` and
//! `/invoices`. When a contact is linked to a member (`linked_user_id`),
//! the chip is enriched with the contact's phone — but a link is never
//! required.
//!
//! Requires a signed-in session (the member list is derived from the
//! caller's token); Guest sees a sign-in prompt.

use std::collections::HashMap;

use dioxus::prelude::*;
use architect_ui::prelude::*;
use uuid::Uuid;

use crate::auth::AuthCtx;
use crate::chrome::resolve_org;
use crate::orgs::{OrgMeta, OrgSelection};
use crate::pages::contacts::PersonChip;

/// A member joined with their org-level rate (0 = unset) and, if one
/// exists, the phone from a linked contact.
#[derive(Clone, PartialEq)]
struct MemberRow {
    user_id: Uuid,
    name: String,
    email: String,
    role: String,
    cents: i64,
    phone: Option<String>,
}

#[component]
pub fn MembersView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let auth = use_context::<AuthCtx>();

    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));
    let token = use_memo(move || auth.active.read().as_ref().map(|a| a.token.clone()));

    let mut reload = use_signal(|| 0u32);
    let rows = use_resource(move || {
        let _ = reload();
        let target = target();
        let token = token();
        async move {
            let (slug, org_id) = target.ok_or("Select an org.")?;
            // The member list is org-scoped by the endpoint, so no token
            // is required; pass one when signed in for the precise
            // membership/role path.
            let members = crate::feeds::fetch_org_members(&slug, token.unwrap_or_default()).await?;
            let rates = crate::feeds::fetch_org_member_rates(&slug, org_id)
                .await
                .unwrap_or_default();
            let by_user: HashMap<Uuid, i64> =
                rates.iter().map(|r| (r.user_id, r.hourly_cents)).collect();
            // Enrich with linked contacts (phone) — best-effort, never
            // required.
            let phones: HashMap<Uuid, String> = crate::feeds::fetch_contacts(&slug)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter_map(|c| {
                    let uid = c.linked_user_id.as_ref()?.parse::<Uuid>().ok()?;
                    let phone = c.primary_phone()?.to_string();
                    Some((uid, phone))
                })
                .collect();
            let mut out: Vec<MemberRow> = members
                .into_iter()
                .map(|m| MemberRow {
                    cents: by_user.get(&m.user_id).copied().unwrap_or(0),
                    phone: phones.get(&m.user_id).cloned(),
                    user_id: m.user_id,
                    name: m.name,
                    email: m.email,
                    role: m.role,
                })
                .collect();
            out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            Ok::<Vec<MemberRow>, String>(out)
        }
    });

    let target_now = target();

    let body = match &*rows.read_unchecked() {
        Some(Ok(list)) => {
            let list = list.clone();
            let (slug, org_id) = target_now.clone().unwrap_or_default();
            if list.is_empty() {
                rsx! {
                    EmptyState { message: "No members in this org yet.".to_string() }
                }
            } else {
                rsx! {
                    Card { class: "overflow-hidden".to_string(),
                        TableContainer {
                            Table {
                                TableHeader {
                                    TableRow {
                                        TableHead { class: "text-[0.7rem] uppercase tracking-wider text-muted-foreground".to_string(), "Member" }
                                        TableHead { class: "text-right text-[0.7rem] uppercase tracking-wider text-muted-foreground".to_string(), "Rate / hr" }
                                    }
                                }
                                TableBody {
                                    for row in list {
                                        MemberRateRow {
                                            key: "{row.user_id}",
                                            row,
                                            slug: slug.clone(),
                                            org_id,
                                            on_saved: move |_| reload += 1,
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(e)) => rsx! {
            EmptyState { message: e.clone() }
        },
        None => rsx! { crate::states::LoadingState {} },
    };

    rsx! {
        div { class: "mx-auto flex w-full max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Organization"
                }
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Members & rates" }
                Text { variant: TextVariant::Muted,
                    "Each member's hourly rate applies across the org's projects. Timers snapshot the rate when they're logged."
                }
            }
            {body}
        }
    }
}

/// One member row — a person-chip + role badge + an inline editable rate.
#[component]
fn MemberRateRow(
    row: MemberRow,
    slug: String,
    org_id: Uuid,
    on_saved: EventHandler<()>,
) -> Element {
    let user_id = row.user_id;

    let (role_variant, role_label) = match row.role.as_str() {
        "owner" => (StatusBadgeVariant::Success, "Owner"),
        "admin" => (StatusBadgeVariant::Warning, "Admin"),
        _ => (StatusBadgeVariant::Neutral, "Member"),
    };

    // Subtitle: email, plus the linked contact's phone when present.
    let subtitle = match &row.phone {
        Some(p) => format!("{} · {p}", row.email),
        None => row.email.clone(),
    };

    // Current rate as a dollars string for the inline editor.
    let rate_str = if row.cents > 0 {
        format!("{:.2}", row.cents as f64 / 100.0)
    } else {
        String::new()
    };

    let commit = move |next: String| {
        let cents = (next.trim().parse::<f64>().unwrap_or(0.0) * 100.0).round() as i64;
        if cents < 0 {
            return;
        }
        let slug = slug.clone();
        spawn(async move {
            let _ =
                crate::feeds::set_org_member_rate(&slug, org_id, user_id, cents, "USD".to_string())
                    .await;
            on_saved.call(());
        });
    };

    rsx! {
        TableRow {
            TableCell {
                PersonChip {
                    name: row.name.clone(),
                    email: row.email.clone(),
                    subtitle: Some(subtitle),
                    badge_label: Some(role_label.to_string()),
                    badge_variant: role_variant,
                }
            }
            TableCell { class: "text-right".to_string(),
                div { class: "flex items-center justify-end gap-1 font-mono tabular-nums",
                    span { class: "text-muted-foreground", "$" }
                    InlineEdit {
                        value: rate_str,
                        placeholder: "0.00".to_string(),
                        class: "min-w-16 text-right".to_string(),
                        on_commit: commit,
                    }
                    span { class: "text-xs text-muted-foreground", "/hr" }
                }
            }
        }
    }
}

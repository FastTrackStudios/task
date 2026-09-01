//! Bookings — bookable event types, and the slots people book.
//!
//! The public front desk: an org publishes event types ("30-minute
//! consult") and people book slots against them. Split out of core
//! `scheduling` because it is a different question. Day plans and
//! calendar events are how *you* organise your own time, which is
//! something every org does; this is selling time by the slot, which
//! most do not. An org with no front desk should be able to not have
//! one.
//!
//! Both stores are **live** — one `SchedulingEvent` stream feeds them
//! both, each folding only its own variants — so a booking made on the
//! public page appears here without a refetch.
//!
//! Mounted by `task-plugin-bookings`.

use architect::Id;
use architect_ui::prelude::*;
use dioxus::prelude::*;
use scheduling_proto::{Booking, BookingStatus, EventType};
use task_stores::{run_create, use_first_org_list};
use task_ui_core::feeds;
use task_ui_core::format::slugify;
use task_ui_core::orgs::{OrgMeta, OrgSelection};
use uuid::Uuid;

/// This app's id in Task's catalog, and the first segment of every
/// link it writes to itself.
pub const APP_ID: &str = "bookings";

feeds! {
    scheduling_proto::EventTypesClient {
        /// All bookable event types for the org (30-min consults, etc.).
        fetch_event_types() -> Vec<scheduling_proto::EventType>
            = list_event_types() as "list event types";

        /// Create (upsert) a bookable event type, returning the persisted draft
        /// so the optimistic store can reconcile against it. The backend derives
        /// the vault `path` from the slug/id; the caller builds the entity (see
        /// [`draft_event_type`]).
        create_event_type(event_type: scheduling_proto::EventType) -> scheduling_proto::EventType
            = upsert_event_type(event_type.clone()) map |()| event_type, as "create event type";
    }

    scheduling_proto::BookingsClient {
        /// All bookings for the org (every status), oldest start first.
        fetch_bookings() -> Vec<scheduling_proto::Booking>
            = list_bookings() as "list bookings";

        /// Cancel a booking by id (sets status to `Cancelled`).
        cancel_booking(id: &str) -> ()
            = update_booking_status(scheduling_proto::BookingId(id.to_owned()), scheduling_proto::BookingStatus::Cancelled) as "cancel booking";
    }
}

task_stores::stores! {
    BookingStore: scheduling_proto::Booking {
        provide: provide_booking_store,
        handle: use_booking_store,
        stream:
            /// Live bookings off the slice's one `SchedulingEvent`
            /// stream (ignores the other sub-resource variants).
            first scheduling_proto::SchedulingEventsStreamClient => fold_booking_event,
        mutations: BookingMutations via use_booking_mutations,
    }

    EventTypeStore: scheduling_proto::EventType {
        provide: provide_event_type_store,
        handle: use_event_type_store,
        list: use_event_type_list -> String = fetch_event_types,
        stream:
            /// Live event types off the same `SchedulingEvent` stream.
            first scheduling_proto::SchedulingEventsStreamClient => fold_event_type_event,
        mutations: EventTypeMutations via use_event_type_mutations,
    }
}

/// Bookings off the slice's one `SchedulingEvent` stream (event types
/// below; day plans stay fetch-shaped and belong to core scheduling).
fn fold_booking_event(store: &BookingStore, _slug: &str, ev: scheduling_proto::SchedulingEvent) {
    if let scheduling_proto::SchedulingEvent::BookingUpserted(b) = ev {
        store.put(b);
    }
}

/// Event types off the same stream (see [`fold_booking_event`]).
fn fold_event_type_event(
    store: &EventTypeStore,
    _slug: &str,
    ev: scheduling_proto::SchedulingEvent,
) {
    match ev {
        scheduling_proto::SchedulingEvent::EventTypeUpserted(et) => store.put(et),
        scheduling_proto::SchedulingEvent::EventTypeDeleted(id) => store.remove_real(&id),
        _ => {}
    }
}

/// Bookings for the first selected org, soonest start first.
pub fn use_booking_list()
-> architect::AtomResult<Vec<(Id<String>, scheduling_proto::Booking)>, String> {
    use_first_org_list(use_booking_store(), |slug| async move {
        fetch_bookings(&slug).await.map(|mut rows| {
            rows.sort_by(|a, b| a.start_utc.cmp(&b.start_utc));
            rows
        })
    })
}

impl BookingMutations {
    /// Cancel a booking: the row vanishes instantly; restored (and the
    /// failure reported) if the server rejects it.
    pub fn cancel(&self, slug: String, id: String) {
        let key = id.clone();
        self.write.run(
            self.store,
            move |s| s.remove_optimistic(Id::Real(key)),
            move || async move { cancel_booking(&slug, &id).await.map(|()| None) },
        );
    }
}

/// Draft a bookable event type (client-minted stable id; the backend
/// derives the vault `path`).
#[must_use]
pub fn draft_event_type(title: String, duration_min: u16) -> scheduling_proto::EventType {
    let url_slug = slugify(&title);
    scheduling_proto::EventType {
        path: String::new(),
        id: scheduling_proto::EventTypeId(Uuid::new_v4().to_string()),
        title,
        slug: url_slug,
        description: None,
        duration_min,
        buffer_min: 0,
        location: scheduling_proto::EventTypeLocation::Tbd,
        schedule_id: None,
        published: true,
    }
}

impl EventTypeMutations {
    pub fn create(&self, slug: String, draft: scheduling_proto::EventType) {
        run_create(self.write, self.store, draft, move |et| async move {
            create_event_type(&slug, et).await
        });
    }
}

const INPUT_CLS: &str = "rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50 placeholder:text-muted-foreground";

/// Duration presets offered in the form's picker (minutes). `duration_min`
/// is free-form on the model; these cover the common Cal.com cases.
const DURATIONS: &[u16] = &[15, 30, 45, 60, 90];

#[component]
pub fn BookingsView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we list / create into (first selected, or home).
    let slug = use_memo(move || {
        task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let mut title = use_signal(String::new);
    let mut duration = use_signal(|| 30u16);
    // Select binds a String; mirror it into the u16 `duration` on change.
    let duration_pick = use_signal(|| "30".to_string());

    // Shared optimistic stores for both halves of the page.
    let types_result = crate::use_event_type_list();
    let type_muts = crate::use_event_type_mutations();
    let event_type_store = crate::use_event_type_store();
    let bookings_result = crate::use_booking_list();
    let booking_store = crate::use_booking_store();

    // Create the drafted event type: it appears instantly, then
    // reconciles against the persisted entity.
    let mut create = move || {
        let t = title.read().trim().to_string();
        if t.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let d = duration();
        title.set(String::new());
        type_muts.create(s, crate::draft_event_type(t, d));
    };

    let types: Vec<(Id<String>, EventType)> = types_result.value().cloned().unwrap_or_default();
    let types_err = types_result.error().cloned();
    let types_first_load = types_result.is_waiting() && types_result.value().is_none();

    let rows: Vec<(Id<String>, Booking)> = bookings_result.value().cloned().unwrap_or_default();
    let bookings_err = bookings_result.error().cloned();
    let bookings_first_load = bookings_result.is_waiting() && bookings_result.value().is_none();

    rsx! {
        div { class: "mx-auto flex max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-10",
            div { class: "flex items-center justify-between gap-3",
                Heading { level: HeadingLevel::H1, "Bookings" }
                Text { variant: TextVariant::Muted, class: "text-sm", "{types.len()} event types" }
            }
            Text {
                variant: TextVariant::Muted,
                class: "text-sm -mt-2",
                "Bookable event types and the bookings people have made against them.",
            }

            // ── Add event type ─────────────────────────────────────
            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Event type name… (e.g. 30-min consult)",
                    value: "{title}",
                    oninput: move |e| title.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                Select {
                    value: duration_pick,
                    placeholder: "Duration".to_string(),
                    on_change: move |v: String| {
                        if let Ok(d) = v.parse::<u16>() {
                            duration.set(d);
                        }
                    },
                    SelectContent {
                        for (i, d) in DURATIONS.iter().enumerate() {
                            SelectItem { key: "{d}", value: "{d}", index: i, "{d} min" }
                        }
                    }
                }
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: move |_| create(),
                    "Add"
                }
            }

            // ── Event types ────────────────────────────────────────
            Heading { level: HeadingLevel::H2, class: "text-base", "Event types" }
            if types_first_load {
                task_ui_core::states::LoadingState {}
            } else if types.is_empty() {
                if let Some(err) = types_err {
                    task_ui_core::states::ErrorState {
                        title: "Couldn't load event types",
                        message: err,
                        on_retry: move |()| event_type_store.reload(),
                    }
                } else {
                    task_ui_core::states::EmptyState {
                        title: "No event types yet",
                        hint: "Add your first event type above.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, et) in types {
                        EventTypeRow { key: "{id}", pending: id.is_temp(), event_type: et }
                    }
                }
            }

            // ── Bookings ───────────────────────────────────────────
            Heading { level: HeadingLevel::H2, class: "text-base mt-2", "Upcoming bookings" }
            if bookings_first_load {
                task_ui_core::states::LoadingState {}
            } else if rows.is_empty() {
                if let Some(err) = bookings_err {
                    task_ui_core::states::ErrorState {
                        title: "Couldn't load bookings",
                        message: err,
                        on_retry: move |()| booking_store.reload(),
                    }
                } else {
                    task_ui_core::states::EmptyState {
                        title: "No bookings yet",
                        hint: "Bookings people make will appear here.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, booking) in rows {
                        BookingRow {
                            key: "{id}",
                            pending: id.is_temp(),
                            slug: slug().unwrap_or_default(),
                            booking,
                        }
                    }
                }
            }
        }
    }
}

/// One event type: title + duration + published badge. `pending` dims
/// an optimistic row whose write-through is in flight.
#[component]
fn EventTypeRow(event_type: EventType, pending: bool) -> Element {
    let title = event_type.title.clone();
    let duration = event_type.duration_min;
    let published = event_type.published;

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                Text { class: "break-words text-sm font-medium", "{title}" }
                span { class: "text-[11px] text-muted-foreground", "{duration} min" }
            }
            div { class: "flex shrink-0 items-center gap-2",
                if published {
                    StatusBadge { variant: StatusBadgeVariant::Success, label: "Published".to_string() }
                } else {
                    StatusBadge { variant: StatusBadgeVariant::Neutral, label: "Draft".to_string() }
                }
            }
        }
    }
}

/// One booking: attendee + when + status badge + cancel.
///
/// Cancelling removes the row instantly; if the server rejects it the
/// store rolls the row back and the failure lands in the notification
/// tray. `pending` dims an in-flight optimistic row.
#[component]
fn BookingRow(booking: Booking, pending: bool, slug: String) -> Element {
    let muts = crate::use_booking_mutations();
    let who = booking.attendee_name.clone();
    let email = booking.attendee_email.clone();
    let when = booking.start_utc.clone();
    let id = booking.id.0.clone();

    let (variant, label) = match booking.status {
        BookingStatus::Pending => (StatusBadgeVariant::Warning, "Pending"),
        BookingStatus::Confirmed => (StatusBadgeVariant::Success, "Confirmed"),
        BookingStatus::Cancelled => (StatusBadgeVariant::Danger, "Cancelled"),
        BookingStatus::NoShow => (StatusBadgeVariant::Danger, "No-show"),
        BookingStatus::Completed => (StatusBadgeVariant::Neutral, "Completed"),
    };

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    let cancel = move |_| {
        muts.cancel(slug.clone(), id.clone());
    };

    // If some enabled app knows how to bill things, a booking that
    // actually happened can be billed. Nothing here knows that app is
    // finance, or that an invoice exists — only that somebody offered
    // `Billing`, and where it says to go.
    //
    // `None` is the ordinary case and not a failure: finance is not in
    // this build, or is off for this org, and the row simply has one
    // fewer action. That is the whole contract — an integration is
    // something an app gains, never something it needs.
    let bill_to = use_billing_href(&booking);

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                Text { class: "break-words text-sm font-medium", "{who}" }
                span { class: "text-[11px] text-muted-foreground", "{email}" }
                span { class: "text-[11px] text-muted-foreground", "{when}" }
            }
            div { class: "flex shrink-0 items-center gap-2",
                StatusBadge { variant, label: label.to_string() }
                if let Some(to) = bill_to {
                    Link {
                        to,
                        class: "text-[11px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline",
                        "Invoice…"
                    }
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: cancel,
                    "Cancel"
                }
            }
        }
    }
}

/// Where to bill this booking, if any enabled app offers billing.
///
/// Every hook runs before any decision — a booking's status changes
/// under this (that is the whole point of a live store), and an early
/// return above a `use_context` would change how many hooks this
/// component ran between renders, which Dioxus cannot survive.
fn use_billing_href(booking: &Booking) -> Option<String> {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    // The event type carries the name and the length; the booking only
    // points at it.
    let types = use_event_type_list();

    let enabled = task_ui_core::orgs::active_plugin_set(&selection.read(), &org_list.read());
    let billing = task_plugin_ui::offered::<finance_contract::Billing>(|id| enabled.contains(id));
    let event_type = types.value().and_then(|rows| {
        rows.iter()
            .map(|(_, et)| et)
            .find(|et| et.id == booking.event_type_id)
            .cloned()
    });
    billing_href(booking, event_type.as_ref(), billing.as_deref())
}

/// Whether this booking gets an "Invoice…" link, and where it goes.
///
/// Separate from the hook above so it can be tested: everything that
/// decides is here, and the hook is only the part that reads context.
/// The rules are small but each one is a way to get it wrong —
/// offering to bill the wrong thing, or offering at all when nobody can.
fn billing_href(
    booking: &Booking,
    event_type: Option<&EventType>,
    billing: Option<&finance_contract::Billing>,
) -> Option<String> {
    // Only a booking that happened. Offering to invoice a cancelled
    // slot, or one that has not occurred yet, is offering a mistake.
    if !matches!(booking.status, BookingStatus::Completed) {
        return None;
    }
    // Nobody enabled offers billing. Not a failure — the row simply has
    // one fewer action.
    let billing = billing?;
    Some((billing.bill_href)(&finance_contract::Billable {
        // The event type may not have loaded yet, or may have been
        // deleted out from under a historical booking. Neither is a
        // reason to withhold the action — the invoice screen asks for
        // the details anyway.
        what: event_type.map_or_else(|| "Booking".to_string(), |et| et.title.clone()),
        client: booking.attendee_name.clone(),
        minutes: event_type.map_or(0, |et| u32::from(et.duration_min)),
    }))
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// The seeded booking, as `Records/bookings/mix-review-sam-reeve.md`
    /// describes it.
    fn seeded_booking(status: BookingStatus) -> Booking {
        Booking {
            path: "Records/bookings/mix-review-sam-reeve.md".into(),
            id: scheduling_proto::BookingId("8f3b1c62-4d5a-4e7f-9a10-6b2c8d4e5f71".into()),
            event_type_id: scheduling_proto::EventTypeId("mix-review-30".into()),
            start_utc: "2026-08-24T15:00:00Z".into(),
            end_utc: "2026-08-24T15:30:00Z".into(),
            attendee_name: "Sam Reeve".into(),
            attendee_email: "sam@example.com".into(),
            note: Some("Rough mix of \"Washed\" — asked about the vocal bus.".into()),
            status,
            created_utc: "2026-08-17T09:12:00Z".into(),
        }
    }

    /// The seeded event type it points at.
    fn seeded_event_type() -> EventType {
        draft_event_type("Mix review".into(), 30)
    }

    /// Stands in for finance: the same shape, so what is exercised is
    /// the handover rather than finance's URL scheme.
    fn billing() -> finance_contract::Billing {
        finance_contract::Billing {
            bill_href: |work| {
                format!(
                    "/app/finance/invoices?what={}&client={}&minutes={}",
                    work.what, work.client, work.minutes
                )
            },
        }
    }

    /// The whole point: a booking that happened, an app that bills, and
    /// the work reaching it intact.
    #[test]
    fn a_completed_booking_can_be_invoiced() {
        let href = billing_href(
            &seeded_booking(BookingStatus::Completed),
            Some(&seeded_event_type()),
            Some(&billing()),
        )
        .expect("a completed booking is billable");
        assert!(href.contains("what=Mix review"), "{href}");
        assert!(href.contains("client=Sam Reeve"), "{href}");
        assert!(href.contains("minutes=30"), "{href}");
    }

    /// The other half of "optional": with nobody offering to bill,
    /// nothing here changes except that the action is absent. This is
    /// the case that must not panic, and must not render a dead link.
    #[test]
    fn without_a_billing_app_there_is_no_action() {
        assert!(
            billing_href(
                &seeded_booking(BookingStatus::Completed),
                Some(&seeded_event_type()),
                None,
            )
            .is_none()
        );
    }

    /// Billing a slot that was cancelled, or has not happened yet, is
    /// offering somebody a mistake.
    #[test]
    fn only_a_booking_that_happened_is_billable() {
        for status in [
            BookingStatus::Pending,
            BookingStatus::Confirmed,
            BookingStatus::Cancelled,
            BookingStatus::NoShow,
        ] {
            assert!(
                billing_href(
                    &seeded_booking(status),
                    Some(&seeded_event_type()),
                    Some(&billing())
                )
                .is_none(),
                "{status:?} is not something that happened"
            );
        }
    }

    /// A historical booking whose event type was deleted still bills —
    /// the invoice screen asks for the details anyway, and withholding
    /// the action would strand the work.
    #[test]
    fn a_booking_with_no_event_type_still_bills() {
        let href = billing_href(
            &seeded_booking(BookingStatus::Completed),
            None,
            Some(&billing()),
        )
        .expect("still billable");
        assert!(href.contains("what=Booking"), "{href}");
        assert!(href.contains("minutes=0"), "{href}");
    }
}

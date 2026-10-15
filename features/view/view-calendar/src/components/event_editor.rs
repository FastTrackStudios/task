//! The event sheet, opened by tapping an event chip.
//!
//! Drawn the way the app's other edit sheets are (the Schedule's block
//! sheet, Display options): on a phone a full-width sheet rising from
//! the bottom edge, on a desk a panel down the right side; the title as
//! the heading with Done beside it, then grouped inset rows — all day,
//! starts, ends, colour, repeats — the notes, and Delete at the end.
//!
//! Edits apply as they are made (each is a [`CalendarMutation`]), so
//! Done only closes.

use architect_ui::lucide_dioxus::{Check, Trash2};
use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc};
use dioxus::prelude::*;

use crate::store::CalendarMutation;
use crate::types::{CalendarEvent, ColorTag, EventId};

const COLOR_OPTIONS: &[ColorTag] = &[
    ColorTag::Neutral,
    ColorTag::Primary,
    ColorTag::Success,
    ColorTag::Warning,
    ColorTag::Danger,
    ColorTag::Info,
];

/// One grouped row's frame (label left, value right).
const ROW: &str =
    "flex min-h-[54px] items-center justify-between gap-3 px-4 text-[16px] text-foreground";
/// A grouped inset list.
const GROUP: &str =
    "mb-3 flex flex-col divide-y divide-foreground/[0.06] rounded-[20px] bg-foreground/[0.05]";
/// A value control sitting at a row's right edge.
const VALUE: &str = "bg-transparent text-right text-[16px] text-muted-foreground outline-none";

#[derive(Props, Clone, PartialEq)]
pub struct EventEditorProps {
    pub event: CalendarEvent,
    pub open: bool,
    pub on_close: EventHandler<()>,
    pub on_event: EventHandler<CalendarMutation>,
}

#[component]
pub fn EventEditor(props: EventEditorProps) -> Element {
    if !props.open {
        return rsx! {};
    }
    let event = props.event.clone();
    let id: EventId = event.id;
    let on_event = props.on_event;
    let on_close = props.on_close;

    rsx! {
        div {
            class: "fixed inset-0 z-50 bg-black/55 supports-[backdrop-filter]:backdrop-blur-[2px]",
            onclick: move |_| on_close.call(()),
        }
        div {
            class: "fixed inset-x-0 bottom-0 z-50 flex max-h-[88vh] flex-col overflow-hidden rounded-t-[30px] bg-popover text-foreground shadow-[0_-12px_40px_-12px_rgb(0_0_0/0.6),inset_0_1px_0_rgb(255_255_255/0.07)] outline-none md:inset-x-auto md:inset-y-0 md:right-0 md:max-h-none md:w-[24rem] md:rounded-none md:rounded-l-[24px]",
            role: "dialog",
            aria_label: "Edit event",
            tabindex: "-1",
            onclick: move |e| e.stop_propagation(),
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    on_close.call(());
                }
            },
            div { class: "mx-auto mt-2 h-[5px] w-9 shrink-0 rounded-full bg-foreground/20 md:hidden" }
            div {
                class: "min-h-0 flex-1 overflow-y-auto px-3 pt-2 md:pt-5",
                style: "padding-bottom: max(0.75rem, env(safe-area-inset-bottom, 0px));",
                // ── title, as the heading ──
                div { class: "flex items-center gap-2 px-2 pb-3",
                    input {
                        class: "min-w-0 flex-1 bg-transparent text-[22px] font-semibold tracking-[-0.01em] text-foreground outline-none placeholder:text-muted-foreground",
                        aria_label: "Event title",
                        placeholder: "Event title",
                        value: "{event.title}",
                        oninput: move |e: FormEvent| {
                            on_event.call(CalendarMutation::Rename { id, title: e.value() });
                        },
                    }
                    button {
                        r#type: "button",
                        class: "h-10 shrink-0 rounded-full bg-primary px-4 text-[15px] font-semibold text-primary-foreground active:scale-95",
                        onclick: move |_| on_close.call(()),
                        "Done"
                    }
                }

                // ── when ──
                div { class: GROUP,
                    label { class: ROW,
                        span { "All day" }
                        input {
                            r#type: "checkbox",
                            role: "switch",
                            class: "peer sr-only",
                            checked: event.all_day,
                            onchange: move |e: FormEvent| {
                                on_event.call(CalendarMutation::SetAllDay { id, all_day: e.checked() });
                            },
                        }
                        span {
                            class: if event.all_day { "relative h-[31px] w-[51px] shrink-0 rounded-full bg-primary p-[2px] transition-colors" } else { "relative h-[31px] w-[51px] shrink-0 rounded-full bg-foreground/15 p-[2px] transition-colors" },
                            span { class: if event.all_day { "block size-[27px] translate-x-5 rounded-full bg-white shadow transition-transform" } else { "block size-[27px] rounded-full bg-white shadow transition-transform" } }
                        }
                    }
                    DateTimeRow {
                        label: "Starts",
                        value: event.start,
                        all_day: event.all_day,
                        on_change: move |dt: chrono::DateTime<chrono::Utc>| {
                            on_event.call(CalendarMutation::Reschedule {
                                id,
                                start: dt,
                                end: event.end.max(dt + chrono::Duration::minutes(15)),
                            });
                        },
                    }
                    DateTimeRow {
                        label: "Ends",
                        value: event.end,
                        all_day: event.all_day,
                        on_change: move |dt: chrono::DateTime<chrono::Utc>| {
                            on_event.call(CalendarMutation::Reschedule {
                                id,
                                start: event.start,
                                end: dt.max(event.start + chrono::Duration::minutes(15)),
                            });
                        },
                    }
                }

                // ── look and repeat ──
                div { class: GROUP,
                    div { class: ROW,
                        span { "Colour" }
                        div { class: "flex items-center gap-2",
                            for color in COLOR_OPTIONS.iter() {
                                {
                                    let c = *color;
                                    let stem = c.stem();
                                    let on = c == event.color;
                                    rsx! {
                                        button {
                                            key: "swatch-{stem}",
                                            r#type: "button",
                                            class: "flex size-7 items-center justify-center rounded-full bg-{stem}-500 text-white active:scale-90",
                                            aria_label: "{stem}",
                                            aria_pressed: "{on}",
                                            onclick: move |_| on_event.call(CalendarMutation::Recolor { id, color: c }),
                                            if on {
                                                Check { size: 14 }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    RepeatRow { event_id: id, current: event.recurrence.clone(), on_event }
                }

                // ── notes ──
                div { class: "mb-3 rounded-[20px] bg-foreground/[0.05] px-4 py-3",
                    textarea {
                        class: "min-h-20 w-full resize-none bg-transparent text-[16px] leading-6 text-foreground outline-none [field-sizing:content] placeholder:text-muted-foreground",
                        aria_label: "Notes",
                        placeholder: "Notes",
                        value: event.description.clone().unwrap_or_default(),
                        oninput: move |e: FormEvent| {
                            let v = e.value();
                            let description = if v.is_empty() { None } else { Some(v) };
                            on_event.call(CalendarMutation::SetDescription { id, description });
                        },
                    }
                }

                div { class: GROUP,
                    button {
                        r#type: "button",
                        class: "flex min-h-[54px] w-full items-center gap-3 px-4 text-left text-[16px] text-red-400 active:bg-foreground/[0.04]",
                        onclick: move |_| {
                            on_event.call(CalendarMutation::Remove { id });
                            on_close.call(());
                        },
                        Trash2 { size: 17 }
                        "Delete event"
                    }
                }
            }
        }
    }
}

/// A row with native date (+ time) pickers at its right edge. The two
/// fields are stitched back together when either commits — the inputs
/// are read as UTC-aligned, as v1 has always done.
#[component]
fn DateTimeRow(
    label: &'static str,
    value: chrono::DateTime<chrono::Utc>,
    all_day: bool,
    on_change: EventHandler<chrono::DateTime<chrono::Utc>>,
) -> Element {
    let naive = value.naive_utc();
    let date_str = naive.format("%Y-%m-%d").to_string();
    let time_str = naive.format("%H:%M").to_string();
    let cur = value;
    rsx! {
        div { class: ROW,
            span { "{label}" }
            div { class: "flex items-center gap-3",
                input {
                    r#type: "date",
                    class: VALUE,
                    value: "{date_str}",
                    onchange: move |e: FormEvent| {
                        if let Some(dt) = recombine(Some(&e.value()), None, cur) {
                            on_change.call(dt);
                        }
                    },
                }
                if !all_day {
                    input {
                        r#type: "time",
                        class: VALUE,
                        value: "{time_str}",
                        onchange: move |e: FormEvent| {
                            if let Some(dt) = recombine(None, Some(&e.value()), cur) {
                                on_change.call(dt);
                            }
                        },
                    }
                }
            }
        }
    }
}

const RRULE_PRESETS: &[(&str, &str)] = &[
    ("Never", ""),
    ("Every day", "FREQ=DAILY"),
    ("Every weekday", "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"),
    ("Every week", "FREQ=WEEKLY"),
    ("Every month", "FREQ=MONTHLY"),
    ("Every year", "FREQ=YEARLY"),
];

/// "Repeats" as one row with a native picker of the common rules; a
/// rule set elsewhere (a raw RRULE) shows as "Custom".
#[component]
fn RepeatRow(
    event_id: EventId,
    current: Option<String>,
    on_event: EventHandler<CalendarMutation>,
) -> Element {
    let id = event_id;
    let cur = current.unwrap_or_default();
    let known = RRULE_PRESETS.iter().any(|(_, r)| *r == cur);
    rsx! {
        label { class: ROW,
            span { "Repeats" }
            select {
                class: "{VALUE} appearance-none",
                onchange: move |e: FormEvent| {
                    let v = e.value();
                    if v == "__custom" {
                        return;
                    }
                    let recurrence = if v.is_empty() { None } else { Some(v) };
                    on_event.call(CalendarMutation::SetRecurrence { id, recurrence });
                },
                for (label, rule) in RRULE_PRESETS.iter() {
                    option { key: "{label}", value: "{rule}", selected: *rule == cur, "{label}" }
                }
                if !known {
                    option { value: "__custom", selected: true, "Custom" }
                }
            }
        }
    }
}

fn recombine(
    date: Option<&str>,
    time: Option<&str>,
    current: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let cur_naive = current.naive_utc();
    let new_date = match date {
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?,
        None => NaiveDate::from_ymd_opt(cur_naive.year(), cur_naive.month(), cur_naive.day())?,
    };
    let new_time = match time {
        Some(s) => NaiveTime::parse_from_str(s, "%H:%M").ok()?,
        None => NaiveTime::from_hms_opt(cur_naive.hour(), cur_naive.minute(), 0)?,
    };
    let combined = NaiveDateTime::new(new_date, new_time);
    Some(Utc.from_utc_datetime(&combined))
}

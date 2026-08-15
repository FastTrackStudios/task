//! Blocker / relates-to chips. Titles are pre-resolved by the
//! page layer; unresolved ids degrade to short-id chips.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{Link as LinkIcon, OctagonAlert};
use uuid::Uuid;

use crate::views::detail_full::{LinkedTaskRef, SectionLabel, short_id};

#[derive(Props, Clone, PartialEq)]
pub struct LinkChipsProps {
    /// Section label, e.g. "Blocked by" / "Relates to".
    pub label: &'static str,
    pub links: Vec<LinkedTaskRef>,
    /// Chip clicked — navigate to that task.
    pub on_open: EventHandler<Uuid>,
}

#[component]
pub fn LinkChips(props: LinkChipsProps) -> Element {
    let danger = props.label.starts_with("Blocked");
    rsx! {
        section { class: "flex flex-col gap-1.5",
            SectionLabel { label: props.label }
            div { class: "flex flex-wrap gap-1",
                for link in props.links.iter() {
                    {
                        let id = link.id;
                        let resolved = link.title.is_some();
                        let label = link.title.clone().unwrap_or_else(|| short_id(&id));
                        let cls = match (danger, resolved) {
                            (true, _) => "inline-flex max-w-full items-center gap-1 rounded-full bg-rose-900/30 px-2 py-0.5 text-[11px] text-rose-200 hover:bg-rose-900/50",
                            (false, true) => "inline-flex max-w-full items-center gap-1 rounded-full bg-muted/50 px-2 py-0.5 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground",
                            (false, false) => "inline-flex max-w-full items-center gap-1 rounded-full border border-dashed border-border px-2 py-0.5 font-mono text-[11px] text-muted-foreground hover:bg-accent",
                        };
                        let title_cls = if resolved { "truncate" } else { "truncate font-mono" };
                        rsx! {
                            button {
                                key: "{id}",
                                r#type: "button",
                                class: "{cls}",
                                title: "{short_id(&id)}",
                                onclick: move |_| props.on_open.call(id),
                                if danger {
                                    OctagonAlert { size: 11 }
                                } else {
                                    LinkIcon { size: 11 }
                                }
                                span { class: "{title_cls}", "{label}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

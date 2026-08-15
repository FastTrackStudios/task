//! Workflow section — estimate picker, cycle chip, assignee
//! list, claim button. Dumb: resolved labels in, intent out.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{Bot, User};
use architect_ui::prelude::*;
use task_proto::model::Estimate;
use uuid::Uuid;
use workflows_proto::AgentRef;

use super::detail_full::{ClaimState, SectionLabel, short_id};

/// Display label for an estimate value.
#[must_use]
pub fn estimate_label(e: Estimate) -> String {
    match e {
        Estimate::XS => "XS".to_string(),
        Estimate::S => "S".to_string(),
        Estimate::M => "M".to_string(),
        Estimate::L => "L".to_string(),
        Estimate::XL => "XL".to_string(),
        Estimate::Points { value } => format!("{value} pts"),
    }
}

const SIZE_CHIPS: [Estimate; 5] = [
    Estimate::XS,
    Estimate::S,
    Estimate::M,
    Estimate::L,
    Estimate::XL,
];

#[derive(Props, Clone, PartialEq)]
pub struct WorkflowSectionProps {
    #[props(default)]
    pub estimate: Option<Estimate>,
    /// Raw cycle id — shown as a short-id chip when no label
    /// was resolved.
    #[props(default)]
    pub cycle_id: Option<Uuid>,
    /// Resolved cycle name (display-only this phase).
    #[props(default)]
    pub cycle_label: Option<String>,
    pub assignees: Vec<AgentRef>,
    #[props(default)]
    pub claim: ClaimState,
    /// Chip clicked: `Some(size)` to set, `None` to clear (the
    /// active chip acts as a toggle).
    pub on_set_estimate: EventHandler<Option<Estimate>>,
    pub on_claim: EventHandler<()>,
}

#[component]
pub fn WorkflowSection(props: WorkflowSectionProps) -> Element {
    rsx! {
        section { class: "flex flex-col gap-3 rounded-lg border border-border bg-card/50 p-3",
            // ---- Estimate chips ----------------------------------------
            div { class: "flex flex-col gap-1.5",
                SectionLabel { label: "Estimate" }
                div { class: "flex flex-wrap items-center gap-1",
                    for size in SIZE_CHIPS {
                        {
                            let active = props.estimate == Some(size);
                            let cls = if active {
                                "rounded-md border border-primary bg-primary/15 text-foreground px-3 py-2 sm:px-2.5 sm:py-1 text-xs font-medium"
                            } else {
                                "rounded-md border border-border text-muted-foreground hover:text-foreground hover:bg-accent px-3 py-2 sm:px-2.5 sm:py-1 text-xs"
                            };
                            rsx! {
                                button {
                                    key: "{estimate_label(size)}",
                                    r#type: "button",
                                    class: "{cls}",
                                    onclick: move |_| {
                                        props
                                            .on_set_estimate
                                            .call(if active { None } else { Some(size) });
                                    },
                                    "{estimate_label(size)}"
                                }
                            }
                        }
                    }
                    // Numeric estimates aren't pickable from chips —
                    // surface the current value so it isn't invisible.
                    if let Some(points @ Estimate::Points { .. }) = props.estimate {
                        span { class: "rounded-md border border-primary bg-primary/15 px-2.5 py-1 text-xs font-medium text-foreground",
                            "{estimate_label(points)}"
                        }
                    }
                }
            }

            // ---- Cycle (display-only) ----------------------------------
            if let Some(cycle) = props.cycle_id {
                div { class: "flex flex-col gap-1.5",
                    SectionLabel { label: "Cycle" }
                    span { class: "inline-flex w-fit items-center rounded-full bg-violet-900/30 px-2 py-0.5 text-[11px] text-violet-200",
                        {props.cycle_label.clone().unwrap_or_else(|| short_id(&cycle))}
                    }
                }
            }

            // ---- Assignees + claim -------------------------------------
            div { class: "flex flex-col gap-1.5",
                SectionLabel { label: "Assignees" }
                div { class: "flex flex-wrap items-center gap-1.5",
                    if props.assignees.is_empty() {
                        span { class: "text-xs text-muted-foreground italic", "Unassigned" }
                    }
                    for (i , a) in props.assignees.iter().enumerate() {
                        AssigneeChip { key: "{i}", agent: a.clone() }
                    }
                    div { class: "ml-auto",
                        ClaimButton { claim: props.claim.clone(), on_claim: props.on_claim }
                    }
                }
            }
        }
    }
}

/// One assignee — humans and agents render distinctly.
#[component]
fn AssigneeChip(agent: AgentRef) -> Element {
    let label = match &agent {
        AgentRef::Human {
            display_name: Some(name),
            ..
        } => name.clone(),
        other => other.short_label(),
    };
    let (cls, icon) = if agent.is_agent() {
        (
            "inline-flex items-center gap-1 rounded-full bg-violet-900/30 px-2 py-0.5 text-[11px] text-violet-200",
            rsx! {
                Bot { size: 11 }
            },
        )
    } else {
        (
            "inline-flex items-center gap-1 rounded-full bg-sky-900/30 px-2 py-0.5 text-[11px] text-sky-200",
            rsx! {
                User { size: 11 }
            },
        )
    };
    rsx! {
        span { class: "{cls}", title: "{agent.short_label()}",
            {icon}
            "{label}"
        }
    }
}

/// Claim affordance, by current claim state:
/// - `Unclaimed` → live "Claim" button
/// - `Mine` → success badge
/// - `Held` → warning badge + a small "claim anyway" escape
///   hatch (the wiring layer decides whether that means `force`)
/// - `Pending` → disabled button while the RPC is in flight
#[component]
fn ClaimButton(claim: ClaimState, on_claim: EventHandler<()>) -> Element {
    match claim {
        ClaimState::Unclaimed => rsx! {
            Button {
                variant: ButtonVariant::Primary,
                size: ButtonSize::Small,
                on_click: move |_| on_claim.call(()),
                "Claim"
            }
        },
        ClaimState::Pending => rsx! {
            Button {
                variant: ButtonVariant::Outline,
                size: ButtonSize::Small,
                disabled: true,
                "Claiming…"
            }
        },
        ClaimState::Mine => rsx! {
            StatusBadge { variant: StatusBadgeVariant::Success, label: "Claimed by you" }
        },
        ClaimState::Held { holder } => rsx! {
            div { class: "flex items-center gap-1.5",
                StatusBadge {
                    variant: StatusBadgeVariant::Warning,
                    label: "Claimed by {holder}",
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    on_click: move |_| on_claim.call(()),
                    "Claim anyway"
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_labels() {
        assert_eq!(estimate_label(Estimate::XS), "XS");
        assert_eq!(estimate_label(Estimate::XL), "XL");
        assert_eq!(estimate_label(Estimate::Points { value: 8 }), "8 pts");
    }
}

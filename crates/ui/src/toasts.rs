//! Task's toasts: the shared primitive's queue and timing, drawn here.
//!
//! The rendering lives in Task rather than `architect-ui` for two
//! reasons: toasts can carry an **action** ("Undo" after completing or
//! deleting a task — `ToastOptions::action`), which architect-ui's
//! renderer does not yet pass through; and the status reads from an
//! icon beside the title instead of a thick coloured left edge.
//!
//! Every `use_toast()` caller (architect-ui's re-export is the same
//! primitive) lands here once this provider is mounted.

use std::time::Duration;

use architect_ui::lucide_dioxus::{CircleAlert, CircleCheck, Info, TriangleAlert, X};
use dioxus::dioxus_core::DynamicNode;
use dioxus::prelude::*;
use dioxus_primitives::toast::{
    Toast as PrimitiveToast, ToastActionButton, ToastCloseButton, ToastContent, ToastDescription,
    ToastProps, ToastPropsWithOwner, ToastProvider as PrimitiveToastProvider, ToastTitle,
    ToastType,
};

/// How long a toast stays: long enough to reach for Undo.
const DEFAULT_DURATION: Duration = Duration::from_secs(6);

#[component]
pub fn ToastProvider(children: Element) -> Element {
    rsx! {
        PrimitiveToastProvider {
            default_duration: Some(DEFAULT_DURATION),
            max_toasts: 4usize,
            // Bottom-centre on a phone (clear of the tab bar), bottom-
            // right on a desk.
            class: "pointer-events-none fixed inset-x-0 bottom-[calc(4.5rem+env(safe-area-inset-bottom,0px))] z-[60] flex justify-center px-3 md:inset-x-auto md:bottom-10 md:right-4 md:justify-end md:px-0 [&_ol]:flex [&_ol]:w-full [&_ol]:max-w-sm [&_ol]:flex-col [&_ol]:gap-2",
            render_toast: Callback::new(|props: ToastPropsWithOwner| {
                rsx! { {DynamicNode::Component(props.into_vcomponent(TaskToast))} }
            }),
            {children}
        }
    }
}

#[component]
fn TaskToast(props: ToastProps) -> Element {
    let (icon, tint) = match props.toast_type {
        ToastType::Success => (rsx! { CircleCheck { size: 16 } }, "text-emerald-500"),
        ToastType::Error => (rsx! { CircleAlert { size: 16 } }, "text-destructive"),
        ToastType::Warning => (rsx! { TriangleAlert { size: 16 } }, "text-amber-500"),
        ToastType::Info => (rsx! { Info { size: 16 } }, "text-muted-foreground"),
    };
    rsx! {
        PrimitiveToast {
            id: props.id,
            index: props.index,
            title: props.title,
            description: props.description,
            toast_type: props.toast_type,
            on_close: props.on_close,
            permanent: props.permanent,
            duration: props.duration,
            action: props.action,
            class: "pointer-events-auto flex w-full items-start gap-3 rounded-lg border border-border bg-popover px-3.5 py-3 text-sm text-popover-foreground shadow-[0_8px_24px_-8px_rgb(0_0_0/0.45)] outline-none focus-visible:ring-2 focus-visible:ring-ring",
            span { class: "mt-0.5 shrink-0 {tint}", aria_hidden: "true", {icon} }
            ToastContent { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                ToastTitle { class: "font-medium leading-snug" }
                ToastDescription { class: "text-muted-foreground leading-snug" }
            }
            ToastActionButton { class: "-my-1 shrink-0 rounded-md px-2.5 py-1.5 text-sm font-semibold text-primary transition-colors hover:bg-primary/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" }
            ToastCloseButton { class: "-my-1 -mr-1.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                X { size: 15 }
            }
        }
    }
}

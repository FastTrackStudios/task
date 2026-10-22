//! Task's toast host: architect-ui's `ToastProvider`, with Task's timing.
//!
//! The renderer is architect-ui's (`architect_ui::components::toast::Toast`). It
//! passes a toast's **action** through (the "Undo" after completing or
//! deleting a task, `ToastOptions::action`), so Task no longer draws its
//! own. Where the toast region sits on screen is set in `apps/tailwind.css`,
//! because architect-ui's provider takes no class for it.
//!
//! Every `use_toast()` caller (architect-ui's re-export is the same
//! primitive) lands here once this provider is mounted.

use std::time::Duration;

use dioxus::prelude::*;

/// How long a toast stays: long enough to reach for Undo.
const DEFAULT_DURATION: Duration = Duration::from_secs(6);

/// How many toasts show at once; older ones drop off.
const MAX_TOASTS: usize = 4;

#[component]
pub fn ToastProvider(children: Element) -> Element {
    rsx! {
        architect_ui::components::toast::ToastProvider {
            default_duration: Some(DEFAULT_DURATION),
            max_toasts: MAX_TOASTS,
            {children}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use architect_ui::components::toast::{ToastOptions, use_toast};
    use dioxus::dioxus_core::NoOpMutations;

    use super::*;

    /// Attributes whose values carry ids from process-wide counters.
    const ID_ATTRS: [&str; 3] = ["id=\"", "aria-labelledby=\"", "aria-describedby=\""];

    /// Raises one toast with an action while the provider first renders.
    #[component]
    fn RaiseUndo() -> Element {
        let toasts = use_toast();
        use_hook(move || {
            toasts.success(
                "Marked done".to_owned(),
                ToastOptions::new()
                    .description("Write the release notes")
                    .permanent(true)
                    .action("Undo", || {}),
            );
        });
        rsx! {}
    }

    fn app() -> Element {
        rsx! {
            ToastProvider { RaiseUndo {} }
        }
    }

    /// Replaces the digits in generated ids, so the snapshot does not
    /// depend on which tests ran first in the process.
    fn normalize_ids(html: &str) -> String {
        let mut out = String::with_capacity(html.len());
        let mut rest = html;
        while let Some(start) = ID_ATTRS
            .iter()
            .filter_map(|attr| rest.find(attr).map(|at| at + attr.len()))
            .min()
        {
            let (before, after) = rest.split_at(start);
            out.push_str(before);
            let end = after.find('"').unwrap_or(after.len());
            let (value, tail) = after.split_at(end);
            out.extend(
                value
                    .chars()
                    .map(|c| if c.is_ascii_digit() { 'N' } else { c }),
            );
            rest = tail;
        }
        out.push_str(rest);
        out
    }

    /// The toast Task shows after "Mark done": architect-ui's renderer,
    /// with the Undo action button. `TASK_UPDATE_SNAPSHOTS=1` rewrites
    /// the snapshot.
    #[test]
    fn toast_with_action_snapshot() {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        // The toast was queued during the first render. The provider's
        // list is a memo, so let the scheduler settle, then draw it.
        for _ in 0..8 {
            dom.process_events();
            dom.render_immediate(&mut NoOpMutations);
        }
        let html = normalize_ids(&dioxus_ssr::render(&dom));

        assert!(
            html.contains("data-toast-action=\"true\"") && html.contains(">Undo</button>"),
            "the toast has no Undo action button: {html}"
        );
        assert!(html.contains("Marked done") && html.contains("Write the release notes"));

        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/snapshots/toast_with_action.html");
        let pretty = html.replace("><", ">\n<") + "\n";
        if std::env::var_os("TASK_UPDATE_SNAPSHOTS").is_some() {
            std::fs::create_dir_all(path.parent().expect("snapshot dir")).expect("mkdir snapshots");
            std::fs::write(&path, &pretty).expect("write snapshot");
            return;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "no snapshot at {} ({e}); run with TASK_UPDATE_SNAPSHOTS=1",
                path.display()
            )
        });
        assert_eq!(
            pretty, want,
            "toast markup changed; re-run with TASK_UPDATE_SNAPSHOTS=1 if intended"
        );
    }
}

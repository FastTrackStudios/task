//! The conversations panel rendered to a picture through Blitz: no
//! window, no device, the paint path a dioxus-native host takes
//! (`blitz_paint` into `anyrender_vello`), at any size.
//!
//! ```text
//! cargo run -p threads-ui --features shot --example thread_shot -- OUT.png [W H]
//! ```
//!
//! `W H` in points (default 1194 834; 834 1194 upright). The panel sits
//! in a side column beside a stand-in for the host's page, themed
//! through architect-ui's tokens the way a host would theme it.

use anyrender::ImageRenderer as _;
use anyrender_vello::VelloImageRenderer;
use blitz_dom::{Document as _, DocumentConfig};
use blitz_traits::shell::{ColorScheme, Viewport};
use chrono::{Duration, Utc};
use dioxus::prelude::*;
use dioxus_native_dom::DioxusDocument;
use threads_proto::{Message, Thread};
use threads_ui::ConversationsPanel;
use uuid::Uuid;

/// Pixels per point, as on an iPad.
const SCALE: f32 = 2.0;

/// A host's palette, given through architect-ui's tokens on the wrapper.
const HOST_TOKENS: &str = "--background:#0e0e11;--foreground:#e4e4e7;--card:#141418;\
--card-foreground:#e4e4e7;--popover:#16161b;--popover-foreground:#e4e4e7;\
--primary:#2563eb;--primary-foreground:#f8fafc;--secondary:#1d1d24;\
--secondary-foreground:#e4e4e7;--muted:#1b1b21;--muted-foreground:#8b8b96;\
--accent:#1d1d24;--accent-foreground:#e4e4e7;--border:#222228;--input:#222228;\
--ring:#bfdbfe;--destructive:#ef4444;";

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "thread.png".into());
    let w: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(1194.0);
    let h: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(834.0);

    let vdom = VirtualDom::new(Shot);
    let (pw, ph) = ((w * SCALE) as u32, (h * SCALE) as u32);
    let mut document = DioxusDocument::new(
        vdom,
        DocumentConfig {
            viewport: Some(Viewport::new(pw, ph, SCALE, ColorScheme::Dark)),
            ..Default::default()
        },
    );
    document.initial_build();
    for _ in 0..12 {
        document.poll(None);
        document.inner_mut().resolve(0.0);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut renderer = VelloImageRenderer::new(pw, ph);
    let mut buffer = Vec::new();
    renderer.render_to_vec(
        |painter| {
            let mut inner = document.inner_mut();
            blitz_paint::paint_scene(painter, &mut inner, f64::from(SCALE), pw, ph, 0, 0);
        },
        &mut buffer,
    );
    image::RgbaImage::from_raw(pw, ph, buffer)
        .expect("a buffer the size of the picture")
        .save(&out)
        .expect("write the picture");
}

#[component]
fn Shot() -> Element {
    let (threads, messages) = conversation();
    let selected = Some(threads[0].id);
    rsx! {
        // Real `<style>` nodes: a headless document has no head for
        // `document::Style` to land in.
        style { {"html,body{margin:0;padding:0;height:100%;background:#0e0e11;overflow:hidden;}*{box-sizing:border-box;}"} }
        threads_ui::Styles {}
        div {
            style: "{HOST_TOKENS}width:100vw;height:100vh;display:flex;background:var(--background);color:var(--foreground);font-family:Inter,ui-sans-serif,system-ui,sans-serif;",
            // A stand-in for the host's page: the task the thread hangs off.
            div { style: "flex:1 1 0;min-width:0;padding:24px;border-right:1px solid var(--border);display:flex;flex-direction:column;gap:8px;",
                div { style: "font-size:20px;font-weight:600;", "Mix vocals for \"Northern Lights\"" }
                div { style: "font-size:13px;color:var(--muted-foreground);", "Trackone EP · In progress · due today" }
            }
            div { style: "flex:0 1 420px;min-width:0;min-height:0;padding:20px;overflow-y:scroll;",
                ConversationsPanel {
                    threads,
                    messages,
                    selected,
                    on_select: move |_| {},
                    on_new_thread: move |_| {},
                    on_post: move |_| {},
                }
            }
        }
    }
}

fn thread(n: u128, title: &str, kind: &str, resolved: bool) -> Thread {
    let at = Utc::now() - Duration::hours(i64::try_from(n).unwrap_or(0) * 7);
    Thread {
        id: Uuid::from_u128(n),
        org_id: Uuid::from_u128(0xF0),
        entity_type: "task".to_owned(),
        entity_id: Uuid::from_u128(0xE1),
        title: title.to_owned(),
        kind: kind.to_owned(),
        resolved,
        resolved_by: None,
        source_kind: "native".to_owned(),
        source_ref: None,
        source_url: None,
        created_by: Uuid::from_u128(0xC1),
        created_at: at,
        updated_at: at,
    }
}

fn message(n: u128, author: &str, minutes_ago: i64, body: &str) -> Message {
    let at = Utc::now() - Duration::minutes(minutes_ago);
    Message {
        id: Uuid::from_u128(0x100 + n),
        thread_id: Uuid::from_u128(1),
        org_id: Uuid::from_u128(0xF0),
        author_id: None,
        author_label: author.to_owned(),
        body: body.to_owned(),
        reply_to: None,
        source_kind: "native".to_owned(),
        external_id: None,
        original_text: None,
        source_url: None,
        posted_at: at,
        created_at: at,
        updated_at: at,
    }
}

fn conversation() -> (Vec<Thread>, Vec<Message>) {
    let threads = vec![
        thread(1, "Which vocal take goes on the single?", "question", false),
        thread(2, "Print stems at 48 kHz for the label", "decision", true),
        thread(
            3,
            "Rough mix notes from Friday's listen-back",
            "discussion",
            false,
        ),
    ];
    let messages = vec![
        message(
            1,
            "Alice Liddell",
            190,
            "Take 4 has the energy, but the second verse drifts sharp around 1:42. Take 6 is cleaner and a little sleepy.",
        ),
        message(
            2,
            "Bob Marley",
            150,
            "Comp it: 4 for the verses, 6 for the bridge. The doubles from 6 sit better under the chorus anyway.",
        ),
        message(
            3,
            "Cody Wright",
            42,
            "Done — comp is on the Lead Vox track, playlist \"comp A\".\nListen from bar 33 and say if the breath before the bridge should stay.",
        ),
    ];
    (threads, messages)
}

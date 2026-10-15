#![allow(clippy::large_futures)]
//! The watch's Suggested glance — `/org/{slug}/watch/v1/suggested` and
//! `…/suggested/act` — through the real router over HTTP.
//!
//! The pick is `task_proto::suggest`, the same function the Tasks page's
//! slot runs; this pins that the bridge feeds it the same world: today's
//! plan resolved with its weekday template (`scheduling_proto::resolve_day`),
//! the wearer's own tasks, the running timer and the inbox's due count,
//! at the date and minute the WATCH sends.
//!
//! 1. A plan block with a task assigned → `planned`; acting starts a
//!    timer on it and moves it in-progress, and it is not suggested again.
//! 2. A maintenance block (from the weekday template — no saved plan
//!    holds it) → `mode`; acting opens a session named "Maintenance hour".
//! 3. Nothing planned → the order falls through: overdue, then captures
//!    to process (declined keys skipped), and Done completes a task.

use scheduling_proto::{
    BlockAssignment, BlockCategory, DayPlan, DayPlans as _, DayTemplate, DayTemplateId,
    DayTemplates as _, PlannedBlock, TimeBlock, TimeBlockId, TimeOfDay,
};
use serde_json::{Value, json};
use task::TaskService as _;
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

const SLUG: &str = "watch-suggest";
const DEVICE_TOKEN: &str = "watch-suggested-test-device-token";
/// A Monday: the weekday template applies.
const DATE: &str = "2026-10-12";

async fn boot() -> eyre::Result<(String, AppState, uuid::Uuid, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let keypair = ServerKeypair::generate_ephemeral();
    let tmp = tempfile::tempdir()?;
    let guard = crate::support::env_lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_WATCH_TOKEN", DEVICE_TOKEN);
    }
    let root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    root.init_org(SLUG, "Watch Suggest", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let org_id = root
        .org(SLUG)
        .manifest()
        .map_err(|e| eyre::eyre!("manifest: {e}"))?
        .id;
    let state = AppState::new_with_auth(auth, keypair).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // The device token's identity is the deterministic local owner.
    let me = timer_proto::local_owner_id(org_id);
    Ok((format!("http://127.0.0.1:{port}"), state, me, tmp))
}

fn tod(h: u16) -> TimeOfDay {
    TimeOfDay {
        minutes_since_midnight: h * 60,
    }
}

/// A task assigned to `me` — the Tasks list's "mine".
fn my_task(title: &str, me: uuid::Uuid, due: Option<&str>) -> task::TaskInfo {
    let mut t = task::capture(title);
    t.title = title.into();
    t.status = "open".into();
    t.due = due.map(Into::into);
    t.workflow = Some(task::model::WorkflowAttrs {
        assignees: vec![task::workflows_proto::AgentRef::human(me.to_string())].into(),
        ..Default::default()
    });
    t
}

#[tokio::test(flavor = "multi_thread")]
// t[verify process.suggested]
async fn the_watch_suggests_and_acts_on_the_same_pick() -> eyre::Result<()> {
    let (base, state, me, _root) = boot().await?;
    let org = state.org(SLUG).ok_or_else(|| eyre::eyre!("org hosted"))?;

    let planned = org.tasks.create(my_task("Comp lead vocals", me, None))?;
    let late = org
        .tasks
        .create(my_task("Mix revisions", me, Some("2026-09-24")))?;
    // Someone else's late task never reaches my wrist.
    let mut theirs = task::capture("Their overdue thing");
    theirs.title = "Their overdue thing".into();
    theirs.due = Some("2026-09-01".into());
    org.tasks.create(theirs)?;

    // The weekday template holds the maintenance hour; the saved plan for
    // DATE is sparse (only the studio block), so the hour comes from the
    // template merge.
    org.scheduling.upsert_day_template(&DayTemplate {
        path: String::new(),
        id: DayTemplateId("weekday".into()),
        name: "Weekday".into(),
        description: None,
        blocks: scheduling_proto::time_block::TimeBlocks(vec![TimeBlock {
            id: TimeBlockId("errands".into()),
            start: tod(14),
            end: tod(15),
            label: "Errands & chores".into(),
            category: BlockCategory::Maintenance,
            note: None,
        }]),
    })?;
    org.scheduling.upsert_day_plan(&DayPlan {
        date: DATE.into(),
        from_template: None,
        blocks: scheduling_proto::day_plan::PlannedBlocks(vec![PlannedBlock {
            id: TimeBlockId("studio".into()),
            start: tod(9),
            end: tod(12),
            label: "Studio block".into(),
            category: BlockCategory::Allocatable,
            note: None,
            assignment: Some(BlockAssignment {
                kind: "task".into(),
                title: planned.title.clone(),
                ref_id: Some(planned.id.to_string()),
            }),
            fixed: false,
        }]),
    })?;

    let http = reqwest::Client::new();
    let get = |minute: u16, declined: &str| {
        let http = http.clone();
        let url = format!(
            "{base}/org/{SLUG}/watch/v1/suggested?date={DATE}&minute={minute}&declined={declined}"
        );
        async move {
            let res = http.get(&url).bearer_auth(DEVICE_TOKEN).send().await?;
            eyre::ensure!(res.status().is_success(), "GET {url}: {}", res.status());
            Ok::<Value, eyre::Report>(res.json().await?)
        }
    };
    let act = |key: &str, minute: u16| {
        let http = http.clone();
        let url = format!("{base}/org/{SLUG}/watch/v1/suggested/act");
        let body = json!({ "key": key, "date": DATE, "minute": minute });
        async move {
            let res = http
                .post(&url)
                .bearer_auth(DEVICE_TOKEN)
                .json(&body)
                .send()
                .await?;
            Ok::<(reqwest::StatusCode, Value), eyre::Report>((
                res.status(),
                res.json().await.unwrap_or(Value::Null),
            ))
        }
    };

    // Unauthenticated → 401, like every bridge route.
    let res = http
        .get(format!(
            "{base}/org/{SLUG}/watch/v1/suggested?date={DATE}&minute=600"
        ))
        .send()
        .await?;
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 1. 10:00 — the calendar planned a task for now.
    let s = get(600, "").await?;
    assert_eq!(s["kind"], "planned", "{s}");
    assert_eq!(s["title"], "Comp lead vocals");
    assert_eq!(s["action"], "Start");
    assert_eq!(s["task_id"], planned.id.to_string());
    assert_eq!(s["key"], format!("planned:{}", planned.id));
    assert!(s["eyebrow"].is_string() && s["detail"].is_string(), "{s}");

    // 2. 14:30 — the template's maintenance hour steers.
    let s = get(14 * 60 + 30, "").await?;
    assert_eq!(s["kind"], "mode", "{s}");
    assert_eq!(s["mode_title"], "Maintenance hour");
    let key = s["key"].as_str().unwrap_or_default().to_owned();
    assert_eq!(key, "mode:Errands & chores");
    let (status, out) = act(&key, 14 * 60 + 30).await?;
    assert_eq!(status, reqwest::StatusCode::OK, "{out}");
    assert_eq!(out["timer"]["description"], "Maintenance hour", "{out}");
    assert_eq!(out["timer"]["running"], true);
    let active: Value = http
        .get(format!("{base}/org/{SLUG}/watch/v1/timer/active"))
        .bearer_auth(DEVICE_TOKEN)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(
        active["description"], "Maintenance hour",
        "the open session IS the mode"
    );

    // A key that no longer applies is refused, not guessed at.
    let (status, _) = act(&key, 20 * 60).await?;
    assert_eq!(status, reqwest::StatusCode::CONFLICT);

    // Acting on the planned task: a timer on it (switching from the
    // mode), and it goes in-progress.
    let (status, out) = act(&format!("planned:{}", planned.id), 600).await?;
    assert_eq!(status, reqwest::StatusCode::OK, "{out}");
    assert_eq!(out["timer"]["description"], "Comp lead vocals");
    assert_eq!(org.tasks.get(planned.id)?.status, "in-progress");
    // Already running, so neither "planned" nor "continue" for it.
    let s = get(600, "").await?;
    assert_ne!(s["task_id"], planned.id.to_string(), "{s}");

    // 3. 20:00 — nothing planned: the oldest overdue task of mine.
    let s = get(20 * 60, "").await?;
    assert_eq!(s["kind"], "overdue", "{s}");
    assert_eq!(s["title"], "Mix revisions");
    assert_eq!(s["action"], "Done");

    // A capture waiting; "Not now" on the overdue task shows it next.
    let res = http
        .post(format!("{base}/org/{SLUG}/watch/v1/inbox"))
        .bearer_auth(DEVICE_TOKEN)
        .json(&json!({ "body": "call the venue" }))
        .send()
        .await?;
    assert_eq!(res.status(), reqwest::StatusCode::CREATED);
    let declined = format!("overdue:{}", late.id);
    let s = get(20 * 60, &declined).await?;
    assert_eq!(s["kind"], "process", "{s}");
    assert_eq!(s["title"], "Process 1 capture");
    let (status, _) = act("process", 20 * 60).await?;
    assert_eq!(
        status,
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "processing is the phone's"
    );

    // Done on the overdue task completes it; what's next is the capture.
    let (status, out) = act(&declined, 20 * 60).await?;
    assert_eq!(status, reqwest::StatusCode::OK, "{out}");
    assert!(out["timer"].is_null());
    assert_eq!(out["next"]["kind"], "process", "{out}");
    let done = org.tasks.get(late.id)?;
    assert_eq!(done.status, "done");
    assert!(done.completed_date.is_some());

    // Bad input is a 400, not a guess.
    let res = http
        .get(format!(
            "{base}/org/{SLUG}/watch/v1/suggested?date=today&minute=600"
        ))
        .bearer_auth(DEVICE_TOKEN)
        .send()
        .await?;
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);
    Ok(())
}

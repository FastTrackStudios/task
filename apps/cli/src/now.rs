//! "Now" for the terminal — the same situation the app assembles, so
//! `task task list --relevant` / `--due` and `task brief` give the one
//! answer Home, the Tasks page and the Work sidebar give
//! (`process.focus.everywhere`, `docs/spec/capture-and-process.md`).
//!
//! The pieces are the app's: the local clock and date, where the person
//! says they are (`--location`), what they're on (`--device`, a computer
//! unless told otherwise — this is a terminal), and their open timer
//! session from the same timer store `task timer` writes. That session's
//! project ranks first and, when it is a mode ("Maintenance hour"), the
//! mode's focus opens its gates — `task_proto::RelevanceContext::now`
//! and `with_session`, the one builder every client calls.

/// What a terminal is, for `@computer` / `@phone` gates.
pub(crate) const DEVICE: &str = "computer";

/// The relevance context for now. `at` overrides the clock (`HH:MM`);
/// a timer store that can't be read leaves the session out (and says
/// so on stderr) rather than failing the read.
pub(crate) async fn context(
    org: Option<&str>,
    at: Option<String>,
    location: Option<&str>,
    device: Option<&str>,
) -> task::RelevanceContext {
    let now = chrono::Local::now();
    let ctx = task::RelevanceContext::now(
        now.format("%Y-%m-%d").to_string(),
        Some(at.unwrap_or_else(|| now.format("%H:%M").to_string())),
        location,
        Some(device.unwrap_or(DEVICE)),
    );
    match open_session(org).await {
        Ok(Some(s)) => ctx.with_session(&s.description, s.project_id),
        Ok(None) => ctx,
        Err(e) => {
            eprintln!("warning: timer: {e} — reading now without the running session");
            ctx
        }
    }
}

/// The person's open timer session, if one is running.
pub(crate) async fn open_session(
    org: Option<&str>,
) -> eyre::Result<Option<timer_proto::WorkSession>> {
    use timer_proto::service::TimerService;
    let t = crate::plan::timer_ctx(org).await?;
    t.store
        .active_timer(t.user_id)
        .await
        .map_err(|e| eyre::eyre!("{e}"))
}

/// Who is signed in on `url` for `slug` — the user id whose Tasks list
/// the due queue is (`task_proto::is_mine`). `None` when nobody is,
/// which the queue reads as next actions, as the app does signed out.
pub(crate) fn signed_in_user(url: &str, slug: &str) -> Option<String> {
    let s = crate::session_store::load().ok().flatten()?;
    s.entry_for_server(url)
        .map(|(_, e)| e)
        .filter(|e| e.slug.is_empty() || e.slug == slug)
        .or_else(|| s.servers.values().find(|e| e.slug == slug))
        .map(|e| e.user_id.to_string())
}

/// The due queue over the wire (`TaskListFilter::due_now`), or — from a
/// server that predates it — the same function run here on `list()`.
pub(crate) async fn due_queue(
    client: &task::TaskServiceClient,
    me: Option<String>,
    ctx: &task::RelevanceContext,
) -> eyre::Result<Vec<task::TaskInfo>> {
    let filter = task::TaskListFilter {
        relevance: Some(ctx.clone()),
        due_now: true,
        me: me.clone(),
        ..Default::default()
    };
    match client.query(filter).await {
        Ok(rows) => Ok(rows),
        Err(e) => {
            eprintln!(
                "warning: server-side due query failed ({e:?}); computing the due queue here \
                 (is task-server stale? run `task doctor`)"
            );
            let all = client
                .list()
                .await
                .map_err(|e| eyre::eyre!("list: {e:?}"))?;
            Ok(task::due_queue(&all, me.as_deref(), ctx))
        }
    }
}

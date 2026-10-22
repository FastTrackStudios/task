//! Recent projects — the short list the sidebar's Projects section
//! shows, instead of every project there is.
//!
//! The sidebar is for getting back to the work in hand. The Projects
//! page (`/projects`) keeps the full list, subprojects and all; the
//! sidebar keeps a handful ([`SIDEBAR_LIMIT`]) of **top-level**
//! projects, ranked:
//!
//! 1. starred first (the ☆ on a project's top bar — [`crate::favorites`]
//!    is the pinning there is);
//! 2. then the person's most recent activity: opening or editing the
//!    project on this device, or creating / finishing a task in it;
//! 3. then how many open tasks in it are assigned to them;
//! 4. then, to fill the list, the project most recently changed by
//!    anyone, then title.
//!
//! A subproject never gets a row of its own. Its activity, its tasks and
//! its star count toward its **top ancestor** (an album's songs light up
//! the album), so a band with an album of ten songs sees the album once.
//! Done, cancelled, on-hold, stale and archived projects are left out —
//! the same "active" rule the rest of the shell uses
//! ([`crate::task_sort::is_active`]).
//!
//! # Where recency lives
//!
//! Per device, per signed-in person, in [`crate::local_store`] — the same
//! place the favorites and the nav layout live. Under ADR 0006 (every
//! device a peer, no central "user state" server) this is the simplest
//! option that stays consistent: "I opened it" is a fact about this
//! device and is never merged, so there is nothing to conflict. The
//! signals that should follow the person between devices already do,
//! because they are read from the replicated task data itself — who
//! created a task (`createdBy`), who it is assigned to, when it was
//! finished. FUTURE: move opens into the account's prefs alongside the
//! favorites, so a phone and a desk share one list.

use std::collections::{BTreeMap, HashMap, HashSet};

use dioxus::prelude::*;
use project_proto::ProjectInfo;
use task_proto::TaskInfo;
use uuid::Uuid;

use crate::task_sort::is_active;

/// How many projects the sidebar lists before "All projects →".
pub const SIDEBAR_LIMIT: usize = 7;

/// How many opens to remember per person (oldest forgotten first).
const KEEP: usize = 50;

const KEY: &str = "task.recent_projects";

// ── ranking (pure) ───────────────────────────────────────────────────

/// What each project's row needs to know to be ranked. Keyed by any
/// project id — a subproject's entries roll up into its top ancestor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signals {
    /// Starred projects.
    pub pinned: HashSet<Uuid>,
    /// The person's latest activity in a project, Unix milliseconds.
    pub touched: HashMap<Uuid, i64>,
    /// Open tasks in a project assigned to the person.
    pub open_mine: HashMap<Uuid, usize>,
}

/// One row of the sidebar's list.
#[derive(Clone, Debug, PartialEq)]
pub struct Ranked {
    pub id: Uuid,
    pub title: String,
    /// Starred, itself or through one of its subprojects.
    pub pinned: bool,
    /// The person's latest activity anywhere in its tree (ms).
    pub last: Option<i64>,
    /// Open tasks in its tree assigned to the person.
    pub open_mine: usize,
}

/// Every project's top ancestor: itself for a root, else the highest
/// project up its `parentId` chain. A parent that is not in `projects`
/// (another org's, not loaded yet) ends the climb, so an orphaned
/// subproject stands in for its missing root rather than vanishing.
/// Cycles end the climb too.
#[must_use]
pub fn top_ancestors(projects: &[&ProjectInfo]) -> HashMap<Uuid, Uuid> {
    let parent: HashMap<Uuid, Option<Uuid>> =
        projects.iter().map(|p| (p.id, p.parent_id)).collect();
    projects
        .iter()
        .map(|p| {
            let mut at = p.id;
            let mut seen = HashSet::from([at]);
            while let Some(Some(up)) = parent.get(&at) {
                if !parent.contains_key(up) || !seen.insert(*up) {
                    break;
                }
                at = *up;
            }
            (p.id, at)
        })
        .collect()
}

/// Whether a project can have a sidebar row of its own: a root (see
/// [`top_ancestors`]) that is still active and not archived.
#[must_use]
pub fn is_listed(p: &ProjectInfo, tops: &HashMap<Uuid, Uuid>) -> bool {
    tops.get(&p.id) == Some(&p.id) && !p.archived && is_active(&p.status)
}

/// The person's activity signals read from the (replicated) task list:
/// creating a task or finishing one assigned to them touches its
/// project; an open one assigned to them counts toward it. `project_of`
/// files a task under a project (see `task_board::ProjectIndex`).
#[must_use]
pub fn task_signals(
    tasks: &[&TaskInfo],
    me: Option<&str>,
    project_of: impl Fn(&TaskInfo) -> Option<Uuid>,
) -> (HashMap<Uuid, i64>, HashMap<Uuid, usize>) {
    let mut touched: HashMap<Uuid, i64> = HashMap::new();
    let mut open: HashMap<Uuid, usize> = HashMap::new();
    let Some(me) = me else {
        return (touched, open);
    };
    let creator = format!("user:{me}");
    for t in tasks {
        let t: &TaskInfo = t;
        let Some(pid) = project_of(t) else { continue };
        let mut bump = |ms: i64| {
            let e = touched.entry(pid).or_insert(ms);
            *e = (*e).max(ms);
        };
        if t.created_by.as_deref() == Some(creator.as_str())
            && let Some(at) = t.date_created
        {
            bump(at.timestamp_millis());
        }
        let assigned = task_proto::is_assigned_to(t, me);
        if assigned && let Some(day) = t.completed_date {
            bump(
                day.and_hms_opt(0, 0, 0)
                    .map_or(0, |d| d.and_utc().timestamp_millis()),
            );
        }
        if assigned && task_proto::status_is_open(&t.status) {
            *open.entry(pid).or_default() += 1;
        }
    }
    (touched, open)
}

/// The sidebar's list: listed roots ([`is_listed`]), each carrying its
/// whole tree's signals, best first (see the module docs), at most
/// `limit` of them.
#[must_use]
pub fn rank(projects: &[&ProjectInfo], signals: &Signals, limit: usize) -> Vec<Ranked> {
    let tops = top_ancestors(projects);
    let mut rows: BTreeMap<Uuid, (Ranked, Option<i64>)> = projects
        .iter()
        .filter(|p| is_listed(p, &tops))
        .map(|p| {
            let row = Ranked {
                id: p.id,
                title: p.title.clone(),
                pinned: false,
                last: None,
                open_mine: 0,
            };
            (p.id, (row, None))
        })
        .collect();
    for p in projects {
        let Some((row, changed)) = tops.get(&p.id).and_then(|top| rows.get_mut(top)) else {
            continue;
        };
        row.pinned |= signals.pinned.contains(&p.id);
        if let Some(&ms) = signals.touched.get(&p.id) {
            row.last = Some(row.last.map_or(ms, |l| l.max(ms)));
        }
        row.open_mine += signals.open_mine.get(&p.id).copied().unwrap_or(0);
        if let Some(at) = p.date_modified {
            let ms = at.timestamp_millis();
            *changed = Some(changed.map_or(ms, |c: i64| c.max(ms)));
        }
    }
    let mut rows: Vec<(Ranked, Option<i64>)> = rows.into_values().collect();
    rows.sort_by(|(a, ac), (b, bc)| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.last.cmp(&a.last))
            .then_with(|| b.open_mine.cmp(&a.open_mine))
            .then_with(|| bc.cmp(ac))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    rows.into_iter().take(limit).map(|(r, _)| r).collect()
}

// ── the device's memory of opens ─────────────────────────────────────

/// Person → project → last open/edit on this device (ms).
type Opens = BTreeMap<String, BTreeMap<String, i64>>;

fn load() -> Opens {
    crate::local_store::get(KEY)
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save(opens: &Opens) {
    if let Ok(j) = serde_json::to_string(opens) {
        crate::local_store::set(KEY, &j);
    }
}

/// Record that `person` used `project` at `ms`, forgetting the oldest
/// beyond [`KEEP`].
fn record(opens: &mut Opens, person: &str, project: &str, ms: i64) {
    let mine = opens.entry(person.to_owned()).or_default();
    mine.insert(project.to_owned(), ms);
    while mine.len() > KEEP {
        let oldest = mine
            .iter()
            .min_by_key(|(_, at)| **at)
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => mine.remove(&k),
            None => break,
        };
    }
}

/// The shell's shared memory of opens, so a touch anywhere reorders the
/// sidebar at once.
#[derive(Clone, Copy)]
pub struct RecentProjects(pub Signal<Opens>);

/// Install the [`RecentProjects`] context. Call once in the app shell.
pub fn provide_recent_projects() {
    use_context_provider(|| RecentProjects(Signal::new(load())));
}

fn person() -> String {
    try_consume_context::<crate::auth::AuthCtx>()
        .and_then(|a| a.active.peek().as_ref().map(|a| a.user_id.to_string()))
        .unwrap_or_default()
}

/// The signed-in person opened or edited `project` just now. A no-op
/// outside the shell (no context).
pub fn touch(project: Uuid) {
    let Some(RecentProjects(mut opens)) = try_consume_context::<RecentProjects>() else {
        return;
    };
    let me = person();
    let now = chrono::Utc::now().timestamp_millis();
    let mut next = opens.peek().clone();
    record(&mut next, &me, &project.to_string(), now);
    save(&next);
    opens.set(next);
}

/// The sidebar's ranked projects (see [`rank`]), live, plus how many
/// projects could be listed at all — what the Projects row counts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarProjects {
    pub rows: Vec<Ranked>,
    pub listed: usize,
    /// Every project's top ancestor, for rolling a task's count up.
    pub tops: HashMap<Uuid, Uuid>,
}

/// [`SidebarProjects`] from the shared stores, the favorites, this
/// device's opens and the signed-in person.
pub fn use_sidebar_projects(limit: usize) -> Memo<SidebarProjects> {
    let task_store = crate::stores::use_task_store();
    let project_store = crate::stores::use_project_store();
    let favorites = use_context::<crate::favorites::Favorites>().0;
    let opens = use_context::<RecentProjects>().0;
    let account = use_context::<crate::auth::AuthCtx>().active;
    use_memo(move || {
        let me = account.read().as_ref().map(|a| a.user_id.to_string());
        let rows = project_store.list();
        let projects: Vec<&ProjectInfo> = rows.iter().map(|r| &r.project).collect();
        let index =
            crate::task_board::ProjectIndex::new(projects.iter().map(|p| (p.id, p.title.clone())));
        let entries = task_store.entries();
        let tasks: Vec<&TaskInfo> = crate::stores::unique_tasks(&entries)
            .into_iter()
            .map(|r| &r.task)
            .collect();
        let (mut touched, open_mine) =
            task_signals(&tasks, me.as_deref(), |t| match index.anchor(t) {
                Some(task_proto::Anchor::Project(p)) => Some(p),
                _ => None,
            });
        if let Some(mine) = opens.read().get(me.as_deref().unwrap_or_default()) {
            for (pid, &ms) in mine {
                if let Ok(pid) = pid.parse::<Uuid>() {
                    let e = touched.entry(pid).or_insert(ms);
                    *e = (*e).max(ms);
                }
            }
        }
        let pinned = favorites
            .read()
            .iter()
            .filter(|f| f.kind == "project")
            .filter_map(|f| f.id.parse().ok())
            .collect();
        let signals = Signals {
            pinned,
            touched,
            open_mine,
        };
        let tops = top_ancestors(&projects);
        let listed = projects.iter().filter(|p| is_listed(p, &tops)).count();
        SidebarProjects {
            rows: rank(&projects, &signals, limit),
            listed,
            tops,
        }
    })
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDate, Utc};

    use super::*;

    fn project(title: &str, parent: Option<Uuid>) -> ProjectInfo {
        ProjectInfo {
            id: Uuid::new_v4(),
            title: title.into(),
            status: "active".into(),
            parent_id: parent,
            ..ProjectInfo::default()
        }
    }

    fn at(ms: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(ms).unwrap()
    }

    /// Crescendum and its ten songs, an older project, a cancelled one,
    /// a finished one, an archived one.
    struct Seed {
        all: Vec<ProjectInfo>,
        crescendum: Uuid,
        songs: Vec<Uuid>,
        older: Uuid,
        cancelled: Uuid,
    }

    fn seed() -> Seed {
        let crescendum = project("Crescendum", None);
        let songs: Vec<ProjectInfo> = (1..=10)
            .map(|n| project(&format!("Song {n}"), Some(crescendum.id)))
            .collect();
        let mut older = project("Older Project", None);
        older.date_modified = Some(at(1_000));
        let mut cancelled = project("Cancelled Project", None);
        cancelled.status = "cancelled".into();
        let mut done = project("Done Project", None);
        done.status = "done".into();
        let mut archived = project("Archived Project", None);
        archived.archived = true;
        let (c, o, x) = (crescendum.id, older.id, cancelled.id);
        let song_ids = songs.iter().map(|s| s.id).collect();
        let mut all = vec![crescendum, older, cancelled, done, archived];
        all.extend(songs);
        Seed {
            all,
            crescendum: c,
            songs: song_ids,
            older: o,
            cancelled: x,
        }
    }

    fn refs(all: &[ProjectInfo]) -> Vec<&ProjectInfo> {
        all.iter().collect()
    }

    #[test]
    fn a_song_rolls_up_to_its_album_and_the_album_is_listed_once() {
        let s = seed();
        let all = refs(&s.all);
        let tops = top_ancestors(&all);
        for song in &s.songs {
            assert_eq!(tops[song], s.crescendum, "a song's top is Crescendum");
        }
        // Activity in one song only — yesterday's work on Song 7.
        let mut signals = Signals::default();
        signals.touched.insert(s.songs[6], 5_000);
        signals.open_mine.insert(s.songs[2], 2);
        signals.open_mine.insert(s.songs[3], 1);
        let rows = rank(&all, &signals, SIDEBAR_LIMIT);
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![s.crescendum, s.older],
            "the album, then the older one"
        );
        assert_eq!(rows[0].last, Some(5_000));
        assert_eq!(
            rows[0].open_mine, 3,
            "the songs' open tasks are the album's"
        );
        assert!(
            ids.iter().all(|id| !s.songs.contains(id)),
            "no song gets a row"
        );
        assert!(!ids.contains(&s.cancelled));
        assert_eq!(
            rows.iter().filter(|r| r.title == "Crescendum").count(),
            1,
            "Crescendum appears exactly once"
        );
    }

    #[test]
    fn starred_then_recent_then_assigned_then_recently_changed() {
        let s = seed();
        let mut all = s.all.clone();
        let mut b = project("B Assigned", None);
        b.date_modified = Some(at(9_000));
        let mut a = project("A Untouched", None);
        a.date_modified = Some(at(8_000));
        let pinned = project("Z Starred", None);
        let (a, b, z) = {
            let ids = (a.id, b.id, pinned.id);
            all.extend([a, b, pinned]);
            ids
        };
        let mut signals = Signals::default();
        signals.pinned.insert(z);
        signals.touched.insert(s.older, 2_000); // opened a while ago
        signals.touched.insert(s.songs[0], 7_000); // Crescendum, recently
        signals.open_mine.insert(b, 4);
        let rows = rank(&refs(&all), &signals, SIDEBAR_LIMIT);
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![z, s.crescendum, s.older, b, a]);
        assert!(rows[0].pinned);
    }

    #[test]
    fn a_starred_song_pins_its_album() {
        let s = seed();
        let mut signals = Signals::default();
        signals.pinned.insert(s.songs[9]);
        signals.touched.insert(s.older, 9_999);
        let rows = rank(&refs(&s.all), &signals, SIDEBAR_LIMIT);
        assert_eq!(rows[0].id, s.crescendum);
        assert!(rows[0].pinned);
    }

    #[test]
    fn the_list_is_short() {
        let all: Vec<ProjectInfo> = (0..20)
            .map(|n| project(&format!("P{n:02}"), None))
            .collect();
        let rows = rank(&refs(&all), &Signals::default(), SIDEBAR_LIMIT);
        assert_eq!(rows.len(), SIDEBAR_LIMIT);
        assert_eq!(rows[0].title, "P00", "no signal at all: by title");
    }

    #[test]
    fn closed_and_archived_roots_hide_their_whole_tree() {
        let s = seed();
        let mut all = s.all.clone();
        let orphan_parent = Uuid::new_v4(); // not loaded
        all.push(project("Orphan", Some(orphan_parent)));
        let under_cancelled = project("Under Cancelled", Some(s.cancelled));
        let hidden = under_cancelled.id;
        all.push(under_cancelled);
        let mut signals = Signals::default();
        signals.touched.insert(hidden, 99_999);
        let rows = rank(&refs(&all), &signals, SIDEBAR_LIMIT);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        // No signals left: the one changed most recently, then by title.
        assert_eq!(titles, vec!["Older Project", "Crescendum", "Orphan"]);
    }

    #[test]
    fn a_cycle_does_not_hang() {
        let mut a = project("A", None);
        let b = project("B", Some(a.id));
        a.parent_id = Some(b.id);
        let all = vec![a, b];
        let tops = top_ancestors(&refs(&all));
        assert_eq!(tops.len(), 2);
    }

    #[test]
    fn task_signals_read_who_did_what() {
        let me = "11111111-1111-1111-1111-111111111111";
        let song = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut created = TaskInfo::new("made by me");
        created.project_id = Some(song);
        created.created_by = Some(format!("user:{me}"));
        created.date_created = Some(at(3_000));
        let mut theirs = TaskInfo::new("made by someone else");
        theirs.project_id = Some(other);
        theirs.created_by = Some("user:someone".into());
        theirs.date_created = Some(at(9_000));
        let mut wf = task_proto::model::WorkflowAttrs::default();
        wf.assignees
            .0
            .push(task_proto::workflows_proto::AgentRef::human(me));
        let mut open = TaskInfo::new("mine, open");
        open.project_id = Some(other);
        open.status = "open".into();
        open.workflow = Some(wf.clone());
        let mut finished = TaskInfo::new("mine, finished");
        finished.project_id = Some(song);
        finished.status = "done".into();
        finished.completed_date = NaiveDate::from_ymd_opt(2026, 10, 20);
        finished.workflow = Some(wf);
        let tasks = [&created, &theirs, &open, &finished];
        let (touched, opens) = task_signals(&tasks, Some(me), |t| t.project_id);
        let finished_ms = NaiveDate::from_ymd_opt(2026, 10, 20)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(
            touched.get(&song),
            Some(&finished_ms),
            "the later of the two"
        );
        assert_eq!(
            touched.get(&other),
            None,
            "someone else's create isn't mine"
        );
        assert_eq!(opens.get(&other), Some(&1));
        assert_eq!(opens.get(&song), None, "finished work isn't open");
        let (t, o) = task_signals(&tasks, None, |t| t.project_id);
        assert!(t.is_empty() && o.is_empty(), "signed out: no signals");
    }

    #[test]
    fn the_device_forgets_the_oldest_opens() {
        let mut opens = Opens::new();
        for n in 0..(KEEP as i64 + 5) {
            record(&mut opens, "me", &format!("p{n}"), n);
        }
        record(&mut opens, "you", "p0", 1);
        assert_eq!(opens["me"].len(), KEEP);
        assert!(!opens["me"].contains_key("p0"), "the oldest went first");
        assert!(opens["me"].contains_key(&format!("p{}", KEEP + 4)));
        assert_eq!(opens["you"].len(), 1, "per person");
    }
}

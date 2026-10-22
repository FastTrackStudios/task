//! The subtree resolver: pure tests over [`OwnerState`], and store tests
//! over real project and task backends on disk.

use super::*;

fn v(kind: &str, id: &str, label: &str) -> VisibleItem {
    VisibleItem {
        kind: kind.into(),
        id: id.into(),
        label: label.into(),
    }
}

#[test]
fn the_digest_is_the_set_not_its_order_or_labels() {
    let root = Uuid::new_v4();
    let a = [v("project", "1", "Crescendum"), v("part", "2", "Song A")];
    let b = [
        v("part", "2", "Song A (renamed)"),
        v("project", "1", "Crescendum"),
    ];
    assert_eq!(digest(root, &a), digest(root, &b));
    assert_eq!(digest(root, &a).len(), 64);
    let more = [a[0].clone(), a[1].clone(), v("task", "3", "Mix")];
    assert_ne!(
        digest(root, &a),
        digest(root, &more),
        "a new object changes it"
    );
    assert_ne!(
        digest(root, &a),
        digest(Uuid::new_v4(), &a),
        "so does the root"
    );
}

// ── pure ─────────────────────────────────────────────────────────────

struct World {
    state: OwnerState,
}

impl World {
    fn new() -> Self {
        Self {
            state: OwnerState {
                projects_dir: Some(PathBuf::from("/org/projects")),
                ..OwnerState::default()
            },
        }
    }

    fn project(&mut self, title: &str, parent: Option<Uuid>, page: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.state.projects.push(ProjectNode {
            id,
            parent_id: parent,
            title: title.into(),
            page: page.into(),
            parts: Vec::new(),
        });
        id
    }

    fn part(&mut self, project: Uuid, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        let p = self
            .state
            .projects
            .iter_mut()
            .find(|p| p.id == project)
            .unwrap();
        p.parts.push((id, name.into()));
        id
    }

    fn task(&mut self, title: &str, project: Option<Uuid>) -> Uuid {
        let id = Uuid::new_v4();
        self.state.tasks.push(Attached {
            id,
            project_id: project,
            title: title.into(),
        });
        id
    }

    fn root(&mut self, name: &str, tree: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.state.roots.push(RootNode {
            id,
            name: name.into(),
            tree: Some(PathBuf::from(tree)),
        });
        id
    }

    fn scope(&self, root: Uuid) -> InScope {
        resolve(&self.state, root).expect("root exists")
    }
}

/// Parent → Crescendum → Song → Stems → Takes (depth 3 below the root),
/// with a sibling album beside Crescendum.
struct Tree {
    w: World,
    parent: Uuid,
    root: Uuid,
    song: Uuid,
    stems: Uuid,
    takes: Uuid,
    sibling: Uuid,
}

fn tree() -> Tree {
    let mut w = World::new();
    let parent = w.project("Label", None, "label/project.md");
    let root = w.project("Crescendum", Some(parent), "label/crescendum/project.md");
    let song = w.project(
        "Song One",
        Some(root),
        "label/crescendum/song-one/project.md",
    );
    let stems = w.project(
        "Stems",
        Some(song),
        "label/crescendum/song-one/stems/project.md",
    );
    let takes = w.project(
        "Takes",
        Some(stems),
        "label/crescendum/song-one/stems/takes/project.md",
    );
    let sibling = w.project("Other Album", Some(parent), "label/other-album/project.md");
    Tree {
        w,
        parent,
        root,
        song,
        stems,
        takes,
        sibling,
    }
}

#[test]
fn nested_sub_projects_at_depth_three_are_in_scope_parent_and_sibling_are_not() {
    let mut t = tree();
    let parent_task = t.w.task("Label budget", Some(t.parent));
    let sibling_task = t.w.task("Mix the other album", Some(t.sibling));
    let deep_task = t.w.task("Comp the takes", Some(t.takes));
    let s = t.w.scope(t.root);

    for id in [t.root, t.song, t.stems, t.takes] {
        assert!(
            s.in_scope(&ObjectRef::Project(id)),
            "{id} is in the subtree"
        );
    }
    assert!(!s.in_scope(&ObjectRef::Project(t.parent)), "parent is out");
    assert!(
        !s.in_scope(&ObjectRef::Project(t.sibling)),
        "sibling is out"
    );
    assert!(s.in_scope(&ObjectRef::Task(deep_task)));
    assert!(!s.in_scope(&ObjectRef::Task(parent_task)), "parent's task");
    assert!(
        !s.in_scope(&ObjectRef::Task(sibling_task)),
        "sibling's task"
    );

    // Notes follow the directories, and stop at them.
    assert!(s.in_scope(&ObjectRef::Note("label/crescendum/notes.md".into())));
    assert!(s.in_scope(&ObjectRef::Note(
        "label/crescendum/song-one/stems/takes/log.md".into()
    )));
    assert!(
        !s.in_scope(&ObjectRef::Note("label/notes.md".into())),
        "parent dir"
    );
    assert!(!s.in_scope(&ObjectRef::Note("label/other-album/x.md".into())));
    assert!(
        !s.in_scope(&ObjectRef::Note("label/crescendum-old/x.md".into())),
        "a prefix match is per component, not per byte"
    );
    assert!(
        !s.in_scope(&ObjectRef::Note("label/crescendum/../budget.md".into())),
        "a path that climbs out is never in"
    );
}

#[test]
fn a_grant_on_a_sub_project_reaches_down_never_up() {
    let t = tree();
    let s = t.w.scope(t.stems);
    assert!(s.in_scope(&ObjectRef::Project(t.takes)));
    assert!(!s.in_scope(&ObjectRef::Project(t.song)), "its parent");
    assert!(!s.in_scope(&ObjectRef::Project(t.root)), "its grandparent");
    assert!(!s.in_scope(&ObjectRef::Note(
        "label/crescendum/song-one/lyrics.md".into()
    )));
}

#[test]
fn a_sub_project_or_song_added_after_the_grant_is_in_with_no_regrant() {
    let mut t = tree();
    let before = t.w.scope(t.root);
    let d_before = digest(t.root, &before.visible());

    let song_two = t.w.project(
        "Song Two",
        Some(t.root),
        "label/crescendum/song-two/project.md",
    );
    let part = t.w.part(t.root, "Interlude");
    let after = t.w.scope(t.root);
    assert!(after.in_scope(&ObjectRef::Project(song_two)));
    assert!(after.in_scope(&ObjectRef::Part(part)));
    assert!(!before.in_scope(&ObjectRef::Project(song_two)));
    assert_ne!(
        d_before,
        digest(t.root, &after.visible()),
        "the subtree changed, so the digest did"
    );
}

#[test]
fn a_promoted_part_stays_in_and_a_sub_project_moved_out_drops_out() {
    let mut t = tree();
    let part = t.w.part(t.root, "Track Two");
    let task = t.w.task("Write the bridge", Some(part));
    let s = t.w.scope(t.root);
    assert!(s.in_scope(&ObjectRef::Part(part)));
    assert!(s.in_scope(&ObjectRef::Task(task)), "a task filed on a part");

    // Promote: the part leaves the roster and becomes a project, same id.
    t.w.state
        .projects
        .iter_mut()
        .find(|p| p.id == t.root)
        .unwrap()
        .parts
        .retain(|(id, _)| *id != part);
    t.w.state.projects.push(ProjectNode {
        id: part,
        parent_id: Some(t.root),
        title: "Track Two".into(),
        page: "label/crescendum/track-two/project.md".into(),
        parts: Vec::new(),
    });
    let s = t.w.scope(t.root);
    assert!(
        s.in_scope(&ObjectRef::Project(part)),
        "promoted: a sub-project"
    );
    assert!(
        s.in_scope(&ObjectRef::Part(part)),
        "links to the part resolve"
    );
    assert!(s.in_scope(&ObjectRef::Task(task)));

    // Move Song One out of Crescendum (reparent to the label). Its
    // directory stays where it was; its whole branch leaves scope.
    t.w.state
        .projects
        .iter_mut()
        .find(|p| p.id == t.song)
        .unwrap()
        .parent_id = Some(t.parent);
    let s = t.w.scope(t.root);
    for id in [t.song, t.stems, t.takes] {
        assert!(!s.in_scope(&ObjectRef::Project(id)), "{id} moved out");
    }
    assert!(
        !s.in_scope(&ObjectRef::Note(
            "label/crescendum/song-one/lyrics.md".into()
        )),
        "its directory is carved out of Crescendum's"
    );
    assert!(s.in_scope(&ObjectRef::Note("label/crescendum/notes.md".into())));
    let kinds: Vec<_> = s
        .visible()
        .into_iter()
        .filter(|i| i.kind == "note_dir_excluded")
        .map(|i| i.id)
        .collect();
    assert_eq!(kinds, ["label/crescendum/song-one/"], "the preview says so");
}

#[test]
fn tasks_outside_the_subtree_or_with_no_project_are_out() {
    let mut t = tree();
    let filed_nowhere = t.w.task("Buy strings", None);
    let elsewhere = t.w.task("Elsewhere", Some(Uuid::new_v4()));
    let s = t.w.scope(t.root);
    assert!(!s.in_scope(&ObjectRef::Task(filed_nowhere)));
    assert!(!s.in_scope(&ObjectRef::Task(elsewhere)));
    assert!(!s.in_scope(&ObjectRef::Task(Uuid::new_v4())), "unknown");
}

#[test]
fn milestones_workstreams_and_threads_follow_their_anchor_goals_never() {
    let mut t = tree();
    let m_in = Uuid::new_v4();
    let m_out = Uuid::new_v4();
    let w_in = Uuid::new_v4();
    t.w.state.milestones = vec![
        Attached {
            id: m_in,
            project_id: Some(t.song),
            title: "Rough mix".into(),
        },
        Attached {
            id: m_out,
            project_id: Some(t.sibling),
            title: "Theirs".into(),
        },
    ];
    t.w.state.workstreams = vec![Attached {
        id: w_in,
        project_id: Some(t.root),
        title: "Mixing".into(),
    }];
    let task = t.w.task("Mix", Some(t.song));
    let s = t.w.scope(t.root);
    assert!(s.in_scope(&ObjectRef::Milestone(m_in)));
    assert!(!s.in_scope(&ObjectRef::Milestone(m_out)));
    assert!(s.in_scope(&ObjectRef::Workstream(w_in)));
    assert!(!s.in_scope(&ObjectRef::Goal(Uuid::new_v4())));

    let thread = |kind: &str, id| ObjectRef::Thread {
        entity_type: kind.into(),
        entity_id: id,
    };
    assert!(s.in_scope(&thread("task", task)));
    assert!(s.in_scope(&thread("project", t.stems)));
    assert!(s.in_scope(&thread("milestone", m_in)));
    assert!(!s.in_scope(&thread("project", t.sibling)));
    assert!(!s.in_scope(&thread("milestone", m_out)));
    assert!(
        !s.in_scope(&thread("forge_issue", task)),
        "an anchor kind a grant cannot reach"
    );
}

#[test]
fn file_roots_adopted_from_subtree_directories_with_prefixes_and_carve_outs() {
    let mut t = tree();
    let whole = t.w.root("Crescendum", "/org/projects/label/crescendum");
    let deep = t.w.root(
        "Takes",
        "/org/projects/label/crescendum/song-one/stems/takes",
    );
    let label = t.w.root("Label", "/org/projects/label");
    let theirs = t.w.root("Other Album", "/org/projects/label/other-album");
    let off_tier = t.w.root("NAS", "/mnt/nas/crescendum");
    let vault = t.w.root("Vault", "/org/vault");
    // An outside project whose directory sits inside Crescendum's.
    t.w.project("Archive", None, "label/crescendum/archive/project.md");
    let s = t.w.scope(t.root);

    let file = |root, path: &str| ObjectRef::File {
        root,
        path: path.into(),
    };
    assert!(s.in_scope(&file(whole, "song-one/mix.wav")));
    assert!(s.in_scope(&file(whole, "")), "the whole root");
    assert!(!s.in_scope(&file(whole, "archive/old.wav")), "carved out");
    assert!(s.in_scope(&file(deep, "take-1.wav")));
    // A root above the subtree: only the subtree's directory inside it.
    assert!(s.in_scope(&file(label, "crescendum/song-one/mix.wav")));
    assert!(!s.in_scope(&file(label, "budget.xlsx")));
    assert!(!s.in_scope(&file(label, "other-album/mix.wav")));
    assert!(!s.in_scope(&file(label, "crescendum/archive/x")));
    assert!(!s.in_scope(&file(label, "crescendum/../budget.xlsx")));
    for out in [theirs, off_tier, vault] {
        assert!(
            !s.file_roots.contains_key(&out),
            "{out} relates to nothing shared"
        );
        assert!(!s.in_scope(&file(out, "anything")));
    }
    let label_scope = &s.file_roots[&label];
    assert_eq!(label_scope.paths.included().collect::<Vec<_>>().len(), 4);

    let ids: BTreeSet<String> = s
        .visible()
        .into_iter()
        .filter(|i| i.kind == "file_root")
        .map(|i| i.id)
        .collect();
    assert!(ids.contains(&whole.to_string()));
    assert!(ids.contains(&format!("{label}:crescendum")));
}

#[test]
fn a_parent_cycle_terminates_and_an_unknown_root_is_absent() {
    let mut w = World::new();
    let a = w.project("A", None, "a/project.md");
    let b = w.project("B", Some(a), "a/b/project.md");
    w.state.projects[0].parent_id = Some(b);
    let s = w.scope(a);
    assert_eq!(s.projects.len(), 2);
    assert!(resolve(&w.state, Uuid::new_v4()).is_none());
}

#[test]
fn the_preview_is_stable_and_never_carries_project_fields() {
    let mut t = tree();
    t.w.task("Mix", Some(t.song));
    t.w.part(t.root, "Interlude");
    let one = t.w.scope(t.root).visible();
    // Same state resolved again, and with the inputs reordered.
    let two = t.w.scope(t.root).visible();
    t.w.state.projects.reverse();
    t.w.state.tasks.reverse();
    let three = t.w.scope(t.root).visible();
    assert_eq!(one, two);
    assert_eq!(one, three);
    assert_eq!(digest(t.root, &one), digest(t.root, &three));
    assert_eq!(one, sorted(one.clone()), "already canonical");
}

// ── store-backed ─────────────────────────────────────────────────────

struct Disk {
    _tmp: tempfile::TempDir,
    projects: project::ProjectBackend,
    tasks: task::TaskBackend,
    store: OwnerStore,
}

fn disk() -> Disk {
    let tmp = tempfile::tempdir().unwrap();
    let tier = tmp.path().join("projects");
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&tier).unwrap();
    std::fs::create_dir_all(&vault).unwrap();
    let projects = project::ProjectBackend::new(tier.clone());
    let tasks = task::TaskBackend::new(vault);
    let store = OwnerStore::new(projects.clone(), tier).with_tasks(tasks.clone());
    Disk {
        _tmp: tmp,
        projects,
        tasks,
        store,
    }
}

impl Disk {
    fn project(&self, title: &str, parent: Option<Uuid>) -> project::ProjectInfo {
        use project::ProjectService as _;
        let parent_page = parent.map(|p| self.projects.get(p).unwrap().path);
        self.projects
            .create(project::ProjectInfo {
                id: Uuid::new_v4(),
                title: title.into(),
                parent_id: parent,
                path: project::write::child_project_path(parent_page.as_deref(), title),
                ..Default::default()
            })
            .unwrap()
    }

    fn task(&self, title: &str, project: Option<Uuid>) -> Uuid {
        use task::TaskService as _;
        let mut t = task::TaskInfo::new(title);
        t.project_id = project;
        self.tasks.create(t).unwrap().id
    }

    fn scope(&self, root: Uuid) -> InScope {
        self.store.scope(root).expect("root exists")
    }
}

#[test]
fn store_depth_parent_sibling_and_unfiled_tasks() {
    let d = disk();
    let label = d.project("Label", None);
    let root = d.project("Crescendum", Some(label.id));
    let song = d.project("Song One", Some(root.id));
    let stems = d.project("Stems", Some(song.id));
    let takes = d.project("Takes", Some(stems.id));
    let sibling = d.project("Other Album", Some(label.id));

    let t_deep = d.task("Comp the takes", Some(takes.id));
    let t_sibling = d.task("Their mix", Some(sibling.id));
    let t_parent = d.task("Label admin", Some(label.id));
    let t_none = d.task("Buy strings", None);

    let s = d.scope(root.id);
    for p in [&root, &song, &stems, &takes] {
        assert!(s.in_scope(&ObjectRef::Project(p.id)), "{}", p.title);
    }
    assert!(!s.in_scope(&ObjectRef::Project(label.id)));
    assert!(!s.in_scope(&ObjectRef::Project(sibling.id)));
    assert!(s.in_scope(&ObjectRef::Task(t_deep)));
    for out in [t_sibling, t_parent, t_none] {
        assert!(!s.in_scope(&ObjectRef::Task(out)));
    }
    assert!(d.store.scope(Uuid::new_v4()).is_none(), "no root, no set");
}

#[test]
fn store_is_evaluated_per_call_promotion_and_moves_included() {
    use project::ProjectService as _;
    let d = disk();
    let label = d.project("Label", None);
    let root = d.project("Crescendum", Some(label.id));
    let song = d.project("Song One", Some(root.id));

    // The preview the owner would confirm today.
    let d0 = digest(root.id, &d.store.visible(root.id).unwrap());
    assert_eq!(
        d0,
        digest(root.id, &d.store.visible(root.id).unwrap()),
        "stable across runs over the same state"
    );

    // Added after the grant: in, with no regrant.
    let later = d.project("Song Two", Some(root.id));
    let part = d.projects.add_part(root.id, "Track Three").unwrap();
    let on_part = d.task("Write the bridge", Some(part.id));
    let s = d.scope(root.id);
    assert!(s.in_scope(&ObjectRef::Project(later.id)));
    assert!(s.in_scope(&ObjectRef::Part(part.id)));
    assert!(s.in_scope(&ObjectRef::Task(on_part)));
    let d1 = digest(root.id, &s.visible());
    assert_ne!(d0, d1, "the subtree changed");

    // Promote the part: same id, now a sub-project, still in.
    let promoted = d.projects.promote_part(root.id, part.id).unwrap();
    assert_eq!(promoted.id, part.id);
    let s = d.scope(root.id);
    assert!(s.in_scope(&ObjectRef::Project(part.id)));
    assert!(s.in_scope(&ObjectRef::Part(part.id)));
    assert!(s.in_scope(&ObjectRef::Task(on_part)));
    assert!(s.in_scope(&ObjectRef::Note(format!(
        "{}/notes.md",
        page_dir(&promoted.path).unwrap()
    ))));

    // Move Song One out to the label: out at once, directory carved out.
    let mut moved = d.projects.get(song.id).unwrap();
    moved.parent_id = Some(label.id);
    let moved = d.projects.update(moved).unwrap();
    let s = d.scope(root.id);
    assert!(!s.in_scope(&ObjectRef::Project(song.id)));
    let song_dir = page_dir(&moved.path).unwrap();
    assert!(!s.in_scope(&ObjectRef::Note(format!("{song_dir}/lyrics.md"))));
    assert_ne!(d1, digest(root.id, &s.visible()));
}

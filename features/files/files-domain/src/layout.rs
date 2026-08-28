//! What a path in a Task tree *is* — `project.location.*`.
//!
//! A studio's disk is organised long before any software reads it. The
//! folders carry meaning a person put there, and the job here is to
//! recover it: which of these directories is an org, which is a project,
//! whose project it is, and which are neither.
//!
//! ```text
//! Data/
//!   <org>/                        one org, and everything it has
//!     Assets/                     its library — content owned by no project
//!     Inbox/                      arrived, not filed into a project yet
//!     Vault/                      its notes — what it is doing
//!     Wiki/                       what it knows
//!     Projects/
//!       <Work> - <Client>/        a project
//!         <Session>/              a DAW session — the adoptable root
//!         Inbox/                  unfiled, this project's
//! ```
//!
//! The org is the top level, so an org's whole disk is one subtree.
//! That is what makes it the unit everything else already works on: a
//! host admits an org, a peer replicates one, a server's data root holds
//! `orgs/<slug>/`. A layout that put `Projects/` first would have those
//! all reaching across a level to reassemble something the tree could
//! simply have said.
//!
//! # Read from the tree, not from a sidecar
//!
//! Some of these projects carry a `project.md`; most do not — 14 of 43 in
//! the tree this was written against. A model that needed one would be a
//! model that knows nothing about most of a real archive, so the
//! directory is the source and a page can only ever add to it.
//!
//! The same tree also shows why a page cannot be trusted as the *only*
//! source even when present: pages there disagree with their own
//! directories about which org owns the work, and disagree with each
//! other about whether a title reads `Work - Client` or `Client - Work`.
//! That is what hand-organisation looks like, and a model that assumes
//! consistency is a model that will call real folders malformed.
//!
//! # Nothing here touches the filesystem
//!
//! Classification is a function of names, so it is testable against a
//! list of them — including the ones that broke the first attempt. What
//! needs the disk (does this folder hold a `.RPP`?) is asked of the
//! caller, so this stays pure and the I/O stays in one place.

/// Where an unfiled pile sits.
///
/// Both scopes belong to an org. Nothing arrives to nobody: material
/// lands on some org's disk, and the open question is only which of its
/// projects it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxScope {
    /// `<org>/Inbox` — this org's, no project yet.
    Org(String),
    /// `<org>/Projects/<Project>/Inbox` — this project's.
    Project { org: String, project: String },
}

/// Why a directory is not a project even though it sits where one would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAProject {
    /// A `Z - …` folder. The prefix was a sort hack — it kept
    /// housekeeping at the bottom of an alphabetical file listing — and it
    /// is load-bearing here for a subtler reason: `Z - Duplicates`
    /// contains ` - `, so a model that splits on the dash first reads it
    /// as the project "Z" for the client "Duplicates".
    Housekeeping,
    /// Detritus a tool left: `ReaperUnsavedMedia` and friends. Recovered
    /// media from a crashed DAW is real content and still not a project.
    ToolDebris,
    /// A lowercase name in a tree where people capitalise. `tasks` is
    /// Task's own storage; a person naming a project would have written
    /// `Tasks`.
    MachineOwned,
}

/// What a path in a Task tree is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// An org's library. Content it owns that belongs to no project — a
    /// template, a LUT, a sample pack.
    Assets(String),
    /// An org's `Projects/` — a container rather than a thing, and it
    /// still needs a name. A caller walking a tree and asking about every
    /// directory should get an answer here, not a shrug that reads the
    /// same as an unrecognised folder.
    Projects(String),
    /// An org's notes: what it is doing. Markdown pages, which is where
    /// projects, tasks and goals already live.
    Vault(String),
    /// An org's wiki: what it knows. Separate from the vault because the
    /// two age differently — a session note is true of one week, and a
    /// page on mic placement is meant to outlast every project that used
    /// it.
    Wiki(String),
    /// An unfiled pile.
    Inbox(InboxScope),
    /// An org's directory — the whole of what it has on this disk.
    ///
    /// Placement, not ownership. A project sitting under an org may be a
    /// collaboration several orgs are on, and in the tree this was
    /// written against, four of fourteen named a different org than the
    /// directory holding them. `files.peering` draws the same line for
    /// hosts, and [`crate::collaboration`] is where the several-orgs case
    /// is actually modelled: where content sits and whose it is are
    /// separate questions, and the tree can only answer the first.
    Org(String),
    /// A project.
    Project(Project),
    /// A folder inside a project: a session, or material a session uses.
    ///
    /// Not broken down further, and deliberately. A video project in the
    /// tree this was written against holds `Video ISO Files`, `Audio
    /// Source Files`, `Graphics` and `Archive - Original Camera Files` — one
    /// person's names for kinds of material, which the next studio spells
    /// differently. Naming them here would be inventing a taxonomy nobody
    /// agreed to; `files.facet.tool-layout` reads the ones the *tools*
    /// impose, which is a smaller and defensible claim.
    ///
    /// Note the fourth of those contains ` - `. Anything that splits on
    /// the dash has to know it is looking at material and not at a
    /// project for the client "Original Camera Files".
    Material { org: String, project: String },
    /// Where a project would be, and isn't.
    NotAProject { org: String, why: NotAProject },
    /// Nothing this model recognises. Not an error — an archive contains
    /// what it contains, and a caller that wants to list the unexplained
    /// is better served than one handed a guess.
    Unknown,
}

/// A project, as its directory name states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The org whose disk this sits on.
    pub org: String,
    /// The directory's own name, verbatim. Kept because it is the only
    /// name guaranteed to round-trip to a path.
    pub folder: String,
    /// The work: everything before the final ` - `, or the whole name.
    pub work: String,
    /// Who it is for. Empty when the folder does not say — most of them.
    ///
    /// Comma-separated in the folder name, because a benefit concert has
    /// three artists and the folder is where a studio writes that down.
    pub clients: Vec<String>,
}

/// Folder names a tool leaves behind.
const TOOL_DEBRIS: &[&str] = &["ReaperUnsavedMedia", "Auto-Save", "Backup", "Bounced Files"];

/// Classify a path relative to a Task root.
///
/// `parts` is the path split into components — `["acme-audio",
/// "Projects", "Example Album"]`. Taking components rather than a string
/// keeps the separator and the encoding out of a decision that is about
/// meaning.
#[must_use]
pub fn classify(parts: &[&str]) -> Entry {
    match parts {
        [] => Entry::Unknown,
        [org] => Entry::Org((*org).to_string()),
        [org, "Assets", ..] => Entry::Assets((*org).to_string()),
        [org, "Vault", ..] => Entry::Vault((*org).to_string()),
        [org, "Wiki", ..] => Entry::Wiki((*org).to_string()),
        [org, name] if is_inbox(name) => Entry::Inbox(InboxScope::Org((*org).to_string())),
        [org, "Projects"] => Entry::Projects((*org).to_string()),
        [org, "Projects", name] if is_inbox(name) => {
            Entry::Inbox(InboxScope::Org((*org).to_string()))
        }
        [org, "Projects", name] => classify_project(org, name),
        [org, "Projects", project, name] if is_inbox(name) => Entry::Inbox(InboxScope::Project {
            org: (*org).to_string(),
            project: (*project).to_string(),
        }),
        // Inside a project: a session, or material a session uses. Which
        // of those needs the disk to answer — a folder is a session
        // because it holds a `.RPP`, not because of its name — so the
        // caller asks [`is_session_file`] and this says only where it is.
        [org, "Projects", project, ..] => Entry::Material {
            org: (*org).to_string(),
            project: (*project).to_string(),
        },
        _ => Entry::Unknown,
    }
}

/// An inbox folder, at any level.
///
/// `Inbox` is the name. `Z - Inbox` is accepted too, and only because
/// existing archives are full of it: the `Z - ` prefix was a sort hack,
/// keeping unfiled material at the bottom of an alphabetical file
/// listing. Nothing here browses folders alphabetically any more — the
/// tree a person sees is built from the catalogue, which can order by
/// whatever it likes — so the prefix buys nothing and new trees do not
/// use it.
///
/// Reading both matters more than it sounds. A studio does not rename six
/// thousand folders because a model prefers a different spelling, so a
/// model that only knew the new name would fail to recognise an inbox on
/// every disk that predates it.
///
/// Checked before anything splits on ` - `, which is the whole reason
/// this is a function: `Z - Inbox` contains the separator, so a
/// dash-splitting project parser reads it as the project "Z" for the
/// client "Inbox" — and `Z - Duplicates`, which has no new-style spelling
/// and is still out there, the same way.
#[must_use]
pub fn is_inbox(name: &str) -> bool {
    name.eq_ignore_ascii_case("Inbox") || name.eq_ignore_ascii_case("Z - Inbox")
}

fn classify_project(org: &str, name: &str) -> Entry {
    let org = org.to_string();
    if name.starts_with("Z - ") || name.starts_with("z - ") {
        return Entry::NotAProject {
            org,
            why: NotAProject::Housekeeping,
        };
    }
    if TOOL_DEBRIS.iter().any(|d| d.eq_ignore_ascii_case(name)) {
        return Entry::NotAProject {
            org,
            why: NotAProject::ToolDebris,
        };
    }
    // Lowercase first letter in a tree where people capitalise. Only when
    // the name is *all* lowercase — `iPhone Footage` is a person's name
    // for something, `tasks` is not.
    if name.chars().all(|c| !c.is_ascii_uppercase()) && name.chars().any(char::is_alphabetic) {
        return Entry::NotAProject {
            org,
            why: NotAProject::MachineOwned,
        };
    }

    let (work, clients) = split_work_and_clients(name);
    Entry::Project(Project {
        org,
        folder: name.to_string(),
        work,
        clients,
    })
}

/// `Work - Client, Client` → the work and who it is for.
///
/// Splits on the **last** ` - `, because the work can contain one:
/// `VILLAGE-CHOIR VIDEOv-vfrom Josue` has hyphens that are not
/// separators, and a first-match split would take the wrong half.
fn split_work_and_clients(name: &str) -> (String, Vec<String>) {
    match name.rsplit_once(" - ") {
        Some((work, clients)) if !work.trim().is_empty() && !clients.trim().is_empty() => (
            work.trim().to_string(),
            clients
                .split(',')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => (name.trim().to_string(), Vec::new()),
    }
}

/// The DAW project files that make a folder a session.
///
/// A session is the unit that gets adopted as a File Root: the folder a
/// DAW treats as one project, holding its own media. Which extensions
/// count is a fact about the tools in use, and the tree this was written
/// against holds all three — 49 Pro Tools, 18 Resolve, 16 REAPER.
#[must_use]
pub fn is_session_file(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        // REAPER, Pro Tools, DaVinci Resolve, Logic, Ableton, Cubase
        "rpp" | "ptx" | "drp" | "logicx" | "als" | "cpr"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names are invented; every *shape* is one a real archive produced,
    /// and the awkward ones are the point. Invented so a failure names
    /// the case rather than somebody's album.
    fn project(org: &str, name: &str) -> Project {
        match classify(&[org, "Projects", name]) {
            Entry::Project(p) => p,
            other => panic!("{name:?} classified as {other:?}"),
        }
    }

    #[test]
    fn an_org_directory_is_a_placement_not_an_owner() {
        assert_eq!(classify(&["acme-audio"]), Entry::Org("acme-audio".into()));
    }

    #[test]
    fn a_plain_name_is_all_work_and_no_client() {
        let p = project("acme-audio", "Example Album");
        assert_eq!(p.work, "Example Album");
        assert!(p.clients.is_empty(), "{:?}", p.clients);
    }

    #[test]
    fn a_dash_separates_the_work_from_the_client() {
        let p = project("acme-audio", "First Single - Example Client");
        assert_eq!(p.work, "First Single");
        assert_eq!(p.clients, ["Example Client"]);
    }

    /// A benefit concert has three artists, and the folder is where that
    /// got written down.
    #[test]
    fn commas_separate_several_clients() {
        let p = project(
            "vnt-video",
            "Example Documentary - First Client, Second Client",
        );
        assert_eq!(p.work, "Example Documentary");
        assert_eq!(p.clients, ["First Client", "Second Client"]);
    }

    #[test]
    fn an_inbox_is_an_inbox_by_either_name() {
        // The name.
        assert_eq!(
            classify(&["acme-audio", "Projects", "Inbox"]),
            Entry::Inbox(InboxScope::Org("acme-audio".into()))
        );
        // And the old one, because six thousand folders already say it.
        assert_eq!(
            classify(&["acme-audio", "Projects", "Z - Inbox"]),
            Entry::Inbox(InboxScope::Org("acme-audio".into()))
        );
    }

    /// The trap the `Z - ` names leave behind. Both contain ` - `, so a
    /// parser that splits before checking the prefix reports the project
    /// "Z" for the client "Duplicates".
    #[test]
    fn the_z_prefix_is_checked_before_the_dash() {
        assert_eq!(
            classify(&["acme-audio", "Projects", "Z - Duplicates"]),
            Entry::NotAProject {
                org: "acme-audio".into(),
                why: NotAProject::Housekeeping,
            }
        );
    }

    /// Hyphens inside a name are not separators.
    #[test]
    fn only_a_spaced_dash_separates() {
        let p = project("vnt-video", "MULTI-CAM VIDEOv-vfrom Card 2");
        assert_eq!(p.work, "MULTI-CAM VIDEOv-vfrom Card 2");
        assert!(p.clients.is_empty());
    }

    #[test]
    fn tool_debris_is_not_a_project() {
        assert_eq!(
            classify(&["acme-audio", "Projects", "ReaperUnsavedMedia"]),
            Entry::NotAProject {
                org: "acme-audio".into(),
                why: NotAProject::ToolDebris,
            }
        );
    }

    #[test]
    fn an_all_lowercase_name_is_machine_owned() {
        assert_eq!(
            classify(&["acme-audio", "Projects", "tasks"]),
            Entry::NotAProject {
                org: "acme-audio".into(),
                why: NotAProject::MachineOwned,
            }
        );
    }

    /// …but a name that merely starts lowercase is a person's.
    #[test]
    fn a_name_with_any_capital_is_a_persons() {
        let p = project("vnt-video", "iPhone Footage");
        assert_eq!(p.work, "iPhone Footage");
    }

    #[test]
    fn inboxes_know_which_level_they_are_at() {
        assert_eq!(
            classify(&["acme-audio", "Inbox"]),
            Entry::Inbox(InboxScope::Org("acme-audio".into()))
        );
        assert_eq!(
            classify(&["acme-audio", "Projects", "Example Album", "Inbox"]),
            Entry::Inbox(InboxScope::Project {
                org: "acme-audio".into(),
                project: "Example Album".into(),
            })
        );
    }

    /// A video project holds its material in folders one person named,
    /// and one of them contains ` - `. The model says "material of that
    /// project" rather than guessing at a taxonomy or reading it as a
    /// project for the client "Original Camera Files".
    #[test]
    fn folders_inside_a_project_are_its_material() {
        for name in [
            "Video ISO Files",
            "Audio Source Files",
            "Graphics",
            "Archive - Original Camera Files",
        ] {
            assert_eq!(
                classify(&[
                    "vnt-video",
                    "Projects",
                    "Example Documentary - First Client",
                    name
                ]),
                Entry::Material {
                    org: "vnt-video".into(),
                    project: "Example Documentary - First Client".into(),
                },
                "{name:?}"
            );
        }
    }

    #[test]
    fn assets_belong_to_an_org_but_to_no_project() {
        assert_eq!(
            classify(&["acme-audio", "Assets"]),
            Entry::Assets("acme-audio".into())
        );
        assert_eq!(
            classify(&["acme-audio", "Assets", "Music", "Templates"]),
            Entry::Assets("acme-audio".into())
        );
    }

    #[test]
    fn the_three_daws_in_use_are_all_sessions() {
        assert!(is_session_file("First Single.RPP"));
        assert!(is_session_file("First Single.ptx"));
        assert!(is_session_file("Example Documentary.drp"));
        assert!(!is_session_file("Piano Left.wav"));
        // A DAW's own saves sit beside the session file and are not one.
        assert!(!is_session_file("Track One-2024-10-11_1700.rpp-bak"));
        assert!(!is_session_file("Track One-2024-10-11_1912.rpp-bak-UNDO"));
        assert!(!is_session_file("project.md"));
    }
}

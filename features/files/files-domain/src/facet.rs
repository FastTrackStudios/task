//! Facets — `files.facet.*`.
//!
//! A facet names a class of content a project holds. It is the unit of
//! **both** selective sync and placement policy: one vocabulary, two
//! consumers, so "this laptop takes sessions" and "sessions on the fast
//! NAS, proxies on cheap object storage" are the same knob read twice.
//!
//! Where a mapping comes from is the whole design:
//!
//! - **Tool layouts belong to the capability.** Pro Tools always writes
//!   `Audio Files`, `Bounced Files` and `Session File Backups`. Reaper
//!   always writes `Media` and `Backups`. Nobody should configure that in
//!   any project, ever.
//! - **The project maps what its tools did not create.** `Project
//!   Assembly`, `Video ISO Files`, `Archive - Original AAC MP4s` are
//!   conventions of a particular job.
//! - **Everything else is unmapped**, syncs with the default, and is
//!   *reported* — never hidden, and never guessed. Guessing is how
//!   footage ends up on the wrong tier.

use std::collections::BTreeMap;

/// What kind of work a project supports. Closed and small, so a sync
/// client, a placement policy and a UI can each reason about every
/// member.
///
/// This is the same vocabulary as [`project_proto::parts::Capability`],
/// duplicated because neither crate can depend on the other and there is
/// no shared leaf; `project::conventions` bridges them and carries a
/// test that fails if either side gains a member the other lacks. Add
/// here and you must add there.
///
/// # Kinds of work only
///
/// ADR 0003 briefly added `Session`, `Signal`, `Ignition` and `Keyflow`
/// — four application names — and ADR 0004 removed them. The test of a
/// member here is mechanical and it is right below: `layouts_for` and
/// `ignore::patterns_for` must have something to say about it. All four
/// returned the empty slice, because none of those applications writes
/// anything into a project tree; what they contributed to this enum was
/// a name, and a name for a *consumer* at that. A primitive that
/// enumerates its consumers is not a primitive — see
/// `project_proto::parts::Capability` for the argument at length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    MusicProduction,
    VideoProduction,
}

impl Capability {
    /// Every capability there is.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::MusicProduction, Self::VideoProduction]
    }

    /// The declaration's spelling — `capabilities: [music-production]`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MusicProduction => "music-production",
            Self::VideoProduction => "video-production",
        }
    }

    /// Parse a declared capability name, exactly as
    /// `project_proto::parts::Capability::parse` does — the two are held
    /// in step by `project::conventions`' tests. `None` for anything
    /// outside the vocabulary: reported by the caller, never guessed.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "music-production" | "music_production" => Some(Self::MusicProduction),
            "video-production" | "video_production" => Some(Self::VideoProduction),
            _ => None,
        }
    }

    /// Read a legacy free-string `projectType`, exactly as
    /// `project_proto::parts::Capability::from_project_type` does.
    #[must_use]
    pub fn from_project_type(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "music" | "audio" | "album" | "song" | "music-production" => {
                Some(Self::MusicProduction)
            }
            "video" | "film" | "documentary" | "video-production" => Some(Self::VideoProduction),
            _ => None,
        }
    }
}

/// The file at the top of a project's directory that says what the
/// project is — its page, with the `capabilities:` list in frontmatter
/// (`project.identity.declaration`).
///
/// A File Root adopted from a project's directory carries the page in
/// its own tree, so the declaration travels with the root: every device
/// that syncs the root reads the same capabilities from the same bytes,
/// with no second channel to keep in step and nothing to lose on a
/// restart.
pub const DECLARATION: &str = "project.md";

/// What a project page declares about the work it holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declaration {
    /// The recognised capabilities, in the page's order.
    pub held: Vec<Capability>,
    /// Names the page carries that are not capabilities — a typo, or an
    /// application name ADR 0004 removed. Kept so a surface can say so.
    pub unrecognised: Vec<String>,
}

impl Declaration {
    fn from_names<I: IntoIterator<Item = String>>(names: I) -> Self {
        let mut out = Self::default();
        for name in names {
            match Capability::parse(&name) {
                Some(c) if !out.held.contains(&c) => out.held.push(c),
                Some(_) => {}
                None if name.trim().is_empty() => {}
                None => out.unrecognised.push(name.trim().to_owned()),
            }
        }
        out
    }
}

/// Read the capabilities a project page declares.
///
/// The same reading `project`'s entity parser makes of the same page:
/// `capabilities:` as written (a single string is a set of one), and
/// only when that is absent the legacy `projectType` / `project_type`.
/// A page with neither, or no frontmatter at all, declares nothing —
/// and a project that declares nothing gets no tool layouts, because we
/// do not know which tool wrote it and guessing is how footage ends up
/// on the wrong tier.
#[must_use]
// t[impl project.identity.declaration] — the root's capabilities are
// read from its own project page and nowhere else
pub fn declaration(page: &str) -> Declaration {
    let Some(front) = frontmatter(page) else {
        return Declaration::default();
    };
    let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(front)
    else {
        return Declaration::default();
    };
    if let Some(value) = map.get("capabilities") {
        let names = match value {
            serde_yaml::Value::String(one) => vec![one.clone()],
            other => serde_yaml::from_value::<Vec<String>>(other.clone()).unwrap_or_default(),
        };
        if !names.is_empty() {
            return Declaration::from_names(names);
        }
    }
    for key in ["projectType", "project_type"] {
        if let Some(serde_yaml::Value::String(t)) = map.get(key) {
            return Declaration {
                held: Capability::from_project_type(t).into_iter().collect(),
                unrecognised: Vec::new(),
            };
        }
    }
    Declaration::default()
}

/// The YAML between a page's opening `---` and the next `---` line.
fn frontmatter(page: &str) -> Option<&str> {
    let page = page.strip_prefix('\u{feff}').unwrap_or(page);
    let rest = page
        .strip_prefix("---\r\n")
        .or_else(|| page.strip_prefix("---\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return rest.get(..offset);
        }
        offset += line.len();
    }
    None
}

/// The facet a session file belongs to, and the one its media follows.
pub const SESSIONS: &str = "sessions";

/// Session-file extensions a capability's tools write, lowercased.
///
/// Part of the convention set for the same reason tool directories are:
/// REAPER always saves `.rpp`, Pro Tools `.ptx`, and nobody should have
/// to tell a project so. A session is the unit a person opens, so it is
/// what selective sync must keep whole (`files.sync.selective`).
fn session_extensions_for(capability: Capability) -> &'static [&'static str] {
    match capability {
        // REAPER; Pro Tools (current, 7–9, 5–6).
        Capability::MusicProduction => &["rpp", "ptx", "ptf", "pts"],
        // DaVinci Resolve's exported project; Premiere.
        Capability::VideoProduction => &["drp", "prproj"],
    }
}

/// A named class of content.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Facet(pub String);

impl Facet {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Facet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a mapping came from. The distinction is not cosmetic: one is
/// knowledge about a tool, the other is a decision about a project, and
/// only the second is a user's to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    ToolLayout,
    Project,
    Unmapped,
}

/// One directory's resolved facet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub facet: Option<Facet>,
    pub source: Source,
    /// Subscribing to an atomic facet brings its dependencies. A session
    /// whose audio streams in on first access will glitch, so a session
    /// arrives with the media it references.
    pub atomic: bool,
}

impl Binding {
    fn tool(facet: &str, atomic: bool) -> Self {
        Self {
            facet: Some(Facet::new(facet)),
            source: Source::ToolLayout,
            atomic,
        }
    }

    #[must_use]
    pub fn unmapped() -> Self {
        Self {
            facet: None,
            source: Source::Unmapped,
            atomic: false,
        }
    }
}

/// A directory an application always produces, and the facet it holds.
struct ToolDir {
    /// Matched case-insensitively against a single path component.
    name: &'static str,
    facet: &'static str,
    atomic: bool,
}

/// Pro Tools. Observed in the real tree: every `.ptx` session folder
/// carries these.
const PRO_TOOLS: &[ToolDir] = &[
    ToolDir {
        name: "audio files",
        facet: "sessions",
        atomic: true,
    },
    ToolDir {
        name: "bounced files",
        facet: "mixes",
        atomic: false,
    },
    ToolDir {
        name: "session file backups",
        facet: "session-backups",
        atomic: false,
    },
    ToolDir {
        name: "ara data",
        facet: "sessions",
        atomic: true,
    },
    ToolDir {
        name: "clip groups",
        facet: "sessions",
        atomic: true,
    },
    ToolDir {
        name: "rendered files",
        facet: "sessions",
        atomic: true,
    },
    ToolDir {
        name: "video files",
        facet: "sessions",
        atomic: true,
    },
    ToolDir {
        name: "wavecache.wfm",
        facet: "peak-cache",
        atomic: false,
    },
];

/// Reaper.
const REAPER: &[ToolDir] = &[
    ToolDir {
        name: "media",
        facet: "sessions",
        atomic: true,
    },
    ToolDir {
        name: "backups",
        facet: "session-backups",
        atomic: false,
    },
    ToolDir {
        name: "peaks",
        facet: "peak-cache",
        atomic: false,
    },
    ToolDir {
        name: "renders",
        facet: "mixes",
        atomic: false,
    },
];

/// Logic — a project is a directory pretending to be a file.
const LOGIC: &[ToolDir] = &[
    ToolDir {
        name: "audio files",
        facet: "sessions",
        atomic: true,
    },
    ToolDir {
        name: "bounces",
        facet: "mixes",
        atomic: false,
    },
    ToolDir {
        name: "freeze files",
        facet: "peak-cache",
        atomic: false,
    },
    ToolDir {
        name: "project file backups",
        facet: "session-backups",
        atomic: false,
    },
];

/// DaVinci Resolve.
const RESOLVE: &[ToolDir] = &[
    ToolDir {
        name: "cachedclip",
        facet: "render-cache",
        atomic: false,
    },
    ToolDir {
        name: "proxy",
        facet: "proxies",
        atomic: false,
    },
    ToolDir {
        name: "proxies",
        facet: "proxies",
        atomic: false,
    },
    ToolDir {
        name: "optimizedmedia",
        facet: "proxies",
        atomic: false,
    },
    ToolDir {
        name: "gallery",
        facet: "grades",
        atomic: false,
    },
];

fn layouts_for(capability: Capability) -> &'static [&'static [ToolDir]] {
    match capability {
        Capability::MusicProduction => &[PRO_TOOLS, REAPER, LOGIC],
        Capability::VideoProduction => &[RESOLVE],
    }
}

/// Resolves a directory to a facet, given a project's capabilities and
/// its own mappings.
#[derive(Debug, Clone, Default)]
// t[impl files.facet.vocabulary]
pub struct FacetMap {
    capabilities: Vec<Capability>,
    /// Project mappings, keyed by lowercased path.
    project: BTreeMap<String, Facet>,
    /// The facet unmapped content syncs with.
    default_facet: Option<Facet>,
}

impl FacetMap {
    #[must_use]
    pub fn new(capabilities: impl IntoIterator<Item = Capability>) -> Self {
        let mut capabilities: Vec<_> = capabilities.into_iter().collect();
        capabilities.sort_unstable();
        capabilities.dedup();
        Self {
            capabilities,
            project: BTreeMap::new(),
            default_facet: None,
        }
    }

    /// Map a path this project's tools did not create.
    pub fn map(&mut self, path: &str, facet: Facet) -> &mut Self {
        self.project.insert(path.to_ascii_lowercase(), facet);
        self
    }

    /// The facet unmapped content syncs with. Without one, unmapped
    /// content still syncs — it simply has no facet to subscribe by.
    pub fn with_default(&mut self, facet: Facet) -> &mut Self {
        self.default_facet = Some(facet);
        self
    }

    /// Capabilities gained and lost over a project's life
    /// (`project.capability.mutable`). Removing one withdraws its tool
    /// layouts; it touches no content.
    pub fn set_capabilities(&mut self, capabilities: impl IntoIterator<Item = Capability>) {
        let mut c: Vec<_> = capabilities.into_iter().collect();
        c.sort_unstable();
        c.dedup();
        self.capabilities = c;
    }

    /// Resolve one path.
    ///
    /// Project mappings win over tool layouts: a project that deliberately
    /// maps a directory has said something more specific than a general
    /// fact about a tool.
    #[must_use]
    // t[impl files.facet.tool-layout] — capability layouts, no configuration
    pub fn resolve(&self, path: &str) -> Binding {
        let lower = path.to_ascii_lowercase();

        if let Some(facet) = self.project.get(&lower) {
            return Binding {
                facet: Some(facet.clone()),
                source: Source::Project,
                atomic: false,
            };
        }

        // A tool directory is recognised wherever it sits: session
        // folders nest arbitrarily deep, so matching the full path would
        // recognise nothing.
        if let Some(last) = lower.rsplit('/').next() {
            for layout in self.capabilities.iter().copied().flat_map(layouts_for) {
                for dir in *layout {
                    if dir.name == last {
                        return Binding::tool(dir.facet, dir.atomic);
                    }
                }
            }
        }

        Binding::unmapped()
    }

    /// The capabilities whose conventions this map applies.
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Whether `path` names a session file one of this project's
    /// capabilities writes — `Song.RPP` in a music project, never in a
    /// project that declared only video.
    #[must_use]
    pub fn is_session(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        let Some((stem, ext)) = name.rsplit_once('.') else {
            return false;
        };
        if stem.is_empty() {
            return false;
        }
        let ext = ext.to_ascii_lowercase();
        self.capabilities
            .iter()
            .copied()
            .any(|c| session_extensions_for(c).contains(&ext.as_str()))
    }

    /// A session file's own binding: the atomic `sessions` facet.
    ///
    /// Asked only of a file nothing else classified (see
    /// `hydration::decide`), so a Pro Tools backup in `Session File
    /// Backups` stays a backup and a session a project deliberately
    /// filed under its own mapping stays there.
    #[must_use]
    pub fn session(&self, path: &str) -> Option<Binding> {
        self.is_session(path).then(|| Binding::tool(SESSIONS, true))
    }

    /// The facet an unmapped path syncs with.
    #[must_use]
    pub fn default_facet(&self) -> Option<&Facet> {
        self.default_facet.as_ref()
    }

    /// Directories belonging to no facet, so the mapping can be
    /// supplied. Reporting these is required: unmapped content is never
    /// hidden and its facet is never guessed.
    // t[impl files.facet.project-override] — unmapped is reported, never guessed
    pub fn unmapped<'a, I>(&'a self, paths: I) -> Vec<&'a str>
    where
        I: IntoIterator<Item = &'a str>,
    {
        paths
            .into_iter()
            .filter(|p| self.resolve(p).source == Source::Unmapped)
            .collect()
    }

    /// Every facet this project can currently produce — the vocabulary a
    /// subscription UI offers.
    #[must_use]
    pub fn vocabulary(&self) -> Vec<Facet> {
        let mut out: Vec<Facet> = self
            .capabilities
            .iter()
            .copied()
            .flat_map(layouts_for)
            .flat_map(|l| l.iter())
            .map(|d| Facet::new(d.facet))
            .chain(self.project.values().cloned())
            // Every capability's tools write a session file, so a
            // project holding any of them can be subscribed by session.
            .chain(
                (!self.capabilities.is_empty())
                    .then(|| Facet::new(SESSIONS))
                    .into_iter(),
            )
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn music() -> FacetMap {
        FacetMap::new([Capability::MusicProduction])
    }

    #[test]
    // t[verify files.facet.tool-layout]
    fn tool_layouts_need_no_configuration() {
        let m = music();
        // Pro Tools and Reaper, from the same capability, no setup.
        assert_eq!(m.resolve("Audio Files").facet, Some(Facet::new("sessions")));
        assert_eq!(m.resolve("Media").facet, Some(Facet::new("sessions")));
        assert_eq!(m.resolve("Backups").source, Source::ToolLayout);
    }

    #[test]
    // t[verify files.facet.tool-layout]
    fn tool_dirs_are_recognised_at_any_depth() {
        let m = music();
        let deep = "01 ALL THAT I AM/Audio Files";
        assert_eq!(m.resolve(deep).facet, Some(Facet::new("sessions")));
    }

    #[test]
    fn sessions_are_atomic_and_bounces_are_not() {
        let m = music();
        assert!(
            m.resolve("Audio Files").atomic,
            "a session must bring its media"
        );
        assert!(!m.resolve("Bounced Files").atomic);
    }

    #[test]
    fn a_capability_it_does_not_hold_maps_nothing() {
        let m = music();
        assert_eq!(m.resolve("Proxies").source, Source::Unmapped);

        let mut both = music();
        both.set_capabilities([Capability::MusicProduction, Capability::VideoProduction]);
        assert_eq!(both.resolve("Proxies").facet, Some(Facet::new("proxies")));
    }

    #[test]
    fn removing_a_capability_withdraws_its_layouts() {
        let mut m = FacetMap::new([Capability::MusicProduction, Capability::VideoProduction]);
        assert_eq!(m.resolve("Proxies").source, Source::ToolLayout);
        m.set_capabilities([Capability::MusicProduction]);
        assert_eq!(m.resolve("Proxies").source, Source::Unmapped);
    }

    #[test]
    // t[verify files.facet.project-override]
    fn the_project_maps_what_no_tool_made() {
        let mut m = music();
        m.map("Project Assembly", Facet::new("assembly"));
        let b = m.resolve("Project Assembly");
        assert_eq!(b.facet, Some(Facet::new("assembly")));
        assert_eq!(b.source, Source::Project);
    }

    #[test]
    fn a_project_mapping_beats_a_tool_layout() {
        let mut m = music();
        m.map("Media", Facet::new("deliverables"));
        assert_eq!(m.resolve("Media").source, Source::Project);
    }

    #[test]
    // t[verify files.facet.project-override]
    fn unmapped_is_reported_not_hidden() {
        let m = music();
        let real = [
            "Audio Files",
            "Project Assembly",
            "Video ISO Files",
            "Archive - Original AAC MP4s",
        ];
        // Everything the tools did not make comes back for a decision,
        // rather than being guessed at or silently dropped.
        assert_eq!(
            m.unmapped(real),
            vec![
                "Project Assembly",
                "Video ISO Files",
                "Archive - Original AAC MP4s"
            ]
        );
    }

    #[test]
    // t[verify project.identity.declaration]
    fn a_project_page_declares_the_capabilities_its_root_is_read_by() {
        let page = "---\ntype: project\ntitle: Album\ncapabilities:\n  - music-production\n  - keyflow\n---\n\n# Album\n";
        let d = declaration(page);
        assert_eq!(d.held, vec![Capability::MusicProduction]);
        assert_eq!(
            d.unrecognised,
            vec!["keyflow".to_string()],
            "an application name is reported, not dropped"
        );

        let inline = declaration("---\ncapabilities: [video-production, music-production]\n---\n");
        assert_eq!(
            inline.held,
            vec![Capability::VideoProduction, Capability::MusicProduction]
        );
        let one = declaration("---\ncapabilities: video-production\n---\nbody");
        assert_eq!(one.held, vec![Capability::VideoProduction]);
    }

    #[test]
    fn the_legacy_project_type_is_read_only_when_capabilities_are_absent() {
        let legacy = declaration("---\nprojectType: film\n---\n");
        assert_eq!(legacy.held, vec![Capability::VideoProduction]);
        let both = declaration("---\nproject_type: film\ncapabilities: [music-production]\n---\n");
        assert_eq!(both.held, vec![Capability::MusicProduction]);
        // `audio-production` is not a type anyone mapped: declared
        // nothing, which is a legible state rather than a guess.
        let unknown = declaration("---\nproject_type: audio-production\n---\n");
        assert!(unknown.held.is_empty());
    }

    #[test]
    fn a_page_without_frontmatter_declares_nothing() {
        assert_eq!(declaration("# Just notes\n"), Declaration::default());
        assert_eq!(declaration(""), Declaration::default());
        assert_eq!(
            declaration("---\ncapabilities: [music-production]\n"),
            Declaration::default(),
            "an unterminated block is not frontmatter"
        );
    }

    #[test]
    fn capability_names_round_trip() {
        for c in Capability::all() {
            assert_eq!(Capability::parse(c.as_str()), Some(*c));
        }
    }

    #[test]
    // t[verify files.sync.selective]
    fn a_session_file_is_known_only_to_the_capability_whose_tools_write_it() {
        let m = music();
        assert!(m.is_session("Track One/Track One.RPP"));
        assert!(m.is_session("01 Song/01 Song.ptx"));
        assert!(!m.is_session("Track One/Track One.rpp-bak"));
        assert!(
            !m.is_session("Track One/.rpp"),
            "a dotfile is not a session"
        );
        assert!(
            !m.is_session("Grade/cut.drp"),
            "Resolve is not music production"
        );
        assert_eq!(
            m.session("a.rpp").map(|b| (b.facet, b.atomic)),
            Some((Some(Facet::new(SESSIONS)), true))
        );

        let video = FacetMap::new([Capability::VideoProduction]);
        assert!(video.is_session("Grade/cut.drp"));
        assert!(!video.is_session("Track One/Track One.RPP"));
        assert!(
            !FacetMap::default().is_session("Track One/Track One.RPP"),
            "a project that declared nothing has no sessions to recognise"
        );
    }

    #[test]
    // t[verify files.facet.vocabulary]
    fn vocabulary_covers_both_sources() {
        let mut m = FacetMap::new([Capability::MusicProduction, Capability::VideoProduction]);
        m.map("Video ISO Files", Facet::new("iso"));
        let v = m.vocabulary();
        assert!(v.contains(&Facet::new("sessions")));
        assert!(v.contains(&Facet::new("proxies")));
        assert!(v.contains(&Facet::new("iso")));
    }
}

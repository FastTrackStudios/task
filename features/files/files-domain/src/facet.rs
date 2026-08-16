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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    MusicProduction,
    VideoProduction,
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
    ToolDir { name: "audio files", facet: "sessions", atomic: true },
    ToolDir { name: "bounced files", facet: "mixes", atomic: false },
    ToolDir { name: "session file backups", facet: "session-backups", atomic: false },
    ToolDir { name: "ara data", facet: "sessions", atomic: true },
    ToolDir { name: "clip groups", facet: "sessions", atomic: true },
    ToolDir { name: "rendered files", facet: "sessions", atomic: true },
    ToolDir { name: "video files", facet: "sessions", atomic: true },
    ToolDir { name: "wavecache.wfm", facet: "peak-cache", atomic: false },
];

/// Reaper.
const REAPER: &[ToolDir] = &[
    ToolDir { name: "media", facet: "sessions", atomic: true },
    ToolDir { name: "backups", facet: "session-backups", atomic: false },
    ToolDir { name: "peaks", facet: "peak-cache", atomic: false },
    ToolDir { name: "renders", facet: "mixes", atomic: false },
];

/// Logic — a project is a directory pretending to be a file.
const LOGIC: &[ToolDir] = &[
    ToolDir { name: "audio files", facet: "sessions", atomic: true },
    ToolDir { name: "bounces", facet: "mixes", atomic: false },
    ToolDir { name: "freeze files", facet: "peak-cache", atomic: false },
    ToolDir { name: "project file backups", facet: "session-backups", atomic: false },
];

/// DaVinci Resolve.
const RESOLVE: &[ToolDir] = &[
    ToolDir { name: "cachedclip", facet: "render-cache", atomic: false },
    ToolDir { name: "proxy", facet: "proxies", atomic: false },
    ToolDir { name: "proxies", facet: "proxies", atomic: false },
    ToolDir { name: "optimizedmedia", facet: "proxies", atomic: false },
    ToolDir { name: "gallery", facet: "grades", atomic: false },
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

    /// The facet an unmapped path syncs with.
    #[must_use]
    pub fn default_facet(&self) -> Option<&Facet> {
        self.default_facet.as_ref()
    }

    /// Directories belonging to no facet, so the mapping can be
    /// supplied. Reporting these is required: unmapped content is never
    /// hidden and its facet is never guessed.
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
    fn tool_layouts_need_no_configuration() {
        let m = music();
        // Pro Tools and Reaper, from the same capability, no setup.
        assert_eq!(m.resolve("Audio Files").facet, Some(Facet::new("sessions")));
        assert_eq!(m.resolve("Media").facet, Some(Facet::new("sessions")));
        assert_eq!(m.resolve("Backups").source, Source::ToolLayout);
    }

    #[test]
    fn tool_dirs_are_recognised_at_any_depth() {
        let m = music();
        let deep = "01 ALL THAT I AM/Audio Files";
        assert_eq!(m.resolve(deep).facet, Some(Facet::new("sessions")));
    }

    #[test]
    fn sessions_are_atomic_and_bounces_are_not() {
        let m = music();
        assert!(m.resolve("Audio Files").atomic, "a session must bring its media");
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
    fn vocabulary_covers_both_sources() {
        let mut m = FacetMap::new([Capability::MusicProduction, Capability::VideoProduction]);
        m.map("Video ISO Files", Facet::new("iso"));
        let v = m.vocabulary();
        assert!(v.contains(&Facet::new("sessions")));
        assert!(v.contains(&Facet::new("proxies")));
        assert!(v.contains(&Facet::new("iso")));
    }
}

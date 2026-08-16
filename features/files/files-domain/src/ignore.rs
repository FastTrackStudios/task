//! Ignoring — `files.ignore.layers`, `files.ignore.retained`.
//!
//! Two independent layers, neither able to defeat the other:
//!
//! - **Platform** — what an operating system leaves behind. Applies
//!   everywhere, whatever the capability. This is not a nicety: in the
//!   real tree, one 14,671-file album carries AppleDouble `._*` siblings
//!   for a large fraction of its files, because a Mac wrote to a
//!   non-Mac share.
//! - **Capability** — what an application leaves behind. `.rpp-bak`,
//!   peak and render caches, internal project versions.
//! - **Project** — this root's own additions.
//!
//! And the rule that stops this becoming a deletion feature: **ignored is
//! not deleted.** An ignored file is absent from listings and from
//! history, and stays on disk and stays synced wherever the user chose to
//! sync the directory holding it. Ignoring changes what is *shown and
//! versioned*, never what exists.

use crate::facet::Capability;

/// Which layer ignored a path. Reported rather than collapsed to a
/// boolean, because "the OS made this" and "your DAW made this" are
/// different answers to "why can't I see my file".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Platform,
    Capability,
    Project,
}

/// What an operating system leaves behind.
const PLATFORM: &[&str] = &[
    ".DS_Store",
    ".AppleDouble",
    ".LSOverride",
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
    ".TemporaryItems",
    ".DocumentRevisions-V100",
    "Thumbs.db",
    "ehthumbs.db",
    "desktop.ini",
    "$RECYCLE.BIN",
    ".directory",
    ".Trash-1000",
    "@eaDir",
];

/// What applications leave behind, per capability.
fn capability_patterns(capability: Capability) -> &'static [&'static str] {
    match capability {
        Capability::MusicProduction => &[
            "*.rpp-bak",
            "*.rpp-undo",
            "*.reapeaks",
            "*.peak",
            "*.wfm",
            "WaveCache.wfm",
            "*.asd",
            "*.pkf",
            "*.sfk",
            "*.ptxt",
        ],
        Capability::VideoProduction => &[
            "*.dvcache",
            "CacheClip",
            "CachedClip",
            ".gallery",
            "*.prproj.lock",
            "Adobe Premiere Pro Auto-Save",
            "*.pek",
            "*.cfa",
        ],
    }
}

/// True for an AppleDouble sidecar — `._Foo` beside `Foo`.
///
/// Not a glob, because `._*` would also swallow a legitimately named
/// file, and because these come in pairs: the sidecar is meaningless
/// without its partner and should never outlive it in a listing.
fn is_apple_double(name: &str) -> bool {
    name.len() > 2 && name.starts_with("._")
}

/// A very small glob: `*` at either end, literal otherwise. Deliberately
/// not a full matcher — these patterns are ours, not user-authored, and a
/// dependency for `*.rpp-bak` would be a poor trade.
fn matches(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_ascii_lowercase(), name.to_ascii_lowercase());
    match (p.strip_prefix('*'), p.strip_suffix('*')) {
        (Some(suffix), None) => n.ends_with(suffix),
        (None, Some(prefix)) => n.starts_with(prefix),
        (Some(_), Some(_)) => {
            let inner = p.trim_matches('*');
            inner.is_empty() || n.contains(inner)
        }
        (None, None) => n == p,
    }
}

/// The ignore rules in force for one root.
#[derive(Debug, Clone, Default)]
pub struct IgnoreSet {
    capabilities: Vec<Capability>,
    project: Vec<String>,
}

impl IgnoreSet {
    #[must_use]
    pub fn new(capabilities: impl IntoIterator<Item = Capability>) -> Self {
        let mut capabilities: Vec<_> = capabilities.into_iter().collect();
        capabilities.sort_unstable();
        capabilities.dedup();
        Self {
            capabilities,
            project: Vec::new(),
        }
    }

    /// This root's own additions. Cannot defeat the platform or
    /// capability layers — there is no un-ignore.
    pub fn with_project(&mut self, patterns: impl IntoIterator<Item = String>) -> &mut Self {
        self.project = patterns.into_iter().collect();
        self
    }

    pub fn set_capabilities(&mut self, capabilities: impl IntoIterator<Item = Capability>) {
        let mut c: Vec<_> = capabilities.into_iter().collect();
        c.sort_unstable();
        c.dedup();
        self.capabilities = c;
    }

    /// Which layer ignores this path, if any. Takes a full relative path
    /// and matches on its final component.
    #[must_use]
    pub fn ignored(&self, path: &str) -> Option<Layer> {
        let name = path.rsplit('/').next().unwrap_or(path);

        if is_apple_double(name) || PLATFORM.iter().any(|p| matches(p, name)) {
            return Some(Layer::Platform);
        }
        for capability in &self.capabilities {
            if capability_patterns(*capability)
                .iter()
                .any(|p| matches(p, name))
            {
                return Some(Layer::Capability);
            }
        }
        if self.project.iter().any(|p| matches(p, name)) {
            return Some(Layer::Project);
        }
        None
    }

    #[must_use]
    pub fn is_ignored(&self, path: &str) -> bool {
        self.ignored(path).is_some()
    }

    /// Partition a listing into what is shown and what is ignored.
    ///
    /// Both halves are returned on purpose: the ignored half still
    /// exists, still occupies disk, and a UI that wants to say "and 8,412
    /// hidden files" needs it.
    pub fn partition<'a, I>(&self, paths: I) -> (Vec<&'a str>, Vec<(&'a str, Layer)>)
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut shown = Vec::new();
        let mut hidden = Vec::new();
        for p in paths {
            match self.ignored(p) {
                Some(layer) => hidden.push((p, layer)),
                None => shown.push(p),
            }
        }
        (shown, hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_junk_goes_regardless_of_capability() {
        let none = IgnoreSet::default();
        assert_eq!(none.ignored(".DS_Store"), Some(Layer::Platform));
        assert_eq!(none.ignored("a/b/.DS_Store"), Some(Layer::Platform));
        assert_eq!(none.ignored("Thumbs.db"), Some(Layer::Platform));
    }

    #[test]
    fn apple_doubles_go_but_their_partners_stay() {
        let s = IgnoreSet::default();
        assert_eq!(
            s.ignored("01 ALL THAT I AM/._01 ALL THAT I AM.mp3"),
            Some(Layer::Platform)
        );
        assert_eq!(s.ignored("01 ALL THAT I AM/01 ALL THAT I AM.mp3"), None);
    }

    #[test]
    fn a_short_underscore_name_is_not_an_apple_double() {
        let s = IgnoreSet::default();
        assert_eq!(s.ignored("._"), None);
        assert_eq!(s.ignored("_notes.md"), None);
    }

    #[test]
    fn capability_patterns_need_their_capability() {
        let none = IgnoreSet::default();
        assert_eq!(none.ignored("Song.rpp-bak"), None);

        let music = IgnoreSet::new([Capability::MusicProduction]);
        assert_eq!(music.ignored("Song.rpp-bak"), Some(Layer::Capability));
        assert_eq!(music.ignored("WaveCache.wfm"), Some(Layer::Capability));
    }

    #[test]
    fn layers_are_independent() {
        // A music project still drops OS junk, and a project pattern
        // cannot rescue a platform-ignored file.
        let mut s = IgnoreSet::new([Capability::MusicProduction]);
        s.with_project(vec!["*.tmp".to_string()]);
        assert_eq!(s.ignored(".DS_Store"), Some(Layer::Platform));
        assert_eq!(s.ignored("scratch.tmp"), Some(Layer::Project));
        assert_eq!(s.ignored("Mix.wav"), None);
    }

    #[test]
    fn the_session_itself_is_never_ignored() {
        let music = IgnoreSet::new([Capability::MusicProduction]);
        assert_eq!(music.ignored("01 ALL THAT I AM 2.1 Somma.ptx"), None);
        assert_eq!(music.ignored("01 All That I Am.RPP"), None);
    }

    #[test]
    fn partition_keeps_both_halves() {
        let music = IgnoreSet::new([Capability::MusicProduction]);
        let listing = [
            "Mix.wav",
            ".DS_Store",
            "._Mix.wav",
            "Song.rpp-bak",
            "Song.RPP",
        ];
        let (shown, hidden) = music.partition(listing);
        assert_eq!(shown, vec!["Mix.wav", "Song.RPP"]);
        assert_eq!(hidden.len(), 3);
        assert_eq!(hidden[0].1, Layer::Platform);
        assert_eq!(hidden[2].1, Layer::Capability);
    }
}

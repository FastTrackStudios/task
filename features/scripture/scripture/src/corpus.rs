//! The installed corpus — Bible editions and the Strong's lexicon — read
//! from disk, and read again when what is on disk changes.
//!
//! Installing scripture (`task-server admin bible install|lexicon`, or a
//! subscription refreshing) is a separate process writing into the
//! org's `resources/` while the server that serves the org is already
//! up. Making the operator restart that server to see the result turns
//! a two-second install into an outage, so the store re-checks its
//! directories instead: at most once per [`CHECK_EVERY`], a request
//! compares what is on disk with what was loaded and reloads the side
//! that changed.
//!
//! The check is a listing and a `stat` per file — sizes and mtimes, never
//! contents — so it costs microseconds against a corpus that takes
//! seconds to parse. A reload that fails keeps the generation already
//! being served: the usual cause is a file caught half-written by an
//! install still in progress, and that install finishing changes the
//! file again, which is what triggers the next attempt. A corpus that is
//! broken for good is therefore parsed once per change, not once per
//! check.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use crate::bible::{Bible, LoadError};
use crate::lexicon::Lexicon;

/// How stale the served corpus may be after an install.
#[cfg(not(test))]
pub(crate) const CHECK_EVERY: Duration = Duration::from_secs(5);
/// Every read re-checks under test, so a test installs and reads back
/// without sleeping.
#[cfg(test)]
pub(crate) const CHECK_EVERY: Duration = Duration::ZERO;

/// One loaded generation of the corpus.
#[derive(Default)]
pub(crate) struct Corpus {
    pub bibles: Arc<BTreeMap<String, Bible>>,
    pub lexicon: Arc<Lexicon>,
}

/// Where the corpus comes from on disk. Empty ⇒ fixed (built in memory,
/// as tests do), and never re-checked.
#[derive(Clone, Default)]
pub(crate) struct Sources {
    /// Bible roots in priority order: an edition id found in an earlier
    /// root shadows the same id in a later one.
    pub bible_roots: Vec<PathBuf>,
    /// `<org>/subscribed/`: every `<domain>/bible` under it is a further
    /// root, after the fixed ones, rediscovered at each check so a
    /// subscription that arrives later is read without a restart.
    pub subscribed: Option<PathBuf>,
    /// The Strong's lexicon directory (`greek.json` + `hebrew.json`).
    pub lexicon_dir: Option<PathBuf>,
}

impl Sources {
    fn is_fixed(&self) -> bool {
        self.bible_roots.is_empty() && self.subscribed.is_none() && self.lexicon_dir.is_none()
    }

    /// The Bible roots as they stand now, in priority order.
    fn bible_roots_now(&self) -> Vec<PathBuf> {
        let mut roots = self.bible_roots.clone();
        if let Some(dir) = &self.subscribed
            && let Ok(domains) = std::fs::read_dir(dir)
        {
            let mut found: Vec<PathBuf> = domains
                .filter_map(Result::ok)
                .map(|d| d.path().join("bible"))
                .filter(|p| p.is_dir())
                .collect();
            found.sort();
            roots.extend(found);
        }
        roots
    }
}

/// What a directory held when it was last loaded: every file's path,
/// size and mtime. Equal stamps ⇒ nothing to reload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stamp(BTreeSet<(PathBuf, u64, Option<SystemTime>)>);

impl Stamp {
    /// The files directly inside `dir` (a missing directory stamps
    /// empty, so installing into it later reads as a change).
    fn add_files(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_file() {
                self.0
                    .insert((entry.path(), meta.len(), meta.modified().ok()));
            }
        }
    }

    /// Every edition folder under each root, file by file.
    fn of_bibles(roots: &[PathBuf]) -> Self {
        let mut stamp = Self::default();
        for root in roots {
            for dir in edition_dirs(root).unwrap_or_default() {
                // The folder itself counts, so an edition that is
                // created empty and filled later is two changes.
                stamp.0.insert((dir.clone(), 0, None));
                stamp.add_files(&dir);
            }
        }
        stamp
    }

    /// The files directly inside one directory.
    pub(crate) fn of_dir(dir: &Path) -> Self {
        let mut stamp = Self::default();
        stamp.add_files(dir);
        stamp
    }
}

/// Every edition directory directly under one Bible root, sorted.
///
/// A root that is not there is not an error: most orgs install nothing,
/// and a subscription that has never refreshed has no directory yet.
pub(crate) fn edition_dirs(bible_root: &Path) -> Result<Vec<PathBuf>, LoadError> {
    let entries = match std::fs::read_dir(bible_root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(LoadError::Io {
                path: bible_root.display().to_string(),
                source,
            });
        }
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    Ok(out)
}

/// Every edition under `roots`, earlier roots winning on id.
pub(crate) fn load_bibles(roots: &[PathBuf]) -> Result<BTreeMap<String, Bible>, LoadError> {
    let mut bibles = BTreeMap::new();
    for root in roots {
        for path in edition_dirs(root)? {
            let id = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if bibles.contains_key(&id) {
                continue;
            }
            let bible = Bible::load_dir(&path, id.clone())?;
            bibles.insert(id, bible);
        }
    }
    Ok(bibles)
}

/// What was on disk at the last load, and when it was last compared.
#[derive(Default)]
struct Checked {
    at: Option<Instant>,
    bibles: Option<Stamp>,
    lexicon: Option<Stamp>,
}

/// The served corpus plus what it was loaded from. Cheap to clone: every
/// clone of a [`crate::Store`] shares one.
#[derive(Clone, Default)]
pub(crate) struct Live {
    sources: Arc<Sources>,
    current: Arc<RwLock<Arc<Corpus>>>,
    checked: Arc<Mutex<Checked>>,
}

impl Live {
    /// A corpus held in memory and never re-read.
    pub fn fixed(bibles: BTreeMap<String, Bible>) -> Self {
        Self {
            sources: Arc::new(Sources::default()),
            current: Arc::new(RwLock::new(Arc::new(Corpus {
                bibles: Arc::new(bibles),
                lexicon: Arc::new(Lexicon::default()),
            }))),
            checked: Arc::default(),
        }
    }

    /// Load the Bible side from `roots` now, and keep re-reading them.
    ///
    /// # Errors
    ///
    /// A root that exists and cannot be read, or an edition that will
    /// not parse — at start-up a broken corpus is a configuration error,
    /// not something to serve around.
    pub fn bibles_from(roots: Vec<PathBuf>) -> Result<Self, LoadError> {
        let stamp = Stamp::of_bibles(&roots);
        let bibles = load_bibles(&roots)?;
        let mut live = Self::fixed(bibles);
        Arc::make_mut(&mut live.sources).bible_roots = roots;
        live.checked.lock().expect("corpus check poisoned").bibles = Some(stamp);
        Ok(live)
    }

    /// Load the lexicon from `dir` now, and keep re-reading it.
    ///
    /// # Errors
    ///
    /// A lexicon file that exists and will not parse.
    pub fn with_lexicon_dir(mut self, dir: PathBuf) -> Result<Self, LoadError> {
        let stamp = Stamp::of_dir(&dir);
        let lexicon = Lexicon::load_dir(&dir)?;
        self.swap(|c| Corpus {
            bibles: c.bibles.clone(),
            lexicon: Arc::new(lexicon),
        });
        Arc::make_mut(&mut self.sources).lexicon_dir = Some(dir);
        self.checked.lock().expect("corpus check poisoned").lexicon = Some(stamp);
        Ok(self)
    }

    /// Replace the lexicon with a fixed one (tests, custom wiring); stop
    /// re-reading any lexicon directory.
    pub fn with_lexicon(mut self, lexicon: Lexicon) -> Self {
        self.swap(|c| Corpus {
            bibles: c.bibles.clone(),
            lexicon: Arc::new(lexicon),
        });
        Arc::make_mut(&mut self.sources).lexicon_dir = None;
        self
    }

    /// Also read every `<domain>/bible` under `dir`, now and as
    /// subscriptions come and go.
    pub fn with_subscribed(mut self, dir: PathBuf) -> Self {
        Arc::make_mut(&mut self.sources).subscribed = Some(dir);
        // The fixed roots were stamped without these; make the next
        // read compare afresh so they are picked up at once.
        let mut checked = self.checked.lock().expect("corpus check poisoned");
        checked.at = None;
        checked.bibles = None;
        drop(checked);
        self
    }

    /// The corpus to answer one request with: re-checked first when the
    /// last check is older than [`CHECK_EVERY`].
    pub fn get(&self) -> Arc<Corpus> {
        self.refresh();
        self.current.read().expect("corpus poisoned").clone()
    }

    fn refresh(&self) {
        if self.sources.is_fixed() {
            return;
        }
        // Another request is already checking: answer from the
        // generation in hand rather than queue behind a reload.
        let Ok(mut checked) = self.checked.try_lock() else {
            return;
        };
        if checked.at.is_some_and(|at| at.elapsed() < CHECK_EVERY) {
            return;
        }
        checked.at = Some(Instant::now());

        let roots = self.sources.bible_roots_now();
        let bibles_now = Stamp::of_bibles(&roots);
        let bibles = (checked.bibles.as_ref() != Some(&bibles_now)).then(|| {
            // Recorded whether or not the load succeeds: a failure waits
            // for the files to change again rather than re-parsing every
            // edition at every check.
            checked.bibles = Some(bibles_now);
            load_bibles(&roots).ok()
        });

        let lexicon = self.sources.lexicon_dir.as_ref().and_then(|dir| {
            let now = Stamp::of_dir(dir);
            (checked.lexicon.as_ref() != Some(&now)).then(|| {
                checked.lexicon = Some(now);
                Lexicon::load_dir(dir).ok()
            })
        });

        let bibles = bibles.flatten();
        let lexicon = lexicon.flatten();
        if bibles.is_none() && lexicon.is_none() {
            return;
        }
        self.swap(|c| Corpus {
            bibles: bibles.map_or_else(|| c.bibles.clone(), Arc::new),
            lexicon: lexicon.map_or_else(|| c.lexicon.clone(), Arc::new),
        });
    }

    fn swap(&self, next: impl FnOnce(&Corpus) -> Corpus) {
        let mut current = self.current.write().expect("corpus poisoned");
        *current = Arc::new(next(&current));
    }
}

#[cfg(test)]
mod tests {
    use scripture_proto::{ScriptureError, ScriptureService};

    use crate::Store;
    use crate::usfm::tests::SAMPLE;

    const GOD: &str =
        r#"{"G2316":{"lemma":"θεός","translit":"theós","strongs_def":" a deity","kjv_def":"God"}}"#;

    fn ids(store: &Store) -> Vec<String> {
        store
            .translations()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect()
    }

    /// A lexicon installed into the directory after the server started
    /// is read on the next request — the restart `admin bible lexicon`
    /// used to ask for.
    #[test]
    fn a_lexicon_installed_while_running_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let lex = dir.path().join("lexicon").join("strongs");
        let store = Store::load_resource_roots([dir.path().join("bible").as_path()])
            .unwrap()
            .with_lexicon_dir(&lex)
            .unwrap();
        assert!(matches!(
            store.lexicon("G2316"),
            Err(ScriptureError::NotFound(_))
        ));

        std::fs::create_dir_all(&lex).unwrap();
        std::fs::write(lex.join("greek.json"), GOD).unwrap();

        assert_eq!(store.lexicon("G2316").unwrap().kjv_def, "God");
    }

    /// An edition installed under the Bible root while running joins
    /// the translations, and its text reads.
    #[test]
    fn an_edition_installed_while_running_is_served() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bible");
        let store = Store::load_resource_roots([root.as_path()]).unwrap();
        assert!(ids(&store).is_empty());

        std::fs::create_dir_all(root.join("WEB")).unwrap();
        std::fs::write(root.join("WEB").join("44-JHN.usfm"), SAMPLE).unwrap();

        assert_eq!(ids(&store), ["WEB"]);
        assert_eq!(
            store.word_study("WEB", "John 3:16").unwrap()[0].surface,
            "For"
        );
    }

    /// A subscription that arrives after start brings its editions with
    /// it, behind the ones this org installed itself.
    #[test]
    fn a_subscription_arriving_while_running_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let subscribed = dir.path().join("subscribed");
        let store = Store::load_resource_roots([dir.path().join("bible").as_path()])
            .unwrap()
            .with_subscribed_bibles(&subscribed);
        assert!(ids(&store).is_empty());

        let edition = subscribed.join("studio.test").join("bible").join("WEB");
        std::fs::create_dir_all(&edition).unwrap();
        std::fs::write(edition.join("44-JHN.usfm"), SAMPLE).unwrap();

        assert_eq!(ids(&store), ["WEB"]);
    }

    /// A file caught half-written leaves the generation already served
    /// in place; the write finishing is what loads it.
    #[test]
    fn a_lexicon_that_will_not_parse_keeps_the_one_in_service() {
        let dir = tempfile::tempdir().unwrap();
        let lex = dir.path().join("strongs");
        std::fs::create_dir_all(&lex).unwrap();
        std::fs::write(lex.join("greek.json"), GOD).unwrap();
        let store = Store::from_bibles([]).with_lexicon_dir(&lex).unwrap();
        assert!(store.lexicon("G2316").is_ok());

        std::fs::write(lex.join("greek.json"), &GOD[..12]).unwrap();
        assert!(
            store.lexicon("G2316").is_ok(),
            "a truncated file must not empty the lexicon being served"
        );

        let both = GOD.replace(
            "}}",
            r#"},"G25":{"lemma":"ἀγαπάω","translit":"agapáō","strongs_def":" to love","kjv_def":"love"}}"#,
        );
        std::fs::write(lex.join("greek.json"), both).unwrap();
        assert_eq!(store.lexicon("G25").unwrap().kjv_def, "love");
    }

    /// An original-language edition is cached after its first read; a
    /// reinstall replaces what the cache serves.
    #[test]
    fn a_reinstalled_original_edition_replaces_the_cached_one() {
        use crate::original::{OrigText, OrigWord};
        use scripture_proto::VerseId;

        let root = tempfile::tempdir().unwrap();
        let install = |gloss: &str| {
            let dir = root.path().join("OSHB");
            std::fs::create_dir_all(&dir).unwrap();
            let text = OrigText::from_verses([(
                VerseId::parse("Genesis 1:1").unwrap(),
                vec![OrigWord {
                    word: "בְּרֵאשִׁית".into(),
                    translit: String::new(),
                    lemma: String::new(),
                    strong: "H7225".into(),
                    morph: String::new(),
                    gloss: gloss.into(),
                }],
            )]);
            std::fs::write(dir.join("text.jsonl"), text.to_jsonl()).unwrap();
            std::fs::write(
                dir.join("meta.json"),
                r#"{"id":"OSHB","name":"Hebrew","language":"Hebrew","license":"x"}"#,
            )
            .unwrap();
        };
        let store = Store::from_bibles([]).with_originals_root(root.path().to_path_buf());

        install("beginning");
        assert_eq!(
            store.interlinear("OSHB", "Genesis 1:1").unwrap()[0].gloss,
            "beginning"
        );
        install("in the beginning");
        assert_eq!(
            store.interlinear("OSHB", "Genesis 1:1").unwrap()[0].gloss,
            "in the beginning"
        );
    }
}

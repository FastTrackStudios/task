//! One concept: **Keyflow, as a client of Task** — and the collections an
//! app arranges its library into.
//!
//! [`Keyflow`] is a line-for-line mirror of keyflow's
//! `apps/web/src/library/vox.rs`: the same RPCs, with the same arguments in
//! the same shapes — a song created with an empty slug so the server
//! derives it, a chart that names its song as a `song:<slug>` token, a song
//! list that is a collection of kind [`SONGLIST_KIND`], an item appended
//! with `after: None` — and [`default_chart`] is keyflow's rule for which
//! chart a song opens. Chapters that exercise Keyflow's library go through
//! it, so a change in Task that would break Keyflow breaks a chapter first,
//! and there is one mirror to keep true rather than one per chapter.
//!
//! [`Collections`] is the primitive under setlists and shows: a collection
//! of a caller-chosen kind, holding references in order. Keyflow has no
//! setlists of its own yet — that is Session's half of the job — so this is
//! what a chapter uses for them.

use collection_proto::{Collection, CollectionKind, Placement};
use links_proto::{NodeKind, NodeRef};
use resources_proto::{ChartDoc, ChartSummary, SongDoc};

use crate::client::Session;

/// Keyflow's `SONGLIST_KIND`.
pub const SONGLIST_KIND: &str = "songlist";

/// The chart to open for a song that names no arrangement: the one the
/// server marks default, else the first. Keyflow's `default_chart`,
/// verbatim — the fallback is there for imported libraries, where a song's
/// only chart may carry no flag.
#[must_use]
pub fn default_chart(charts: &[ChartSummary]) -> Option<&ChartSummary> {
    charts
        .iter()
        .find(|chart| chart.is_default)
        .or_else(|| charts.first())
}

/// One chart as Keyflow's editor saves it.
pub struct Chart<'a> {
    /// Empty for a new chart; the stored slug to save an edit.
    pub slug: &'a str,
    pub title: &'a str,
    pub song: &'a str,
    pub arrangement: &'a str,
    pub key: &'a str,
    pub source: &'a str,
    /// Ask to become the song's default. `false` is *no opinion*.
    pub make_default: bool,
}

/// Keyflow, signed in as somebody, in one org. One method per call
/// keyflow's library makes; nothing here is a convenience Keyflow does not
/// have.
pub struct Keyflow<'a> {
    pub who: &'a Session,
    pub org: String,
}

impl Keyflow<'_> {
    /// `create_song`: an empty slug so the server derives it, the title and
    /// key trimmed the way the editor's fields are.
    pub async fn create_song(&self, title: &str, key: &str) -> String {
        self.who
            .resources()
            .await
            .upsert_song(SongDoc {
                slug: String::new(),
                title: title.trim().to_owned(),
                writers: Vec::new(),
                key: key.trim().to_owned(),
                tags: Vec::new(),
                updated_at: String::new(),
            })
            .await
            .unwrap_or_else(|e| panic!("Keyflow creates `{title}`: {e:?}"))
            .slug
    }

    /// `save_chart`: the chart names its song as a `song:<slug>` token.
    pub async fn save_chart(&self, chart: Chart<'_>) -> String {
        self.who
            .resources()
            .await
            .upsert_chart(ChartDoc {
                slug: chart.slug.to_owned(),
                title: chart.title.to_owned(),
                source: chart.source.to_owned(),
                key: chart.key.to_owned(),
                song: NodeRef::song(chart.song).to_token(),
                arrangement: chart.arrangement.to_owned(),
                is_default: chart.make_default,
                updated_at: "2026-09-21T09:00:00Z".to_owned(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("Keyflow saves `{}`: {e:?}", chart.title))
            .slug
    }

    pub async fn list_charts(&self, song: &str) -> Vec<ChartSummary> {
        self.who
            .resources()
            .await
            .list_charts(song.to_owned())
            .await
            .expect("list charts")
    }

    /// What tapping a song does: its charts, the default one, its source.
    pub async fn open_song(&self, song: &str) -> ChartDoc {
        let charts = self.list_charts(song).await;
        let chart = default_chart(&charts)
            .unwrap_or_else(|| panic!("`{song}` has no chart to open"))
            .slug
            .clone();
        self.who
            .resources()
            .await
            .chart(chart.clone())
            .await
            .unwrap_or_else(|e| panic!("open `{chart}`: {e:?}"))
    }

    pub async fn list_songlists(&self) -> Vec<Collection> {
        Collections {
            who: self.who,
            org: self.org.clone(),
        }
        .list(SONGLIST_KIND)
        .await
    }

    pub async fn create_songlist(&self, title: &str) -> Collection {
        Collections {
            who: self.who,
            org: self.org.clone(),
        }
        .create(title.trim(), SONGLIST_KIND)
        .await
    }

    pub async fn add_to_songlist(&self, list: &str, song: &str) -> Collection {
        self.who
            .collections()
            .await
            .add_item(Placement {
                collection_id: list.to_owned(),
                node: NodeRef::song(song),
                after: None,
            })
            .await
            .unwrap_or_else(|e| panic!("add `{song}` to a list: {e:?}"))
    }

    pub async fn remove_from_songlist(&self, list: &str, song: &str) -> Collection {
        self.who
            .collections()
            .await
            .remove_item(list.to_owned(), NodeRef::song(song))
            .await
            .unwrap_or_else(|e| panic!("remove `{song}` from a list: {e:?}"))
    }
}

/// Collections of any kind — setlists, shows — in one org.
pub struct Collections<'a> {
    pub who: &'a Session,
    pub org: String,
}

impl Collections<'_> {
    pub async fn create(&self, title: &str, kind: &str) -> Collection {
        self.who
            .collections()
            .await
            .create(
                self.org.clone(),
                title.to_owned(),
                CollectionKind::new(kind),
            )
            .await
            .unwrap_or_else(|e| panic!("create {kind} `{title}`: {e:?}"))
    }

    pub async fn list(&self, kind: &str) -> Vec<Collection> {
        self.who
            .collections()
            .await
            .list(self.org.clone(), Some(CollectionKind::new(kind)))
            .await
            .unwrap_or_else(|e| panic!("list {kind}s: {e:?}"))
    }

    /// The one titled `title` of this kind; panics if there is none.
    pub async fn named(&self, kind: &str, title: &str) -> Collection {
        self.list(kind)
            .await
            .into_iter()
            .find(|c| c.title == title)
            .unwrap_or_else(|| panic!("no {kind} titled `{title}`"))
    }

    pub async fn get(&self, id: &str) -> Collection {
        self.who
            .collections()
            .await
            .get(id.to_owned())
            .await
            .expect("get")
            .unwrap_or_else(|| panic!("collection `{id}` is gone"))
    }

    /// Put `node` straight after `after`, or at the end.
    pub async fn place(
        &self,
        into: &str,
        node: NodeRef,
        after: Option<NodeRef>,
    ) -> Result<Collection, collection_proto::CollectionError> {
        self.who
            .collections()
            .await
            .add_item(Placement {
                collection_id: into.to_owned(),
                node,
                after,
            })
            .await
            .map_err(|e| match e {
                architect::vox::VoxError::User(e) => *e,
                other => collection_proto::CollectionError::Io(other.to_string()),
            })
    }

    pub async fn reorder(&self, into: &str, node: NodeRef, after: Option<NodeRef>) -> Collection {
        self.who
            .collections()
            .await
            .reorder(Placement {
                collection_id: into.to_owned(),
                node,
                after,
            })
            .await
            .expect("reorder")
    }
}

/// The ids a collection holds, in its order.
#[must_use]
pub fn ids(c: &Collection) -> Vec<String> {
    c.items.iter().map(|i| i.node.id.clone()).collect()
}

/// A reference to a collection — what a show holds of a setlist.
#[must_use]
pub fn collection(id: &str) -> NodeRef {
    NodeRef::new(NodeKind::Collection, id)
}

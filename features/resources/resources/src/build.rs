//! Turn annotation *specs* into the two artifacts that make an annotation
//! real: a row in the resource's [`AnnotationFile`] sidecar (the geometry
//! / label) and one or more [`TypedLink`]s in the shared link graph (the
//! `song:slug#anchor → verse:osis` edges). This is the headless,
//! UI-free authoring path — the same shape a future "annotate" button
//! would call.

use links_proto::{Confidence, NodeKind, NodeRef, Provenance, Relation, TypedLink, Visibility};

use crate::types::{Annotation, AnnotationFile, Geometry};

/// One target of an annotation — the node a span links to (a verse, a
/// topic, a wiki page…), with the relation + confidence of *that* link.
pub struct Target {
    pub node: NodeRef,
    pub relation: Relation,
    pub confidence: Confidence,
}

impl Target {
    #[must_use]
    pub fn node(node: NodeRef, relation: Relation, confidence: Confidence) -> Self {
        Self {
            node,
            relation,
            confidence,
        }
    }

    /// A verse target — `osis` is a verse id or range (`Luke.15.20`,
    /// `Matt.7.7-Matt.7.8`).
    #[must_use]
    pub fn verse(osis: impl Into<String>, relation: Relation, confidence: Confidence) -> Self {
        Self::node(NodeRef::verse(osis), relation, confidence)
    }

    /// A topic / tag target (`topic:humility`).
    #[must_use]
    pub fn topic(slug: impl Into<String>, relation: Relation, confidence: Confidence) -> Self {
        Self::node(NodeRef::new(NodeKind::Topic, slug), relation, confidence)
    }
}

/// A single annotation to author: an anchor on the resource, its
/// sidecar metadata, and the targets it links to.
pub struct AnnotationSpec {
    /// Anchor string — the part after `#` (`chorus.L1`, `t:90`, `p3.h2`).
    pub anchor: String,
    pub label: String,
    pub text: String,
    pub color: Option<String>,
    pub geometry: Option<Geometry>,
    pub targets: Vec<Target>,
    /// Free-text note carried on every link this annotation emits.
    pub note: String,
}

impl AnnotationSpec {
    /// A lyric/text span annotation (no geometry — located by the anchor
    /// label against the resource body).
    #[must_use]
    pub fn span(anchor: impl Into<String>, text: impl Into<String>, targets: Vec<Target>) -> Self {
        let text = text.into();
        Self {
            anchor: anchor.into(),
            label: text.clone(),
            text,
            color: None,
            geometry: None,
            targets,
            note: String::new(),
        }
    }

    /// A recording-moment annotation (anchor `t:<secs>`), with the
    /// timestamp also recorded as sidecar [`Geometry::Timestamp`] so a
    /// player can render a seek chip without re-parsing the anchor.
    #[must_use]
    pub fn moment(secs: u32, text: impl Into<String>, targets: Vec<Target>) -> Self {
        let text = text.into();
        Self {
            anchor: format!("t:{secs}"),
            label: text.clone(),
            text,
            color: None,
            geometry: Some(Geometry::Timestamp { secs }),
            targets,
            note: String::new(),
        }
    }

    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }
}

/// The output of [`build`]: the links to persist into `links.jsonl` and
/// the sidecar to write next to the resource.
pub struct Built {
    pub links: Vec<TypedLink>,
    pub sidecar: AnnotationFile,
}

/// Materialise annotation specs for a resource. Every spec becomes one
/// sidecar row (keyed by anchor) plus one [`TypedLink`] per target,
/// sourced from `kind:slug#anchor`. `visibility` applies to all emitted
/// links (allusion scholarship is typically `Public`); `created_by` /
/// `source_ref` stamp provenance (`created_at` is filled by the store on
/// write).
#[must_use]
pub fn build(
    source: NodeRef,
    specs: Vec<AnnotationSpec>,
    visibility: Visibility,
    created_by: &str,
    source_ref: &str,
) -> Built {
    let mut sidecar = AnnotationFile::new(&source.id);
    let mut links = Vec::new();

    for spec in specs {
        let anchored = source.clone().with_anchor(&spec.anchor);
        for t in &spec.targets {
            let mut link =
                TypedLink::new(anchored.clone(), t.node.clone(), t.relation, t.confidence);
            link.visibility = visibility;
            link.note = spec.note.clone();
            link.provenance = Provenance {
                created_at: String::new(),
                created_by: created_by.to_string(),
                source_ref: source_ref.to_string(),
                derived: true,
            };
            links.push(link);
        }
        sidecar.upsert(Annotation {
            anchor: spec.anchor,
            label: spec.label,
            text: spec.text,
            color: spec.color,
            geometry: spec.geometry,
        });
    }

    Built { links, sidecar }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_links_and_sidecar() {
        let built = build(
            NodeRef::song("a-forgiving-god"),
            vec![
                AnnotationSpec::span(
                    "verse2.L4",
                    "My Father ran to me, said welcome home",
                    vec![
                        Target::verse("Luke.15.20", Relation::AlludesTo, Confidence::Certain),
                        Target::verse("Luke.15.21", Relation::AlludesTo, Confidence::Likely),
                    ],
                )
                .with_note("the father runs to the prodigal"),
            ],
            Visibility::Public,
            "test",
            "a-forgiving-god-analysis",
        );

        assert_eq!(built.links.len(), 2);
        assert_eq!(built.sidecar.annotations.len(), 1);
        let l = &built.links[0];
        assert_eq!(l.source.to_token(), "song:a-forgiving-god#verse2.L4");
        assert_eq!(l.target.to_token(), "verse:Luke.15.20");
        assert_eq!(l.visibility, Visibility::Public);
        assert_eq!(l.provenance.source_ref, "a-forgiving-god-analysis");
        assert!(l.provenance.derived);
        assert_eq!(l.note, "the father runs to the prodigal");
        assert_eq!(
            built.sidecar.get("verse2.L4").unwrap().text,
            "My Father ran to me, said welcome home"
        );
    }
}

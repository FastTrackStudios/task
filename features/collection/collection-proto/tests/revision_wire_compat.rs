//! A collection's `revision` is a new field, and apps already shipped
//! read collections without it — and stores written before it hold none.
//! vox decodes through a writer→reader compat program; these run that
//! program (`vox_phon::build_decode_program`) over both directions.

use collection_proto::{Collection, CollectionItem, CollectionKind, NodeRef};

mod legacy {
    use collection_proto::{CollectionItem, CollectionKind};
    use facet::Facet;

    /// `Collection` before revisions.
    #[derive(Facet, Debug)]
    pub struct Collection {
        pub id: String,
        pub org: String,
        pub title: String,
        pub kind: CollectionKind,
        pub items: Vec<CollectionItem>,
    }
}

fn decode<W, R>(old: &W) -> R
where
    W: facet::Facet<'static>,
    R: facet::Facet<'static>,
{
    let schema = vox_phon::schema_bytes::<W>().expect("writer schema");
    let bundle = vox_phon::parse_schema_bytes(&schema).expect("parse writer schema");
    let wire = vox_phon::to_vec(old).expect("encode");
    let program = vox_phon::build_decode_program::<R>(&bundle)
        .expect("the older shape must stay compatible");
    vox_phon::decode_owned_with_program(&program, &wire).expect("decode")
}

fn items() -> Vec<CollectionItem> {
    vec![CollectionItem::new(NodeRef::song("opener"), "m")]
}

#[test]
fn an_old_app_reads_a_collection_with_a_revision() {
    let got: legacy::Collection = decode(&Collection {
        id: "set".into(),
        org: "band".into(),
        title: "Friday".into(),
        kind: CollectionKind::new("setlist"),
        items: items(),
        revision: 7,
    });
    assert_eq!(got.title, "Friday");
    assert_eq!(got.items, items());
}

#[test]
fn a_collection_without_a_revision_reads_as_revision_zero() {
    let got: Collection = decode(&legacy::Collection {
        id: "set".into(),
        org: "band".into(),
        title: "Friday".into(),
        kind: CollectionKind::new("setlist"),
        items: items(),
    });
    assert_eq!(got.revision, 0);
    assert_eq!(got.items, items());
}

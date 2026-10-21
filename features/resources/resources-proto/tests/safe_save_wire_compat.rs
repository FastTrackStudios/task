//! Revisions are new fields, and apps already shipped do not send them.
//!
//! Keyflow, Session and the desktop builds already in people's hands
//! send `ChartDoc` and `SongDoc` without `revision`, and read
//! `ChartUpsert` / `SongUpsert` without it. vox decodes through a
//! writer→reader compat program, and a field only the reader has is
//! allowed only with a default — so these run that program
//! (`vox_phon::build_decode_program`) over the old shapes both ways.
//! An old save decodes as a save with no revision, which is the
//! unconditional save it always was.

use resources_proto::{ChartDoc, ChartUpsert, SongDoc};

/// The shapes as apps before revisions send and read them.
mod legacy {
    use facet::Facet;

    #[derive(Facet)]
    pub struct ChartDoc {
        pub slug: String,
        pub title: String,
        pub source: String,
        pub key: String,
        pub notation: String,
        pub sections: Vec<String>,
        pub song: String,
        pub arrangement: String,
        pub is_default: bool,
        pub updated_at: String,
    }

    #[derive(Facet)]
    pub struct SongDoc {
        pub slug: String,
        pub title: String,
        pub writers: Vec<String>,
        pub key: String,
        pub tags: Vec<String>,
        pub updated_at: String,
    }

    #[derive(Facet, Debug)]
    pub struct ChartUpsert {
        pub slug: String,
        pub rel_path: String,
        pub created: bool,
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

#[test]
fn an_old_chart_save_decodes_with_no_revision() {
    let got: ChartDoc = decode(&legacy::ChartDoc {
        slug: "opener".into(),
        title: "Opener".into(),
        source: "| G |".into(),
        key: "G".into(),
        notation: String::new(),
        sections: vec!["verse".into()],
        song: "song:opener".into(),
        arrangement: String::new(),
        is_default: true,
        updated_at: String::new(),
    });
    assert_eq!(
        got,
        ChartDoc {
            slug: "opener".into(),
            title: "Opener".into(),
            source: "| G |".into(),
            key: "G".into(),
            sections: vec!["verse".into()],
            song: "song:opener".into(),
            is_default: true,
            ..ChartDoc::default()
        }
    );
    assert!(got.revision.is_empty(), "no revision: the old, unconditional save");
}

#[test]
fn an_old_song_save_decodes_with_no_revision() {
    let got: SongDoc = decode(&legacy::SongDoc {
        slug: "opener".into(),
        title: "Opener".into(),
        writers: vec![],
        key: "G".into(),
        tags: vec![],
        updated_at: String::new(),
    });
    assert_eq!(got.title, "Opener");
    assert!(got.revision.is_empty());
}

#[test]
fn an_old_app_reads_a_new_upsert() {
    let got: legacy::ChartUpsert = decode(&ChartUpsert {
        slug: "opener".into(),
        rel_path: "opener.md".into(),
        created: false,
        revision: "ab".repeat(32),
    });
    assert_eq!(got.slug, "opener");
    assert!(!got.created);
}

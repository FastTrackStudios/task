#![allow(clippy::large_futures)]
//! End-to-end check for ADR 0003's other three asset lanes — patches,
//! samples and lighting — and for the claim the decision rests on:
//! *nothing new is built for libraries*.
//!
//! Each lane is four ordinary RPCs on the resources tier, and a library
//! of any of them is an existing `Collection` of kind `Library` over
//! `patch:` / `sample:` / `lighting:` node references. No asset
//! service, no asset store, no per-app lane — which is what this file
//! asserts by using only what was already mounted.
//!
//! It also pins the two honest halves of the decision:
//!
//! - deleting an asset does not reach into the collections that
//!   referenced it. The reference stays, parses, and reads as
//!   unresolved — a legible state, not an error.
//! - a sample's audio never passes through this lane. The manifest says
//!   where the bytes are in a File Root, and that is all it does.

// This binary uses the boot helpers only; the seed constants are for
// the vault suites.
#[allow(dead_code)]
mod support;

use resources_proto::{ContentRef, LightingDoc, PatchDoc, ResourcesServiceClient, SampleDoc};

const PAD: &str = "{\"blocks\":[\"reverb\"]}\n";
const LEAD: &str = "{\"blocks\":[\"drive\",\"delay\"]}\n";
const KICK: &str = "{\"mic\":\"D112\"}\n";
const SNARE: &str = "{\"mic\":\"SM57\"}\n";
const SET: &str = "{\"cues\":[{\"label\":\"12\"}]}\n";
const ENCORE: &str = "{\"cues\":[{\"label\":\"1\"}]}\n";

fn patch(title: &str, body: &str) -> PatchDoc {
    PatchDoc {
        slug: String::new(),
        title: title.into(),
        rig: "helix".into(),
        tags: vec!["e2e".into()],
        body: body.into(),
        content: ContentRef::default(),
        updated_at: "2026-09-05T10:00:00Z".into(),
    }
}

fn sample(title: &str, body: &str) -> SampleDoc {
    SampleDoc {
        slug: String::new(),
        title: title.into(),
        tags: vec!["e2e".into()],
        duration_secs: 2,
        sample_rate: 48_000,
        body: body.into(),
        content: ContentRef {
            root_id: "acme-library".into(),
            path: format!("Samples/{title}.wav"),
        },
        updated_at: "2026-09-05T10:00:00Z".into(),
    }
}

fn lighting(title: &str, body: &str, scope: &str) -> LightingDoc {
    LightingDoc {
        slug: String::new(),
        title: title.into(),
        scope: scope.into(),
        cues: vec!["12".into(), "13".into()],
        body: body.into(),
        content: ContentRef::default(),
        updated_at: "2026-09-05T10:00:00Z".into(),
    }
}

/// Every slug a lane currently holds. The seed plants one of each kind,
/// so a test asserts what it wrote is there rather than that nothing
/// else is.
async fn slugs(be: &ResourcesServiceClient, kind: &str) -> Vec<String> {
    match kind {
        "patch" => be
            .list_patches()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect(),
        "sample" => be
            .list_samples()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.slug)
            .collect(),
        _ => be
            .list_lighting()
            .await
            .unwrap()
            .into_iter()
            .map(|l| l.slug)
            .collect(),
    }
}

/// Two patches saved, listed, read back verbatim, gathered into a
/// `Library`, and one deleted out from under it.
#[tokio::test(flavor = "multi_thread")]
async fn patches_round_trip_and_a_library_collects_them() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let be: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    let first = be.upsert_patch(patch("Warm Pad", PAD)).await.unwrap();
    assert_eq!(first.slug, "warm-pad");
    assert_eq!(first.rel_path, "patches/warm-pad/patch.md");
    assert!(first.created);
    let second = be.upsert_patch(patch("Bright Lead", LEAD)).await.unwrap();
    assert_eq!(second.slug, "bright-lead");

    // On disk: a directory per patch, the definition beside an ordinary
    // `type: resource` page.
    let org_root = org_proto::DataRoot::from_env().unwrap().org(support::ORG);
    let dir = org_root.resources_dir().join("patches/warm-pad");
    assert_eq!(
        std::fs::read_to_string(dir.join("patch.json")).unwrap(),
        PAD
    );
    let manifest = std::fs::read_to_string(dir.join("patch.md")).unwrap();
    assert!(manifest.contains("resource_kind: patch"), "{manifest}");
    assert!(manifest.contains("rig: helix"), "{manifest}");

    let listed = slugs(&be, "patch").await;
    assert!(listed.contains(&"warm-pad".to_owned()), "{listed:?}");
    assert!(listed.contains(&"bright-lead".to_owned()), "{listed:?}");
    let one = be.patch("warm-pad".to_owned()).await.unwrap();
    assert_eq!(one.body, PAD, "the definition round-trips byte for byte");
    assert_eq!(one.tags, ["e2e"]);

    collect_and_delete(&url, &be, "patch", "warm-pad", "bright-lead").await;
    assert!(be.patch("warm-pad".to_owned()).await.is_err());
    assert!(!be.delete_patch("warm-pad".to_owned()).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn samples_round_trip_and_a_library_collects_them() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let be: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    let first = be.upsert_sample(sample("Room Kick", KICK)).await.unwrap();
    assert_eq!(first.slug, "room-kick");
    assert_eq!(first.rel_path, "samples/room-kick/sample.md");
    be.upsert_sample(sample("Room Snare", SNARE)).await.unwrap();

    // Two files in the directory, and neither of them is audio. That
    // is the whole point of this lane.
    let org_root = org_proto::DataRoot::from_env().unwrap().org(support::ORG);
    let dir = org_root.resources_dir().join("samples/room-kick");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["sample.json", "sample.md"]);

    let listed = slugs(&be, "sample").await;
    assert!(listed.contains(&"room-kick".to_owned()), "{listed:?}");
    assert!(listed.contains(&"room-snare".to_owned()), "{listed:?}");

    let one = be.sample("room-kick".to_owned()).await.unwrap();
    assert_eq!(one.body, KICK);
    assert_eq!(one.duration_secs, 2);
    assert_eq!(one.sample_rate, 48_000);
    // The manifest names where the bytes are; it does not hold them.
    assert!(one.content.is_bound());
    assert_eq!(one.content.root_id, "acme-library");
    assert_eq!(one.content.path, "Samples/Room Kick.wav");

    collect_and_delete(&url, &be, "sample", "room-kick", "room-snare").await;
    assert!(be.sample("room-kick".to_owned()).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn lighting_round_trips_and_a_library_collects_it() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let be: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    let first = be
        .upsert_lighting(lighting("Sunday Set", SET, "setlist"))
        .await
        .unwrap();
    assert_eq!(first.slug, "sunday-set");
    assert_eq!(first.rel_path, "lighting/sunday-set/show.md");
    be.upsert_lighting(lighting("Encore", ENCORE, "song"))
        .await
        .unwrap();

    let org_root = org_proto::DataRoot::from_env().unwrap().org(support::ORG);
    let dir = org_root.resources_dir().join("lighting/sunday-set");
    assert_eq!(std::fs::read_to_string(dir.join("show.json")).unwrap(), SET);
    let manifest = std::fs::read_to_string(dir.join("show.md")).unwrap();
    assert!(manifest.contains("scope: setlist"), "{manifest}");
    assert!(manifest.contains("- '12'"), "declared cues: {manifest}");

    let listed = slugs(&be, "lighting").await;
    assert!(listed.contains(&"sunday-set".to_owned()), "{listed:?}");
    assert!(listed.contains(&"encore".to_owned()), "{listed:?}");

    let one = be.lighting("sunday-set".to_owned()).await.unwrap();
    assert_eq!(one.body, SET);
    assert_eq!(one.scope, "setlist");
    assert_eq!(one.cues, ["12", "13"], "only these anchor as #cue:<label>");

    // A scope outside the vocabulary is refused over the wire too, and
    // writes nothing.
    let before = slugs(&be, "lighting").await.len();
    assert!(
        be.upsert_lighting(lighting("Whole Tour", ENCORE, "tour"))
            .await
            .is_err(),
        "`tour` is not a scope"
    );
    assert_eq!(slugs(&be, "lighting").await.len(), before);

    collect_and_delete(&url, &be, "lighting", "sunday-set", "encore").await;
    assert!(be.lighting("sunday-set".to_owned()).await.is_err());
}

/// The shared half of all three: put the two assets in a `Library`,
/// delete the first, and check the reference outlives the file.
///
/// This is the claim ADR 0003 makes about libraries — the collection is
/// the library, and resolution is the reader's problem — so it is
/// exercised identically for every kind rather than paraphrased three
/// times.
#[allow(unused_variables)]
async fn collect_and_delete(
    url: &str,
    be: &ResourcesServiceClient,
    kind: &str,
    gone: &str,
    kept: &str,
) {
    #[cfg(feature = "plugin-fasttrackstudio")]
    {
        use collection_proto::{CollectionKind, CollectionServiceClient, NodeKind, NodeRef};

        let node_kind = match kind {
            "patch" => NodeKind::Patch,
            "sample" => NodeKind::Sample,
            _ => NodeKind::Lighting,
        };
        let collections: CollectionServiceClient =
            vox::connect_lane(url).establish().await.unwrap();
        let library = collections
            .create(
                support::ORG.to_owned(),
                format!("{kind} library"),
                CollectionKind::Library,
            )
            .await
            .unwrap();
        for slug in [gone, kept] {
            collections
                .add_item(collection_proto::Placement {
                    collection_id: library.id.clone(),
                    node: NodeRef::new(node_kind, slug),
                    after: None,
                })
                .await
                .unwrap();
        }

        let back = collections
            .get(library.id.clone())
            .await
            .unwrap()
            .expect("the library exists");
        assert_eq!(back.kind, CollectionKind::Library);
        let tokens: Vec<String> = back.items.iter().map(|i| i.node.to_token()).collect();
        assert_eq!(
            tokens,
            [format!("{kind}:{gone}"), format!("{kind}:{kept}")],
            "items come back in rank order, appended in the order added"
        );
        let ranks: Vec<&str> = back.items.iter().map(|i| i.rank.as_str()).collect();
        assert!(ranks[0] < ranks[1], "ranks ascend: {ranks:?}");

        // Delete the asset and the reference outlives it: the directory
        // is gone, the collection still parses, and the item still
        // names what it wanted.
        let deleted = match kind {
            "patch" => be.delete_patch(gone.to_owned()).await.unwrap(),
            "sample" => be.delete_sample(gone.to_owned()).await.unwrap(),
            _ => be.delete_lighting(gone.to_owned()).await.unwrap(),
        };
        assert!(deleted);
        // The library directory is the one `node_homes::library_of`
        // names — `lighting` is its own plural.
        let library_dir = match kind {
            "lighting" => "lighting".to_owned(),
            other => format!("{other}s"),
        };
        let org_root = org_proto::DataRoot::from_env().unwrap().org(support::ORG);
        let dir = org_root.resources_dir().join(library_dir).join(gone);
        assert!(!dir.exists(), "the directory is the unit of deletion");

        let dangling = collections
            .get(library.id.clone())
            .await
            .unwrap()
            .expect("the library survived the delete");
        let tokens: Vec<String> = dangling.items.iter().map(|i| i.node.to_token()).collect();
        assert_eq!(
            tokens,
            [format!("{kind}:{gone}"), format!("{kind}:{kept}")],
            "the dangling reference is kept, in order, and still parses"
        );
        assert!(
            dangling.items[0].node.is_local(),
            "a local reference names this org, not another"
        );
    }

    // Without the collections plugin there is no library to hold the
    // reference, but the delete contract is the same.
    #[cfg(not(feature = "plugin-fasttrackstudio"))]
    {
        let deleted = match kind {
            "patch" => be.delete_patch(gone.to_owned()).await.unwrap(),
            "sample" => be.delete_sample(gone.to_owned()).await.unwrap(),
            _ => be.delete_lighting(gone.to_owned()).await.unwrap(),
        };
        assert!(deleted);
    }
}

/// A local `patch:` / `sample:` / `lighting:` reference is `Local` —
/// the near half of ADR 0003's read path. The cross-org half (unknown
/// domain, unsubscribed, reachable) is pinned in
/// `cross_org_nodes_e2e.rs`.
#[tokio::test(flavor = "multi_thread")]
async fn local_asset_references_resolve_as_local() {
    use links_proto::{LinksServiceClient, NodeKind, NodeRef, Reach};

    let (url, _tmp) = support::boot_ws().await.unwrap();
    let links: LinksServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    let asked = vec![
        NodeRef::new(NodeKind::Patch, "warm-analog-pad"),
        NodeRef::new(NodeKind::Sample, "room-kick-48k"),
        NodeRef::new(NodeKind::Lighting, "album-launch-show"),
        // An anchor does not change where a node lives.
        NodeRef::new(NodeKind::Lighting, "album-launch-show").with_anchor("cue:12"),
        // Neither does naming something that is not on disk: locality
        // is a property of the reference, not of the file.
        NodeRef::new(NodeKind::Patch, "never-written"),
    ];
    let answers = links.resolve_nodes(asked.clone()).await.unwrap();

    assert_eq!(answers.len(), asked.len(), "one answer per question");
    for (answer, asked) in answers.iter().zip(&asked) {
        assert_eq!(
            answer.reach,
            Reach::Local,
            "{} is this org's own",
            asked.to_token()
        );
    }
}

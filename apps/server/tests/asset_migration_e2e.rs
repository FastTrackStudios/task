#![allow(clippy::large_futures)]
//! **A server that boots on an ADR 0003 disk comes up holding ADR 0004
//! assets** — and has deleted nothing.
//!
//! The migration in `ResourcesBackend::migrate_charts` runs at boot, on
//! every boot, for every org. There is real data it has to be right
//! about: `chart:doxology`, `chart:doxology-2`, `chart:agent-smoke-test`
//! and `chart:vox` in org `codywright`, plus whatever song folders
//! `task song add` wrote. A migration that has to be remembered is one
//! that gets skipped on the deployment that mattered, so it is not a
//! command — it is a consequence of starting the server, and this test
//! is the thing that says so.
//!
//! # Why it boots twice
//!
//! Idempotence is the whole safety argument, and it is only worth
//! anything if it survives an *edit*. So the second boot happens after
//! a chart has been changed through the lane: if the migration
//! re-composed from the frozen originals it would silently revert a
//! day's work, and that failure would look exactly like nothing
//! happening.
//!
//! # And why nothing is deleted
//!
//! Three readers still depend on the originals, and none of them is
//! this repository's to fix in one change:
//!
//! - `node_homes::LocalHomes::locate` and the `SourceKind::Resource`
//!   materialiser resolve a *foreign* org's chart through
//!   `resources/charts/`;
//! - `GET /org/{slug}/media/songs/<slug>/song.md` is how the global
//!   player finds a song's arrangements (`player_ui::song_session`);
//! - a person who wants to check the migration wants to read both
//!   copies.
//!
//! So it copies, leaves a breadcrumb saying so, and the gaps are
//! recorded in `docs/spec/unmet.md` rather than closed by a deletion.

#[allow(dead_code)]
mod support;

use resources_proto::ResourcesServiceClient;

/// The ADR 0003 world, written onto a data root before the server sees
/// it: a flat chart pair, and a vendored song folder with two
/// arrangements and its media beside them.
fn lay_down_an_adr_0003_disk(org: &org_proto::OrgRoot) {
    let charts = org.resources_dir().join("charts");
    std::fs::create_dir_all(&charts).unwrap();
    std::fs::write(
        charts.join("doxology.md"),
        "---\ntype: resource\nresource_kind: chart\nslug: doxology\ntitle: Doxology\ncapo: 2\n---\n\
         <!-- The chart itself is `doxology.kf` beside this file; edit it there. -->\n\
         # Doxology\n\n## Notes\n\n- from the hymnal\n",
    )
    .unwrap();
    std::fs::write(charts.join("doxology.kf"), "[Verse]\n| G | C | D | G |\n").unwrap();

    let song = org.resources_dir().join("songs/opening-night");
    std::fs::create_dir_all(song.join("arrangements/default")).unwrap();
    std::fs::create_dir_all(song.join("arrangements/live")).unwrap();
    std::fs::write(
        song.join("song.md"),
        "---\nid: 75e30481-6a81-4759-98b7-816c8c605d46\ntitle: Opening Night\ntags: []\n\
         defaultArrangement: be760d4e-e43a-4297-9bae-30ef49925f89\narrangements:\n\
         - id: be760d4e-e43a-4297-9bae-30ef49925f89\n  name: Default\n  dir: default\n  key: C Major\n---\n",
    )
    .unwrap();
    std::fs::write(
        song.join("arrangements/default/arrangement.md"),
        "---\nid: be760d4e-e43a-4297-9bae-30ef49925f89\nname: Default\nkey: C Major\n\
         chartRef:\n  path: arrangements/default/opening-night.kf\n---\n",
    )
    .unwrap();
    std::fs::write(
        song.join("arrangements/default/opening-night.kf"),
        "[Verse]\n| C | F | G |\n",
    )
    .unwrap();
    std::fs::write(
        song.join("arrangements/live/arrangement.md"),
        "---\nid: aa11bb22-0000-0000-0000-000000000000\nname: Live\nkey: D Major\n---\n",
    )
    .unwrap();
    std::fs::write(
        song.join("arrangements/live/live.kf"),
        "[Verse]\n| D | A |\n",
    )
    .unwrap();
    // The media half, which is a Resource and does not move.
    std::fs::write(
        song.join("manifest.json"),
        "{\"slug\":\"opening-night\",\"title\":\"Opening Night\",\"stems\":[]}",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_boot_on_an_adr_0003_disk_migrates_and_deletes_nothing() {
    let (url, tmp) = support::boot_ws_with(lay_down_an_adr_0003_disk)
        .await
        .unwrap();
    let resources: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    let org_root = support::org_root(&tmp);

    // ── the flat chart pair became one asset document ────────────────
    let doxology = resources.chart("doxology".to_owned()).await.unwrap();
    assert_eq!(doxology.source, "[Verse]\n| G | C | D | G |\n");
    let text = std::fs::read_to_string(
        org_root
            .vault_dir()
            .join(resources_proto::assets::chart_path("doxology")),
    )
    .expect("the chart is on the shelf");
    assert!(text.contains("type: asset"), "{text}");
    assert!(text.contains("capo: 2"), "a foreign key survives: {text}");
    assert!(
        text.contains("- from the hymnal"),
        "somebody's prose survives: {text}"
    );

    // ── the vendored song folder became a song and two charts ────────
    let song = resources.song("opening-night".to_owned()).await.unwrap();
    assert_eq!(song.title, "Opening Night");

    let arrangements = resources
        .list_charts("song:opening-night".to_owned())
        .await
        .unwrap();
    let slugs: Vec<&str> = arrangements.iter().map(|c| c.slug.as_str()).collect();
    assert_eq!(slugs, ["opening-night", "opening-night-live"], "{slugs:?}");
    let defaults: Vec<&str> = arrangements
        .iter()
        .filter(|c| c.is_default)
        .map(|c| c.slug.as_str())
        .collect();
    assert_eq!(
        defaults,
        ["opening-night"],
        "`defaultArrangement`'s uuid became a flag on the chart it named"
    );
    assert_eq!(
        resources
            .chart("opening-night-live".to_owned())
            .await
            .unwrap()
            .source,
        "[Verse]\n| D | A |\n",
        "each arrangement's `.kf` became that chart's own fence"
    );

    // ── nothing was deleted, and the media never moved ───────────────
    let resources_dir = org_root.resources_dir();
    for still_there in [
        "charts/doxology.md",
        "charts/doxology.kf",
        "songs/opening-night/song.md",
        "songs/opening-night/arrangements/default/opening-night.kf",
        // Not a snapshot — a live Resource the `/media` route serves.
        "songs/opening-night/manifest.json",
    ] {
        assert!(
            resources_dir.join(still_there).is_file(),
            "the migration deleted `{still_there}` — it copies, so that \
             a person can diff the two and so that cross-org resolution \
             and the player keep reading what they read"
        );
    }
    for note in ["charts/_MIGRATED.md", "songs/_MIGRATED.md"] {
        let text = std::fs::read_to_string(resources_dir.join(note))
            .unwrap_or_else(|e| panic!("no breadcrumb at `{note}`: {e}"));
        assert!(text.contains("vault"), "{note}: {text}");
    }

    // ── an edit, and then a second boot over the same disk ───────────
    //
    // The case that would be silent data loss: a re-run must not
    // re-compose the chart from the originals it left behind.
    let mut edited = doxology.clone();
    edited.slug = "doxology".into();
    edited.source = "[Verse]\n| Em | C |\n".into();
    resources.upsert_chart(edited).await.unwrap();
    drop(resources);

    let (url, _second) = support::boot_ws_over(&tmp).await.unwrap();
    let resources: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    assert_eq!(
        resources.chart("doxology".to_owned()).await.unwrap().source,
        "[Verse]\n| Em | C |\n",
        "a second boot reverted an edit to the frozen ADR 0003 snapshot"
    );
    assert_eq!(
        resources
            .list_charts("song:opening-night".to_owned())
            .await
            .unwrap()
            .len(),
        2,
        "a second boot duplicated the arrangements"
    );
}

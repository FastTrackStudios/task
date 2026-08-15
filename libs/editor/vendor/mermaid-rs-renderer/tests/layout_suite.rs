use std::path::Path;

use mermaid_rs_renderer::layout::DiagramData;
use mermaid_rs_renderer::{Layout, LayoutConfig, Theme, compute_layout, parse_mermaid, render_svg};

fn assert_valid_svg(svg: &str, fixture: &str) {
    assert!(svg.contains("<svg"), "{fixture}: missing <svg tag");
    assert!(svg.contains("</svg>"), "{fixture}: missing </svg tag");
    assert!(!svg.contains("NaN"), "{fixture}: svg contains NaN");
    assert!(!svg.contains("inf"), "{fixture}: svg contains inf");
}

fn assert_finite(value: f32, fixture: &str, label: &str) {
    assert!(value.is_finite(), "{fixture}: {label} is not finite");
}

fn segment_intersects_rect(a: (f32, f32), b: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    let (rx, ry, rw, rh) = rect;
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let p = [-dx, dx, -dy, dy];
    let q = [a.0 - rx, rx + rw - a.0, a.1 - ry, ry + rh - a.1];
    let mut u1 = 0.0f32;
    let mut u2 = 1.0f32;

    for (pi, qi) in p.into_iter().zip(q) {
        if pi.abs() <= f32::EPSILON {
            if qi < 0.0 {
                return false;
            }
            continue;
        }
        let t = qi / pi;
        if pi < 0.0 {
            if t > u2 {
                return false;
            }
            if t > u1 {
                u1 = t;
            }
        } else {
            if t < u1 {
                return false;
            }
            if t < u2 {
                u2 = t;
            }
        }
    }

    true
}
fn assert_layout_is_well_formed(layout: &Layout, fixture: &str) {
    assert_finite(layout.width, fixture, "layout.width");
    assert_finite(layout.height, fixture, "layout.height");
    assert!(
        layout.width > 0.0,
        "{fixture}: layout.width must be positive"
    );
    assert!(
        layout.height > 0.0,
        "{fixture}: layout.height must be positive"
    );

    for (id, node) in &layout.nodes {
        assert_finite(node.x, fixture, &format!("node {id} x"));
        assert_finite(node.y, fixture, &format!("node {id} y"));
        assert_finite(node.width, fixture, &format!("node {id} width"));
        assert_finite(node.height, fixture, &format!("node {id} height"));
        assert!(
            node.width >= 0.0,
            "{fixture}: node {id} width must be non-negative"
        );
        assert!(
            node.height >= 0.0,
            "{fixture}: node {id} height must be non-negative"
        );
        assert!(
            node.x >= -0.1,
            "{fixture}: node {id} x should not be negative"
        );
        assert!(
            node.y >= -0.1,
            "{fixture}: node {id} y should not be negative"
        );
        assert!(
            node.x + node.width <= layout.width + 0.1,
            "{fixture}: node {id} exceeds layout width"
        );
        assert!(
            node.y + node.height <= layout.height + 0.1,
            "{fixture}: node {id} exceeds layout height"
        );
        assert_finite(node.label.width, fixture, &format!("node {id} label width"));
        assert_finite(
            node.label.height,
            fixture,
            &format!("node {id} label height"),
        );
    }

    for sub in &layout.subgraphs {
        assert_finite(sub.x, fixture, &format!("subgraph {} x", sub.label));
        assert_finite(sub.y, fixture, &format!("subgraph {} y", sub.label));
        assert_finite(sub.width, fixture, &format!("subgraph {} width", sub.label));
        assert_finite(
            sub.height,
            fixture,
            &format!("subgraph {} height", sub.label),
        );
        assert!(
            sub.width >= 0.0,
            "{fixture}: subgraph {} width must be non-negative",
            sub.label
        );
        assert!(
            sub.height >= 0.0,
            "{fixture}: subgraph {} height must be non-negative",
            sub.label
        );
    }

    for edge in &layout.edges {
        for (idx, point) in edge.points.iter().enumerate() {
            assert_finite(
                point.0,
                fixture,
                &format!("edge {}->{} point {idx} x", edge.from, edge.to),
            );
            assert_finite(
                point.1,
                fixture,
                &format!("edge {}->{} point {idx} y", edge.from, edge.to),
            );
        }
        if let Some((x, y)) = edge.label_anchor {
            assert_finite(
                x,
                fixture,
                &format!("edge {}->{} label anchor x", edge.from, edge.to),
            );
            assert_finite(
                y,
                fixture,
                &format!("edge {}->{} label anchor y", edge.from, edge.to),
            );
        }
        if let Some((x, y)) = edge.start_label_anchor {
            assert_finite(
                x,
                fixture,
                &format!("edge {}->{} start label anchor x", edge.from, edge.to),
            );
            assert_finite(
                y,
                fixture,
                &format!("edge {}->{} start label anchor y", edge.from, edge.to),
            );
        }
        if let Some((x, y)) = edge.end_label_anchor {
            assert_finite(
                x,
                fixture,
                &format!("edge {}->{} end label anchor x", edge.from, edge.to),
            );
            assert_finite(
                y,
                fixture,
                &format!("edge {}->{} end label anchor y", edge.from, edge.to),
            );
        }
    }

    if let DiagramData::Graph { state_notes } = &layout.diagram {
        for (idx, note) in state_notes.iter().enumerate() {
            assert_finite(note.x, fixture, &format!("state note {idx} x"));
            assert_finite(note.y, fixture, &format!("state note {idx} y"));
            assert_finite(note.width, fixture, &format!("state note {idx} width"));
            assert_finite(note.height, fixture, &format!("state note {idx} height"));
        }
    }
}

fn assert_flowchart_visual_invariants(layout: &Layout, fixture: &str) {
    if !fixture.starts_with("flowchart/") {
        return;
    }

    for (idx, left) in layout.subgraphs.iter().enumerate() {
        let left_nodes: std::collections::HashSet<&str> =
            left.nodes.iter().map(std::string::String::as_str).collect();
        for right in layout.subgraphs.iter().skip(idx + 1) {
            let shares_nodes = right
                .nodes
                .iter()
                .any(|node| left_nodes.contains(node.as_str()));
            if shares_nodes {
                continue;
            }
            let overlaps_x = left.x < right.x + right.width && right.x < left.x + left.width;
            let overlaps_y = left.y < right.y + right.height && right.y < left.y + left.height;
            assert!(
                !(overlaps_x && overlaps_y),
                "{fixture}: subgraphs {} and {} overlap",
                left.label,
                right.label
            );
        }
    }

    for edge in &layout.edges {
        let (Some(label), Some(anchor)) = (&edge.label, edge.label_anchor) else {
            continue;
        };
        let label_rect = (
            anchor.0 - label.width / 2.0,
            anchor.1 - label.height / 2.0,
            label.width,
            label.height,
        );
        let intersects = edge
            .points
            .windows(2)
            .any(|segment| segment_intersects_rect(segment[0], segment[1], label_rect));
        assert!(
            !intersects,
            "{fixture}: edge {}->{} route overlaps its own label box",
            edge.from, edge.to
        );
    }
}

fn assert_sequence_label_clear_of_lifelines(layout: &Layout, fixture: &str) {
    let DiagramData::Sequence(seq) = &layout.diagram else {
        return;
    };

    for edge in &layout.edges {
        let (Some(label), Some(anchor)) = (&edge.label, edge.label_anchor) else {
            continue;
        };
        let label_rect = (
            anchor.0 - label.width / 2.0 - 4.0,
            anchor.1 - label.height / 2.0 - 2.0,
            label.width + 8.0,
            label.height + 4.0,
        );
        for lifeline in &seq.lifelines {
            let line_rect = (
                lifeline.x - 1.5,
                lifeline.y1,
                3.0,
                lifeline.y2 - lifeline.y1,
            );
            let overlaps_x = label_rect.0 < line_rect.0 + line_rect.2
                && line_rect.0 < label_rect.0 + label_rect.2;
            let overlaps_y = label_rect.1 < line_rect.1 + line_rect.3
                && line_rect.1 < label_rect.1 + label_rect.3;
            assert!(
                !(overlaps_x && overlaps_y),
                "{fixture}: edge label for {}->{} overlaps lifeline {}",
                edge.from,
                edge.to,
                lifeline.id
            );
        }
    }
}

fn render_fixture(path: &Path) -> (Layout, String) {
    let input = std::fs::read_to_string(path).expect("fixture read failed");
    let parsed = parse_mermaid(&input).expect("parse failed");
    let theme = Theme::modern();
    let layout_config = LayoutConfig::default();
    let layout = compute_layout(&parsed.graph, &theme, &layout_config);
    let svg = render_svg(&layout, &theme, &layout_config);
    (layout, svg)
}

fn parse_viewbox(svg: &str) -> Option<(f32, f32, f32, f32)> {
    let marker = "viewBox=\"";
    let start = svg.find(marker)? + marker.len();
    let end = svg[start..].find('"')? + start;
    let parts: Vec<f32> = svg[start..end]
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<f32>().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() == 4 {
        Some((parts[0], parts[1], parts[2], parts[3]))
    } else {
        None
    }
}

#[test]
fn render_all_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let mut fixtures: Vec<String> = Vec::new();

    // Keep this list explicit so new diagram types must be added intentionally.
    let candidates = [
        "architecture/basic.mmd",
        "block/basic.mmd",
        "c4/basic.mmd",
        "class/basic.mmd",
        "class/multiplicity.mmd",
        "er/basic.mmd",
        "flowchart/basic.mmd",
        "flowchart/complex.mmd",
        "flowchart/edges.mmd",
        "flowchart/dense.mmd",
        "flowchart/ports.mmd",
        "flowchart/styles.mmd",
        "flowchart/subgraph.mmd",
        "flowchart/subgraph_direction.mmd",
        "flowchart/subgraph_empty.mmd",
        "flowchart/cycles.mmd",
        "gantt/basic.mmd",
        "gitgraph/basic.mmd",
        "journey/basic.mmd",
        "kanban/basic.mmd",
        "mindmap/basic.mmd",
        "packet/basic.mmd",
        "pie/basic.mmd",
        "quadrant/basic.mmd",
        "radar/basic.mmd",
        "requirement/basic.mmd",
        "sankey/basic.mmd",
        "sequence/basic.mmd",
        "sequence/frames.mmd",
        "state/basic.mmd",
        "state/note.mmd",
        "timeline/basic.mmd",
        "treemap/basic.mmd",
        "xychart/basic.mmd",
        "zenuml/basic.mmd",
    ];

    for rel in candidates {
        fixtures.push(rel.to_string());
    }

    for rel in fixtures {
        let path = root.join(&rel);
        assert!(path.exists(), "fixture missing: {rel}");
        let (layout, svg) = render_fixture(&path);
        assert_layout_is_well_formed(&layout, &rel);
        assert_flowchart_visual_invariants(&layout, &rel);
        assert_sequence_label_clear_of_lifelines(&layout, &rel);
        assert_valid_svg(&svg, &rel);
    }
}

#[test]
fn sequence_nested_alt_wide_section_labels_do_not_panic() {
    let fixture = "sequence/nested_alt.mmd";
    let input = std::fs::read_to_string(Path::new("tests/fixtures").join(fixture)).unwrap();
    let parsed = parse_mermaid(&input).unwrap();
    let theme = Theme::mermaid_default();
    let config = LayoutConfig::default();
    let layout = compute_layout(&parsed.graph, &theme, &config);
    let svg = render_svg(&layout, &theme, &config);
    assert_valid_svg(&svg, fixture);
}

#[test]
fn sequence_basic_uses_mermaid_like_actor_geometry_and_framing() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sequence")
        .join("basic.mmd");
    let (layout, svg) = render_fixture(&root);

    let alice = layout.nodes.get("Alice").expect("Alice node");
    let bob = layout.nodes.get("Bob").expect("Bob node");

    assert!(
        (alice.width - 150.0).abs() < 0.01,
        "Alice width={}",
        alice.width
    );
    assert!((bob.width - 150.0).abs() < 0.01, "Bob width={}", bob.width);
    assert!(
        (alice.height - 65.0).abs() < 0.01,
        "Alice height={}",
        alice.height
    );
    assert!(
        (bob.height - 65.0).abs() < 0.01,
        "Bob height={}",
        bob.height
    );
    let alice_center = alice.x + alice.width / 2.0;
    let bob_center = bob.x + bob.width / 2.0;
    assert!(
        (alice_center - 75.0).abs() < 0.01,
        "Alice center={alice_center}"
    );
    assert!((bob_center - 275.0).abs() < 0.01, "Bob center={bob_center}");
    assert!(
        (bob_center - alice_center - 200.0).abs() < 0.01,
        "lane pitch={}",
        bob_center - alice_center
    );

    let viewbox = parse_viewbox(&svg).expect("sequence viewBox");
    assert!((viewbox.0 + 50.0).abs() < 0.01, "viewBox x={}", viewbox.0);
    assert!((viewbox.1 + 10.0).abs() < 0.01, "viewBox y={}", viewbox.1);
    assert!(
        (viewbox.2 - 450.0).abs() < 0.01,
        "viewBox width={}",
        viewbox.2
    );
    assert!(
        (viewbox.3 - 265.0).abs() < 8.0,
        "viewBox height={}",
        viewbox.3
    );
}

#[test]
fn sequence_frames_keeps_mermaid_like_lane_pitch() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sequence")
        .join("frames.mmd");
    let (layout, _svg) = render_fixture(&root);

    let client = layout.nodes.get("Client").expect("Client node");
    let api = layout.nodes.get("API").expect("API node");
    let db = layout.nodes.get("DB").expect("DB node");
    let centers = [
        client.x + client.width / 2.0,
        api.x + api.width / 2.0,
        db.x + db.width / 2.0,
    ];
    assert!(
        (centers[1] - centers[0] - 200.0).abs() < 0.01,
        "first pitch={}",
        centers[1] - centers[0]
    );
    assert!(
        (centers[2] - centers[1] - 200.0).abs() < 0.01,
        "second pitch={}",
        centers[2] - centers[1]
    );
    assert!(
        (layout.width - 550.0).abs() < 0.01,
        "layout width={}",
        layout.width
    );
}

#[test]
fn sequence_alt_frame_geometry_matches_mermaid() {
    use mermaid_rs_renderer::layout::DiagramData;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sequence")
        .join("frames.mmd");
    let (layout, _svg) = render_fixture(&root);

    let DiagramData::Sequence(seq) = &layout.diagram else {
        panic!("expected sequence diagram data");
    };

    assert!(!seq.frames.is_empty(), "should have at least one frame");
    let frame = &seq.frames[0];

    let client = layout.nodes.get("Client").expect("Client node");
    let api = layout.nodes.get("API").expect("API node");
    let client_center = client.x + client.width / 2.0;
    let api_center = api.x + api.width / 2.0;

    assert!(
        frame.x < client_center,
        "frame x ({}) should be left of Client center ({})",
        frame.x,
        client_center
    );
    assert!(
        frame.x + frame.width > api_center,
        "frame right edge ({}) should be right of API center ({})",
        frame.x + frame.width,
        api_center
    );

    assert!(
        (frame.x - 64.0).abs() < 5.0,
        "frame x should be ~64 (got {})",
        frame.x
    );
    assert!(
        (frame.width - 226.0).abs() < 12.0,
        "frame width should be ~226 (got {})",
        frame.width
    );

    let (lbx, lby, lbw, lbh) = frame.label_box;
    assert!(
        (lbx - frame.x).abs() < 0.01,
        "label box x should match frame x"
    );
    assert!(
        (lby - frame.y).abs() < 0.01,
        "label box y should match frame y"
    );
    assert!(
        lbw > 30.0 && lbw < 80.0,
        "label box width should be reasonable (got {lbw})"
    );
    assert!(
        lbh > 10.0 && lbh < 30.0,
        "label box height should be reasonable (got {lbh})"
    );

    assert!(
        !frame.dividers.is_empty(),
        "alt frame should have at least one divider"
    );
    let div_y = frame.dividers[0];
    assert!(
        div_y > frame.y && div_y < frame.y + frame.height,
        "divider y ({}) should be inside frame ({} to {})",
        div_y,
        frame.y,
        frame.y + frame.height
    );
}

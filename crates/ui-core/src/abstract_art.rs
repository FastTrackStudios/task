//! A project's abstract: a square, album-cover-like image painted from a
//! seed, so every project has its own picture with nothing stored.
//!
//! The same seed paints the same picture everywhere (web, phone, desk,
//! server), so a project id is all it takes: no image to upload, sync or
//! lose. A seeded palette (analogous, split, mono or duotone; mostly on a
//! deep ground) and one of seven compositions — soft orbs, a horizon, a
//! ring, Bauhaus bands, shards, waves, a lone form — with a film grain
//! over it. Rendered as SVG, so it is crisp at any size: a 20px swatch in
//! a list, a full-width header.
//!
//! Ported from NEO's seeded book covers (`covers.js`, © 2026 Hugh Howey,
//! MIT License — <https://github.com/hughhowey/neo>), redrawn square:
//! the palette, the styles and the deterministic randomness follow it;
//! the canvas becomes SVG and the grain an SVG noise filter.

use std::fmt::Write as _;

/// The square the styles draw in; SVG scales it to any size.
const S: f64 = 300.0;

/// FNV-1a, 32-bit — the seed's hash.
fn hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.encode_utf16() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// A small, fast, deterministic generator (the mulberry32 family).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
    fn between(&mut self, a: f64, b: f64) -> f64 {
        a + self.next() * (b - a)
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let i = (self.next() * items.len() as f64) as usize;
        &items[i.min(items.len() - 1)]
    }
}

fn hsl(h: f64, s: f64, l: f64) -> String {
    format!("hsl({:.0},{:.0}%,{:.0}%)", h.rem_euclid(360.0), s, l)
}

struct Palette {
    ground: String,
    ground2: String,
    mid: String,
    accent: String,
    pale: String,
}

fn palette(r: &mut Rng) -> Palette {
    let base = (r.next() * 360.0).floor();
    let scheme = *r.pick(&["analogous", "split", "mono", "duotone"]);
    let sat = r.between(28.0, 62.0);
    // Most sit on a deep ground; some go bright.
    let dark = r.next() < 0.7;
    let (l0, l1) = if dark { (10.0, 22.0) } else { (78.0, 92.0) };
    let hues = match scheme {
        "analogous" => [base, base + 30.0, base - 25.0],
        "split" => [base, base + 150.0, base + 210.0],
        "mono" => [base, base + 8.0, base - 8.0],
        _ => [base, base + 180.0, base + 180.0],
    };
    let ground = hsl(hues[0], sat * 0.8, r.between(l0, l1));
    let g2 = if dark {
        r.between(6.0, 18.0)
    } else {
        r.between(70.0, 86.0)
    };
    let ground2 = hsl(hues[1], sat * 0.7, g2);
    let m = if dark {
        r.between(30.0, 48.0)
    } else {
        r.between(45.0, 62.0)
    };
    let mid = hsl(hues[1], sat, m);
    let a = if dark {
        r.between(52.0, 68.0)
    } else {
        r.between(38.0, 55.0)
    };
    let accent = hsl(hues[2], (sat + 30.0).min(90.0), a);
    let p = if dark {
        r.between(60.0, 80.0)
    } else {
        r.between(20.0, 34.0)
    };
    let pale = hsl(hues[0], sat * 0.5, p);
    Palette {
        ground,
        ground2,
        mid,
        accent,
        pale,
    }
}

/// The two-colour ground every style starts from.
fn ground(r: &mut Rng, p: &Palette, defs: &mut String, body: &mut String) {
    let angle = r.between(0.0, std::f64::consts::TAU);
    let (c, s) = (angle.cos(), angle.sin());
    let _ = write!(
        defs,
        r#"<linearGradient id="g" gradientUnits="userSpaceOnUse" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient>"#,
        S / 2.0 - c * S,
        S / 2.0 - s * S,
        S / 2.0 + c * S,
        S / 2.0 + s * S,
        p.ground,
        p.ground2
    );
    let _ = write!(body, r#"<rect width="{S}" height="{S}" fill="url(#g)"/>"#);
}

const STYLES: [&str; 7] = [
    "orbs", "horizon", "ring", "bands", "shards", "waves", "solitary",
];

fn paint(style: &str, r: &mut Rng, p: &Palette, defs: &mut String, body: &mut String) {
    ground(r, p, defs, body);
    match style {
        // Soft blurred spheres of colour drifting on the ground.
        "orbs" => {
            let n = 2 + (r.next() * 3.0) as usize;
            for i in 0..n {
                let (x, y) = (r.between(-20.0, S + 20.0), r.between(-20.0, S + 20.0));
                let rad = r.between(60.0, 170.0);
                let c = if i == 0 {
                    &p.accent
                } else if r.next() < 0.5 {
                    &p.mid
                } else {
                    &p.pale
                };
                let o = r.between(0.45, 0.85);
                let _ = write!(
                    defs,
                    r#"<radialGradient id="o{i}"><stop offset="0" stop-color="{c}"/><stop offset="1" stop-color="{c}" stop-opacity="0"/></radialGradient>"#
                );
                let _ = write!(
                    body,
                    r#"<circle cx="{x:.1}" cy="{y:.1}" r="{rad:.1}" fill="url(#o{i})" opacity="{o:.2}"/>"#
                );
            }
        }
        // A flat horizon, a glow along it, sometimes a small sun.
        "horizon" => {
            let y = r.between(S * 0.45, S * 0.75);
            let _ = write!(
                defs,
                r#"<linearGradient id="h" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{a}" stop-opacity="0"/><stop offset="1" stop-color="{a}"/></linearGradient>"#,
                a = p.accent
            );
            let _ = write!(
                body,
                r#"<rect y="{:.1}" width="{S}" height="60" fill="url(#h)" opacity="0.55"/><rect y="{y:.1}" width="{S}" height="{:.1}" fill="{}"/>"#,
                y - 60.0,
                S - y,
                p.ground2
            );
            if r.next() < 0.6 {
                let (sx, sy, sr) = (
                    r.between(40.0, S - 40.0),
                    y - r.between(10.0, 50.0),
                    r.between(9.0, 24.0),
                );
                let _ = write!(
                    body,
                    r#"<circle cx="{sx:.1}" cy="{sy:.1}" r="{sr:.1}" fill="{}"/>"#,
                    p.accent
                );
            }
            let _ = write!(
                body,
                r#"<rect y="{:.1}" width="{S}" height="1.5" fill="{}" opacity="0.5"/>"#,
                y - 1.0,
                p.pale
            );
        }
        // One big ring or disc, off-centre, and a thin crossing line.
        "ring" => {
            let (x, y) = (r.between(S * 0.3, S * 0.7), r.between(S * 0.3, S * 0.7));
            let rad = r.between(45.0, 95.0);
            if r.next() < 0.5 {
                let w = r.between(2.0, 9.0);
                let _ = write!(
                    body,
                    r#"<circle cx="{x:.1}" cy="{y:.1}" r="{rad:.1}" fill="none" stroke="{}" stroke-width="{w:.1}"/>"#,
                    p.accent
                );
            } else {
                let _ = write!(
                    body,
                    r#"<circle cx="{x:.1}" cy="{y:.1}" r="{rad:.1}" fill="{}" opacity="0.9"/>"#,
                    p.accent
                );
            }
            let a = r.between(-0.5, 0.5);
            let _ = write!(
                body,
                r#"<line x1="-10" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{}" stroke-width="1" opacity="0.5"/>"#,
                y + a.tan() * (x + 10.0),
                S + 10.0,
                y - a.tan() * (S - x + 10.0),
                p.pale
            );
        }
        // Bauhaus bands of related colour.
        "bands" => {
            let vertical = r.next() < 0.5;
            let cols = [&p.mid, &p.accent, &p.ground2, &p.pale];
            let mut pos = r.between(-20.0, 30.0);
            while pos < S + 20.0 {
                let w = r.between(10.0, 70.0);
                let c = *r.pick(&cols);
                let o = r.between(0.5, 1.0);
                let rect = if vertical {
                    format!(r#"x="{pos:.1}" width="{w:.1}" height="{S}""#)
                } else {
                    format!(r#"y="{pos:.1}" width="{S}" height="{w:.1}""#)
                };
                let _ = write!(body, r#"<rect {rect} fill="{c}" opacity="{o:.2}"/>"#);
                pos += w + r.between(0.0, 40.0);
            }
        }
        // Overlapping translucent shards.
        "shards" => {
            let n = 3 + (r.next() * 3.0) as usize;
            for _ in 0..n {
                let c = (*r.pick(&[&p.mid, &p.accent, &p.pale, &p.ground2])).clone();
                let o = r.between(0.35, 0.8);
                let mut pts = String::new();
                for _ in 0..3 {
                    let _ = write!(
                        pts,
                        "{:.1},{:.1} ",
                        r.between(-30.0, S + 30.0),
                        r.between(-30.0, S + 30.0)
                    );
                }
                let _ = write!(
                    body,
                    r#"<polygon points="{}" fill="{c}" opacity="{o:.2}"/>"#,
                    pts.trim_end()
                );
            }
        }
        // Stacked sine lines, like a topographic map or a signal.
        "waves" => {
            let rows = 6 + (r.next() * 10.0) as usize;
            let amp = r.between(6.0, 26.0);
            let freq = r.between(0.02, 0.06);
            let phase = r.between(0.0, 6.0);
            let lw = r.between(1.0, 2.5);
            for i in 0..rows {
                let y0 = S / (rows as f64 + 1.0) * (i as f64 + 1.0);
                let c = if i % 3 == 0 { &p.accent } else { &p.pale };
                let o = r.between(0.35, 0.9);
                let mut d = String::new();
                let mut x = -2.0;
                while x <= S + 2.0 {
                    let y = y0
                        + (x * freq + phase + i as f64 * 0.6).sin()
                            * amp
                            * (i as f64 / rows as f64 * std::f64::consts::PI).sin();
                    let _ = write!(d, "{}{x:.1},{y:.1}", if d.is_empty() { "M" } else { " L" });
                    x += 3.0;
                }
                let _ = write!(
                    body,
                    r#"<path d="{d}" fill="none" stroke="{c}" stroke-width="{lw:.1}" opacity="{o:.2}"/>"#
                );
            }
        }
        // A lone small form in a great deal of space.
        _ => {
            let (x, y) = (r.between(S * 0.25, S * 0.75), r.between(S * 0.3, S * 0.7));
            let s = r.between(10.0, 26.0);
            let a = &p.accent;
            let shape = match *r.pick(&["square", "dot", "tri", "bar"]) {
                "square" => format!(
                    r#"<rect x="{:.1}" y="{:.1}" width="{s:.1}" height="{s:.1}" fill="{a}"/>"#,
                    x - s / 2.0,
                    y - s / 2.0
                ),
                "dot" => format!(
                    r#"<circle cx="{x:.1}" cy="{y:.1}" r="{:.1}" fill="{a}"/>"#,
                    s / 2.0
                ),
                "tri" => format!(
                    r#"<polygon points="{x:.1},{:.1} {:.1},{:.1} {:.1},{:.1}" fill="{a}"/>"#,
                    y - s / 2.0,
                    x + s / 2.0,
                    y + s / 2.0,
                    x - s / 2.0,
                    y + s / 2.0
                ),
                _ => format!(
                    r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="4" fill="{a}"/>"#,
                    x - s * 1.5,
                    y - 2.0,
                    s * 3.0
                ),
            };
            body.push_str(&shape);
        }
    }
}

/// The abstract for `seed` (a project's id), as a square SVG document.
/// `uid` keeps its gradient ids apart from another abstract's on the
/// same page.
#[must_use]
pub fn svg(seed: &str) -> String {
    let mut r = Rng(hash(seed));
    let p = palette(&mut r);
    let style = *r.pick(&STYLES);
    let (mut defs, mut body) = (String::new(), String::new());
    paint(style, &mut r, &p, &mut defs, &mut body);
    // Film grain: fractal noise, strength seeded like the rest.
    let grain = r.between(0.06, 0.18);
    let uid = format!("{:08x}", hash(seed));
    // Gradient ids are made unique per seed so many abstracts can share
    // one page without one's `url(#g)` resolving to another's.
    let scope = |s: &str| {
        s.replace("id=\"", &format!("id=\"a{uid}"))
            .replace("url(#", &format!("url(#a{uid}"))
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}" preserveAspectRatio="xMidYMid slice"><defs>{}<filter id="a{uid}n"><feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" stitchTiles="stitch"/><feColorMatrix values="0 0 0 0 0.5  0 0 0 0 0.5  0 0 0 0 0.5  0 0 0 1 0"/></filter></defs>{}<rect width="{S}" height="{S}" filter="url(#a{uid}n)" opacity="{grain:.2}" style="mix-blend-mode:overlay"/></svg>"#,
        scope(&defs),
        scope(&body),
    )
}

/// [`svg`] as a `data:` URL, for an `<img src>`.
#[must_use]
pub fn data_url(seed: &str) -> String {
    let svg = svg(seed);
    let mut out = String::from("data:image/svg+xml;utf8,");
    for c in svg.chars() {
        match c {
            '%' => out.push_str("%25"),
            '#' => out.push_str("%23"),
            '"' => out.push('\''),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_always_paints_the_same_picture() {
        assert_eq!(svg("project-a"), svg("project-a"));
        assert_ne!(svg("project-a"), svg("project-b"));
    }

    #[test]
    fn every_style_appears_across_seeds() {
        let mut seen = std::collections::HashSet::new();
        for i in 0..400 {
            let mut r = Rng(hash(&format!("seed-{i}")));
            let _ = palette(&mut r);
            seen.insert(*r.pick(&STYLES));
        }
        assert_eq!(seen.len(), STYLES.len());
    }

    #[test]
    fn the_picture_is_square_and_self_contained() {
        let s = svg("x");
        assert!(s.starts_with("<svg") && s.contains(r#"viewBox="0 0 300 300""#));
        assert!(!s.contains("url(#g)"), "ids are scoped per seed");
        assert!(data_url("x").starts_with("data:image/svg+xml;utf8,"));
    }
}

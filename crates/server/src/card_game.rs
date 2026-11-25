//! The game look of the post picture: like the app's game view (layers.ts,
//! basemap.ts NIGHT), streets glowing by side and strength, a hot core,
//! sparks on held junctions, rings for claimed sources.

use std::collections::HashMap;
use std::fmt::Write;

use elements_shared::elements::{ElementState, Faction};

use crate::card::{escape, px, Geo, Sides, Spec, H, W};

// Same values as web/src/geo.ts GLOW, CORE and basemap.ts NIGHT.
const BG: &str = "#0b0e13";
const EARTH: &str = "#10141b";
const WATER: &str = "#0a1a2b";
const NEUTRAL: &str = "#6c7889";
const GLOW: [&str; 3] = ["#ff7a1a", "#2f9bff", "#d05cf0"];
const CORE: [&str; 3] = ["#ffe0b3", "#c9e8ff", "#f5d0ff"];
const TEXT: &str = "#f1f3f5";
const MUTED: &str = "#8a94a6";
/// Strength steps: one path per side and step keeps the SVG small.
const STEPS: usize = 5;

/// The app's zoom for this frame on a phone (1080 px = 540 CSS px; MapLibre
/// 512 px tiles).
fn zoom(spec: &Spec) -> f64 {
    (78_271.5 * spec.lat.to_radians().cos() * (W / 2.0) / spec.width_m).log2()
}

/// MapLibre's `interpolate` over stops (base 1: linear), in device pixels.
fn interp(z: f64, base: f64, stops: &[(f64, f64)]) -> f64 {
    let (first, last) = (stops[0], stops[stops.len() - 1]);
    if z <= first.0 {
        return first.1 * 2.0;
    }
    if z >= last.0 {
        return last.1 * 2.0;
    }
    for w in stops.windows(2) {
        let ((z0, v0), (z1, v1)) = (w[0], w[1]);
        if z <= z1 {
            let t = if (base - 1.0).abs() < 1e-9 {
                (z - z0) / (z1 - z0)
            } else {
                (base.powf(z - z0) - 1.0) / (base.powf(z1 - z0) - 1.0)
            };
            return (v0 + (v1 - v0) * t) * 2.0;
        }
    }
    last.1 * 2.0
}

fn d_of(spec: &Spec, line: &[[f64; 2]], d: &mut String) {
    for (i, p) in line.iter().enumerate() {
        let (x, y) = px(spec, p[0], p[1]);
        let _ = write!(d, "{}{x:.1} {y:.1}", if i == 0 { "M" } else { "L" });
    }
}

/// Side (0 fire, 1 water, 2 front) and strength, as world.ts colours a street.
fn side_of(f: f32, w: f32) -> Option<(usize, f32)> {
    let es = ElementState { fire: f, water: w };
    let s = f.max(w).min(1.0);
    match es.owner() {
        Some(Faction::Fire) => Some((0, s)),
        Some(Faction::Water) => Some((1, s)),
        None if es.contested() => Some((2, s)),
        None => None,
    }
}

fn step(s: f32) -> usize {
    ((s * STEPS as f32).ceil() as usize).clamp(1, STEPS) - 1
}

/// A box with rounded corners behind text, its width guessed from the letters.
fn text_width(t: &str, size: f64, bold: bool) -> f64 {
    t.chars().count() as f64 * size * if bold { 0.58 } else { 0.52 }
}

pub fn svg(spec: &Spec, geo: &Geo, sides: &Sides) -> String {
    let z = zoom(spec);
    let vals = sides.values_all();
    let val = |id: i64| vals.get(&id).copied().unwrap_or((0.0, 0.0));
    let mut s = String::with_capacity(1 << 20);
    let glow_w = interp(z, 1.6, &[(13.0, 3.0), (16.0, 10.0), (19.0, 24.0)]);
    let glow_blur = interp(z, 1.6, &[(13.0, 2.0), (16.0, 7.0), (19.0, 16.0)]);
    let line_w = interp(z, 1.0, &[(14.0, 1.5), (18.0, 5.0)]);
    let core_w = interp(z, 1.0, &[(14.0, 0.4), (18.0, 1.6)]);
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" font-family="Noto Sans">
<defs>
<filter id="glow" x="-10%" y="-10%" width="120%" height="120%"><feGaussianBlur stdDeviation="{:.1}"/></filter>
<radialGradient id="vign" cx="50%" cy="50%" r="75%"><stop offset="60%" stop-color="black" stop-opacity="0"/><stop offset="100%" stop-color="black" stop-opacity="0.45"/></radialGradient>
</defs>
<rect width="{W}" height="{H}" fill="{EARTH}"/>
"#,
        glow_blur / 2.0
    );
    if !geo.water.is_empty() {
        let mut d = String::new();
        for poly in &geo.water {
            for ring in poly {
                d_of(spec, ring, &mut d);
                d.push('Z');
            }
        }
        let _ = writeln!(s, r#"<path fill="{WATER}" fill-rule="evenodd" d="{d}"/>"#);
    }

    // Streets by side and strength step; neutral ones in one path.
    let mut neutral = String::new();
    let mut owned: HashMap<(usize, usize), String> = HashMap::new();
    for (line, &(a, b)) in geo.streets.iter().zip(&geo.ends) {
        let ((f1, w1), (f2, w2)) = (val(a), val(b));
        match side_of((f1 + f2) / 2.0, (w1 + w2) / 2.0) {
            Some((k, st)) => d_of(spec, line, owned.entry((k, step(st))).or_default()),
            None => d_of(spec, line, &mut neutral),
        }
    }
    let mut keys: Vec<_> = owned.keys().copied().collect();
    keys.sort_unstable();
    let strength = |i: usize| (i + 1) as f64 / STEPS as f64;
    s.push_str(r#"<g fill="none" stroke-linecap="round" stroke-linejoin="round">"#);
    let _ = write!(s, r#"<g filter="url(#glow)" stroke-width="{glow_w:.1}">"#);
    for &(k, i) in &keys {
        let _ = write!(
            s,
            r#"<path stroke="{}" stroke-opacity="{:.2}" d="{}"/>"#,
            GLOW[k],
            0.25 + 0.45 * strength(i),
            owned[&(k, i)]
        );
    }
    s.push_str("</g>\n");
    if !neutral.is_empty() {
        let _ = write!(
            s,
            r#"<path stroke="{NEUTRAL}" stroke-opacity="0.8" stroke-width="{line_w:.1}" d="{neutral}"/>"#
        );
    }
    for &(k, i) in &keys {
        let _ = write!(
            s,
            r#"<path stroke="{}" stroke-opacity="{:.2}" stroke-width="{line_w:.1}" d="{}"/>"#,
            GLOW[k],
            0.55 + 0.45 * strength(i),
            owned[&(k, i)]
        );
    }
    for &(k, i) in &keys {
        let _ = write!(
            s,
            r#"<path stroke="{}" stroke-opacity="{:.2}" stroke-width="{core_w:.1}" d="{}"/>"#,
            CORE[k],
            0.9 * strength(i),
            owned[&(k, i)]
        );
    }
    s.push_str("</g>\n");

    // Sparks on held junctions when close enough (as in the app from 16.5).
    let pos: HashMap<i64, (f64, f64)> = geo
        .junctions
        .iter()
        .map(|&(id, lon, lat)| (id, px(spec, lon, lat)))
        .collect();
    if z >= 16.0 {
        let r = interp(z, 1.0, &[(15.0, 1.0), (18.0, 3.5)]);
        for (k, core) in CORE.iter().enumerate() {
            let _ = write!(s, r#"<g fill="{core}" fill-opacity="0.9">"#);
            for &(id, _, _) in &geo.junctions {
                let (f, w) = val(id);
                if side_of(f, w).map(|v| v.0) == Some(k) {
                    let (x, y) = pos[&id];
                    let _ = write!(s, r#"<circle cx="{x:.1}" cy="{y:.1}" r="{r:.1}"/>"#);
                }
            }
            s.push_str("</g>\n");
        }
    }

    // Claimed sources: a ring in the side's colour, permanent ones with a dot.
    let ring = (line_w * 3.2).max(9.0);
    for &(id, side, keep) in &sides.sources {
        let Some(&(x, y)) = pos.get(&id) else {
            continue;
        };
        let c = GLOW[if side == 1 { 0 } else { 1 }];
        let _ = write!(
            s,
            r#"<circle cx="{x:.1}" cy="{y:.1}" r="{:.1}" fill="{c}" fill-opacity="0.25" filter="url(#glow)"/><circle cx="{x:.1}" cy="{y:.1}" r="{ring:.1}" fill="{BG}" fill-opacity="0.6" stroke="{c}" stroke-width="{:.1}"/>"#,
            ring * 1.8,
            ring * 0.38
        );
        if keep {
            let _ = write!(
                s,
                r#"<circle cx="{x:.1}" cy="{y:.1}" r="{:.1}" fill="{c}"/>"#,
                ring * 0.42
            );
        }
    }
    let _ = writeln!(s, r#"<rect width="{W}" height="{H}" fill="url(#vign)"/>"#);

    // Title in a dark card at the top, like the app's cards.
    let t = spec.title.trim();
    if !t.is_empty() {
        let size = 54.0;
        let bw = text_width(t, size, true) + size * 1.2;
        let bh = size * 1.75;
        let y = H * 0.045;
        let _ = writeln!(
            s,
            r#"<rect x="{:.1}" y="{y:.1}" width="{bw:.1}" height="{bh:.1}" rx="{:.1}" fill="{BG}" fill-opacity="0.82" stroke="white" stroke-opacity="0.12" stroke-width="2"/>
        <text x="{:.1}" y="{:.1}" font-size="{size}" font-weight="bold" fill="{TEXT}" text-anchor="middle">{}</text>"#,
            W / 2.0 - bw / 2.0,
            bh * 0.3,
            W / 2.0,
            y + bh / 2.0 + size * 0.36,
            escape(t)
        );
    }

    // Note bottom left.
    let lines: Vec<&str> = spec
        .note
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if !lines.is_empty() {
        let size = 32.0;
        let lh = size * 1.3;
        let x = W * 0.06;
        let pad = size * 0.6;
        let bh = lh * lines.len() as f64 + pad * 1.2;
        let bw = lines
            .iter()
            .map(|l| text_width(l, size, false))
            .fold(0.0, f64::max)
            + 2.0 * pad;
        let top = H * (1.0 - 0.06) - bh;
        let _ = write!(
            s,
            r#"<rect x="{x:.1}" y="{top:.1}" width="{bw:.1}" height="{bh:.1}" rx="{:.1}" fill="{BG}" fill-opacity="0.82" stroke="white" stroke-opacity="0.12" stroke-width="2"/><text font-size="{size}" fill="{TEXT}">"#,
            size * 0.6
        );
        for (i, l) in lines.iter().enumerate() {
            let _ = write!(
                s,
                r#"<tspan x="{:.1}" y="{:.1}">{}</tspan>"#,
                x + pad,
                top + pad * 0.6 + lh * (i as f64 + 0.8),
                escape(l)
            );
        }
        s.push_str("</text>\n");
    }

    if !spec.credit.is_empty() {
        let _ = writeln!(
            s,
            r#"<text x="{:.1}" y="{:.1}" font-size="16" fill="{MUTED}" text-anchor="end">{}</text>"#,
            W - 24.0,
            H - 18.0,
            escape(&spec.credit)
        );
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_fits_the_app() {
        // 900 m across a phone at Konstanz's latitude: about zoom 15.
        let spec = Spec {
            look: crate::card::Look::Game,
            lat: 47.66,
            lon: 9.176,
            width_m: 900.0,
            title: String::new(),
            note: String::new(),
            credit: String::new(),
        };
        let z = zoom(&spec);
        assert!((14.5..15.5).contains(&z), "{z}");
        assert!((interp(16.0, 1.0, &[(14.0, 1.5), (18.0, 5.0)]) - 6.5).abs() < 1e-9);
    }

    #[test]
    fn streets_take_their_ends_sides() {
        assert_eq!(side_of(0.8, 0.0).map(|v| v.0), Some(0));
        assert_eq!(side_of(0.0, 0.5).map(|v| v.0), Some(1));
        assert_eq!(side_of(0.05, 0.0), None);
        assert_eq!(step(1.0), STEPS - 1);
        assert_eq!(step(0.01), 0);
    }
}

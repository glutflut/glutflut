//! A picture of the game state for posts: the city as an old map, streets in
//! ink on paper, junctions red (fire), blue (water), violet (front). Drawn as
//! SVG here, rasterised with resvg, JPEG; 1080×1350 (4:5, Instagram).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use resvg::{tiny_skia, usvg};

pub const W: f64 = 1080.0;
pub const H: f64 = 1350.0;
const PAPER: &str = "#eadcbc";
const INK: &str = "#4a3b2a";
const FIRE: &str = "#b23a1e";
const WATER: &str = "#24508f";
const FRONT: &str = "#7a5a8a";

/// What to draw: the frame and the texts on it.
#[derive(Clone, Debug)]
pub struct Spec {
    pub lat: f64,
    pub lon: f64,
    /// Width of the frame in metres (the height follows 4:5).
    pub width_m: f64,
    pub title: String,
    /// Bottom left, in italics; `\n` breaks lines.
    pub note: String,
    pub credit: String,
}

/// Node ids per side.
#[derive(Clone, Debug, Default)]
pub struct Sides {
    pub fire: Vec<i64>,
    pub water: Vec<i64>,
    pub front: Vec<i64>,
}

/// Streets and junctions inside the frame (lon, lat).
#[derive(Default)]
pub struct Geo {
    pub streets: Vec<Vec<[f64; 2]>>,
    pub junctions: Vec<(i64, f64, f64)>,
}

impl Spec {
    fn k(&self) -> (f64, f64) {
        (111_320.0 * self.lat.to_radians().cos(), 110_540.0)
    }

    /// The frame (min lon, min lat, max lon, max lat), plus a margin.
    pub fn bbox(&self) -> [f64; 4] {
        let (kx, ky) = self.k();
        let hw = self.width_m / 2.0 * 1.05;
        let hh = hw * H / W;
        [
            self.lon - hw / kx,
            self.lat - hh / ky,
            self.lon + hw / kx,
            self.lat + hh / ky,
        ]
    }

    fn px(&self, lon: f64, lat: f64) -> (f64, f64) {
        let (kx, ky) = self.k();
        let s = W / self.width_m;
        (
            W / 2.0 + (lon - self.lon) * kx * s,
            H / 2.0 - (lat - self.lat) * ky * s,
        )
    }
}

/// Streets and junctions of a region in the frame (active graph only).
pub fn load(c: &mut postgres::Client, slug: &str, spec: &Spec) -> Result<Geo, postgres::Error> {
    let [x0, y0, x1, y1] = spec.bbox();
    let env = "ST_MakeEnvelope($2, $3, $4, $5, 4326)";
    let streets = c
        .query(
            &format!(
                "SELECT ST_AsGeoJSON(e.geom, 6)::text FROM geo.edges e \
                 JOIN geo.regions r ON r.id = e.region_id \
                 WHERE r.slug = $1 AND e.is_active AND e.geom && {env}"
            ),
            &[&slug, &x0, &y0, &x1, &y1],
        )?
        .iter()
        .filter_map(|r| {
            let v: serde_json::Value = serde_json::from_str(&r.get::<_, String>(0)).ok()?;
            serde_json::from_value(v["coordinates"].clone()).ok()
        })
        .collect();
    let junctions = c
        .query(
            &format!(
                "SELECT n.id, n.lon, n.lat FROM geo.nodes n \
                 JOIN geo.regions r ON r.id = n.region_id \
                 WHERE r.slug = $1 AND n.is_active AND n.geom && {env}"
            ),
            &[&slug, &x0, &y0, &x1, &y1],
        )?
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();
    Ok(Geo { streets, junctions })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A hand-drawn line: densified, pushed sideways by a slow wave.
fn wobble(pts: &[(f64, f64)], rng: &mut StdRng, amp: f64, wl: f64) -> Vec<(f64, f64)> {
    let phase: f64 = rng.random_range(0.0..std::f64::consts::TAU);
    let mut out = Vec::with_capacity(pts.len() * 2);
    let mut dist = 0.0;
    for w in pts.windows(2) {
        let ((ax, ay), (bx, by)) = (w[0], w[1]);
        let len = (bx - ax).hypot(by - ay);
        if len < 1e-9 {
            continue;
        }
        let (nx, ny) = (-(by - ay) / len, (bx - ax) / len);
        let steps = (len / 12.0).ceil().max(1.0) as usize;
        for i in 0..steps {
            let t = i as f64 / steps as f64;
            let d = dist + t * len;
            let o = amp * (phase + d / wl * std::f64::consts::TAU).sin();
            out.push((ax + (bx - ax) * t + nx * o, ay + (by - ay) * t + ny * o));
        }
        dist += len;
    }
    if let Some(&last) = pts.last() {
        out.push(last);
    }
    out
}

fn path_d(pts: &[(f64, f64)], d: &mut String) {
    use std::fmt::Write;
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(d, "{}{x:.1} {y:.1}", if i == 0 { "M" } else { "L" });
    }
}

/// A box behind a line of text, its width guessed from the letters
/// (Garamond: about half an em per letter).
fn text_width(s: &str, size: f64) -> f64 {
    s.chars().count() as f64 * size * 0.47
}

/// The picture as SVG.
pub fn svg(spec: &Spec, geo: &Geo, sides: &Sides) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(1 << 20);
    // The pencil frame's inset; streets and junctions stay inside it.
    let (mx, my) = (W * 0.035, W * 0.035);
    let (iw, ih) = (W - 2.0 * mx, H - 2.0 * my);
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
<defs>
<radialGradient id="vign" cx="50%" cy="50%" r="75%"><stop offset="55%" stop-color="{INK}" stop-opacity="0"/><stop offset="100%" stop-color="{INK}" stop-opacity="0.22"/></radialGradient>
<filter id="grain" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="7"/><feColorMatrix type="matrix" values="0 0 0 0 0.29  0 0 0 0 0.23  0 0 0 0 0.16  0 0 0 0.10 0"/></filter>
<clipPath id="frame"><rect x="{mx:.1}" y="{my:.1}" width="{iw:.1}" height="{ih:.1}"/></clipPath>
</defs>
<rect width="{W}" height="{H}" fill="{PAPER}"/>
<rect width="{W}" height="{H}" filter="url(#grain)"/>
<rect width="{W}" height="{H}" fill="url(#vign)"/>
<g clip-path="url(#frame)" fill="none" stroke="{INK}" stroke-linecap="round" stroke-linejoin="round" stroke-opacity="0.75">
"#
    );
    // Streets in three pen widths.
    let mut rng = StdRng::seed_from_u64(31);
    let amp = 1.2 * 900.0 / spec.width_m.max(100.0);
    let mut pens = [String::new(), String::new(), String::new()];
    for st in &geo.streets {
        let pts: Vec<_> = st.iter().map(|p| spec.px(p[0], p[1])).collect();
        let pen = rng.random_range(0..3);
        path_d(&wobble(&pts, &mut rng, amp, 90.0), &mut pens[pen]);
    }
    for (i, d) in pens.iter().enumerate() {
        if !d.is_empty() {
            let _ = writeln!(
                s,
                r#"<path stroke-width="{:.1}" d="{d}"/>"#,
                1.9 + 0.5 * i as f64
            );
        }
    }
    s.push_str("</g>\n");

    // Junctions: plain ones as specks, sides as dabs of ink.
    let side: HashMap<i64, usize> = [(&sides.fire, 1), (&sides.water, 2), (&sides.front, 3)]
        .into_iter()
        .flat_map(|(ids, k)| ids.iter().map(move |&id| (id, k)))
        .collect();
    let r_dot = 4.6;
    let mut rng = StdRng::seed_from_u64(40);
    let _ = writeln!(s, r#"<g clip-path="url(#frame)">"#);
    let _ = writeln!(s, r#"<g fill="{INK}" fill-opacity="0.5">"#);
    for &(id, lon, lat) in &geo.junctions {
        if !side.contains_key(&id) {
            let (x, y) = spec.px(lon, lat);
            let _ = write!(s, r#"<circle cx="{x:.1}" cy="{y:.1}" r="1.5"/>"#);
        }
    }
    s.push_str("</g>\n");
    for (k, color, r) in [(3, FRONT, r_dot * 0.7), (1, FIRE, r_dot), (2, WATER, r_dot)] {
        let _ = writeln!(s, r#"<g fill="{color}">"#);
        for &(id, lon, lat) in &geo.junctions {
            if side.get(&id) != Some(&k) {
                continue;
            }
            let (x, y) = spec.px(lon, lat);
            let _ = write!(
                s,
                r#"<circle cx="{x:.1}" cy="{y:.1}" r="{r:.1}" fill-opacity="0.8"/>"#
            );
            for _ in 0..2 {
                let (dx, dy) = (
                    rng.random_range(-0.35..0.35) * r,
                    rng.random_range(-0.35..0.35) * r,
                );
                let rr = r * rng.random_range(0.6..0.9);
                let _ = write!(
                    s,
                    r#"<circle cx="{:.1}" cy="{:.1}" r="{rr:.1}" fill-opacity="0.35"/>"#,
                    x + dx,
                    y + dy
                );
            }
        }
        s.push_str("</g>\n");
    }
    s.push_str("</g>\n");

    // A pencil frame, the lines running a little past the corners.
    let mut rng = StdRng::seed_from_u64(3);
    let over = 7.0;
    let corners = [
        ((mx - over, H - my), (W - mx + over, H - my)),
        ((W - mx, H - my + over), (W - mx, my - over)),
        ((W - mx + over, my), (mx - over, my)),
        ((mx, my - over), (mx, H - my + over)),
    ];
    let mut d = String::new();
    for (a, b) in corners {
        path_d(&wobble(&[a, b], &mut rng, 1.5, 360.0), &mut d);
    }
    let _ = writeln!(
        s,
        r#"<path d="{d}" fill="none" stroke="{INK}" stroke-width="2.6" stroke-opacity="0.9" stroke-linecap="round"/>"#
    );

    // Title in a box at the top.
    let t = spec.title.trim();
    if !t.is_empty() {
        let size = 64.0;
        let bw = text_width(t, size) + 2.0 * 0.45 * size;
        let bh = size * 1.55;
        let cy = H * 0.075;
        let _ = writeln!(
            s,
            r#"<rect x="{:.1}" y="{:.1}" width="{bw:.1}" height="{bh:.1}" fill="{PAPER}" fill-opacity="0.93" stroke="{INK}" stroke-width="2.2"/>
<text x="{:.1}" y="{:.1}" font-size="{size}" fill="{INK}" text-anchor="middle">{}</text>"#,
            W / 2.0 - bw / 2.0,
            cy - bh / 2.0,
            W / 2.0,
            cy + size * 0.3,
            esc(t)
        );
    }

    // Note bottom left, italic, slightly turned.
    let lines: Vec<&str> = spec
        .note
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if !lines.is_empty() {
        let size = 34.0;
        let lh = size * 1.2;
        let x = W * 0.08;
        let bottom = H * (1.0 - 0.075);
        let top = bottom - lh * lines.len() as f64;
        let bw = lines
            .iter()
            .map(|l| text_width(l, size))
            .fold(0.0, f64::max)
            + size * 0.7;
        let _ = write!(
            s,
            r#"<g transform="rotate(-2 {x:.1} {bottom:.1})"><rect x="{:.1}" y="{:.1}" width="{bw:.1}" height="{:.1}" fill="{PAPER}" fill-opacity="0.93"/><text font-size="{size}" font-style="italic" fill="{INK}">"#,
            x - size * 0.35,
            top - size * 0.3,
            lh * lines.len() as f64 + size * 0.5,
        );
        for (i, l) in lines.iter().enumerate() {
            let _ = write!(
                s,
                r#"<tspan x="{x:.1}" y="{:.1}">{}</tspan>"#,
                top + lh * (i as f64 + 0.8),
                esc(l)
            );
        }
        s.push_str("</text></g>\n");
    }

    if !spec.credit.is_empty() {
        let _ = writeln!(
            s,
            r#"<text x="{:.1}" y="{:.1}" font-size="17" fill="{INK}" text-anchor="end">{}</text>"#,
            W - mx,
            H - my / 2.0 + 6.0,
            esc(&spec.credit)
        );
    }
    s.push_str("</svg>\n");
    s
}

static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();

fn fonts() -> Arc<usvg::fontdb::Database> {
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_font_data(include_bytes!("../fonts/EBGaramond12-Regular.ttf").to_vec());
            db.load_font_data(include_bytes!("../fonts/EBGaramond12-Italic.ttf").to_vec());
            Arc::new(db)
        })
        .clone()
}

/// The SVG as JPEG (Instagram takes only JPEG; the paper's grain makes a
/// PNG four times as big).
pub fn jpeg(svg: &str) -> Result<Vec<u8>, String> {
    let db = fonts();
    let family = db
        .faces()
        .next()
        .and_then(|f| f.families.first())
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| "serif".into());
    let opt = usvg::Options {
        font_family: family,
        fontdb: db,
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let mut pix = tiny_skia::Pixmap::new(W as u32, H as u32).ok_or("pixmap")?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pix.as_mut());
    // Opaque (the paper covers it all): premultiplied RGBA is plain RGB.
    let rgb: Vec<u8> = pix
        .data()
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let img = image::RgbImage::from_raw(W as u32, H as u32, rgb).ok_or("image")?;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&img)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        Spec {
            lat: 47.66,
            lon: 9.176,
            width_m: 900.0,
            title: "Ergebnis · 1. Advent".into(),
            note: "Sonntag, 21 Uhr.\n<Glut> & Flut".into(),
            credit: "© OpenStreetMap-Mitwirkende".into(),
        }
    }

    #[test]
    fn draws_streets_sides_and_texts() {
        let s = spec();
        let geo = Geo {
            streets: vec![vec![[9.172, 47.658], [9.18, 47.662]]],
            junctions: vec![(1, 9.172, 47.658), (2, 9.18, 47.662), (3, 9.176, 47.66)],
        };
        let sides = Sides {
            fire: vec![1],
            water: vec![2],
            front: vec![],
        };
        let svg = svg(&s, &geo, &sides);
        assert!(svg.contains(FIRE) && svg.contains(WATER));
        assert!(svg.contains("&lt;Glut&gt; &amp; Flut"), "texts escaped");
        let jpg = jpeg(&svg).expect("renders");
        assert_eq!(&jpg[..3], &[0xff, 0xd8, 0xff]);
    }

    #[test]
    fn the_centre_is_the_middle_and_north_is_up() {
        let s = spec();
        let (x, y) = s.px(s.lon, s.lat);
        assert!((x - W / 2.0).abs() < 1e-6 && (y - H / 2.0).abs() < 1e-6);
        assert!(s.px(s.lon, s.lat + 0.001).1 < H / 2.0);
        let [x0, y0, x1, y1] = s.bbox();
        assert!(x0 < s.lon && s.lon < x1 && y0 < s.lat && s.lat < y1);
    }
}

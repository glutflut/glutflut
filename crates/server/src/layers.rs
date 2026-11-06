//! Optional raster layers for the dashboard map, rendered here from public
//! cloud-optimised GeoTIFFs (one per layer, Web Mercator, 8 bit, the last band
//! a score where 254 is favourable). The browser stays on our origin (CSP), and
//! only people with access to the region (and the group, if set) see them.
//!
//!   ELEMENTS_LAYER_COGS=https://host/{mode}/{layer}_3857.tiff   (unset: off)
//!     `{mode}` is optional: tried as `car,foot`, then `none` (GeoSci's travel
//!     modes; a layer lives under one of them).
//!   ELEMENTS_LAYERS=id=Label,id=Label
//!   ELEMENTS_LAYER_CREDIT=Source shown under the map
//!   ELEMENTS_LAYER_GROUP=elements-layers    (optional; admins always pass)

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{who, AppState, Client, RegionQ, Role};
use crate::cog::{self, Cog};

const ZOOM_MAX: u32 = 18;
const MODES: [&str; 2] = ["car,foot", "none"];

struct Config {
    cogs: String,
    layers: Vec<(String, String)>,
    credit: String,
    group: Option<String>,
}

fn parse(
    cogs: Option<String>,
    layers: Option<String>,
    credit: Option<String>,
    group: Option<String>,
) -> Option<Config> {
    let cogs = cogs.filter(|t| t.starts_with("https://") && t.contains("{layer}"))?;
    let layers: Vec<(String, String)> = layers?
        .split(',')
        .filter_map(|l| l.split_once('='))
        .map(|(id, label)| (id.trim().to_string(), label.trim().to_string()))
        .filter(|(id, _)| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .collect();
    (!layers.is_empty()).then(|| Config {
        cogs,
        layers,
        credit: credit.unwrap_or_default(),
        group: group.filter(|g| !g.is_empty()),
    })
}

fn config() -> Option<&'static Config> {
    static CFG: std::sync::OnceLock<Option<Config>> = std::sync::OnceLock::new();
    CFG.get_or_init(|| {
        let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string());
        parse(
            env("ELEMENTS_LAYER_COGS"),
            env("ELEMENTS_LAYERS"),
            env("ELEMENTS_LAYER_CREDIT"),
            env("ELEMENTS_LAYER_GROUP"),
        )
    })
    .as_ref()
}

/// Is `z/x/y` a tile of a known layer on the web map?
fn valid(cfg: &Config, id: &str, z: u32, x: u32, y: u32) -> bool {
    z <= ZOOM_MAX && x < (1 << z) && y < (1 << z) && cfg.layers.iter().any(|(l, _)| l == id)
}

/// The file addresses to try for a layer, in order.
fn urls(cfg: &Config, id: &str) -> Vec<String> {
    let one = |m: &str| cfg.cogs.replace("{layer}", id).replace("{mode}", m);
    if cfg.cogs.contains("{mode}") {
        MODES.iter().map(|m| one(m)).collect()
    } else {
        vec![one("")]
    }
}

/// The opened file of a layer, kept once found.
fn open(cfg: &Config, id: &str) -> Result<Arc<Cog>, cog::Error> {
    static OPEN: Mutex<Option<HashMap<String, Arc<Cog>>>> = Mutex::new(None);
    if let Some(c) = OPEN
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(id)
    {
        return Ok(c.clone());
    }
    for url in urls(cfg, id) {
        match Cog::open(Box::new(cog::Http(url))) {
            Ok(c) => {
                let c = Arc::new(c);
                OPEN.lock()
                    .unwrap()
                    .get_or_insert_with(HashMap::new)
                    .insert(id.to_string(), c.clone());
                return Ok(c);
            }
            Err(cog::Error::NotFound) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(cog::Error::NotFound)
}

/// Red (0, unfavourable) over yellow to green (254), like ColorBrewer RdYlGn;
/// nodata is transparent.
fn palette() -> &'static [[u8; 4]; 256] {
    static P: std::sync::OnceLock<[[u8; 4]; 256]> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        const STOPS: [u32; 11] = [
            0xa50026, 0xd73027, 0xf46d43, 0xfdae61, 0xfee08b, 0xffffbf, 0xd9ef8b, 0xa6d96a,
            0x66bd63, 0x1a9850, 0x006837,
        ];
        let mut p = [[0u8; 4]; 256];
        for (v, c) in p.iter_mut().enumerate().take(255) {
            let t = v as f64 / 254.0 * 10.0;
            let i = (t.floor() as usize).min(9);
            let f = t - i as f64;
            let ch = |k: u32, s: u32| f64::from((k >> s) & 0xff);
            let mix = |s| (ch(STOPS[i], s) * (1.0 - f) + ch(STOPS[i + 1], s) * f).round() as u8;
            *c = [mix(16), mix(8), mix(0), 255];
        }
        p
    })
}

fn png(values: Option<&[u8]>) -> Option<Vec<u8>> {
    let pal = palette();
    let n = cog::TILE * cog::TILE;
    let mut rgba = Vec::with_capacity(n * 4);
    match values {
        Some(v) => v
            .iter()
            .for_each(|&v| rgba.extend_from_slice(&pal[v as usize])),
        None => rgba.resize(n * 4, 0),
    }
    let mut out = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut out),
        &rgba,
        cog::TILE as u32,
        cog::TILE as u32,
        image::ExtendedColorType::Rgba8,
    )
    .ok()?;
    Some(out)
}

/// Access to the region, and the group if there is one.
async fn allowed(
    s: &AppState,
    c: Client,
    headers: &HeaderMap,
    cfg: &Config,
    region: Option<&str>,
) -> bool {
    let Ok(slug) = s.slug(region) else {
        return false;
    };
    let w = who(s, c, headers).await;
    w.may(&slug, Role::Read)
        && cfg
            .group
            .as_deref()
            .is_none_or(|g| w.may(&slug, Role::Admin) || crate::login::in_group(headers, g))
}

/// `GET /api/ops/layers?region=`: what this person can switch on.
pub(super) async fn list(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    let none = || Json(json!({ "layers": [] })).into_response();
    let Some(cfg) = config() else {
        return none();
    };
    if !allowed(&s, c, &headers, cfg, q.region.as_deref()).await {
        return none();
    }
    let layers: Vec<_> = cfg
        .layers
        .iter()
        .map(|(id, label)| json!({ "id": id, "label": label }))
        .collect();
    Json(json!({ "layers": layers, "credit": cfg.credit })).into_response()
}

/// `GET /api/ops/layers/{id}/{z}/{x}/{y}.png?region=`
pub(super) async fn tile(
    State(s): State<AppState>,
    Path((id, z, x, y)): Path<(String, u32, u32, String)>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    let Some(cfg) = config() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !allowed(&s, c, &headers, cfg, q.region.as_deref()).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(y) = y.strip_suffix(".png").and_then(|y| y.parse().ok()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !valid(cfg, &id, z, x, y) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let got = tokio::task::spawn_blocking(move || {
        let values = open(cfg, &id)?.render(z, x, y)?;
        png(values.as_deref()).ok_or(cog::Error::Format("png"))
    })
    .await
    .unwrap_or(Err(cog::Error::Upstream))
    .map_err(|e| {
        if !matches!(e, cog::Error::NotFound) {
            tracing::warn!(error = ?e, "layer tile");
        }
        match e {
            cog::Error::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_GATEWAY,
        }
    });
    match got {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "private, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        Err(code) => code.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        parse(
            Some("https://t.example/v1/{mode}/{layer}_3857.tiff".into()),
            Some("food=Essen und Trinken, green=Grün,bad/id=Nein,=leer".into()),
            None,
            Some(String::new()),
        )
        .unwrap()
    }

    #[test]
    fn only_https_and_clean_ids() {
        assert!(parse(
            Some("http://t/{layer}".into()),
            Some("a=A".into()),
            None,
            None
        )
        .is_none());
        assert!(parse(
            Some("https://t/x.tiff".into()),
            Some("a=A".into()),
            None,
            None
        )
        .is_none());
        assert!(parse(Some("https://t/{layer}".into()), None, None, None).is_none());
        let c = cfg();
        assert_eq!(c.layers.len(), 2);
        assert_eq!(c.layers[0], ("food".into(), "Essen und Trinken".into()));
        assert!(c.group.is_none());
    }

    #[test]
    fn tiles_only_for_known_layers_on_the_map() {
        let c = cfg();
        assert!(valid(&c, "green", 14, 8609, 5718));
        assert!(!valid(&c, "other", 14, 1, 1));
        assert!(!valid(&c, "../x", 14, 1, 1));
        assert!(!valid(&c, "green", 2, 4, 0));
        assert!(!valid(&c, "green", 2, 0, 4));
        assert!(!valid(&c, "green", 40, 0, 0));
    }

    #[test]
    fn modes_are_tried_in_order() {
        assert_eq!(
            urls(&cfg(), "green"),
            [
                "https://t.example/v1/car,foot/green_3857.tiff",
                "https://t.example/v1/none/green_3857.tiff"
            ]
        );
        let one = parse(
            Some("https://t/{layer}.tif".into()),
            Some("a=A".into()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(urls(&one, "a"), ["https://t/a.tif"]);
    }

    /// Against GeoSci's files: `COG_OUT=dir cargo test -- --ignored live_konstanz`.
    #[test]
    #[ignore]
    fn live_konstanz() {
        let c = parse(
            Some("https://ratings-free.geosci.de/2026-10-28/{mode}/{layer}_3857.tiff".into()),
            Some("overall_noise_day=Lärm,eating_out=Gastronomie".into()),
            None,
            None,
        )
        .unwrap();
        let out = std::env::var("COG_OUT").unwrap_or_else(|_| ".".into());
        for id in ["overall_noise_day", "eating_out"] {
            let t0 = std::time::Instant::now();
            let cog = open(&c, id).unwrap();
            for (z, x, y) in [(15, 17219, 11436), (13, 4304, 2859), (10, 538, 357)] {
                let v = cog.render(z, x, y).unwrap().expect("data in Konstanz");
                std::fs::write(format!("{out}/{id}-{z}.png"), png(Some(&v)).unwrap()).unwrap();
            }
            eprintln!("{id}: {:?}", t0.elapsed());
        }
    }

    #[test]
    fn palette_runs_red_to_green_and_png_encodes() {
        let p = palette();
        assert_eq!(p[0], [0xa5, 0x00, 0x26, 255]);
        assert_eq!(p[127][3], 255);
        assert_eq!(p[254], [0x00, 0x68, 0x37, 255]);
        assert_eq!(p[255], [0, 0, 0, 0]);
        let t = png(None).unwrap();
        assert_eq!(&t[1..4], b"PNG");
        assert!(png(Some(&vec![200u8; cog::TILE * cog::TILE])).is_some());
    }
}

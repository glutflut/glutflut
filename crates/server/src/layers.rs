//! Optional raster layers for the dashboard map, rendered here from public
//! cloud-optimised GeoTIFFs (one per layer, Web Mercator, 8 bit, the last band
//! a score where 254 is favourable). The browser stays on our origin (CSP), and
//! only people with access to the region (and the group, if set) see them.
//!
//!   ELEMENTS_LAYER_INDEX=https://api.geosci.de/ratings?api_key=…
//!     JSON index of the current files (GeoSci's ratings list: `api_name`,
//!     `geotiff_url_3857` with `$TRANSPORT_MODE`, nested `sub_ratings`).
//!     Read lazily, again every few hours, so a new data version is picked up.
//!   ELEMENTS_LAYER_COGS=https://host/{mode}/{layer}_3857.tiff
//!     Fallback while the index is unset or unreachable, or lacks a layer.
//!     `{mode}` is optional: tried as `car,foot`, then `none` (GeoSci's travel
//!     modes; a layer lives under one of them).
//!   One of the two must be set, else the layers are off.
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
use std::time::{Duration, Instant};

use super::{who, AppState, Client, RegionQ, Role};
use crate::cog::{self, Cog};

const ZOOM_MAX: u32 = 18;
const MODES: [&str; 2] = ["car,foot", "none"];
/// How long a fetched index counts as current, and the pause after a failure.
const REFRESH: Duration = Duration::from_secs(6 * 3600);
const RETRY: Duration = Duration::from_secs(600);

struct Config {
    index: Option<String>,
    cogs: Option<String>,
    layers: Vec<(String, String)>,
    credit: String,
    group: Option<String>,
}

fn parse(
    index: Option<String>,
    cogs: Option<String>,
    layers: Option<String>,
    credit: Option<String>,
    group: Option<String>,
) -> Option<Config> {
    let index = index.filter(|t| t.starts_with("https://"));
    let cogs = cogs.filter(|t| t.starts_with("https://") && t.contains("{layer}"));
    if index.is_none() && cogs.is_none() {
        return None;
    }
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
        index,
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
            env("ELEMENTS_LAYER_INDEX"),
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

/// Layer id to file address, from an index like GeoSci's ratings list.
fn index_urls(v: &serde_json::Value) -> HashMap<String, String> {
    fn walk(v: &serde_json::Value, out: &mut HashMap<String, String>) {
        let items: Vec<&serde_json::Value> = match v {
            serde_json::Value::Object(m) => m.values().collect(),
            serde_json::Value::Array(a) => a.iter().collect(),
            _ => return,
        };
        for r in items {
            if let (Some(id), Some(url)) = (
                r.get("api_name").and_then(|x| x.as_str()),
                r.get("geotiff_url_3857").and_then(|x| x.as_str()),
            ) {
                if url.starts_with("https://") {
                    out.insert(id.to_string(), url.replace("$TRANSPORT_MODE", MODES[0]));
                }
            }
            if let Some(sub) = r.get("sub_ratings") {
                walk(sub, out);
            }
        }
    }
    let mut out = HashMap::new();
    walk(v, &mut out);
    out
}

#[derive(Default)]
struct Index {
    urls: HashMap<String, String>,
    at: Option<Instant>,
    tried: Option<Instant>,
}

static INDEX: Mutex<Option<Index>> = Mutex::new(None);

fn with_index<T>(f: impl FnOnce(&mut Index) -> T) -> T {
    f(INDEX.lock().unwrap().get_or_insert_with(Index::default))
}

/// The address the index gives for a layer; fetches the index when it is due
/// (blocking, one caller at a time; the others use what is there).
fn indexed(cfg: &Config, id: &str) -> Option<String> {
    let src = cfg.index.as_deref()?;
    let due = with_index(|ix| {
        let due = ix.at.is_none_or(|t| t.elapsed() > REFRESH)
            && ix.tried.is_none_or(|t| t.elapsed() > RETRY);
        if due {
            ix.tried = Some(Instant::now());
        }
        due
    });
    if due {
        let got = crate::shards::agent()
            .ok_or("no agent".to_string())
            .and_then(|a| a.get(src).call().map_err(|e| e.kind().to_string()))
            .and_then(|r| {
                r.into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())
            })
            .map(|v| index_urls(&v));
        match got {
            Ok(urls) if !urls.is_empty() => {
                tracing::info!(layers = urls.len(), "layer index read");
                with_index(|ix| {
                    ix.urls = urls;
                    ix.at = Some(Instant::now());
                });
            }
            Ok(_) => tracing::warn!("layer index: no layers"),
            Err(e) => tracing::warn!(error = %e, "layer index"),
        }
    }
    with_index(|ix| ix.urls.get(id).cloned())
}

/// Is the layer missing from an index that has been read? Never fetches.
fn dropped(cfg: &Config, id: &str) -> bool {
    cfg.index.is_some()
        && cfg.cogs.is_none()
        && with_index(|ix| ix.at.is_some() && !ix.urls.contains_key(id))
}

/// The file addresses to try for a layer, in order: the index's, else the
/// template's.
fn urls(cfg: &Config, id: &str) -> Vec<String> {
    if let Some(u) = indexed(cfg, id) {
        return vec![u];
    }
    let Some(cogs) = cfg.cogs.as_deref() else {
        return Vec::new();
    };
    let one = |m: &str| cogs.replace("{layer}", id).replace("{mode}", m);
    if cogs.contains("{mode}") {
        MODES.iter().map(|m| one(m)).collect()
    } else {
        vec![one("")]
    }
}

/// The opened file of a layer, kept while its address stays the same.
fn open(cfg: &Config, id: &str) -> Result<Arc<Cog>, cog::Error> {
    static OPEN: Mutex<Option<HashMap<String, (String, Arc<Cog>)>>> = Mutex::new(None);
    let tries = urls(cfg, id);
    if let Some((_, c)) = OPEN
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(id)
        .filter(|(u, _)| tries.contains(u))
    {
        return Ok(c.clone());
    }
    for url in tries {
        match Cog::open(Box::new(cog::Http(url.clone()))) {
            Ok(c) => {
                let c = Arc::new(c);
                OPEN.lock()
                    .unwrap()
                    .get_or_insert_with(HashMap::new)
                    .insert(id.to_string(), (url, c.clone()));
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
        .filter(|(id, _)| !dropped(cfg, id))
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
            None,
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
            None,
            Some("http://t/{layer}".into()),
            Some("a=A".into()),
            None,
            None
        )
        .is_none());
        assert!(parse(
            Some("http://t/index".into()),
            Some("https://t/x.tiff".into()),
            Some("a=A".into()),
            None,
            None
        )
        .is_none());
        assert!(parse(None, Some("https://t/{layer}".into()), None, None, None).is_none());
        let ix = parse(
            Some("https://t/ratings".into()),
            None,
            Some("a=A".into()),
            None,
            None,
        );
        assert!(ix.is_some_and(|c| c.cogs.is_none()));
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
            None,
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
            None,
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
    fn index_gives_current_files_by_mode() {
        let v = serde_json::json!({
            "shopping": {
                "api_name": "shopping",
                "geotiff_url_3857": "https://h/2026-11-19/$TRANSPORT_MODE/shopping_3857.tiff",
                "sub_ratings": {
                    "daily_needs": {
                        "api_name": "daily_needs",
                        "geotiff_url_3857": "https://h/2026-11-19/$TRANSPORT_MODE/daily_needs_3857.tiff"
                    }
                }
            },
            "all_noise": {
                "api_name": "all_noise",
                "geotiff_url_3857": null,
                "sub_ratings": [{
                    "api_name": "overall_noise_day",
                    "geotiff_url_3857": "https://h/2026-11-19/none/overall_noise_day_3857.tiff"
                }]
            },
            "bad": { "api_name": "bad", "geotiff_url_3857": "http://h/x.tiff" }
        });
        let u = index_urls(&v);
        assert_eq!(u.len(), 3);
        assert_eq!(
            u["daily_needs"],
            "https://h/2026-11-19/car,foot/daily_needs_3857.tiff"
        );
        assert_eq!(
            u["overall_noise_day"],
            "https://h/2026-11-19/none/overall_noise_day_3857.tiff"
        );
    }

    /// Against the live index: `LAYER_INDEX=https://… cargo test -- --ignored live_index`.
    #[test]
    #[ignore]
    fn live_index() {
        let c = parse(
            std::env::var("LAYER_INDEX").ok(),
            None,
            Some("eating_out=Gastronomie,overall_noise_day=Ruhe".into()),
            None,
            None,
        )
        .unwrap();
        for id in ["eating_out", "overall_noise_day"] {
            let u = urls(&c, id);
            eprintln!("{id}: {u:?}");
            assert_eq!(u.len(), 1);
            let v = open(&c, id).unwrap().render(15, 17219, 11436).unwrap();
            assert!(v.is_some(), "data in Konstanz");
        }
        assert!(dropped(&c, "local_green_spaces"));
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

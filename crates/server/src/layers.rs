//! Optional raster layers for the dashboard map, from a tile service of the
//! operator's choice. The server fetches the tiles: the browser stays on our
//! origin (CSP, CORS), the service's address stays out of the page, and only
//! people with access to the region (and the group, if set) get them.
//!
//!   ELEMENTS_LAYER_TILES=https://host/{layer}/{z}/{x}/{y}.png   (unset: off)
//!   ELEMENTS_LAYERS=id=Label,id=Label
//!   ELEMENTS_LAYER_CREDIT=Source shown under the map
//!   ELEMENTS_LAYER_GROUP=elements-layers    (optional; admins always pass)

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::io::Read;

use super::{who, AppState, Client, RegionQ, Role};

/// A tile is far smaller; a bigger answer is not one.
const TILE_MAX: u64 = 4 << 20;
const ZOOM_MAX: u32 = 18;

struct Config {
    tiles: String,
    layers: Vec<(String, String)>,
    credit: String,
    group: Option<String>,
}

fn parse(
    tiles: Option<String>,
    layers: Option<String>,
    credit: Option<String>,
    group: Option<String>,
) -> Option<Config> {
    let tiles = tiles.filter(|t| t.starts_with("https://"))?;
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
        tiles,
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
            env("ELEMENTS_LAYER_TILES"),
            env("ELEMENTS_LAYERS"),
            env("ELEMENTS_LAYER_CREDIT"),
            env("ELEMENTS_LAYER_GROUP"),
        )
    })
    .as_ref()
}

/// The tile's address, if it exists on a web map at all.
fn tile_url(cfg: &Config, id: &str, z: u32, x: u32, y: u32) -> Option<String> {
    if z > ZOOM_MAX || x >= (1 << z) || y >= (1 << z) {
        return None;
    }
    cfg.layers.iter().find(|(l, _)| l == id)?;
    Some(
        cfg.tiles
            .replace("{layer}", id)
            .replace("{z}", &z.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string()),
    )
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
    let url = y
        .strip_suffix(".png")
        .and_then(|y| y.parse().ok())
        .and_then(|y| tile_url(cfg, &id, z, x, y));
    let (Some(url), Some(agent)) = (url, crate::shards::agent()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let got = tokio::task::spawn_blocking(move || match agent.get(&url).call() {
        Ok(r) => {
            let mut bytes = Vec::new();
            r.into_reader()
                .take(TILE_MAX + 1)
                .read_to_end(&mut bytes)
                .ok()
                .filter(|n| *n as u64 <= TILE_MAX)
                .map_or(Err(StatusCode::BAD_GATEWAY), |_| Ok(bytes))
        }
        Err(ureq::Error::Status(404, _)) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    })
    .await
    .unwrap_or(Err(StatusCode::BAD_GATEWAY));
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
            Some("https://t.example/{layer}/{z}/{x}/{y}.png?c=1".into()),
            Some("food=Essen und Trinken, green=Grün,bad/id=Nein,=leer".into()),
            None,
            Some(String::new()),
        )
        .unwrap()
    }

    #[test]
    fn only_https_and_clean_ids() {
        assert!(parse(Some("http://t".into()), Some("a=A".into()), None, None).is_none());
        assert!(parse(Some("https://t".into()), None, None, None).is_none());
        let c = cfg();
        assert_eq!(c.layers.len(), 2);
        assert_eq!(c.layers[0], ("food".into(), "Essen und Trinken".into()));
        assert!(c.group.is_none());
    }

    #[test]
    fn tile_only_for_known_layers_on_the_map() {
        let c = cfg();
        assert_eq!(
            tile_url(&c, "green", 14, 8609, 5718).unwrap(),
            "https://t.example/green/14/8609/5718.png?c=1"
        );
        assert!(tile_url(&c, "other", 14, 1, 1).is_none());
        assert!(tile_url(&c, "../x", 14, 1, 1).is_none());
        assert!(tile_url(&c, "green", 2, 4, 0).is_none());
        assert!(tile_url(&c, "green", 2, 0, 4).is_none());
        assert!(tile_url(&c, "green", 40, 0, 0).is_none());
    }
}

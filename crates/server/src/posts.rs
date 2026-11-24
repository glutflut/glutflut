//! Posts from the game (social posts): the picture of the game state
//! (`card.rs`) for the editor. A child module of the gateway.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::{Extension, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::{who, AppState, Client, Role};
use crate::card::{self, Sides, Spec};

pub const CREDIT: &str = "© OpenStreetMap";

/// The sides at a chapter's end (`game.chapter_snapshots`), if it has one.
pub fn snapshot_sides(
    db: &mut postgres::Client,
    slug: &str,
    key: &str,
) -> Result<Option<Sides>, postgres::Error> {
    Ok(db
        .query_opt(
            "SELECT fire, water, front FROM game.chapter_snapshots WHERE region = $1 AND key = $2",
            &[&slug, &key],
        )?
        .map(|r| Sides {
            fire: r.get(0),
            water: r.get(1),
            front: r.get(2),
        }))
}

/// The sides now: asks the running region and waits for it (a few seconds).
// Err is the ready answer, as axum handlers return it.
#[allow(clippy::result_large_err)]
async fn live_sides(s: &AppState, slug: &str) -> Result<Sides, Response> {
    let r = s.region(Some(slug))?;
    let t0 = crate::analytics::now_s();
    for _ in 0..50 {
        let json = r.nodes.json.read().unwrap().clone();
        if let Some(v) = json
            .as_deref()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
            .filter(|v| v["t"].as_f64().is_some_and(|t| t >= t0 - 1.0))
        {
            let ids = |k: &str| serde_json::from_value(v[k].clone()).unwrap_or_default();
            return Ok(Sides {
                fire: ids("fire"),
                water: ids("water"),
                front: ids("front"),
            });
        }
        r.nodes.wanted.store(true, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "2")],
        "building",
    )
        .into_response())
}

#[derive(Deserialize)]
pub(super) struct CardQ {
    region: Option<String>,
    /// The state at this chapter's end, else now.
    chapter: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    width_m: Option<f64>,
    title: Option<String>,
    note: Option<String>,
    credit: Option<String>,
}

/// `GET /api/ops/card.png?region=&chapter=&lat=&lon=&width_m=&title=&note=`
/// (editor): the picture of the game state, centred on the region if no
/// place is given.
pub(super) async fn card_png(
    State(s): State<AppState>,
    Query(q): Query<CardQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !who(&s, c, &headers).await.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let width_m = q.width_m.unwrap_or(900.0);
    if !(200.0..=5000.0).contains(&width_m) {
        return (StatusCode::BAD_REQUEST, "width_m: 200 to 5000").into_response();
    }
    let clip = |t: Option<String>, n: usize| t.unwrap_or_default().chars().take(n).collect();
    let mut spec = Spec {
        lat: q.lat.unwrap_or(f64::NAN),
        lon: q.lon.unwrap_or(f64::NAN),
        width_m,
        title: clip(q.title, 40),
        note: clip(q.note, 120),
        credit: q
            .credit
            .map_or_else(|| CREDIT.into(), |c| clip(Some(c), 120)),
    };
    let live = match q.chapter {
        None => Some(match live_sides(&s, &slug).await {
            Ok(v) => v,
            Err(e) => return e,
        }),
        Some(_) => None,
    };
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            if !(spec.lat.is_finite() && spec.lon.is_finite()) {
                let r = db.query_one(
                    "SELECT ST_Y(centroid_geom), ST_X(centroid_geom) FROM geo.regions WHERE slug = $1",
                    &[&slug],
                )?;
                (spec.lat, spec.lon) = (r.get(0), r.get(1));
            }
            let sides = match (live, &q.chapter) {
                (Some(v), _) => Some(v),
                (None, Some(k)) => snapshot_sides(db, &slug, k)?,
                (None, None) => None,
            };
            let geo = card::load(db, &slug, &spec)?;
            Ok(sides.map(|sd| card::svg(&spec, &geo, &sd)))
        })
    })
    .await
    .ok()
    .flatten();
    let svg = match out {
        Some(Some(v)) => v,
        Some(None) => {
            return (StatusCode::NOT_FOUND, "no snapshot for this chapter").into_response()
        }
        None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match tokio::task::spawn_blocking(move || card::png(&svg)).await {
        Ok(Ok(png)) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            png,
        )
            .into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

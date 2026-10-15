//! Areas in the dashboard: held area per side and hour
//! (`analytics.area_hour`) and the blocks each side held longest
//! (`analytics.block_day`), from `area::AreaLog`. Game state the map shows
//! anyway, so no k; the demo day has none (it simulates walks, not the game).
//! A child module of the gateway.

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::{access, AnalyticsQuery, AppState, Client};

/// Blocks on the map at most (those held longest).
const BLOCKS_MAX: i64 = 3000;

/// `GET /api/areas?region=&days=|day=` (token with access to the region; the
/// demo day is public and empty).
pub(super) async fn areas(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Query(q): Query<AnalyticsQuery>,
) -> Response {
    let region = match s.slug(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if q.is_demo() {
        return Json(json!({ "hours": [], "blocks": { "type": "FeatureCollection", "features": [] }, "days": 1 }))
            .into_response();
    }
    if !access(&s, c, &headers).await.allows(&region) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Last n days up to now, or one UTC day.
    let (days, day): (i32, Option<String>) = match q.day.as_deref() {
        Some(d) if crate::db::Period::day(d).is_some() => (1, Some(d.to_string())),
        Some(_) => return (StatusCode::BAD_REQUEST, "day must be YYYY-MM-DD").into_response(),
        None => (q.days.unwrap_or(7).clamp(1, 365), None),
    };
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_read(|db| {
            let win = "($3::text IS NULL AND {c} > now() - make_interval(days => $2)) \
                       OR ({c} >= $3::text::date AND {c} < $3::text::date + 1)";
            let hours: Vec<Value> = db
                .query(
                    &format!(
                        "SELECT extract(epoch FROM hour_start)::int8, fire_ha, water_ha, fire_player_ha, water_player_ha, \
                         fire_max_ha, water_max_ha, minutes FROM analytics.area_hour \
                         WHERE region = $1 AND ({}) ORDER BY hour_start",
                        win.replace("{c}", "hour_start")
                    ),
                    &[&region, &days, &day],
                )?
                .iter()
                .map(|r| {
                    json!([
                        r.get::<_, i64>(0),
                        r.get::<_, f32>(1),
                        r.get::<_, f32>(2),
                        r.get::<_, f32>(3),
                        r.get::<_, f32>(4),
                        r.get::<_, f32>(5),
                        r.get::<_, f32>(6),
                        r.get::<_, i16>(7)
                    ])
                })
                .collect();
            // Days count whole for blocks (UTC dates).
            let features: Vec<Value> = db
                .query(
                    &format!(
                        "WITH m AS (SELECT block_id, sum(fire_min)::int8 AS f, sum(water_min)::int8 AS w \
                           FROM analytics.block_day WHERE region = $1 AND ({}) GROUP BY 1 \
                           ORDER BY sum(fire_min) + sum(water_min) DESC LIMIT {BLOCKS_MAX}) \
                         SELECT m.block_id, m.f, m.w, b.area_m2, ST_AsGeoJSON(ST_SimplifyPreserveTopology(b.geom, 0.00002), 6) \
                         FROM m JOIN geo.blocks b ON b.region = $1 AND b.id = m.block_id",
                        win.replace("{c}", "day").replace(
                            "now() - make_interval(days => $2)",
                            "(now() - make_interval(days => $2))::date"
                        )
                    ),
                    &[&region, &days, &day],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "type": "Feature",
                        "properties": { "id": r.get::<_, i32>(0), "f": r.get::<_, i64>(1), "w": r.get::<_, i64>(2), "ha": r.get::<_, f32>(3) / 1e4 },
                        "geometry": serde_json::from_str::<Value>(r.get(4)).unwrap_or(Value::Null),
                    })
                })
                .collect();
            let total_ha: f64 = db
                .query_one(
                    "SELECT coalesce(sum(area_m2), 0)::float8 / 1e4 FROM geo.blocks WHERE region = $1",
                    &[&region],
                )?
                .get(0);
            Ok(json!({
                "days": days,
                "hours": hours,
                "blocks": { "type": "FeatureCollection", "features": features },
                "total_ha": total_ha,
            }))
        })
    })
    .await
    .ok()
    .flatten();
    match out {
        Some(v) => Json(v).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

//! Before/after report for a focus place: a zone's
//! circle and two periods A and B, side by side. Everything comes from the
//! analytics tables, which hold only rows with at least k persons; the
//! report only sums. Per-day means make periods of different length
//! comparable; the place's share of the whole city's dwell evens out
//! weather, holidays and promotion. A child module of the gateway.

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{access, AppState, Client};

/// Zone radius when the zone has none (`analytics::ZONE_R_M`).
const RADIUS_DEFAULT: f64 = 400.0;
/// H3 resolution for the hours of the day (~66 m cells).
const HEX_RES: i16 = 10;
const DAYS_MAX: i64 = 366;
/// Below these the report says the data is thin.
const THIN_DAYS: i64 = 7;
const THIN_PLAYERS: f64 = 20.0;

#[derive(Deserialize)]
pub(super) struct ReportQ {
    region: Option<String>,
    zone: String,
    /// `YYYY-MM-DD..YYYY-MM-DD`, end included, in the region's time zone.
    a: String,
    b: String,
    /// `synthetic` (tests, demo days) or `all`; default real data only.
    data: Option<String>,
}

/// A period: first and last day (inclusive).
#[derive(Clone, Debug, PartialEq)]
pub struct Days {
    pub from: String,
    pub to: String,
    pub n: i64,
}

/// `YYYY-MM-DD..YYYY-MM-DD` -> days (end not before start, at most a year).
pub fn parse_days(s: &str) -> Result<Days, &'static str> {
    let (a, b) = s.split_once("..").ok_or("period: from..to")?;
    let day = |d: &str| -> Option<i64> {
        let d = d.trim();
        if d.len() != 10 || d.as_bytes()[4] != b'-' || d.as_bytes()[7] != b'-' {
            return None;
        }
        let (y, m, dd): (i64, i64, i64) = (
            d[..4].parse().ok()?,
            d[5..7].parse().ok()?,
            d[8..].parse().ok()?,
        );
        if !(1..=12).contains(&m) || !(1..=31).contains(&dd) {
            return None;
        }
        // Days from civil (Howard Hinnant), only to count days.
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + dd - 1;
        Some(era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy)
    };
    let (x, y) = (
        day(a).ok_or("period: dates as YYYY-MM-DD")?,
        day(b).ok_or("period: dates as YYYY-MM-DD")?,
    );
    let n = y - x + 1;
    if n < 1 {
        return Err("period: the end is before the start");
    }
    if n > DAYS_MAX {
        return Err("period: at most a year");
    }
    Ok(Days {
        from: a.trim().into(),
        to: b.trim().into(),
        n,
    })
}

/// The measures of one period at one place.
pub fn period(
    db: &mut postgres::Client,
    region: &str,
    zone: &str,
    d: &Days,
    synthetic: &[bool],
) -> Result<Value, postgres::Error> {
    // Shared bits: the circle, the window in the region's time zone.
    // `syn` types $5 for the queries that don't use it (interventions).
    let base = "WITH syn AS (SELECT $5::bool[] AS s), z AS (SELECT ST_SetSRID(ST_MakePoint(lon, lat), 4326) AS p, \
                  coalesce(radius_m, $6::float8)::float8 AS r FROM geo.zones WHERE region = $1 AND id = $2), \
                tz AS (SELECT coalesce(timezone, 'Europe/Berlin') AS tz, id AS rid FROM geo.regions WHERE slug = $1), \
                w AS (SELECT ($3::text::date)::timestamp AT TIME ZONE tz.tz AS t0, \
                             ($4::text::date + 1)::timestamp AT TIME ZONE tz.tz AS t1, tz.tz, tz.rid FROM tz), \
                e AS (SELECT ed.id FROM geo.edges ed, z, w WHERE ed.region_id = w.rid \
                  AND ed.geom && ST_Expand(z.p, z.r / 111000.0 * 2) \
                  AND ST_DWithin(ed.geom::geography, z.p::geography, z.r)) ";
    let p: [&(dyn postgres::types::ToSql + Sync); 6] =
        [&region, &zone, &d.from, &d.to, &synthetic, &RADIUS_DEFAULT];
    let q = |db: &mut postgres::Client, tail: &str| db.query(&format!("{base}{tail}"), &p);

    let r = &q(
        db,
        "SELECT coalesce(sum(dwell_s) FILTER (WHERE edge_id IN (SELECT id FROM e)), 0)::int8, \
                coalesce(sum(dwell_s), 0)::int8, \
                count(DISTINCT (window_start AT TIME ZONE (SELECT tz FROM w))::date) \
                  FILTER (WHERE edge_id IN (SELECT id FROM e)), \
                (SELECT count(*) FROM e) \
         FROM analytics.dwell, w WHERE region = $1 AND synthetic = ANY($5) \
           AND window_start >= w.t0 AND window_start < w.t1",
    )?[0];
    // No count of visits: persons per street and window would count one
    // walk through many streets many times.
    let (dwell_s, city_s, days_with, edges): (i64, i64, i64, i64) =
        (r.get(0), r.get(1), r.get(2), r.get(3));

    let hours: Vec<f64> = {
        let mut h = vec![0.0; 24];
        for row in q(
            db,
            &format!(
                "SELECT extract(hour FROM hour_start AT TIME ZONE w.tz)::int, sum(dwell_s)::int8 \
                 FROM analytics.hex_dwell, w, z WHERE region = $1 AND res = {HEX_RES} AND synthetic = ANY($5) AND span_s < 86400 \
                   AND hour_start >= w.t0 AND hour_start < w.t1 \
                   AND ST_DWithin(h3_cell_to_geometry(cell::h3index)::geography, z.p::geography, z.r) \
                 GROUP BY 1"
            ),
        )? {
            let (i, s): (i32, i64) = (row.get(0), row.get(1));
            if let Some(x) = h.get_mut(i as usize) {
                // Person-hours per day at that hour.
                *x = s as f64 / 3600.0 / d.n as f64;
            }
        }
        h
    };

    let perception: serde_json::Map<String, Value> = q(
        db,
        "SELECT category, sum(count)::int8 FROM analytics.perception pc \
           JOIN geo.nodes n ON n.id = pc.node_id, z \
         WHERE pc.region = $1 AND pc.synthetic = ANY($5) AND pc.day BETWEEN $3::text::date AND $4::text::date \
           AND pc.category NOT LIKE '#%' \
           AND n.geom && ST_Expand(z.p, z.r / 111000.0 * 2) \
           AND ST_DWithin(n.geom::geography, z.p::geography, z.r) \
         GROUP BY 1 ORDER BY 1",
    )?
    .iter()
    .map(|r| (r.get::<_, String>(0), Value::from(r.get::<_, i64>(1))))
    .collect();

    let flows = |rows: Vec<postgres::Row>| -> Vec<Value> {
        rows.iter()
            .map(|r| json!({ "zone": r.get::<_, String>(0), "name": r.get::<_, Option<String>>(1), "persons": r.get::<_, i64>(2) }))
            .collect()
    };
    let inflow = flows(q(
        db,
        &format!(
            "SELECT f.from_zone, (SELECT name FROM geo.zones WHERE region = $1 AND id = f.from_zone), sum(f.persons)::int8 \
             FROM {} f, w WHERE f.region = $1 AND f.to_zone = $2 AND f.synthetic = ANY($5) \
               AND f.t >= w.t0 AND f.t < w.t1 GROUP BY 1 ORDER BY 3 DESC LIMIT 10",
            crate::db::flows_all()
        ),
    )?);
    let outflow = flows(q(
        db,
        &format!(
            "SELECT f.to_zone, (SELECT name FROM geo.zones WHERE region = $1 AND id = f.to_zone), sum(f.persons)::int8 \
             FROM {} f, w WHERE f.region = $1 AND f.from_zone = $2 AND f.synthetic = ANY($5) \
               AND f.t >= w.t0 AND f.t < w.t1 GROUP BY 1 ORDER BY 3 DESC LIMIT 10",
            crate::db::flows_all()
        ),
    )?);

    let interventions: Vec<Value> = q(
        db,
        "SELECT i.source, count(*)::int8, \
                coalesce(sum(extract(epoch FROM least(i.ends_at, w.t1) - greatest(i.starts_at, w.t0))), 0)::float8 / 3600 \
         FROM analytics.interventions i JOIN geo.nodes n ON n.id = i.node_id, w, z \
         WHERE i.region = $1 AND i.starts_at < w.t1 AND i.ends_at > w.t0 \
           AND ST_DWithin(n.geom::geography, z.p::geography, z.r) GROUP BY 1 ORDER BY 1",
    )?
    .iter()
    .map(|r| json!({ "source": r.get::<_, String>(0), "count": r.get::<_, i64>(1), "hours": r.get::<_, f64>(2) }))
    .collect();

    // Visitors counted at the focus place itself: per hour, plus
    // leftovers pooled per day; with noise like every published count.
    let pn = crate::db::noisy("persons", crate::db::PLACE_KEY);
    let mut visit_hours = vec![0.0; 24];
    let (mut visits, mut visit_s, mut pooled) = (0.0, 0.0, 0i64);
    for row in q(
        db,
        &format!(
            "SELECT CASE WHEN span_s = 3600 THEN extract(hour FROM start AT TIME ZONE w.tz)::int ELSE -1 END, \
               sum(greatest(0, {pn}))::float8, sum(dwell_s)::float8, count(*) FILTER (WHERE span_s > 3600) \
             FROM analytics.place_visits, w WHERE region = $1 AND zone_id = $2 AND synthetic = ANY($5) \
               AND start >= w.t0 AND start < w.t1 GROUP BY 1"
        ),
    )? {
        let (h, n, s, days): (i32, f64, f64, i64) = (row.get(0), row.get(1), row.get(2), row.get(3));
        visits += n;
        visit_s += s;
        pooled += days;
        if let Some(x) = usize::try_from(h).ok().and_then(|h| visit_hours.get_mut(h)) {
            *x += n / d.n as f64;
        }
    }

    let r = &q(
        db,
        "SELECT coalesce(avg(players), 0)::float8, count(*) FROM (SELECT day, sum(players) AS players \
           FROM analytics.daily WHERE region = $1 AND synthetic = ANY($5) \
             AND day BETWEEN $3::text::date AND $4::text::date GROUP BY day) t",
    )?[0];
    let (players, player_days): (f64, i64) = (r.get(0), r.get(1));

    let n = d.n as f64;
    let thin = days_with < THIN_DAYS || players < THIN_PLAYERS;
    Ok(json!({
        "from": d.from, "to": d.to, "days": d.n,
        "days_with_data": days_with,
        "streets": edges,
        "dwell_h_per_day": dwell_s as f64 / 3600.0 / n,
        "share_of_city": if city_s > 0 { dwell_s as f64 / city_s as f64 } else { 0.0 },
        "hours": hours,
        "perception": perception,
        "inflow": inflow,
        "outflow": outflow,
        "interventions": interventions,
        "players_per_day": players,
        "visits_per_day": visits / n,
        "visit_h_per_day": visit_s / 3600.0 / n,
        "visit_hours": visit_hours,
        "visit_pooled_days": pooled,
        "player_days": player_days,
        "thin": thin,
    }))
}

/// `GET /api/report?region=&zone=&a=from..to&b=from..to[&data=synthetic]`
/// (token with access to the region).
pub(super) async fn report(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Query(q): Query<ReportQ>,
) -> Response {
    let region = match s.slug(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !access(&s, c, &headers).await.allows(&region) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (a, b) = match (parse_days(&q.a), parse_days(&q.b)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": e })),
            )
                .into_response()
        }
    };
    let synthetic = match q.data.as_deref() {
        Some("synthetic") => vec![true],
        Some("all") => vec![false, true],
        _ => vec![false],
    };
    let st = s.clone();
    let zone = q.zone.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_read(|db| {
            if let Err(e) = crate::db::set_noise_key(db) {
                tracing::warn!(error = %e, "report: noise key");
            }
            let z = db.query_opt(
                "SELECT name, lat, lon, coalesce(radius_m, $3::float8)::float8, focus FROM geo.zones \
                 WHERE region = $1 AND id = $2",
                &[&region, &zone, &RADIUS_DEFAULT],
            )?;
            let Some(z) = z else { return Ok(None) };
            Ok(Some(json!({
                "region": region,
                "zone": { "id": zone, "name": z.get::<_, String>(0), "lat": z.get::<_, f64>(1),
                          "lon": z.get::<_, f64>(2), "radius_m": z.get::<_, f64>(3), "focus": z.get::<_, bool>(4) },
                "data": if synthetic == [true] { "synthetic" } else if synthetic.len() == 2 { "all" } else { "real" },
                "a": period(db, &region, &zone, &a, &synthetic)?,
                "b": period(db, &region, &zone, &b, &synthetic)?,
            })))
        })
    })
    .await
    .ok()
    .flatten();
    match out {
        Some(Some(v)) => Json(v).into_response(),
        Some(None) => (StatusCode::NOT_FOUND, "no such zone").into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods() {
        assert_eq!(parse_days("2026-12-01..2026-12-07").unwrap().n, 7);
        assert_eq!(parse_days("2026-12-31..2027-01-01").unwrap().n, 2);
        assert_eq!(
            parse_days("2028-02-28..2028-03-01").unwrap().n,
            3,
            "leap year"
        );
        assert_eq!(parse_days("2026-12-06..2026-12-06").unwrap().n, 1);
        assert!(
            parse_days("2026-12-07..2026-12-01").is_err(),
            "end before start"
        );
        assert!(
            parse_days("2025-01-01..2026-12-31").is_err(),
            "more than a year"
        );
        assert!(parse_days("2026-12-01").is_err());
        assert!(parse_days("2026-13-01..2026-13-02").is_err());
        assert!(parse_days("01.12.2026..07.12.2026").is_err());
    }
}

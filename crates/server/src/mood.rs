//! The mood map (`GET /api/mood`, public): how places felt ("Wie ist es
//! hier?"), per H3 cell of about 170 m, for one past day: yesterday, or the
//! latest of the week before with data. A cell shows only if at least k
//! different persons rated in it that day (`analytics::RATERS`), and only
//! shares in steps of 10 %, no counts and no hours. Real players only; a
//! demo region shows its synthetic ones. Answers to the city's questions
//! (`q<id>.<n>`) are the city's, not shown here.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::{AppState, RegionQ};

/// Look back this many days for a day with data.
const DAYS: i32 = 7;

pub(super) async fn mood(State(s): State<AppState>, Query(q): Query<RegionQ>) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let demo = s.host.listing(&slug).is_some_and(|l| l.demo_day.is_some());
    let k = crate::analytics::k_min() as i64;
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || st.with_db(|db| load(db, &slug, demo, k)))
        .await
        .ok()
        .flatten();
    match out {
        Some(v) => ([(header::CACHE_CONTROL, "public, max-age=600")], Json(v)).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

fn load(
    db: &mut postgres::Client,
    slug: &str,
    demo: bool,
    k: i64,
) -> Result<Value, postgres::Error> {
    let rows = db.query(
        "SELECT p.day::text, ST_Y(n.geom), ST_X(n.geom), p.category, sum(p.count)::int8 \
         FROM analytics.perception p JOIN geo.nodes n ON n.id = p.node_id \
         WHERE p.region = $1 AND p.synthetic = $2 \
           AND p.day BETWEEN (now() AT TIME ZONE 'UTC')::date - $3::int \
                         AND (now() AT TIME ZONE 'UTC')::date - 1 \
         GROUP BY 1, 2, 3, 4",
        &[&slug, &demo, &DAYS],
    )?;
    // day -> cell -> (persons, category -> ratings)
    type Cell = (i64, HashMap<String, i64>);
    let mut days: HashMap<String, HashMap<u64, Cell>> = HashMap::new();
    for r in &rows {
        let cell = crate::analytics::mood_cell(r.get(1), r.get(2));
        let e = days.entry(r.get(0)).or_default().entry(cell).or_default();
        let (cat, n): (String, i64) = (r.get(3), r.get(4));
        if cat == crate::analytics::RATERS {
            e.0 += n;
        } else if !cat.starts_with('#') && !cat.starts_with('q') {
            *e.1.entry(cat).or_default() += n;
        }
    }
    // The latest day with at least one cell of k persons.
    let mut list: Vec<_> = days.into_iter().collect();
    list.sort_by(|a, b| b.0.cmp(&a.0));
    for (day, cells) in list {
        let shown: Vec<Value> = cells
            .into_iter()
            .filter(|(c, (persons, _))| *c != 0 && *persons >= k)
            .map(|(c, (_, cats))| {
                let total: i64 = cats.values().sum::<i64>().max(1);
                let shares: serde_json::Map<String, Value> = cats
                    .into_iter()
                    .map(|(cat, n)| {
                        let pct = ((n * 100 / total + 5) / 10 * 10).min(100);
                        (cat, json!(pct))
                    })
                    .filter(|(_, v)| v.as_i64().unwrap_or(0) > 0)
                    .collect();
                json!([format!("{c:x}"), shares])
            })
            .collect();
        if !shown.is_empty() {
            return Ok(json!({ "day": day, "k": k, "cells": shown }));
        }
    }
    Ok(json!({ "day": null, "k": k, "cells": [] }))
}

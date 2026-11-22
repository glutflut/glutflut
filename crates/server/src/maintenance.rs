//! Planned maintenance (`ops.maintenance`, one table per shard database): the
//! admin page sets windows, the app announces the next one 48 h ahead (banner,
//! toast 10 min before, overlay while it runs). The page during the downtime
//! comes from the proxy, not from here. A child module of the gateway.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Extension, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::audit_entity;
use super::{who, Access, AppState, Client, RegionQ, Role};

/// How far ahead the app shows a window.
const AHEAD_H: i32 = 48;
/// Fixed extra sentences (keys the app translates).
const NOTES: [&str; 3] = ["move", "update", "database"];
const MINUTES: std::ops::RangeInclusive<i32> = 5..=720;
/// Re-read from the database at most this often.
const TTL: Duration = Duration::from_secs(60);

struct Cached {
    at: Instant,
    window: Option<Value>,
    tz: HashMap<String, String>,
}

static CACHE: Mutex<Option<Arc<Cached>>> = Mutex::new(None);

fn forget() {
    *CACHE.lock().unwrap() = None;
}

async fn current(s: &AppState) -> Option<Arc<Cached>> {
    if let Some(c) = CACHE.lock().unwrap().as_ref() {
        if c.at.elapsed() < TTL {
            return Some(c.clone());
        }
    }
    let shard = s.host.shard().to_string();
    let st = s.clone();
    let (window, tz) = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let w = db.query_opt(
                "SELECT id, extract(epoch FROM starts)::float8, extract(epoch FROM ends)::float8, note \
                 FROM ops.maintenance WHERE cancelled_at IS NULL AND ends > now() \
                   AND starts < now() + make_interval(hours => $1) \
                 ORDER BY starts LIMIT 1",
                &[&AHEAD_H],
            )?;
            let tz = db
                .query(
                    "SELECT slug, coalesce(timezone, 'Europe/Berlin') FROM geo.regions WHERE shard = $1",
                    &[&shard],
                )?
                .iter()
                .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
                .collect::<HashMap<_, _>>();
            Ok((w.map(|r| window_json(&r)), tz))
        })
    })
    .await
    .ok()
    .flatten()?;
    let c = Arc::new(Cached {
        at: Instant::now(),
        window,
        tz,
    });
    *CACHE.lock().unwrap() = Some(c.clone());
    Some(c)
}

fn window_json(r: &postgres::Row) -> Value {
    json!({
        "id": r.get::<_, i64>(0),
        "starts": r.get::<_, f64>(1),
        "ends": r.get::<_, f64>(2),
        "note": r.get::<_, Option<String>>(3),
    })
}

/// `GET /api/maintenance?region=` (public): the next window within 48 h or
/// null, and the region's time zone for showing it. During maintenance the
/// proxy answers this path itself (503, `{"maintenance": true, "until"}`).
pub(super) async fn public(State(s): State<AppState>, Query(q): Query<RegionQ>) -> Response {
    let Some(c) = current(&s).await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let tz = q.region.as_deref().and_then(|r| c.tz.get(r));
    (
        [(header::CACHE_CONTROL, "public, max-age=60")],
        Json(json!({ "maintenance": false, "window": c.window, "tz": tz })),
    )
        .into_response()
}

async fn admin(s: &AppState, c: Client, headers: &HeaderMap) -> Option<String> {
    let w = who(s, c, headers).await;
    (matches!(w.access(), Access::All) && w.role() >= Role::Admin)
        .then(|| w.user.clone().unwrap_or(w.label))
}

/// `GET /api/ops/maintenance` (admin for all regions): upcoming windows and
/// the last past or cancelled ones of this shard.
pub(super) async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    if admin(&s, c, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let st = s.clone();
    let rows = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            db.query(
                "(SELECT id, extract(epoch FROM starts)::float8, extract(epoch FROM ends)::float8, note, \
                   created_by, extract(epoch FROM cancelled_at)::float8, cancelled_by \
                 FROM ops.maintenance WHERE ends > now() AND cancelled_at IS NULL) \
                 UNION ALL \
                 (SELECT id, extract(epoch FROM starts)::float8, extract(epoch FROM ends)::float8, note, \
                   created_by, extract(epoch FROM cancelled_at)::float8, cancelled_by \
                 FROM ops.maintenance WHERE ends <= now() OR cancelled_at IS NOT NULL \
                 ORDER BY starts DESC LIMIT 20)",
                &[],
            )
        })
    })
    .await
    .ok()
    .flatten();
    let Some(rows) = rows else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut v = window_json(r);
            v["by"] = r.get::<_, String>(4).into();
            v["cancelled"] = r.get::<_, Option<f64>>(5).into();
            v["cancelled_by"] = r.get::<_, Option<String>>(6).into();
            v
        })
        .collect();
    Json(json!({ "shard": s.host.shard(), "items": items })).into_response()
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(untagged)]
enum Change {
    Cancel {
        cancel: i64,
    },
    New {
        /// Unix seconds.
        starts: f64,
        minutes: i32,
        #[serde(default)]
        note: Option<String>,
    },
}

/// Checks a new window against now (Unix seconds).
fn check(starts: f64, minutes: i32, note: Option<&str>, now: f64) -> Result<(), &'static str> {
    if !MINUTES.contains(&minutes) {
        return Err("minutes: 5 to 720");
    }
    if !starts.is_finite() || starts < now - 60.0 || starts > now + 60.0 * 86400.0 {
        return Err("starts: from now up to 60 days ahead");
    }
    if note.is_some_and(|n| !NOTES.contains(&n)) {
        return Err("note: move, update or database");
    }
    Ok(())
}

/// `POST /api/ops/maintenance` (admin for all regions), JSON in a text body
/// (simple request from another shard's admin page):
/// `{starts, minutes, note?}` adds a window, `{cancel: id}` cancels one.
pub(super) async fn change(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    body: String,
) -> Response {
    let Some(by) = admin(&s, c, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let ch = match serde_json::from_str::<Change>(&body) {
        Ok(ch) => ch,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "body: {starts, minutes, note?} or {cancel: id}",
            )
                .into_response()
        }
    };
    if let Change::New {
        starts,
        minutes,
        note,
    } = &ch
    {
        if let Err(e) = check(
            *starts,
            *minutes,
            note.as_deref(),
            crate::analytics::now_s(),
        ) {
            return (StatusCode::BAD_REQUEST, e).into_response();
        }
    }
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            let id: Option<i64> = match &ch {
                Change::Cancel { cancel } => t
                    .query_opt(
                        "UPDATE ops.maintenance SET cancelled_at = now(), cancelled_by = $2 \
                         WHERE id = $1 AND cancelled_at IS NULL AND ends > now() RETURNING id",
                        &[cancel, &by],
                    )?
                    .map(|r| r.get(0)),
                Change::New {
                    starts,
                    minutes,
                    note,
                } => Some(
                    t.query_one(
                        "INSERT INTO ops.maintenance (starts, ends, note, created_by) \
                         VALUES (to_timestamp($1), to_timestamp($1) + make_interval(mins => $2), $3, $4) \
                         RETURNING id",
                        &[starts, minutes, note, &by],
                    )?
                    .get(0),
                ),
            };
            if let Some(id) = id {
                let (event, payload) = match &ch {
                    Change::Cancel { .. } => ("cancel", json!({})),
                    Change::New {
                        starts,
                        minutes,
                        note,
                    } => (
                        "create",
                        json!({ "starts": starts, "minutes": minutes, "note": note }),
                    ),
                };
                audit_entity(&mut t, &format!("maintenance {id}"), event, &by, payload)?;
            }
            t.commit()?;
            Ok(id)
        })
    })
    .await
    .ok()
    .flatten();
    match out {
        Some(Some(id)) => {
            forget();
            Json(json!({ "id": id })).into_response()
        }
        Some(None) => StatusCode::NOT_FOUND.into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_changes() {
        assert_eq!(
            serde_json::from_str::<Change>(r#"{"cancel": 4}"#).unwrap(),
            Change::Cancel { cancel: 4 }
        );
        assert_eq!(
            serde_json::from_str::<Change>(r#"{"starts": 100, "minutes": 30}"#).unwrap(),
            Change::New {
                starts: 100.0,
                minutes: 30,
                note: None
            }
        );
        assert!(serde_json::from_str::<Change>(r#"{"minutes": 30}"#).is_err());
    }

    #[test]
    fn checks_windows() {
        let now = 1_000_000.0;
        assert!(check(now + 3600.0, 30, None, now).is_ok());
        assert!(check(now + 3600.0, 30, Some("move"), now).is_ok());
        assert!(check(now + 3600.0, 4, None, now).is_err());
        assert!(check(now + 3600.0, 721, None, now).is_err());
        assert!(check(now - 3600.0, 30, None, now).is_err(), "past");
        assert!(check(now + 61.0 * 86400.0, 30, None, now).is_err());
        assert!(check(f64::NAN, 30, None, now).is_err());
        assert!(check(now + 3600.0, 30, Some("party"), now).is_err());
    }
}

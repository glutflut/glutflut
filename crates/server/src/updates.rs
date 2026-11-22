//! OSM updates of running regions through a queue (`ops.update_jobs`,
//! `ops.region_updates`): the admin page sets how often a region is checked
//! and asks for a dry run now; an updater next to the database runs the jobs
//! (`db/import/update-region.sh`) and stores the reports. A dry run with
//! changes waits for approval here; an approved one writes (stage 2). A child
//! module of the gateway.

use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::audit_entity;
use super::{who, Access, AppState, Client, Role};

const RHYTHMS: [&str; 3] = ["off", "monthly", "quarterly"];

async fn admin(s: &AppState, c: Client, headers: &HeaderMap) -> Option<String> {
    let w = who(s, c, headers).await;
    (matches!(w.access(), Access::All) && w.role() >= Role::Admin)
        .then(|| w.user.clone().unwrap_or(w.label))
}

const EPOCH: &str = "extract(epoch FROM {})::float8";

fn epoch(col: &str) -> String {
    EPOCH.replace("{}", col)
}

/// `GET /api/ops/updates` (admin for all regions): this shard's regions with
/// their update settings, and the last 50 jobs (without the reports).
pub(super) async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    if admin(&s, c, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let shard = s.host.shard().to_string();
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let regions = db
                .query(
                    &format!(
                        "SELECT r.slug, r.name, r.active, u.rhythm, coalesce(u.auto, false), {}, {} \
                         FROM geo.regions r LEFT JOIN ops.region_updates u ON u.region = r.slug \
                         WHERE r.shard = $1 ORDER BY r.name",
                        epoch("u.last_check"),
                        epoch("u.last_write")
                    ),
                    &[&shard],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "region": r.get::<_, String>(0),
                        "name": r.get::<_, String>(1),
                        "active": r.get::<_, bool>(2),
                        "rhythm": r.get::<_, Option<String>>(3),
                        "auto": r.get::<_, bool>(4),
                        "last_check": r.get::<_, Option<f64>>(5),
                        "last_write": r.get::<_, Option<f64>>(6),
                    })
                })
                .collect::<Vec<_>>();
            let jobs = db
                .query(
                    &format!(
                        "SELECT id, region, kind, status, created_by, {}, {}, {}, summary::text, error, \
                           approved_by, run_now \
                         FROM ops.update_jobs ORDER BY created_at DESC LIMIT 50",
                        epoch("created_at"),
                        epoch("started_at"),
                        epoch("finished_at")
                    ),
                    &[],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<_, i64>(0),
                        "region": r.get::<_, String>(1),
                        "kind": r.get::<_, String>(2),
                        "status": r.get::<_, String>(3),
                        "by": r.get::<_, String>(4),
                        "created": r.get::<_, f64>(5),
                        "started": r.get::<_, Option<f64>>(6),
                        "finished": r.get::<_, Option<f64>>(7),
                        "summary": r.get::<_, Option<String>>(8)
                            .and_then(|t| serde_json::from_str::<Value>(&t).ok()),
                        "error": r.get::<_, Option<String>>(9),
                        "approved_by": r.get::<_, Option<String>>(10),
                        "run_now": r.get::<_, bool>(11),
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({ "shard": shard, "regions": regions, "jobs": jobs }))
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

/// `GET /api/ops/updates/{id}` (admin for all regions): one job's report
/// (Markdown).
pub(super) async fn report(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Path(id): Path<i64>,
) -> Response {
    if admin(&s, c, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let row = tokio::task::spawn_blocking(move || {
        s.with_db(|db| {
            db.query_opt(
                "SELECT region, report FROM ops.update_jobs WHERE id = $1",
                &[&id],
            )
            .map(|r| r.map(|r| (r.get::<_, String>(0), r.get::<_, Option<String>>(1))))
        })
    })
    .await
    .ok()
    .flatten();
    match row {
        Some(Some((region, report))) => {
            Json(json!({ "id": id, "region": region, "report": report })).into_response()
        }
        Some(None) => StatusCode::NOT_FOUND.into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(untagged)]
enum Change {
    /// Write a waiting job's update: the next night (02-05 local), or at the
    /// next tick with `now`.
    Approve {
        approve: i64,
        #[serde(default)]
        now: bool,
    },
    /// Drop a waiting, pending or approved job.
    Discard { discard: i64 },
    /// A dry run at the updater's next tick.
    Run { region: String, run: bool },
    Settings {
        region: String,
        rhythm: String,
        #[serde(default)]
        auto: bool,
    },
}

fn check(ch: &Change) -> Result<(), &'static str> {
    match ch {
        Change::Run { run: false, .. } => Err("run: true"),
        Change::Settings { rhythm, .. } if !RHYTHMS.contains(&rhythm.as_str()) => {
            Err("rhythm: off, monthly or quarterly")
        }
        Change::Settings { auto: true, .. } => Err("auto: not before stage 3"),
        _ => Ok(()),
    }
}

/// `POST /api/ops/updates` (admin for all regions), JSON in a text body:
/// `{region, rhythm, auto?}` sets how often the region is checked,
/// `{region, run: true}` queues a dry run (409 while one is open),
/// `{approve: id, now?}` lets a waiting job write (next night, or `now`),
/// `{discard: id}` drops an open job (409 if not open).
pub(super) async fn change(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    body: String,
) -> Response {
    let Some(by) = admin(&s, c, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Ok(ch) = serde_json::from_str::<Change>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            "body: {region, rhythm, auto?} or {region, run: true}",
        )
            .into_response();
    };
    if let Err(e) = check(&ch) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
    let shard = s.host.shard().to_string();
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            // A job: its state changes, its region is this shard's.
            let job = match &ch {
                Change::Approve { approve, now } => Some((
                    *approve,
                    "approve",
                    t.query_opt(
                        "UPDATE ops.update_jobs SET status = 'approved', kind = 'write', approved_by = $2, \
                         approved_at = now(), run_now = $3 WHERE id = $1 AND status = 'waiting' RETURNING region",
                        &[approve, &by, now],
                    )?,
                    json!({ "now": now }),
                )),
                Change::Discard { discard } => Some((
                    *discard,
                    "discard",
                    t.query_opt(
                        "UPDATE ops.update_jobs SET status = 'skipped', finished_at = now(), \
                         error = 'discarded by ' || $2 \
                         WHERE id = $1 AND status IN ('pending', 'waiting', 'approved') RETURNING region",
                        &[discard, &by],
                    )?,
                    json!({}),
                )),
                _ => None,
            };
            if let Some((id, event, row, payload)) = job {
                let Some(row) = row else {
                    return Ok(Err(StatusCode::CONFLICT));
                };
                let region: String = row.get(0);
                audit_entity(
                    &mut t,
                    &format!("update {region}"),
                    event,
                    &by,
                    json!({ "job": id, "detail": payload }),
                )?;
                t.commit()?;
                return Ok(Ok(Some(id)));
            }
            let region = match &ch {
                Change::Run { region, .. } | Change::Settings { region, .. } => region.clone(),
                _ => unreachable!("jobs handled above"),
            };
            let known = t
                .query_opt(
                    "SELECT 1 FROM geo.regions WHERE slug = $1 AND shard = $2",
                    &[&region, &shard],
                )?
                .is_some();
            if !known {
                return Ok(Err(StatusCode::NOT_FOUND));
            }
            let (event, payload, id) = match &ch {
                Change::Approve { .. } | Change::Discard { .. } => unreachable!("jobs handled above"),
                Change::Run { .. } => {
                    let id: Option<i64> = t
                        .query_opt(
                            "INSERT INTO ops.update_jobs (region, kind, created_by) VALUES ($1, 'dry', $2) \
                             ON CONFLICT (region) WHERE status IN ('pending', 'running', 'waiting', 'approved') \
                             DO NOTHING RETURNING id",
                            &[&region, &by],
                        )?
                        .map(|r| r.get(0));
                    let Some(id) = id else {
                        return Ok(Err(StatusCode::CONFLICT));
                    };
                    ("run", json!({ "job": id }), Some(id))
                }
                Change::Settings { rhythm, auto, .. } => {
                    t.execute(
                        "INSERT INTO ops.region_updates (region, rhythm, auto, updated_by) \
                         VALUES ($1, $2, $3, $4) ON CONFLICT (region) DO UPDATE SET \
                         rhythm = EXCLUDED.rhythm, auto = EXCLUDED.auto, \
                         updated_by = EXCLUDED.updated_by, updated_at = now()",
                        &[&region, rhythm, auto, &by],
                    )?;
                    ("settings", json!({ "rhythm": rhythm, "auto": auto }), None)
                }
            };
            audit_entity(&mut t, &format!("update {region}"), event, &by, payload)?;
            t.commit()?;
            Ok(Ok(id))
        })
    })
    .await
    .ok()
    .flatten();
    match out {
        Some(Ok(id)) => Json(json!({ "id": id })).into_response(),
        Some(Err(StatusCode::CONFLICT)) => (
            StatusCode::CONFLICT,
            "a job for this region is open, or the job is not in that state",
        )
            .into_response(),
        Some(Err(code)) => code.into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_checks() {
        let p = |s: &str| serde_json::from_str::<Change>(s).unwrap();
        let run = p(r#"{"region": "region-konstanz", "run": true}"#);
        assert!(matches!(run, Change::Run { .. }));
        assert!(check(&run).is_ok());
        assert!(check(&p(r#"{"region": "r", "run": false}"#)).is_err());
        assert!(check(&p(r#"{"region": "r", "rhythm": "monthly"}"#)).is_ok());
        assert!(check(&p(r#"{"region": "r", "rhythm": "weekly"}"#)).is_err());
        assert!(check(&p(r#"{"region": "r", "rhythm": "off", "auto": true}"#)).is_err());
        assert!(serde_json::from_str::<Change>(r#"{"region": "r"}"#).is_err());
        assert_eq!(
            p(r#"{"approve": 7}"#),
            Change::Approve {
                approve: 7,
                now: false
            }
        );
        assert_eq!(p(r#"{"discard": 7}"#), Change::Discard { discard: 7 });
    }
}

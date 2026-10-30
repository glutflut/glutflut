//! City keys (`game.keep_keys`, origin `city`): the city hands out links
//! that open one of its hotspot sites to players elsewhere: standing at
//! their own permanent source in another region, they claim around it, on
//! either side (`handlers/far.rs`). Made in batches with a label (a partner
//! city, a chapter, a letter); only the key's hash is kept, so the links are
//! shown once. A loaded region takes them at once (`FromWeb::FarKey`). A
//! child module of the gateway.

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::{audit_entity, bad};
use super::{who, AppState, Client, FromWeb, RegionQ, Role};

const LABEL_MAX: usize = 60;
/// Links per batch, claims per link, days.
const COUNT: (u32, u32) = (1, 200);
const USES: (u32, u32) = (1, 50);
const DAYS: (u32, u32) = (1, 90);
/// Batches not expired per region.
const OPEN_MAX: i64 = 20;

#[derive(Deserialize, Debug)]
pub struct KeysIn {
    /// The hotspot site (its id).
    pub site: i64,
    pub label: String,
    pub count: u32,
    pub uses: u32,
    pub days: u32,
}

/// Checks that need no database.
pub fn check(k: &KeysIn) -> Result<String, &'static str> {
    let label = k.label.trim();
    let n = label.chars().count();
    if n == 0 || n > LABEL_MAX || label.chars().any(char::is_control) {
        return Err("label: 1-60 characters");
    }
    if !(COUNT.0..=COUNT.1).contains(&k.count) {
        return Err("links: 1-200");
    }
    if !(USES.0..=USES.1).contains(&k.uses) {
        return Err("claims per link: 1-50");
    }
    if !(DAYS.0..=DAYS.1).contains(&k.days) {
        return Err("days: 1-90");
    }
    Ok(label.to_string())
}

/// `GET /api/ops/keys?region=` (editor): the hotspot sites to choose from
/// and the batches made, newest first.
pub(super) async fn list(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
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
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let sites: Vec<Value> = db
                .query(
                    "SELECT id, name FROM geo.hotspot_sites WHERE region = $1 ORDER BY name",
                    &[&slug],
                )?
                .iter()
                .map(|r| json!({ "id": r.get::<_, i64>(0), "name": r.get::<_, String>(1) }))
                .collect();
            let batches: Vec<Value> = db
                .query(
                    "SELECT k.batch, min(k.label), min(s.name), count(*), sum(k.uses)::int8, \
                     extract(epoch FROM min(k.made))::float8, extract(epoch FROM max(k.expires))::float8 \
                     FROM game.keep_keys k LEFT JOIN geo.hotspot_sites s ON s.region = k.region AND s.node_id = k.spot \
                     WHERE k.region = $1 AND k.origin = 'city' AND k.batch IS NOT NULL \
                     GROUP BY k.batch ORDER BY 6 DESC",
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "batch": r.get::<_, String>(0),
                        "label": r.get::<_, Option<String>>(1),
                        "site": r.get::<_, Option<String>>(2),
                        "links": r.get::<_, i64>(3),
                        "uses": r.get::<_, i64>(4),
                        "made": r.get::<_, f64>(5),
                        "expires": r.get::<_, f64>(6),
                    })
                })
                .collect();
            Ok(json!({ "sites": sites, "batches": batches }))
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

/// `POST /api/ops/keys?region=` (editor): a batch of links; the keys come
/// back once (only their hashes are kept).
pub(super) async fn make(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<KeysIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let label = match check(&req) {
        Ok(l) => l,
        Err(e) => return bad(e),
    };
    let st = s.clone();
    let by = w.label;
    let region = slug.clone();
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let Some(spot) = db
                .query_opt(
                    "SELECT node_id FROM geo.hotspot_sites WHERE region = $1 AND id = $2",
                    &[&region, &req.site],
                )?
                .map(|r| r.get::<_, i64>(0))
            else {
                return Ok(Err("no such hotspot site"));
            };
            let mut t = db.transaction()?;
            let open: i64 = t
                .query_one(
                    "SELECT count(DISTINCT batch) FROM game.keep_keys \
                     WHERE region = $1 AND origin = 'city' AND expires > now()",
                    &[&region],
                )?
                .get(0);
            if open >= OPEN_MAX {
                return Ok(Err("at most 20 batches running"));
            }
            let batch = format!("{:016x}", rand::random::<u64>());
            let now = crate::analytics::now_s();
            let expires = now + f64::from(req.days) * 86_400.0;
            let mut out = Vec::new();
            for _ in 0..req.count {
                let (key, kh, _) = crate::keep::new_ticket();
                t.execute(
                    "INSERT INTO game.keep_keys (key, region, keep, origin, uses, made, expires, spot, batch, label) \
                     VALUES ($1, $2, NULL, 'city', $3, to_timestamp($4), to_timestamp($5), $6, $7, $8)",
                    &[&&kh[..], &region, &(req.uses as i32), &now, &expires, &spot, &batch, &label],
                )?;
                out.push((
                    key,
                    kh,
                    crate::keep::Key {
                        origin: "city",
                        uses: req.uses,
                        made: now,
                        expires,
                        spot: Some(spot),
                        batch: Some(batch.clone()),
                        label: Some(label.clone()),
                        ..Default::default()
                    },
                ));
            }
            audit_entity(
                &mut t,
                &format!("keys {region}/{batch}"),
                "add",
                &by,
                json!({ "label": label, "site": req.site, "links": req.count, "uses": req.uses, "days": req.days }),
            )?;
            t.commit()?;
            Ok(Ok((batch, out)))
        })
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(Ok((batch, out))) => {
            // A loaded (or loading) region read its keys before these.
            if s.host.peek(&slug).is_some() {
                for (_, hash, key) in &out {
                    crate::remote::deliver(
                        &s.host,
                        &slug,
                        FromWeb::FarKey(crate::remote::AddKey {
                            hash: *hash,
                            key: key.clone(),
                        }),
                    );
                }
            }
            let keys: Vec<&str> = out.iter().map(|k| k.0.as_str()).collect();
            Json(json!({ "batch": batch, "keys": keys })).into_response()
        }
        Some(Err(e)) => bad(e),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct BatchQ {
    region: Option<String>,
    batch: String,
}

/// `DELETE /api/ops/keys?region=&batch=` (editor): withdraw a batch.
pub(super) async fn withdraw(
    State(s): State<AppState>,
    Query(q): Query<BatchQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let st = s.clone();
    let (by, batch, region) = (w.label, q.batch.clone(), slug.clone());
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            let n = t.execute(
                "DELETE FROM game.keep_keys WHERE region = $1 AND origin = 'city' AND batch = $2",
                &[&region, &batch],
            )?;
            if n > 0 {
                audit_entity(
                    &mut t,
                    &format!("keys {region}/{batch}"),
                    "delete",
                    &by,
                    Value::Null,
                )?;
            }
            t.commit()?;
            Ok(n)
        })
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(0) => bad("no such batch"),
        Some(_) => {
            if let Some(r) = s.host.peek(&slug) {
                let _ = r.tx.send(FromWeb::FarKeysGone(q.batch));
            }
            StatusCode::NO_CONTENT.into_response()
        }
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks() {
        let k = |label: &str, count, uses, days| KeysIn {
            site: 1,
            label: label.into(),
            count,
            uses,
            days,
        };
        assert_eq!(
            check(&k(" Fontainebleau ", 10, 5, 30)),
            Ok("Fontainebleau".into())
        );
        assert!(check(&k("", 10, 5, 30)).is_err());
        assert!(check(&k(&"x".repeat(61), 10, 5, 30)).is_err());
        assert!(check(&k("a", 0, 5, 30)).is_err());
        assert!(check(&k("a", 201, 5, 30)).is_err());
        assert!(check(&k("a", 1, 51, 30)).is_err());
        assert!(check(&k("a", 1, 5, 91)).is_err());
    }
}

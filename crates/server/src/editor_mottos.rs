//! Campaign phrases: the city's own tag lines for
//! permanent sources. The editor proposes them per campaign and side, an
//! admin approves them (we read them once; no free text from players). While
//! the campaign runs, the region offers approved ones (`campaign.rs`).
//! Editing an approved phrase without admin rights sends it back to review.
//! A child module of the gateway.

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::{audit_entity, bad, primary, LANGS};
use super::{who, AppState, Client, RegionQ, Role};

/// Characters per phrase (map labels stay short).
const TEXT_MAX: usize = 40;
/// Phrases per campaign and side.
const PER_SIDE: i64 = 8;

#[derive(Deserialize, Debug)]
pub struct MottoIn {
    /// Absent: a new phrase.
    pub num: Option<i16>,
    pub campaign: String,
    /// `fire`, `water` or `both`.
    pub side: String,
    /// `{lang: text}`
    pub names: Value,
    /// Admins: approve with this save.
    #[serde(default)]
    pub approve: bool,
}

/// Texts: the city's language required, known languages only, short, no control characters.
pub fn clean_names(v: &Value, primary: &str) -> Result<Value, &'static str> {
    let mut out = serde_json::Map::new();
    for lang in LANGS {
        let t = v.get(lang).and_then(Value::as_str).unwrap_or("").trim();
        if t.is_empty() {
            continue;
        }
        if t.chars().count() > TEXT_MAX {
            return Err("at most 40 characters");
        }
        if t.chars().any(char::is_control) {
            return Err("control characters");
        }
        out.insert(lang.into(), t.into());
    }
    if !out.contains_key(primary) {
        return Err("the text in the city's language is required");
    }
    Ok(Value::Object(out))
}

/// The side, or why not.
pub fn clean_side(s: &str) -> Result<&'static str, &'static str> {
    match s {
        "fire" => Ok("fire"),
        "water" => Ok("water"),
        "both" => Ok("both"),
        _ => Err("side: fire, water or both"),
    }
}

/// `GET /api/ops/mottos?region=` (editor): the region's phrases and its campaigns.
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
            let mottos: Vec<Value> = db
                .query(
                    "SELECT num, campaign, side, names::text, status, created_by, approved_by \
                     FROM game.campaign_mottos WHERE region = $1 ORDER BY campaign, num",
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "num": r.get::<_, i16>(0),
                        "campaign": r.get::<_, String>(1),
                        "side": r.get::<_, String>(2),
                        "names": serde_json::from_str::<Value>(r.get(3)).unwrap_or(Value::Null),
                        "status": r.get::<_, String>(4),
                        "by": r.get::<_, Option<String>>(5),
                        "approved_by": r.get::<_, Option<String>>(6),
                    })
                })
                .collect();
            // Campaigns with their span (published chapters; drafts count too,
            // so phrases can be written before publishing).
            let campaigns: Vec<Value> = db
                .query(
                    "SELECT campaign, extract(epoch FROM min(starts))::float8, extract(epoch FROM max(ends))::float8 \
                     FROM game.chapters WHERE region = $1 AND kind IS NULL GROUP BY campaign ORDER BY min(starts) DESC",
                    &[&slug],
                )?
                .iter()
                .map(|r| json!({ "campaign": r.get::<_, String>(0), "starts": r.get::<_, f64>(1), "ends": r.get::<_, f64>(2) }))
                .collect();
            Ok(json!({ "mottos": mottos, "campaigns": campaigns }))
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

/// `POST /api/ops/mottos?region=` (editor): propose or change a phrase; an
/// admin may approve it with the same call. The region takes approved ones
/// within 5 minutes (apps on their next connect).
pub(super) async fn save(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<MottoIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let admin = w.may(&slug, Role::Admin);
    if req.approve && !admin {
        return bad("approving needs an admin token");
    }
    let side = match clean_side(&req.side) {
        Ok(s) => s,
        Err(e) => return bad(e),
    };
    let st = s.clone();
    let by = w.label;
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let names = match clean_names(&req.names, &primary(db, &slug)?) {
                Ok(n) => n,
                Err(e) => return Ok(Err(e)),
            };
            let mut t = db.transaction()?;
            let known: bool = t
                .query_one(
                    "SELECT EXISTS (SELECT 1 FROM game.chapters WHERE region = $1 AND kind IS NULL AND campaign = $2)",
                    &[&slug, &req.campaign],
                )?
                .get(0);
            if !known {
                return Ok(Err("no chapter of this campaign yet"));
            }
            let used: i64 = t
                .query_one(
                    "SELECT count(*) FROM game.campaign_mottos WHERE region = $1 AND campaign = $2 \
                     AND (side = $3 OR side = 'both' OR $3 = 'both') AND num IS DISTINCT FROM $4",
                    &[&slug, &req.campaign, &side, &req.num],
                )?
                .get(0);
            if used >= PER_SIDE {
                return Ok(Err("at most 8 phrases per campaign and side"));
            }
            let status = if req.approve { "approved" } else { "proposed" };
            let approved_by = req.approve.then_some(by.as_str());
            let num: Option<i16> = match req.num {
                Some(n) => t
                    .query_opt(
                        "UPDATE game.campaign_mottos SET campaign = $3, side = $4, names = $5::text::jsonb, \
                         status = $6, approved_by = $7 WHERE region = $1 AND num = $2 RETURNING num",
                        &[&slug, &n, &req.campaign, &side, &names.to_string(), &status, &approved_by],
                    )?
                    .map(|r| r.get(0)),
                None => t
                    .query_opt(
                        "INSERT INTO game.campaign_mottos (region, num, campaign, side, names, status, created_by, approved_by) \
                         SELECT $1, coalesce(max(num), 0) + 1, $2, $3, $4::text::jsonb, $5, $6, $7 \
                         FROM game.campaign_mottos WHERE region = $1 HAVING coalesce(max(num), 0) < 999 RETURNING num",
                        &[&slug, &req.campaign, &side, &names.to_string(), &status, &by, &approved_by],
                    )?
                    .map(|r| r.get(0)),
            };
            let Some(num) = num else {
                return Ok(Err("no such phrase"));
            };
            audit_entity(
                &mut t,
                &format!("motto {slug}/{num}"),
                status,
                &by,
                json!({ "campaign": req.campaign, "side": side, "names": names }),
            )?;
            t.commit()?;
            Ok(Ok(num))
        })
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(Ok(num)) => Json(json!({ "num": num })).into_response(),
        Some(Err(e)) => bad(e),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct NumQ {
    region: Option<String>,
    num: i16,
}

/// `DELETE /api/ops/mottos?region=&num=` (editor). Sources that carry it show
/// no phrase from then on.
pub(super) async fn delete(
    State(s): State<AppState>,
    Query(q): Query<NumQ>,
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
    let (by, num) = (w.label, q.num);
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            let n = t.execute(
                "DELETE FROM game.campaign_mottos WHERE region = $1 AND num = $2",
                &[&slug, &num],
            )?;
            if n > 0 {
                audit_entity(
                    &mut t,
                    &format!("motto {slug}/{num}"),
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
        Some(1) => StatusCode::NO_CONTENT.into_response(),
        Some(_) => bad("no such phrase"),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_and_sides() {
        let ok = clean_names(&json!({ "de": " Helau! ", "en": "", "xx": "?" }), "de").unwrap();
        assert_eq!(
            ok,
            json!({ "de": "Helau!" }),
            "trimmed, empty and unknown dropped"
        );
        assert!(
            clean_names(&json!({ "en": "Hi" }), "de").is_err(),
            "the city's language required"
        );
        assert!(
            clean_names(&json!({ "pl": "Cześć!" }), "pl").is_ok(),
            "a Polish city"
        );
        assert!(clean_names(&json!({ "de": "x".repeat(41) }), "de").is_err());
        assert!(clean_names(&json!({ "de": "a\nb" }), "de").is_err());
        assert_eq!(clean_side("both"), Ok("both"));
        assert!(clean_side("earth").is_err());
    }
}

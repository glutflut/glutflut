//! News flashes (`game.notices`): a live event, a market opening, a
//! warning. The editor writes them with a period; the app shows a running
//! one as a card once per device and keeps it under "what's new"
//! (`web/src/notices.ts`). The region takes changes within 5 minutes
//! (`campaign.rs`). A child module of the gateway.

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::{audit_entity, bad, clean_text, primary, LANGS};
use super::{who, AppState, Client, RegionQ, Role};

const TITLE_MAX: usize = 60;
const TEXT_MAX: usize = 400;
/// At most this many days.
const DAYS_MAX: f64 = 60.0;
/// Notices not over yet per region.
const OPEN_MAX: i64 = 10;

#[derive(Deserialize, Debug)]
pub struct NoticeIn {
    /// Absent: a new notice.
    pub id: Option<i64>,
    /// `{lang: {title, text}}`
    pub names: Value,
    #[serde(default)]
    pub url: String,
    pub starts: f64,
    pub ends: f64,
}

/// Titles and texts: the city's language required, other known languages
/// optional; a title alone is fine.
pub fn clean_names(v: &Value, primary: &str) -> Result<Value, &'static str> {
    let mut out = serde_json::Map::new();
    for lang in LANGS {
        let Some(e) = v.get(lang) else { continue };
        let title = e.get("title").and_then(Value::as_str).unwrap_or("").trim();
        let text = clean_text(e.get("text"), TEXT_MAX)?.unwrap_or_default();
        if title.is_empty() && text.is_empty() {
            continue;
        }
        if title.is_empty() {
            return Err("title missing");
        }
        if title.chars().count() > TITLE_MAX {
            return Err("title: at most 60 characters");
        }
        if title.chars().any(char::is_control) {
            return Err("control characters");
        }
        out.insert(lang.into(), json!({ "title": title, "text": text }));
    }
    if !out.contains_key(primary) {
        return Err("the title in the city's language is required");
    }
    Ok(Value::Object(out))
}

/// The link: http(s), no spaces; empty: none.
pub fn clean_url(u: &str) -> Result<Option<String>, &'static str> {
    let u = u.trim();
    if u.is_empty() {
        return Ok(None);
    }
    let ok = (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= 500
        && !u.chars().any(|c| c.is_whitespace() || c.is_control());
    if !ok {
        return Err("link: an http(s) address");
    }
    Ok(Some(u.to_string()))
}

fn check_period(starts: f64, ends: f64) -> Result<(), &'static str> {
    if !(starts.is_finite() && ends.is_finite()) || ends <= starts {
        return Err("ends before it starts");
    }
    if (ends - starts) / 86_400.0 > DAYS_MAX {
        return Err("at most 60 days");
    }
    Ok(())
}

/// `GET /api/ops/notices?region=` (editor): the region's notices not older
/// than a month, newest first.
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
            Ok(db
                .query(
                    "SELECT id, names::text, url, extract(epoch FROM starts)::float8, \
                     extract(epoch FROM ends)::float8, created_by \
                     FROM game.notices WHERE region = $1 AND ends > now() - interval '30 days' \
                     ORDER BY starts DESC, id DESC",
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<_, i64>(0),
                        "names": serde_json::from_str::<Value>(r.get(1)).unwrap_or(Value::Null),
                        "url": r.get::<_, Option<String>>(2).unwrap_or_default(),
                        "starts": r.get::<_, f64>(3),
                        "ends": r.get::<_, f64>(4),
                        "by": r.get::<_, Option<String>>(5),
                    })
                })
                .collect::<Vec<_>>())
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

/// `POST /api/ops/notices?region=` (editor): write a notice (new, or by id).
pub(super) async fn save(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<NoticeIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Err(e) = check_period(req.starts, req.ends) {
        return bad(e);
    }
    let url = match clean_url(&req.url) {
        Ok(u) => u,
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
            let id: Option<i64> = match req.id {
                Some(id) => t
                    .query_opt(
                        "UPDATE game.notices SET names = $3::text::jsonb, url = $4, \
                         starts = to_timestamp($5), ends = to_timestamp($6) \
                         WHERE region = $1 AND id = $2 RETURNING id",
                        &[&slug, &id, &names.to_string(), &url, &req.starts, &req.ends],
                    )?
                    .map(|r| r.get(0)),
                None => {
                    let open: i64 = t
                        .query_one(
                            "SELECT count(*) FROM game.notices WHERE region = $1 AND ends > now()",
                            &[&slug],
                        )?
                        .get(0);
                    if open >= OPEN_MAX {
                        return Ok(Err("at most 10 notices not over yet"));
                    }
                    Some(
                        t.query_one(
                            "INSERT INTO game.notices (region, names, url, starts, ends, created_by) \
                             VALUES ($1, $2::text::jsonb, $3, to_timestamp($4), to_timestamp($5), $6) RETURNING id",
                            &[&slug, &names.to_string(), &url, &req.starts, &req.ends, &by],
                        )?
                        .get(0),
                    )
                }
            };
            let Some(id) = id else {
                return Ok(Err("no such notice"));
            };
            audit_entity(
                &mut t,
                &format!("notice {slug}/{id}"),
                if req.id.is_some() { "change" } else { "add" },
                &by,
                json!({ "names": names, "url": url, "starts": req.starts, "ends": req.ends }),
            )?;
            t.commit()?;
            Ok(Ok(id))
        })
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(Ok(id)) => Json(json!({ "id": id })).into_response(),
        Some(Err(e)) => bad(e),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct IdQ {
    region: Option<String>,
    id: i64,
}

/// `DELETE /api/ops/notices?region=&id=` (editor).
pub(super) async fn delete(
    State(s): State<AppState>,
    Query(q): Query<IdQ>,
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
    let (by, id) = (w.label, q.id);
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            let n = t.execute(
                "DELETE FROM game.notices WHERE region = $1 AND id = $2",
                &[&slug, &id],
            )?;
            if n > 0 {
                audit_entity(
                    &mut t,
                    &format!("notice {slug}/{id}"),
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
        Some(_) => bad("no such notice"),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_links_and_periods() {
        let ok = clean_names(
            &json!({ "de": { "title": " Spielen am Samstag ", "text": "Treffpunkt Münster,\n14 Uhr." }, "en": { "title": "Play day" }, "xx": { "title": "?" } }),
            "de",
        )
        .unwrap();
        assert_eq!(
            ok,
            json!({ "de": { "title": "Spielen am Samstag", "text": "Treffpunkt Münster,\n14 Uhr." }, "en": { "title": "Play day", "text": "" } })
        );
        assert!(clean_names(&json!({ "en": { "title": "Hi" } }), "de").is_err());
        assert!(clean_names(&json!({ "de": { "text": "no title" } }), "de").is_err());
        assert!(clean_names(&json!({ "de": { "title": "x".repeat(61) } }), "de").is_err());
        assert_eq!(clean_url(" "), Ok(None));
        assert_eq!(
            clean_url("https://glutflut.com/x?y=1"),
            Ok(Some("https://glutflut.com/x?y=1".into()))
        );
        assert!(clean_url("glutflut.com").is_err());
        assert!(clean_url("https://a b").is_err());
        assert!(check_period(10.0, 5.0).is_err());
        assert!(check_period(0.0, 61.0 * 86_400.0).is_err());
        assert!(check_period(0.0, 3600.0).is_ok());
    }
}

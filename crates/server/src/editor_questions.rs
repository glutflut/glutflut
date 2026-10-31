//! The city's questions (`game.questions`): "Wie sicher fühlst du dich hier
//! gerade?" with two to four answers, near a hotspot site or anywhere in the
//! region, for a period. The app asks them in the "Wie ist es hier?" card
//! (`web/src/rate.ts`); answers are counted like ratings
//! (`analytics.perception`, `q<id>.<n>`, persons once a day in `#q<id>`).
//! The editor sees the results only once at least k persons answered. The
//! region takes changes within 5 minutes (`campaign.rs`). A child module of
//! the gateway.

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::{audit_entity, bad, primary, LANGS};
use super::{who, AppState, Client, RegionQ, Role};

const QUESTION_MAX: usize = 120;
const ANSWER_MAX: usize = 40;
/// A pilot season (December to February) fits.
const DAYS_MAX: f64 = 92.0;
/// Questions not over yet per region.
const OPEN_MAX: i64 = 5;

#[derive(Deserialize, Debug)]
pub struct QuestionIn {
    /// Absent: a new question.
    pub id: Option<i64>,
    /// `{lang: question}`
    pub names: Value,
    /// `[{lang: answer}]`
    pub answers: Value,
    /// A hotspot site of the region; absent: anywhere.
    pub site: Option<i64>,
    #[serde(default = "radius")]
    pub radius_m: i32,
    pub starts: f64,
    pub ends: f64,
}

fn radius() -> i32 {
    250
}

/// `{lang: text}`: the city's language required, other known ones optional.
fn clean_langs(v: &Value, primary: &str, max: usize) -> Result<Value, &'static str> {
    let mut out = serde_json::Map::new();
    for lang in LANGS {
        let Some(t) = v.get(lang).and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if t.is_empty() {
            continue;
        }
        if t.chars().count() > max || t.chars().any(char::is_control) {
            return Err("text too long or with control characters");
        }
        out.insert(lang.into(), json!(t));
    }
    if !out.contains_key(primary) {
        return Err("the text in the city's language is required");
    }
    Ok(Value::Object(out))
}

/// The question and two to four answers.
pub fn clean(
    names: &Value,
    answers: &Value,
    primary: &str,
) -> Result<(Value, Value), &'static str> {
    let names = clean_langs(names, primary, QUESTION_MAX)?;
    let list = answers.as_array().ok_or("answers: a list")?;
    if !(2..=4).contains(&list.len()) {
        return Err("two to four answers");
    }
    let answers = list
        .iter()
        .map(|a| clean_langs(a, primary, ANSWER_MAX))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((names, Value::Array(answers)))
}

fn check(q: &QuestionIn) -> Result<(), &'static str> {
    if !(q.starts.is_finite() && q.ends.is_finite()) || q.ends <= q.starts {
        return Err("ends before it starts");
    }
    if (q.ends - q.starts) / 86_400.0 > DAYS_MAX {
        return Err("at most 92 days");
    }
    if !(50..=2000).contains(&q.radius_m) {
        return Err("radius: 50 to 2000 m");
    }
    Ok(())
}

/// `GET /api/ops/questions?region=` (editor): the hotspot sites to choose
/// from and the questions not older than three months, newest first, each
/// with its results once k persons answered.
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
    let k = crate::analytics::k_min() as i64;
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
            let mut questions = Vec::new();
            for r in db.query(
                "SELECT id, names::text, answers::text, site, radius_m, extract(epoch FROM starts)::float8, \
                 extract(epoch FROM ends)::float8, created_by \
                 FROM game.questions WHERE region = $1 AND ends > now() - interval '90 days' \
                 ORDER BY starts DESC, id DESC",
                &[&slug],
            )? {
                let id: i64 = r.get(0);
                let counts = db.query(
                    "SELECT category, sum(count)::int8 FROM analytics.perception \
                     WHERE region = $1 AND NOT synthetic AND (category LIKE $2 OR category = $3) \
                     GROUP BY 1",
                    &[&slug, &format!("q{id}.%"), &format!("#q{id}")],
                )?;
                // Persons summed over days (a person answering on two days counts twice).
                let persons: i64 = counts
                    .iter()
                    .filter(|c| c.get::<_, String>(0).starts_with('#'))
                    .map(|c| c.get::<_, i64>(1))
                    .sum();
                let results: Option<Vec<i64>> = (persons >= k).then(|| {
                    let n = serde_json::from_str::<Value>(r.get(2))
                        .ok()
                        .and_then(|a| a.as_array().map(Vec::len))
                        .unwrap_or(0);
                    (0..n)
                        .map(|i| {
                            let cat = format!("q{id}.{i}");
                            counts
                                .iter()
                                .find(|c| c.get::<_, String>(0) == cat)
                                .map_or(0, |c| c.get(1))
                        })
                        .collect()
                });
                questions.push(json!({
                    "id": id,
                    "names": serde_json::from_str::<Value>(r.get(1)).unwrap_or(Value::Null),
                    "answers": serde_json::from_str::<Value>(r.get(2)).unwrap_or(Value::Null),
                    "site": r.get::<_, Option<i64>>(3),
                    "radius_m": r.get::<_, i32>(4),
                    "starts": r.get::<_, f64>(5),
                    "ends": r.get::<_, f64>(6),
                    "by": r.get::<_, Option<String>>(7),
                    "persons": if persons >= k { json!(persons) } else { Value::Null },
                    "results": results,
                }));
            }
            Ok(json!({ "sites": sites, "questions": questions, "k": k }))
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

/// `POST /api/ops/questions?region=` (editor): write a question (new, or by id).
pub(super) async fn save(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<QuestionIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Err(e) = check(&req) {
        return bad(e);
    }
    let st = s.clone();
    let by = w.label;
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let (names, answers) = match clean(&req.names, &req.answers, &primary(db, &slug)?) {
                Ok(v) => v,
                Err(e) => return Ok(Err(e)),
            };
            let mut t = db.transaction()?;
            if let Some(site) = req.site {
                let ok = t
                    .query_opt(
                        "SELECT 1 FROM geo.hotspot_sites WHERE region = $1 AND id = $2",
                        &[&slug, &site],
                    )?
                    .is_some();
                if !ok {
                    return Ok(Err("no such place"));
                }
            }
            let (n, a) = (names.to_string(), answers.to_string());
            let id: Option<i64> = match req.id {
                // Answers can't change once given (the counts belong to them).
                Some(id) => t
                    .query_opt(
                        "UPDATE game.questions SET names = $3::text::jsonb, \
                         answers = CASE WHEN starts > now() THEN $4::text::jsonb ELSE answers END, \
                         site = $5, radius_m = $6, starts = to_timestamp($7), ends = to_timestamp($8) \
                         WHERE region = $1 AND id = $2 RETURNING id",
                        &[&slug, &id, &n, &a, &req.site, &req.radius_m, &req.starts, &req.ends],
                    )?
                    .map(|r| r.get(0)),
                None => {
                    let open: i64 = t
                        .query_one(
                            "SELECT count(*) FROM game.questions WHERE region = $1 AND ends > now()",
                            &[&slug],
                        )?
                        .get(0);
                    if open >= OPEN_MAX {
                        return Ok(Err("at most 5 questions not over yet"));
                    }
                    Some(
                        t.query_one(
                            "INSERT INTO game.questions (region, names, answers, site, radius_m, starts, ends, created_by) \
                             VALUES ($1, $2::text::jsonb, $3::text::jsonb, $4, $5, to_timestamp($6), to_timestamp($7), $8) \
                             RETURNING id",
                            &[&slug, &n, &a, &req.site, &req.radius_m, &req.starts, &req.ends, &by],
                        )?
                        .get(0),
                    )
                }
            };
            let Some(id) = id else {
                return Ok(Err("no such question"));
            };
            audit_entity(
                &mut t,
                &format!("question {slug}/{id}"),
                if req.id.is_some() { "change" } else { "add" },
                &by,
                json!({ "names": names, "answers": answers, "site": req.site, "radius_m": req.radius_m,
                        "starts": req.starts, "ends": req.ends }),
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

/// `DELETE /api/ops/questions?region=&id=` (editor). Counted answers stay
/// in the analytics (without the question they can't be read any more).
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
                "DELETE FROM game.questions WHERE region = $1 AND id = $2",
                &[&slug, &id],
            )?;
            if n > 0 {
                audit_entity(
                    &mut t,
                    &format!("question {slug}/{id}"),
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
        Some(_) => bad("no such question"),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_and_answers() {
        let (n, a) = clean(
            &json!({ "de": " Wie sicher fühlst du dich hier? ", "en": "How safe do you feel here?", "xx": "?" }),
            &json!([{ "de": "sicher" }, { "de": "geht so", "en": "so-so" }, { "de": "unsicher" }]),
            "de",
        )
        .unwrap();
        assert_eq!(
            n,
            json!({ "de": "Wie sicher fühlst du dich hier?", "en": "How safe do you feel here?" })
        );
        assert_eq!(a.as_array().unwrap().len(), 3);
        assert!(clean(&json!({ "de": "?" }), &json!([{ "de": "ja" }]), "de").is_err());
        assert!(clean(
            &json!({ "en": "?" }),
            &json!([{ "de": "ja" }, { "de": "nein" }]),
            "de"
        )
        .is_err());
        assert!(clean(
            &json!({ "de": "?" }),
            &json!([{ "de": "ja" }, { "en": "no" }]),
            "de"
        )
        .is_err());
        assert!(clean(
            &json!({ "de": "?" }),
            &json!([{ "de": "x".repeat(41) }, { "de": "nein" }]),
            "de"
        )
        .is_err());
    }
}

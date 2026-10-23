//! Authoring tools for cities, stage 1: chapters.
//! A token with role `editor` (or `admin`) lists, writes, publishes and
//! deletes the chapters of its regions; each change goes to `ops.audit_log`
//! with the token's label. Drafts don't play (`campaign.rs` reads published
//! ones only). A child module of the gateway (its state and token checks).

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{who, AppState, Client, RegionQ, Role};

/// Languages a chapter may have; the city's own (`geo.regions.language`) is required.
pub(super) const LANGS: [&str; 7] = ["de", "en", "fr", "es", "pl", "it", "pt"];
// "hold" only keeps an existing one: its junctions are set by SQL.
const GOALS: [&str; 6] = ["claims", "fronts", "finds", "points", "area", "hold"];
const TITLE_MAX: usize = 60;
const TEXT_MAX: usize = 240;
/// A story text (intro, an ending): paragraphs allowed.
pub(super) const STORY_MAX: usize = 1500;
const DAYS_MAX: f64 = 31.0;
/// Parts of a chapter's story.
const STORY_PARTS: [&str; 4] = ["intro", "fire", "water", "tie"];

/// A chapter as the editor sends it (times as unix seconds).
#[derive(Deserialize, Clone, Debug)]
pub struct ChapterIn {
    /// Absent: a new chapter (the key is made from the campaign).
    pub key: Option<String>,
    pub campaign: String,
    /// `{lang: {title, text}}`
    pub names: Value,
    pub starts: f64,
    pub ends: f64,
    pub goal: String,
    pub target: i32,
    #[serde(default)]
    pub publish: bool,
    /// `{lang: {intro?, fire?, water?, tie?}}`
    #[serde(default)]
    pub story: Value,
}

/// A stored chapter, as far as the checks need it.
#[derive(Clone, Debug)]
pub struct Stored {
    pub key: String,
    pub campaign: String,
    pub starts: f64,
    pub ends: f64,
    pub goal: String,
    pub target: i32,
    pub published: bool,
}

/// What may be written, after the checks.
#[derive(Debug, PartialEq)]
pub struct Clean {
    pub key: String,
    pub campaign: String,
    pub names: Value,
    pub starts: f64,
    pub ends: f64,
    pub goal: String,
    pub target: i32,
    pub published: bool,
    pub story: Value,
}

fn slug_ok(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// The city's language (`geo.regions.language`), German if unknown: texts
/// in it are required.
pub(super) fn primary(db: &mut postgres::Client, slug: &str) -> Result<String, postgres::Error> {
    let l: Option<String> = db
        .query_opt("SELECT language FROM geo.regions WHERE slug = $1", &[&slug])?
        .and_then(|r| r.get(0));
    Ok(l.filter(|l| LANGS.contains(&l.as_str()))
        .unwrap_or_else(|| "de".into()))
}

/// Titles and texts: the city's language required, other known languages optional, nothing else.
fn clean_names(v: &Value, primary: &str) -> Result<Value, &'static str> {
    let mut out = serde_json::Map::new();
    for lang in LANGS {
        let Some(e) = v.get(lang) else { continue };
        let title = e.get("title").and_then(Value::as_str).unwrap_or("").trim();
        let text = e.get("text").and_then(Value::as_str).unwrap_or("").trim();
        if title.is_empty() && text.is_empty() {
            continue;
        }
        if title.is_empty() || text.is_empty() {
            return Err("title and text, both or neither");
        }
        if title.chars().count() > TITLE_MAX || text.chars().count() > TEXT_MAX {
            return Err("too long");
        }
        if title.chars().chain(text.chars()).any(char::is_control) {
            return Err("control characters");
        }
        out.insert(lang.into(), json!({ "title": title, "text": text }));
    }
    if !out.contains_key(primary) {
        return Err("title and text in the city's language are required");
    }
    Ok(Value::Object(out))
}

/// A longer text (story parts, letters): trimmed, paragraphs kept, no other
/// control characters, at most `max` characters; `None` when empty.
pub(super) fn clean_text(v: Option<&Value>, max: usize) -> Result<Option<String>, &'static str> {
    let t = v.and_then(Value::as_str).unwrap_or("").trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().count() > max {
        return Err("text too long");
    }
    if t.chars().any(|c| c.is_control() && c != '\n' && c != '\r') {
        return Err("control characters");
    }
    Ok(Some(t.replace("\r\n", "\n").replace('\r', "\n")))
}

/// The story: per known language the parts given (any may be missing);
/// `Null` when nothing is.
fn clean_story(v: &Value) -> Result<Value, &'static str> {
    let mut out = serde_json::Map::new();
    for lang in LANGS {
        let Some(e) = v.get(lang) else { continue };
        let mut parts = serde_json::Map::new();
        for p in STORY_PARTS {
            if let Some(t) = clean_text(e.get(p), STORY_MAX)? {
                parts.insert(p.into(), t.into());
            }
        }
        if !parts.is_empty() {
            out.insert(lang.into(), Value::Object(parts));
        }
    }
    Ok(if out.is_empty() {
        Value::Null
    } else {
        Value::Object(out)
    })
}

/// Check a chapter against the region's others at `now`.
pub fn check(
    c: &ChapterIn,
    now: f64,
    others: &[Stored],
    primary: &str,
) -> Result<Clean, &'static str> {
    if !slug_ok(&c.campaign, 3, 40) {
        return Err("campaign: 3-40 of a-z, 0-9, -");
    }
    let names = clean_names(&c.names, primary)?;
    let story = clean_story(&c.story)?;
    if !GOALS.contains(&c.goal.as_str()) {
        return Err("unknown goal");
    }
    if !(1..=1_000_000).contains(&c.target) {
        return Err("target: 1 to 1,000,000");
    }
    if c.ends <= c.starts {
        return Err("ends before it starts");
    }
    if (c.ends - c.starts) / 86_400.0 > DAYS_MAX {
        return Err("at most 31 days");
    }
    let old = match &c.key {
        Some(k) => Some(
            others
                .iter()
                .find(|o| &o.key == k)
                .ok_or("unknown chapter")?,
        ),
        None => None,
    };
    let started = old.is_some_and(|o| o.published && o.starts <= now);
    if started {
        // Running or over: only the texts.
        let o = old.unwrap();
        if o.campaign != c.campaign
            || o.starts != c.starts
            || o.ends != c.ends
            || o.goal != c.goal
            || o.target != c.target
        {
            return Err("started: only titles and texts can change");
        }
    } else if c.starts < now - 300.0 {
        return Err("starts in the past");
    }
    let key = match &c.key {
        Some(k) => k.clone(),
        None => {
            let n = others.iter().filter(|o| o.campaign == c.campaign).count() + 1;
            let mut k = format!("{}-{n}", c.campaign);
            while others.iter().any(|o| o.key == k) {
                k.push('x');
            }
            k
        }
    };
    if !slug_ok(&key, 3, 60) {
        return Err("bad key");
    }
    let published = c.publish || old.is_some_and(|o| o.published);
    // Published chapters don't overlap (drafts may).
    if published
        && others
            .iter()
            .any(|o| o.published && o.key != key && o.starts < c.ends && c.starts < o.ends)
    {
        return Err("overlaps another chapter");
    }
    Ok(Clean {
        key,
        campaign: c.campaign.clone(),
        names,
        starts: c.starts,
        ends: c.ends,
        goal: c.goal.clone(),
        target: c.target,
        published,
        story,
    })
}

pub(super) fn bad(msg: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": msg })),
    )
        .into_response()
}

/// Campaign chapters of a region (not weather days).
fn stored(c: &mut postgres::Client, region: &str) -> Result<Vec<Stored>, postgres::Error> {
    Ok(c.query(
        "SELECT key, campaign, extract(epoch FROM starts)::float8, extract(epoch FROM ends)::float8, \
         goal, target, status = 'published' FROM game.chapters WHERE region = $1 AND kind IS NULL",
        &[&region],
    )?
    .iter()
    .map(|r| Stored {
        key: r.get(0),
        campaign: r.get(1),
        starts: r.get(2),
        ends: r.get(3),
        goal: r.get(4),
        target: r.get(5),
        published: r.get(6),
    })
    .collect())
}

fn audit(
    t: &mut postgres::Transaction,
    region: &str,
    key: &str,
    event: &str,
    by: &str,
    payload: Value,
) -> Result<(), postgres::Error> {
    audit_entity(t, &format!("chapter {region}/{key}"), event, by, payload)
}

/// One change in `ops.audit_log` (token label, no IP).
pub(super) fn audit_entity(
    t: &mut postgres::Transaction,
    entity: &str,
    event: &str,
    by: &str,
    payload: Value,
) -> Result<(), postgres::Error> {
    t.execute(
        "INSERT INTO ops.audit_log (entity, event, by_label, payload) \
         VALUES ($1, $2, $3, $4::text::jsonb)",
        &[&entity, &event, &by, &payload.to_string()],
    )?;
    Ok(())
}

/// `GET /api/ops/chapters?region=` (editor): the region's campaign chapters
/// with progress, newest first.
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
    let rows = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            db.query(
                "SELECT ch.key, ch.campaign, ch.names::text, extract(epoch FROM ch.starts)::float8, \
                 extract(epoch FROM ch.ends)::float8, ch.goal, ch.target, ch.status, ch.created_by, \
                 coalesce((SELECT value FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'fire'), 0), \
                 coalesce((SELECT value FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'water'), 0), \
                 ch.story::text \
                 FROM game.chapters ch WHERE ch.region = $1 AND ch.kind IS NULL ORDER BY ch.starts DESC",
                &[&slug],
            )
        })
    })
    .await
    .ok()
    .flatten();
    let Some(rows) = rows else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "key": r.get::<_, String>(0),
                "campaign": r.get::<_, String>(1),
                "names": serde_json::from_str::<Value>(r.get(2)).unwrap_or(Value::Null),
                "starts": r.get::<_, f64>(3),
                "ends": r.get::<_, f64>(4),
                "goal": r.get::<_, String>(5),
                "target": r.get::<_, i32>(6),
                "status": r.get::<_, String>(7),
                "by": r.get::<_, Option<String>>(8),
                "progress": [r.get::<_, i64>(9), r.get::<_, i64>(10)],
                "story": r.get::<_, Option<String>>(11).and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null),
            })
        })
        .collect();
    Json(out).into_response()
}

/// `POST /api/ops/chapters?region=` (editor): write a chapter (draft, or
/// published with `publish`). Publishing removes future weather days it
/// overlaps; the server picks it up within 5 minutes.
pub(super) async fn save(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<ChapterIn>,
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
    let by = w.label;
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let now = crate::analytics::now_s();
            let others = stored(db, &slug)?;
            let lang = primary(db, &slug)?;
            let clean = match check(&req, now, &others, &lang) {
                Ok(v) => v,
                Err(e) => return Ok(Err(e)),
            };
            let mut t = db.transaction()?;
            let story: Option<String> = (!clean.story.is_null()).then(|| clean.story.to_string());
            t.execute(
                "INSERT INTO game.chapters (region, key, campaign, names, starts, ends, goal, target, status, created_by, story) \
                 VALUES ($1, $2, $3, $4::text::jsonb, to_timestamp($5), to_timestamp($6), $7, $8, $9, $10, $11::text::jsonb) \
                 ON CONFLICT (region, key) DO UPDATE SET campaign = EXCLUDED.campaign, names = EXCLUDED.names, \
                 starts = EXCLUDED.starts, ends = EXCLUDED.ends, goal = EXCLUDED.goal, target = EXCLUDED.target, \
                 status = EXCLUDED.status, story = EXCLUDED.story",
                &[
                    &slug,
                    &clean.key,
                    &clean.campaign,
                    &clean.names.to_string(),
                    &clean.starts,
                    &clean.ends,
                    &clean.goal,
                    &clean.target,
                    &if clean.published { "published" } else { "draft" },
                    &by,
                    &story,
                ],
            )?;
            if clean.published {
                t.execute(
                    "DELETE FROM game.chapters WHERE region = $1 AND kind IS NOT NULL AND starts > now() \
                     AND starts < to_timestamp($3) AND ends > to_timestamp($2)",
                    &[&slug, &clean.starts, &clean.ends],
                )?;
            }
            let event = if clean.published { "publish" } else { "draft" };
            audit(
                &mut t,
                &slug,
                &clean.key,
                event,
                &by,
                json!({ "goal": clean.goal, "target": clean.target, "starts": clean.starts, "ends": clean.ends }),
            )?;
            t.commit()?;
            Ok(Ok(clean.key))
        })
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(Ok(key)) => Json(json!({ "key": key })).into_response(),
        Some(Err(e)) => bad(e),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct KeyQ {
    region: Option<String>,
    key: String,
}

/// `DELETE /api/ops/chapters?region=&key=` (editor): a draft, or a published
/// chapter that hasn't started.
pub(super) async fn delete(
    State(s): State<AppState>,
    Query(q): Query<KeyQ>,
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
    let (by, key) = (w.label, q.key);
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            let n = t.execute(
                "DELETE FROM game.chapters WHERE region = $1 AND key = $2 AND kind IS NULL \
                 AND (status = 'draft' OR starts > now())",
                &[&slug, &key],
            )?;
            if n > 0 {
                audit(&mut t, &slug, &key, "delete", &by, Value::Null)?;
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
        Some(_) => bad("no such draft or future chapter"),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct SuggestQ {
    region: Option<String>,
    goal: String,
    starts: f64,
    ends: f64,
}

/// `GET /api/ops/chapters/suggest?region=&goal=&starts=&ends=` (editor): a
/// target per side from the last two weeks (claims, points), about what one
/// side reaches in that time; `null` where there is no basis.
pub(super) async fn suggest(
    State(s): State<AppState>,
    Query(q): Query<SuggestQ>,
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
    let col = match q.goal.as_str() {
        "claims" => "claims",
        "points" => "points",
        _ => return Json(json!({ "target": null })).into_response(),
    };
    let hours = ((q.ends - q.starts) / 3600.0).clamp(1.0, DAYS_MAX * 24.0);
    let st = s.clone();
    let per_day = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            db.query_one(
                &format!(
                    "SELECT coalesce(sum({col}), 0)::float8 / 14 / 2 FROM game.faction_day \
                     WHERE region = $1 AND day >= current_date - 14 AND day < current_date"
                ),
                &[&slug],
            )
            .map(|r| r.get::<_, f64>(0))
        })
    })
    .await
    .ok()
    .flatten();
    // A side's daily mean over the chapter's length, a fifth more than it
    // would reach anyway.
    let target = per_day
        .filter(|&d| d > 0.0)
        .map(|d| ((d * hours / 24.0 * 1.2).ceil() as i64).max(1));
    Json(json!({ "target": target })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(key: Option<&str>, starts: f64, ends: f64) -> ChapterIn {
        ChapterIn {
            key: key.map(Into::into),
            campaign: "advent-2026".into(),
            names: json!({ "de": { "title": "Glühwein", "text": "Wer zuerst?" }, "xx": { "title": "a", "text": "b" } }),
            starts,
            ends,
            goal: "claims".into(),
            target: 300,
            publish: true,
            story: json!({ "de": { "intro": " Es beginnt.\r\n\r\nZweiter Absatz. ", "fire": "" }, "xx": { "intro": "no" } }),
        }
    }

    fn stored(key: &str, starts: f64, ends: f64, published: bool) -> Stored {
        Stored {
            key: key.into(),
            campaign: "advent-2026".into(),
            starts,
            ends,
            goal: "claims".into(),
            target: 300,
            published,
        }
    }

    #[test]
    fn new_chapter_gets_a_key_and_clean_names() {
        let now = 1_000_000.0;
        let c = check(&input(None, now + 3600.0, now + 7200.0), now, &[], "de").unwrap();
        assert_eq!(c.key, "advent-2026-1");
        assert!(c.names.get("xx").is_none(), "unknown languages dropped");
        assert!(c.published);
        assert_eq!(
            c.story,
            json!({ "de": { "intro": "Es beginnt.\n\nZweiter Absatz." } }),
            "story: trimmed, empty parts and unknown languages dropped"
        );
        let mut long = input(None, now + 3600.0, now + 7200.0);
        long.story = json!({ "de": { "tie": "x".repeat(STORY_MAX + 1) } });
        assert_eq!(check(&long, now, &[], "de").unwrap_err(), "text too long");
        long.story = json!({ "de": { "tie": "a\u{7}b" } });
        assert_eq!(
            check(&long, now, &[], "de").unwrap_err(),
            "control characters"
        );
        long.story = Value::Null;
        assert!(check(&long, now, &[], "de").unwrap().story.is_null());
    }

    #[test]
    fn checks_refuse_what_cannot_play() {
        let now = 1_000_000.0;
        let ok = input(None, now + 3600.0, now + 7200.0);
        let mut c = ok.clone();
        c.names = json!({ "en": { "title": "x", "text": "y" } });
        assert_eq!(
            check(&c, now, &[], "de"),
            Err("title and text in the city's language are required")
        );
        assert!(
            check(&c, now, &[], "en").is_ok(),
            "an English-speaking city"
        );
        let mut c = ok.clone();
        c.goal = "dance".into();
        assert_eq!(check(&c, now, &[], "de"), Err("unknown goal"));
        assert_eq!(
            check(&input(None, now - 7200.0, now + 3600.0), now, &[], "de"),
            Err("starts in the past")
        );
        assert_eq!(
            check(
                &input(None, now + 3600.0, now + 40.0 * 86_400.0),
                now,
                &[],
                "de"
            ),
            Err("at most 31 days")
        );
        // Overlaps a published one; as a draft it may.
        let others = [stored("other", now + 5000.0, now + 9000.0, true)];
        assert_eq!(
            check(&ok, now, &others, "de"),
            Err("overlaps another chapter")
        );
        let mut draft = ok.clone();
        draft.publish = false;
        assert!(check(&draft, now, &others, "de").is_ok());
    }

    #[test]
    fn a_running_chapter_only_changes_its_texts() {
        let now = 1_000_000.0;
        let others = [stored("advent-2026-1", now - 3600.0, now + 3600.0, true)];
        let mut c = input(Some("advent-2026-1"), now - 3600.0, now + 3600.0);
        assert!(
            check(&c, now, &others, "de").is_ok(),
            "same values, new texts"
        );
        c.target = 500;
        assert_eq!(
            check(&c, now, &others, "de"),
            Err("started: only titles and texts can change")
        );
    }
}

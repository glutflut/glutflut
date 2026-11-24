//! Posts from the game (social posts): planned in the editor (`game.posts`),
//! a trigger (a time, a chapter's start or end) makes a draft
//! (`game.post_runs`) with the texts for the outcome and the picture of the
//! game state (`card.rs`); someone posts it and marks it. A child module of
//! the gateway.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::{Extension, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::{audit_entity, bad, clean_text, primary};
use super::{who, AppState, Client, RegionQ, Role};
use crate::card::{self, Sides, Spec};

pub const CREDIT: &str = "© OpenStreetMap";

/// The sides at a chapter's end (`game.chapter_snapshots`), if it has one.
pub fn snapshot_sides(
    db: &mut postgres::Client,
    slug: &str,
    key: &str,
) -> Result<Option<Sides>, postgres::Error> {
    Ok(db
        .query_opt(
            "SELECT fire, water, front FROM game.chapter_snapshots WHERE region = $1 AND key = $2",
            &[&slug, &key],
        )?
        .map(|r| Sides {
            fire: r.get(0),
            water: r.get(1),
            front: r.get(2),
        }))
}

/// The sides now: asks the running region and waits for it (a few seconds).
// Err is the ready answer, as axum handlers return it.
#[allow(clippy::result_large_err)]
async fn live_sides(s: &AppState, slug: &str) -> Result<Sides, Response> {
    let r = s.region(Some(slug))?;
    let t0 = crate::analytics::now_s();
    for _ in 0..50 {
        let json = r.nodes.json.read().unwrap().clone();
        if let Some(v) = json
            .as_deref()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
            .filter(|v| v["t"].as_f64().is_some_and(|t| t >= t0 - 1.0))
        {
            let ids = |k: &str| serde_json::from_value(v[k].clone()).unwrap_or_default();
            return Ok(Sides {
                fire: ids("fire"),
                water: ids("water"),
                front: ids("front"),
            });
        }
        r.nodes.wanted.store(true, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "2")],
        "building",
    )
        .into_response())
}

#[derive(Deserialize)]
pub(super) struct CardQ {
    region: Option<String>,
    /// The state at this chapter's end, else now.
    chapter: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    width_m: Option<f64>,
    title: Option<String>,
    note: Option<String>,
    credit: Option<String>,
}

/// `GET /api/ops/card.png?region=&chapter=&lat=&lon=&width_m=&title=&note=`
/// (editor): the picture of the game state, centred on the region if no
/// place is given.
pub(super) async fn card_png(
    State(s): State<AppState>,
    Query(q): Query<CardQ>,
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
    let width_m = q.width_m.unwrap_or(900.0);
    if !(200.0..=5000.0).contains(&width_m) {
        return (StatusCode::BAD_REQUEST, "width_m: 200 to 5000").into_response();
    }
    let clip = |t: Option<String>, n: usize| t.unwrap_or_default().chars().take(n).collect();
    let mut spec = Spec {
        lat: q.lat.unwrap_or(f64::NAN),
        lon: q.lon.unwrap_or(f64::NAN),
        width_m,
        title: clip(q.title, 40),
        note: clip(q.note, 120),
        credit: q
            .credit
            .map_or_else(|| CREDIT.into(), |c| clip(Some(c), 120)),
    };
    let live = match q.chapter {
        None => Some(match live_sides(&s, &slug).await {
            Ok(v) => v,
            Err(e) => return e,
        }),
        Some(_) => None,
    };
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            if !(spec.lat.is_finite() && spec.lon.is_finite()) {
                let r = db.query_one(
                    "SELECT ST_Y(centroid_geom), ST_X(centroid_geom) FROM geo.regions WHERE slug = $1",
                    &[&slug],
                )?;
                (spec.lat, spec.lon) = (r.get(0), r.get(1));
            }
            let sides = match (live, &q.chapter) {
                (Some(v), _) => Some(v),
                (None, Some(k)) => snapshot_sides(db, &slug, k)?,
                (None, None) => None,
            };
            let geo = card::load(db, &slug, &spec)?;
            Ok(sides.map(|sd| card::svg(&spec, &geo, &sd)))
        })
    })
    .await
    .ok()
    .flatten();
    let svg = match out {
        Some(Some(v)) => v,
        Some(None) => {
            return (StatusCode::NOT_FOUND, "no snapshot for this chapter").into_response()
        }
        None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match tokio::task::spawn_blocking(move || card::png(&svg)).await {
        Ok(Ok(png)) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            png,
        )
            .into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// --- Planned posts and their drafts (game.posts, game.post_runs) -------------

const VARIANTS: [&str; 4] = ["any", "fire", "water", "tie"];
pub const CHANNELS: [&str; 2] = ["instagram", "mastodon"];
const TRIGGERS: [&str; 4] = ["at", "chapter_start", "chapter_end", "manual"];
/// Per channel the most a post may have (Instagram 2200, Mastodon 500).
const MAX: [(&str, usize); 3] = [("instagram", 2200), ("mastodon", 500), ("alt", 1000)];
const POSTS_MAX: i64 = 100;

#[derive(Deserialize, Debug)]
pub struct PostIn {
    /// Absent: a new post.
    pub id: Option<i64>,
    pub title: String,
    pub trigger: String,
    /// Unix seconds (`at`).
    pub at: Option<f64>,
    pub chapter: Option<String>,
    #[serde(default)]
    pub offset_min: i32,
    /// `{any|fire|water|tie: {instagram, mastodon, alt}}`
    pub texts: Value,
    /// `{lat?, lon?, width_m, title, note}`; null: no picture.
    pub card: Option<Value>,
    pub channels: Vec<String>,
}

/// The texts: known variants and channels only, within the lengths; at least one.
pub fn clean_texts(v: &Value) -> Result<Value, &'static str> {
    let mut out = serde_json::Map::new();
    for var in VARIANTS {
        let Some(e) = v.get(var) else { continue };
        let mut m = serde_json::Map::new();
        for (k, max) in MAX {
            if let Some(t) = clean_text(e.get(k), max).map_err(|_| match k {
                "instagram" => "Instagram: at most 2200 characters",
                "mastodon" => "Mastodon: at most 500 characters",
                _ => "alt text: at most 1000 characters",
            })? {
                m.insert(k.into(), t.into());
            }
        }
        if m.contains_key("instagram") || m.contains_key("mastodon") {
            out.insert(var.into(), Value::Object(m));
        }
    }
    if out.is_empty() {
        return Err("a text is needed");
    }
    Ok(Value::Object(out))
}

/// The picture's frame and texts; place optional (the region's centre).
pub fn clean_card(v: &Value) -> Result<Value, &'static str> {
    let num = |k: &str| v.get(k).and_then(Value::as_f64);
    let width_m = num("width_m").unwrap_or(900.0);
    if !(200.0..=5000.0).contains(&width_m) {
        return Err("picture: width 200 to 5000 m");
    }
    let mut out = json!({ "width_m": width_m });
    match (num("lat"), num("lon")) {
        (Some(lat), Some(lon))
            if (-85.0..=85.0).contains(&lat) && (-180.0..=180.0).contains(&lon) =>
        {
            out["lat"] = lat.into();
            out["lon"] = lon.into();
        }
        (None, None) => {}
        _ => return Err("picture: place as lat, lon"),
    }
    for (k, max) in [("title", 40), ("note", 120)] {
        let t = clean_text(v.get(k), max).map_err(|_| "picture: title 40, note 120 characters")?;
        out[k] = t.unwrap_or_default().into();
    }
    Ok(out)
}

fn clean_post(p: &PostIn) -> Result<(String, Value, Option<Value>, Vec<String>), &'static str> {
    let title = p.title.trim();
    if title.is_empty() || title.chars().count() > 80 || title.chars().any(char::is_control) {
        return Err("name: 1 to 80 characters");
    }
    if !TRIGGERS.contains(&p.trigger.as_str()) {
        return Err("unknown trigger");
    }
    if p.trigger == "at" && !p.at.is_some_and(f64::is_finite) {
        return Err("a time is needed");
    }
    if p.trigger.starts_with("chapter_") && p.chapter.as_deref().is_none_or(|c| c.trim().is_empty())
    {
        return Err("a chapter is needed");
    }
    if !(-7 * 1440..=7 * 1440).contains(&p.offset_min) {
        return Err("offset: at most 7 days");
    }
    let mut channels: Vec<String> = Vec::new();
    for c in &p.channels {
        if !CHANNELS.contains(&c.as_str()) {
            return Err("unknown channel");
        }
        if !channels.contains(c) {
            channels.push(c.clone());
        }
    }
    if channels.is_empty() {
        return Err("a channel is needed");
    }
    let card = match &p.card {
        Some(v) if !v.is_null() => Some(clean_card(v)?),
        _ => None,
    };
    Ok((title.to_string(), clean_texts(&p.texts)?, card, channels))
}

/// Thousands as the city writes them.
fn number(v: i64, lang: &str) -> String {
    let sep = match lang {
        "en" => ',',
        "fr" | "pl" => '\u{202f}',
        _ => '.',
    };
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(ch);
    }
    if v < 0 {
        out.insert(0, '-');
    }
    out
}

/// The texts for an outcome with the numbers filled in: the outcome's own
/// variant, else `any`.
pub fn fill(texts: &Value, outcome: Option<&str>, vars: &[(&str, String)]) -> Option<Value> {
    let v = outcome
        .and_then(|o| texts.get(o))
        .or_else(|| texts.get("any"))?;
    let mut out = serde_json::Map::new();
    for (k, _) in MAX {
        if let Some(t) = v.get(k).and_then(Value::as_str) {
            let mut t = t.to_string();
            for (name, val) in vars {
                t = t.replace(&format!("{{{name}}}"), val);
            }
            out.insert(k.into(), t.into());
        }
    }
    Some(Value::Object(out))
}

/// A chapter's title in the city's language, numbers and outcome (`None`
/// while it runs and nobody has reached the target).
struct ChapterNow {
    title: String,
    progress: [i64; 2],
    outcome: Option<&'static str>,
}

fn chapter_now(
    db: &mut postgres::Client,
    slug: &str,
    key: &str,
    lang: &str,
) -> Result<Option<ChapterNow>, postgres::Error> {
    let Some(r) = db.query_opt(
        "SELECT ch.names::text, extract(epoch FROM ch.ends)::float8, \
           CASE ch.goal WHEN 'hold' THEN greatest(1, cardinality(ch.nodes)) ELSE ch.target END, \
           CASE ch.goal WHEN 'hold' THEN greatest(1, cardinality(ch.nodes)) ELSE coalesce(ch.target_water, ch.target) END, \
           coalesce((SELECT value FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'fire'), 0), \
           coalesce((SELECT value FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'water'), 0), \
           (SELECT extract(epoch FROM reached_at)::float8 FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'fire'), \
           (SELECT extract(epoch FROM reached_at)::float8 FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'water') \
         FROM game.chapters ch WHERE ch.region = $1 AND ch.key = $2",
        &[&slug, &key],
    )?
    else {
        return Ok(None);
    };
    let names: Value = serde_json::from_str(r.get(0)).unwrap_or(Value::Null);
    let title = names[lang]["title"]
        .as_str()
        .or_else(|| {
            names
                .as_object()?
                .values()
                .find_map(|v| v["title"].as_str())
        })
        .unwrap_or(key)
        .to_string();
    let ended = crate::analytics::now_s() >= r.get::<_, f64>(1);
    let targets = [i64::from(r.get::<_, i32>(2)), i64::from(r.get::<_, i32>(3))];
    let progress = [r.get::<_, i64>(4), r.get::<_, i64>(5)];
    let reached = [r.get::<_, Option<f64>>(6), r.get::<_, Option<f64>>(7)];
    let outcome = match crate::campaign::decide(progress, reached, targets, ended) {
        Some(f) => Some(f.as_str()),
        None if ended => Some("tie"),
        None => None,
    };
    Ok(Some(ChapterNow {
        title,
        progress,
        outcome,
    }))
}

struct Planned {
    trigger: String,
    chapter: Option<String>,
    texts: Value,
    card: Option<Value>,
}

/// Make a draft of a post now: texts for the outcome, the picture.
/// `auto`: the trigger's one draft (none if it exists already).
pub(super) async fn draft(
    s: &AppState,
    slug: &str,
    id: i64,
    auto: bool,
) -> Result<Option<i64>, String> {
    let st = s.clone();
    let sl = slug.to_string();
    let first = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let Some(r) = db.query_opt(
                "SELECT trigger, chapter, texts::text, card::text FROM game.posts WHERE region = $1 AND id = $2",
                &[&sl, &id],
            )?
            else {
                return Ok(None);
            };
            let p = Planned {
                trigger: r.get(0),
                chapter: r.get(1),
                texts: serde_json::from_str(r.get(2)).unwrap_or(Value::Null),
                card: r.get::<_, Option<String>>(3).and_then(|c| serde_json::from_str(&c).ok()),
            };
            let lang = primary(db, &sl)?;
            let ch = match &p.chapter {
                Some(k) => chapter_now(db, &sl, k, &lang)?,
                None => None,
            };
            // The end's picture from the snapshot, if there is one.
            let snap = match (&p.chapter, p.trigger.as_str()) {
                (Some(k), "chapter_end") => snapshot_sides(db, &sl, k)?,
                _ => None,
            };
            let centre = db
                .query_opt(
                    "SELECT ST_Y(centroid_geom), ST_X(centroid_geom) FROM geo.regions WHERE slug = $1",
                    &[&sl],
                )?
                .map(|r| (r.get::<_, f64>(0), r.get::<_, f64>(1)));
            Ok(Some((p, lang, ch, snap, centre)))
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or("database")?;
    let Some((p, lang, ch, snap, centre)) = first else {
        return Ok(None);
    };
    let mut notes: Vec<String> = Vec::new();
    let outcome = ch.as_ref().and_then(|c| c.outcome);
    let vars: Vec<(&str, String)> = match &ch {
        Some(c) => vec![
            ("fire", number(c.progress[0], &lang)),
            ("water", number(c.progress[1], &lang)),
            ("chapter", c.title.clone()),
        ],
        None => Vec::new(),
    };
    if p.chapter.is_some() && ch.is_none() {
        notes.push("chapter not found".into());
    }
    let texts = fill(&p.texts, outcome, &vars).unwrap_or_else(|| {
        notes.push(format!("no text for {}", outcome.unwrap_or("any")));
        json!({})
    });
    let mut image: Option<Vec<u8>> = None;
    if let Some(c) = &p.card {
        let sides = match snap {
            Some(v) => Some(v),
            None => {
                if p.trigger == "chapter_end" {
                    notes.push("no snapshot of the chapter's end: picture shows now".into());
                }
                match live_sides(s, slug).await {
                    Ok(v) => Some(v),
                    Err(_) => {
                        notes.push("region not reachable: no picture".into());
                        None
                    }
                }
            }
        };
        if let Some(sides) = sides {
            let (lat, lon) = match (c["lat"].as_f64(), c["lon"].as_f64()) {
                (Some(a), Some(b)) => (a, b),
                _ => centre.unwrap_or((0.0, 0.0)),
            };
            let fillv = |k: &str| {
                let mut t = c[k].as_str().unwrap_or("").to_string();
                for (name, val) in &vars {
                    t = t.replace(&format!("{{{name}}}"), val);
                }
                t
            };
            let spec = Spec {
                lat,
                lon,
                width_m: c["width_m"].as_f64().unwrap_or(900.0),
                title: fillv("title"),
                note: fillv("note"),
                credit: CREDIT.into(),
            };
            let st = s.clone();
            let sl = slug.to_string();
            let svg = tokio::task::spawn_blocking(move || {
                st.with_db(|db| card::load(db, &sl, &spec).map(|g| card::svg(&spec, &g, &sides)))
            })
            .await
            .ok()
            .flatten();
            match svg.map(|v| card::png(&v)) {
                Some(Ok(png)) => image = Some(png),
                Some(Err(e)) => notes.push(format!("picture failed: {e}")),
                None => notes.push("picture failed: database".into()),
            }
        }
    }
    let numbers = ch
        .as_ref()
        .map(|c| json!({ "fire": c.progress[0], "water": c.progress[1] }));
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    let st = s.clone();
    let sl = slug.to_string();
    tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            Ok(db
                .query_opt(
                    "INSERT INTO game.post_runs (post_id, region, auto, outcome, numbers, texts, image, note) \
                     VALUES ($1, $2, $3, $4, $5::text::jsonb, $6::text::jsonb, $7, $8) \
                     ON CONFLICT (post_id) WHERE auto DO NOTHING RETURNING id",
                    &[
                        &id,
                        &sl,
                        &auto,
                        &outcome,
                        &numbers.map(|n| n.to_string()),
                        &texts.to_string(),
                        &image,
                        &note,
                    ],
                )?
                .map(|r| r.get::<_, i64>(0)))
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "database".to_string())
}

/// Every minute on the main cell: drafts for the posts that are due (at
/// most a week late; posts of this shard's regions).
pub(super) async fn tick_loop(s: AppState) {
    let mut every = tokio::time::interval(Duration::from_secs(60));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        if !s.host.is_main() {
            continue;
        }
        let st = s.clone();
        let shard = s.host.shard().to_string();
        let due = tokio::task::spawn_blocking(move || {
            st.with_db(|db| {
                Ok(db
                    .query(
                        "SELECT p.region, p.id FROM game.posts p \
                         JOIN geo.regions r ON r.slug = p.region \
                         LEFT JOIN game.chapters ch ON ch.region = p.region AND ch.key = p.chapter \
                           AND ch.status = 'published' \
                         CROSS JOIN LATERAL (SELECT CASE p.trigger WHEN 'at' THEN p.at \
                           WHEN 'chapter_start' THEN ch.starts + make_interval(mins => p.offset_min) \
                           WHEN 'chapter_end' THEN ch.ends + make_interval(mins => p.offset_min) END AS due) d \
                         WHERE r.shard = $1 AND p.trigger <> 'manual' \
                           AND d.due <= now() AND d.due > now() - interval '7 days' \
                           AND NOT EXISTS (SELECT 1 FROM game.post_runs x WHERE x.post_id = p.id AND x.auto) \
                         ORDER BY d.due LIMIT 20",
                        &[&shard],
                    )?
                    .iter()
                    .map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1)))
                    .collect::<Vec<_>>())
            })
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
        for (slug, id) in due {
            match draft(&s, &slug, id, true).await {
                Ok(Some(run)) => tracing::info!(slug, post = id, run, "posts: draft"),
                Ok(None) => {}
                Err(e) => tracing::warn!(slug, post = id, error = %e, "posts: draft failed"),
            }
        }
    }
}

// --- Editor endpoints ----------------------------------------------------------

const EPOCH: &str = "extract(epoch FROM {})::float8";

fn epoch(col: &str) -> String {
    EPOCH.replace("{}", col)
}

/// `GET /api/ops/posts?region=` (editor): the region's posts and the drafts
/// of the last 60 days (without pictures), newest first.
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
            let posts: Vec<Value> = db
                .query(
                    &format!(
                        "SELECT id, title, trigger, {}, chapter, offset_min, texts::text, card::text, \
                         channels, created_by FROM game.posts WHERE region = $1 ORDER BY id DESC",
                        epoch("at")
                    ),
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<_, i64>(0),
                        "title": r.get::<_, String>(1),
                        "trigger": r.get::<_, String>(2),
                        "at": r.get::<_, Option<f64>>(3),
                        "chapter": r.get::<_, Option<String>>(4),
                        "offset_min": r.get::<_, i32>(5),
                        "texts": serde_json::from_str::<Value>(r.get(6)).unwrap_or(Value::Null),
                        "card": r.get::<_, Option<String>>(7).and_then(|c| serde_json::from_str::<Value>(&c).ok()),
                        "channels": r.get::<_, Vec<String>>(8),
                        "by": r.get::<_, String>(9),
                    })
                })
                .collect();
            let runs: Vec<Value> = db
                .query(
                    &format!(
                        "SELECT id, post_id, auto, {}, state, outcome, numbers::text, texts::text, \
                         image IS NOT NULL, note, decided_by, {} FROM game.post_runs \
                         WHERE region = $1 AND created_at > now() - interval '60 days' \
                         ORDER BY id DESC",
                        epoch("created_at"),
                        epoch("decided_at")
                    ),
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<_, i64>(0),
                        "post": r.get::<_, i64>(1),
                        "auto": r.get::<_, bool>(2),
                        "created": r.get::<_, f64>(3),
                        "state": r.get::<_, String>(4),
                        "outcome": r.get::<_, Option<String>>(5),
                        "numbers": r.get::<_, Option<String>>(6).and_then(|n| serde_json::from_str::<Value>(&n).ok()),
                        "texts": serde_json::from_str::<Value>(r.get(7)).unwrap_or(Value::Null),
                        "image": r.get::<_, bool>(8),
                        "note": r.get::<_, Option<String>>(9),
                        "decided_by": r.get::<_, Option<String>>(10),
                        "decided": r.get::<_, Option<f64>>(11),
                    })
                })
                .collect();
            Ok(json!({ "posts": posts, "runs": runs }))
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

/// `POST /api/ops/posts?region=` (editor): write a post (new, or by id).
pub(super) async fn save(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<PostIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (title, texts, card, channels) = match clean_post(&req) {
        Ok(v) => v,
        Err(e) => return bad(e),
    };
    let at = (req.trigger == "at").then_some(req.at).flatten();
    let chapter = req
        .trigger
        .starts_with("chapter_")
        .then(|| req.chapter.as_deref().map(str::trim).map(String::from))
        .flatten();
    let st = s.clone();
    let by = w.label;
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            if let Some(k) = &chapter {
                let known = t
                    .query_opt(
                        "SELECT 1 FROM game.chapters WHERE region = $1 AND key = $2",
                        &[&slug, k],
                    )?
                    .is_some();
                if !known {
                    return Ok(Err("no such chapter"));
                }
            }
            let card_s = card.as_ref().map(Value::to_string);
            let id: Option<i64> = match req.id {
                Some(id) => t
                    .query_opt(
                        "UPDATE game.posts SET title = $3, trigger = $4, at = to_timestamp($5), \
                         chapter = $6, offset_min = $7, texts = $8::text::jsonb, card = $9::text::jsonb, \
                         channels = $10 WHERE region = $1 AND id = $2 RETURNING id",
                        &[&slug, &id, &title, &req.trigger, &at, &chapter, &req.offset_min,
                          &texts.to_string(), &card_s, &channels],
                    )?
                    .map(|r| r.get(0)),
                None => {
                    let n: i64 = t
                        .query_one("SELECT count(*) FROM game.posts WHERE region = $1", &[&slug])?
                        .get(0);
                    if n >= POSTS_MAX {
                        return Ok(Err("at most 100 posts"));
                    }
                    Some(
                        t.query_one(
                            "INSERT INTO game.posts (region, title, trigger, at, chapter, offset_min, \
                             texts, card, channels, created_by) VALUES ($1, $2, $3, to_timestamp($4), \
                             $5, $6, $7::text::jsonb, $8::text::jsonb, $9, $10) RETURNING id",
                            &[&slug, &title, &req.trigger, &at, &chapter, &req.offset_min,
                              &texts.to_string(), &card_s, &channels, &by],
                        )?
                        .get(0),
                    )
                }
            };
            let Some(id) = id else {
                return Ok(Err("no such post"));
            };
            audit_entity(
                &mut t,
                &format!("post {slug}/{id}"),
                if req.id.is_some() { "change" } else { "add" },
                &by,
                json!({ "title": title, "trigger": req.trigger, "at": at, "chapter": chapter,
                        "offset_min": req.offset_min, "texts": texts, "card": card, "channels": channels }),
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

/// `DELETE /api/ops/posts?region=&id=` (editor): the post and its drafts.
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
                "DELETE FROM game.posts WHERE region = $1 AND id = $2",
                &[&slug, &id],
            )?;
            if n > 0 {
                audit_entity(
                    &mut t,
                    &format!("post {slug}/{id}"),
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
        Some(_) => bad("no such post"),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /api/ops/posts/draft?region=&id=` (editor): a draft of the post now.
pub(super) async fn draft_now(
    State(s): State<AppState>,
    Query(q): Query<IdQ>,
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
    match draft(&s, &slug, q.id, false).await {
        Ok(Some(run)) => Json(json!({ "run": run })).into_response(),
        Ok(None) => bad("no such post"),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, e).into_response(),
    }
}

/// `GET /api/ops/posts/run.png?region=&id=` (editor): a draft's picture.
pub(super) async fn run_png(
    State(s): State<AppState>,
    Query(q): Query<IdQ>,
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
    let id = q.id;
    let img = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            Ok(db
                .query_opt(
                    "SELECT image FROM game.post_runs WHERE region = $1 AND id = $2",
                    &[&slug, &id],
                )?
                .and_then(|r| r.get::<_, Option<Vec<u8>>>(0)))
        })
    })
    .await
    .ok()
    .flatten();
    match img {
        Some(Some(png)) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "private, max-age=86400"),
            ],
            png,
        )
            .into_response(),
        Some(None) => StatusCode::NOT_FOUND.into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct DecideIn {
    /// `sent` (posted by hand) or `discarded`; `draft` takes it back.
    state: String,
    /// The texts as they went out, if changed.
    texts: Option<Value>,
}

/// `POST /api/ops/posts/run?region=&id=` (editor): what became of a draft.
pub(super) async fn decide(
    State(s): State<AppState>,
    Query(q): Query<IdQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<DecideIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !["sent", "discarded", "draft"].contains(&req.state.as_str()) {
        return bad("unknown state");
    }
    let texts = match &req.texts {
        Some(v) => match clean_texts(&json!({ "any": v })) {
            Ok(t) => Some(t["any"].clone()),
            Err(e) => return bad(e),
        },
        None => None,
    };
    let st = s.clone();
    let (by, id) = (w.label, q.id);
    let res =
        tokio::task::spawn_blocking(move || {
            st.with_db(|db| {
                let mut t = db.transaction()?;
                let n = t.execute(
                "UPDATE game.post_runs SET state = $3, texts = coalesce($4::text::jsonb, texts), \
                 decided_by = CASE WHEN $3 = 'draft' THEN NULL ELSE $5 END, \
                 decided_at = CASE WHEN $3 = 'draft' THEN NULL ELSE now() END \
                 WHERE region = $1 AND id = $2",
                &[&slug, &id, &req.state, &texts.as_ref().map(Value::to_string), &by],
            )?;
                if n > 0 {
                    audit_entity(
                        &mut t,
                        &format!("post run {slug}/{id}"),
                        &req.state,
                        &by,
                        json!({ "texts": texts }),
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
        Some(_) => bad("no such draft"),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_the_city_writes_them() {
        assert_eq!(number(1234567, "de"), "1.234.567");
        assert_eq!(number(1234, "en"), "1,234");
        assert_eq!(number(999, "de"), "999");
        assert_eq!(number(-1000, "it"), "-1.000");
    }

    #[test]
    fn the_outcome_picks_its_text_else_any() {
        let t = clean_texts(&json!({
            "fire": { "instagram": "Glut {fire}:{water}", "alt": "Plan" },
            "any": { "mastodon": "{chapter} vorbei" },
            "bogus": { "instagram": "x" },
        }))
        .unwrap();
        assert!(t.get("bogus").is_none());
        let vars = [
            ("fire", "12".to_string()),
            ("water", "3".to_string()),
            ("chapter", "1. Advent".to_string()),
        ];
        let f = fill(&t, Some("fire"), &vars).unwrap();
        assert_eq!(f["instagram"], "Glut 12:3");
        assert_eq!(f["alt"], "Plan");
        let w = fill(&t, Some("water"), &vars).unwrap();
        assert_eq!(w["mastodon"], "1. Advent vorbei");
        assert!(fill(&json!({ "fire": {} }), Some("water"), &vars).is_none());
    }

    #[test]
    fn posts_are_checked() {
        let ok = PostIn {
            id: None,
            title: "Ergebnis 1. Advent".into(),
            trigger: "chapter_end".into(),
            at: None,
            chapter: Some("gg26-1".into()),
            offset_min: 5,
            texts: json!({ "any": { "mastodon": "x" } }),
            card: Some(json!({ "width_m": 950, "title": "Ergebnis" })),
            channels: vec!["mastodon".into(), "mastodon".into()],
        };
        let (_, _, card, ch) = clean_post(&ok).unwrap();
        assert_eq!(ch, vec!["mastodon"]);
        assert_eq!(card.unwrap()["note"], "");
        let no_chapter = PostIn {
            chapter: None,
            ..ok
        };
        assert_eq!(clean_post(&no_chapter).unwrap_err(), "a chapter is needed");
        let bad_card = PostIn {
            chapter: Some("k".into()),
            card: Some(json!({ "lat": 47.6 })),
            ..no_chapter
        };
        assert_eq!(
            clean_post(&bad_card).unwrap_err(),
            "picture: place as lat, lon"
        );
        let too_long = PostIn {
            card: None,
            texts: json!({ "any": { "mastodon": "x".repeat(501) } }),
            ..bad_card
        };
        assert!(clean_post(&too_long).is_err());
    }
}

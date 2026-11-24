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

/// Where the game happens: the middle (median) of the junctions with a
/// side, else the region's centre.
pub fn centre(
    db: &mut postgres::Client,
    slug: &str,
    sides: &Sides,
) -> Result<(f64, f64), postgres::Error> {
    let ids: Vec<i64> = sides.fire.iter().chain(&sides.water).copied().collect();
    let r = db.query_one(
        "SELECT coalesce((SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY n.lat) FROM geo.nodes n WHERE n.id = ANY($2)), ST_Y(r.centroid_geom)), \
                coalesce((SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY n.lon) FROM geo.nodes n WHERE n.id = ANY($2)), ST_X(r.centroid_geom)) \
         FROM geo.regions r WHERE r.slug = $1",
        &[&slug, &ids],
    )?;
    Ok((r.get(0), r.get(1)))
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

/// `GET /api/ops/card.jpg?region=&chapter=&lat=&lon=&width_m=&title=&note=`
/// (editor): the picture of the game state, centred on the region if no
/// place is given.
pub(super) async fn card_jpg(
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
            let sides = match (live, &q.chapter) {
                (Some(v), _) => Some(v),
                (None, Some(k)) => snapshot_sides(db, &slug, k)?,
                (None, None) => None,
            };
            let Some(sides) = sides else {
                return Ok(None);
            };
            if !(spec.lat.is_finite() && spec.lon.is_finite()) {
                (spec.lat, spec.lon) = centre(db, &slug, &sides)?;
            }
            let geo = card::load(db, &slug, &spec)?;
            Ok(Some(card::svg(&spec, &geo, &sides)))
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
    match tokio::task::spawn_blocking(move || card::jpeg(&svg)).await {
        Ok(Ok(jpg)) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            jpg,
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
            Ok(Some((p, lang, ch, snap)))
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or("database")?;
    let Some((p, lang, ch, snap)) = first else {
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
            let given = c["lat"].as_f64().zip(c["lon"].as_f64());
            let fillv = |k: &str| {
                let mut t = c[k].as_str().unwrap_or("").to_string();
                for (name, val) in &vars {
                    t = t.replace(&format!("{{{name}}}"), val);
                }
                t
            };
            let mut spec = Spec {
                lat: f64::NAN,
                lon: f64::NAN,
                width_m: c["width_m"].as_f64().unwrap_or(900.0),
                title: fillv("title"),
                note: fillv("note"),
                credit: CREDIT.into(),
            };
            let st = s.clone();
            let sl = slug.to_string();
            let svg = tokio::task::spawn_blocking(move || {
                st.with_db(|db| {
                    (spec.lat, spec.lon) = match given {
                        Some(v) => v,
                        None => centre(db, &sl, &sides)?,
                    };
                    card::load(db, &sl, &spec).map(|g| card::svg(&spec, &g, &sides))
                })
            })
            .await
            .ok()
            .flatten();
            match svg.map(|v| card::jpeg(&v)) {
                Some(Ok(jpg)) => image = Some(jpg),
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
    let mut n: u64 = 0;
    loop {
        every.tick().await;
        if !s.host.is_main() {
            continue;
        }
        // Tokens hourly, from the second minute on (the lease settles first).
        n += 1;
        if n % 60 == 2 {
            renew_accounts(&s).await;
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
                         image IS NOT NULL, note, decided_by, {}, results::text FROM game.post_runs \
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
                        "results": r.get::<_, Option<String>>(12).and_then(|n| serde_json::from_str::<Value>(&n).ok()),
                    })
                })
                .collect();
            // The Instagram account as the server sees it (expiry, renewal error).
            let instagram = match instagram_env(&slug) {
                Some(env) => db
                    .query_opt(
                        &format!(
                            "SELECT username, {}, error FROM game.post_accounts \
                             WHERE region = $1 AND channel = 'instagram' AND seed = $2",
                            epoch("expires_at")
                        ),
                        &[&slug, &seed_of(&env)],
                    )?
                    .map(|r| {
                        json!({
                            "username": r.get::<_, Option<String>>(0),
                            "expires": r.get::<_, Option<f64>>(1),
                            "error": r.get::<_, Option<String>>(2),
                        })
                    }),
                None => None,
            };
            Ok(json!({ "posts": posts, "runs": runs, "ready": ready(&slug), "instagram": instagram }))
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

/// `GET /api/ops/posts/run.jpg?region=&id=` (editor): a draft's picture.
pub(super) async fn run_jpg(
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
        Some(Some(jpg)) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "private, max-age=86400"),
            ],
            jpg,
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

// --- Sending -----------------------------------------------------------------

/// The region's name in environment variables: `konstanz-altstadt` → `KONSTANZ_ALTSTADT`.
fn env_name(slug: &str) -> String {
    slug.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// A Mastodon account: `ELEMENTS_POST_MASTODON_<REGION>="https://instance token"`
/// (token with `write:statuses write:media`).
struct Mastodon {
    base: String,
    token: String,
}

fn mastodon_of(v: &str) -> Option<Mastodon> {
    let (base, token) = v.trim().split_once(char::is_whitespace)?;
    let base = base.trim_end_matches('/');
    let token = token.trim();
    (base.starts_with("https://") && !token.is_empty() && !token.contains(char::is_whitespace))
        .then(|| Mastodon {
            base: base.into(),
            token: token.into(),
        })
}

fn mastodon(slug: &str) -> Option<Mastodon> {
    mastodon_of(&std::env::var(format!("ELEMENTS_POST_MASTODON_{}", env_name(slug))).ok()?)
}

/// The channels the server can send to for the region.
fn ready(slug: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    if mastodon(slug).is_some() {
        out.push("mastodon");
    }
    if instagram_env(slug).is_some() && crate::media::store().is_some() {
        out.push("instagram");
    }
    out
}

/// An error short enough for the editor, without the request's URL.
fn brief(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| {
                    // Mastodon: {error}; Meta: {error: {message}}.
                    let e = &v["error"];
                    e.as_str().or(e["message"].as_str()).map(str::to_string)
                })
                .unwrap_or(body);
            format!("HTTP {code}: {}", msg.chars().take(200).collect::<String>())
        }
        ureq::Error::Transport(t) => format!("{}: {}", t.kind(), t.message().unwrap_or("")),
    }
}

/// Posts text (and picture with alt text) as a public status; the status's URL.
/// `key` makes a second try of the same send no second status.
fn send_mastodon(
    m: &Mastodon,
    key: &str,
    text: &str,
    alt: &str,
    image: Option<&[u8]>,
    lang: &str,
) -> Result<String, String> {
    let agent = crate::media::agent().ok_or("no TLS")?;
    let auth = format!("Bearer {}", m.token);
    let mut media = Vec::new();
    if let Some(jpg) = image {
        let b = format!("glutflut{:016x}", rand::random::<u64>());
        let mut body = Vec::with_capacity(jpg.len() + 512);
        body.extend_from_slice(
            format!(
                "--{b}\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\n{alt}\r\n\
                 --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"post.jpg\"\r\n\
                 Content-Type: image/jpeg\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(jpg);
        body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
        let r = agent
            .post(&format!("{}/api/v2/media", m.base))
            .set("Authorization", &auth)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={b}"),
            )
            .send_bytes(&body)
            .map_err(|e| format!("picture: {}", brief(e)))?;
        let processing = r.status() == 202;
        let v: Value = r.into_json().map_err(|e| format!("picture: {e}"))?;
        let id = v["id"].as_str().ok_or("picture: no id")?.to_string();
        // 202: still being processed; the status waits for it.
        if processing {
            let mut done = false;
            for _ in 0..30 {
                std::thread::sleep(Duration::from_secs(1));
                let r = agent
                    .get(&format!("{}/api/v1/media/{id}", m.base))
                    .set("Authorization", &auth)
                    .call()
                    .map_err(|e| format!("picture: {}", brief(e)))?;
                if r.status() == 200 {
                    done = true;
                    break;
                }
            }
            if !done {
                return Err("picture: still processing after 30 s".into());
            }
        }
        media.push(id);
    }
    let r = agent
        .post(&format!("{}/api/v1/statuses", m.base))
        .set("Authorization", &auth)
        .set("Idempotency-Key", key)
        .send_json(json!({
            "status": text,
            "media_ids": media,
            "visibility": "public",
            "language": lang,
        }))
        .map_err(brief)?;
    let v: Value = r.into_json().map_err(|e| e.to_string())?;
    v["url"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "no URL in the answer".into())
}

#[derive(Deserialize)]
pub(super) struct SendQ {
    region: Option<String>,
    id: i64,
    channel: String,
}

#[derive(Deserialize)]
pub(super) struct SendIn {
    /// The texts as edited in the editor; stored before sending.
    texts: Option<Value>,
}

/// `POST /api/ops/posts/send?region=&id=&channel=` (editor): sends a draft
/// to a channel. Marks `results.<channel>` as `{sending}` first (a second
/// click waits 10 minutes), then stores `{url}` or `{error}`; once every
/// channel of the post has its URL, the draft counts as sent.
pub(super) async fn send(
    State(s): State<AppState>,
    Query(q): Query<SendQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<SendIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let channel = q.channel.clone();
    let via = match channel.as_str() {
        "mastodon" => match mastodon(&slug) {
            Some(m) => Via::Mastodon(m),
            None => return bad("Mastodon is not set up for this region"),
        },
        "instagram" => {
            if crate::media::store().is_none() {
                return bad("Instagram needs the media bucket for the picture");
            }
            match instagram(&s, &slug).await {
                Ok(a) => Via::Instagram(a),
                Err(e) => return bad(&e),
            }
        }
        _ => return bad("unknown channel"),
    };
    let texts = match &req.texts {
        Some(v) => match clean_texts(&json!({ "any": v })) {
            Ok(t) => Some(t["any"].clone()),
            Err(e) => return bad(e),
        },
        None => None,
    };
    let (st, sl, id, ch) = (s.clone(), slug.clone(), q.id, channel.clone());
    // Claim the channel and store the edited texts; else say why not.
    let got = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            if let Some(r) = db.query_opt(
                "UPDATE game.post_runs SET texts = coalesce($3::text::jsonb, texts), \
                   results = jsonb_set(coalesce(results, '{}'), ARRAY[$4::text], \
                     jsonb_build_object('sending', extract(epoch FROM now()))) \
                 WHERE region = $1 AND id = $2 AND state = 'draft' \
                   AND results -> $4 ->> 'url' IS NULL \
                   AND coalesce((results -> $4 ->> 'sending')::float8, 0) \
                     < extract(epoch FROM now()) - 600 \
                 RETURNING texts::text, image",
                &[&sl, &id, &texts.as_ref().map(Value::to_string), &ch],
            )? {
                return Ok(Ok((
                    serde_json::from_str::<Value>(r.get(0)).unwrap_or(Value::Null),
                    r.get::<_, Option<Vec<u8>>>(1),
                    primary(db, &sl)?,
                )));
            }
            let why = db
                .query_opt(
                    "SELECT state, results -> $3 ->> 'url' FROM game.post_runs \
                     WHERE region = $1 AND id = $2",
                    &[&sl, &id, &ch],
                )?
                .map(|r| (r.get::<_, String>(0), r.get::<_, Option<String>>(1)));
            Ok(Err(why))
        })
    })
    .await
    .ok()
    .flatten();
    let (texts, image, lang) = match got {
        Some(Ok(v)) => v,
        Some(Err(None)) => return bad("no such draft"),
        Some(Err(Some((_, Some(url))))) => return bad_url("already sent", &url),
        Some(Err(Some((state, None)))) if state != "draft" => return bad("only drafts are sent"),
        Some(Err(Some(_))) => return bad("being sent, try again in 10 minutes"),
        None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let text = texts[&channel].as_str().unwrap_or("").to_string();
    let alt = texts["alt"].as_str().unwrap_or("").to_string();
    let key = format!("glutflut-run-{}-{channel}", q.id);
    let (sl, id) = (slug.clone(), q.id);
    let sent = tokio::task::spawn_blocking(move || {
        if text.is_empty() {
            return Err("no text for this channel".to_string());
        }
        match via {
            Via::Mastodon(m) => send_mastodon(&m, &key, &text, &alt, image.as_deref(), &lang),
            Via::Instagram(a) => {
                let jpg = image.ok_or("Instagram needs a picture")?;
                let store = crate::media::store().ok_or("no media bucket")?;
                let k = store.key(&format!(
                    "{sl}/posts/{id}-{:08x}.jpg",
                    rand::random::<u32>()
                ));
                store.put(&k, &jpg).map_err(|e| format!("picture: {e}"))?;
                send_instagram(&a, &text, &alt, &store.fetch_url(&k))
            }
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let result = match &sent {
        Ok(url) => json!({ "url": url, "at": now_s() }),
        Err(e) => json!({ "error": e, "at": now_s() }),
    };
    let (st, sl, id, ch, by) = (s.clone(), slug.clone(), q.id, channel.clone(), w.label);
    let res = result.clone();
    let stored = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            t.execute(
                "UPDATE game.post_runs r SET results = jsonb_set(coalesce(r.results, '{}'), \
                   ARRAY[$3::text], $4::text::jsonb) \
                 WHERE r.region = $1 AND r.id = $2",
                &[&sl, &id, &ch, &res.to_string()],
            )?;
            // Sent once every channel of the post has its URL.
            t.execute(
                "UPDATE game.post_runs r SET state = 'sent', decided_by = $3, decided_at = now() \
                 FROM game.posts p WHERE p.id = r.post_id AND r.region = $1 AND r.id = $2 \
                   AND r.state = 'draft' \
                   AND NOT EXISTS (SELECT 1 FROM unnest(p.channels) c \
                                   WHERE r.results -> c ->> 'url' IS NULL)",
                &[&sl, &id, &by],
            )?;
            audit_entity(
                &mut t,
                &format!("post run {sl}/{id}"),
                &format!("send {ch}"),
                &by,
                res,
            )?;
            t.commit()?;
            Ok(())
        })
    })
    .await
    .ok()
    .flatten();
    if stored.is_none() {
        tracing::error!(region = %slug, run = q.id, ?result, "post sent, result not stored");
    }
    match sent {
        Ok(url) => Json(json!({ "url": url })).into_response(),
        Err(e) => {
            tracing::warn!(region = %slug, run = q.id, channel = %channel, error = %e, "post not sent");
            (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response()
        }
    }
}

enum Via {
    Mastodon(Mastodon),
    Instagram(Insta),
}

// --- Instagram -----------------------------------------------------------------

/// Instagram API with Instagram Login.
const IG: &str = "https://graph.instagram.com";
// Meta retires a version about two years after its release; bump then.
const IG_VERSION: &str = "v24.0";

/// An Instagram professional account as stored (`game.post_accounts`).
struct Insta {
    token: String,
    user_id: String,
    username: Option<String>,
}

/// The token in the environment: `ELEMENTS_POST_INSTAGRAM_<REGION>`.
fn instagram_env(slug: &str) -> Option<String> {
    std::env::var(format!("ELEMENTS_POST_INSTAGRAM_{}", env_name(slug)))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && !v.contains(char::is_whitespace))
}

fn seed_of(token: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn ig_get(agent: &ureq::Agent, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let mut r = agent.get(&format!("{IG}/{path}"));
    for (k, v) in query {
        r = r.query(k, v);
    }
    r.call()
        .map_err(brief)?
        .into_json()
        .map_err(|e| e.to_string())
}

fn ig_post(agent: &ureq::Agent, path: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    agent
        .post(&format!("{IG}/{path}"))
        .send_form(form)
        .map_err(brief)?
        .into_json()
        .map_err(|e| e.to_string())
}

/// The region's account: the stored one, or the environment's token stored
/// first (with the account's id) when it is new there.
async fn instagram(s: &AppState, slug: &str) -> Result<Insta, String> {
    let env = instagram_env(slug).ok_or("Instagram is not set up for this region")?;
    let seed = seed_of(&env);
    let (st, sl, sd) = (s.clone(), slug.to_string(), seed.clone());
    let stored = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            Ok(db
                .query_opt(
                    "SELECT token, user_id, username FROM game.post_accounts \
                     WHERE region = $1 AND channel = 'instagram' AND seed = $2",
                    &[&sl, &sd],
                )?
                .map(|r| Insta {
                    token: r.get(0),
                    user_id: r.get(1),
                    username: r.get(2),
                }))
        })
    })
    .await
    .ok()
    .flatten()
    .ok_or("database")?;
    if let Some(a) = stored {
        return Ok(a);
    }
    let tok = env.clone();
    let me = tokio::task::spawn_blocking(move || {
        let agent = crate::media::agent().ok_or("no TLS")?;
        ig_get(
            &agent,
            &format!("{IG_VERSION}/me"),
            &[("fields", "user_id,username"), ("access_token", &tok)],
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Instagram account: {e}"))?;
    let user_id = match &me["user_id"] {
        Value::String(v) => v.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err("Instagram account: no user_id".into()),
    };
    let username = me["username"].as_str().map(str::to_string);
    let a = Insta {
        token: env,
        user_id,
        username,
    };
    let (st, sl) = (s.clone(), slug.to_string());
    let (tok, uid, un) = (a.token.clone(), a.user_id.clone(), a.username.clone());
    tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            db.execute(
                "INSERT INTO game.post_accounts (region, channel, seed, token, user_id, username) \
                 VALUES ($1, 'instagram', $2, $3, $4, $5) \
                 ON CONFLICT (region, channel) DO UPDATE SET seed = $2, token = $3, user_id = $4, \
                   username = $5, expires_at = NULL, refreshed_at = now(), error = NULL",
                &[&sl, &seed, &tok, &uid, &un],
            )
        })
    })
    .await
    .ok()
    .flatten()
    .ok_or("database")?;
    tracing::info!(region = %slug, user = ?a.username, "posts: Instagram account from the environment");
    Ok(a)
}

/// Publishes one picture with caption and alt text; the post's link.
fn send_instagram(a: &Insta, caption: &str, alt: &str, image_url: &str) -> Result<String, String> {
    let agent = crate::media::agent().ok_or("no TLS")?;
    let tok = a.token.as_str();
    let mut form = vec![
        ("image_url", image_url),
        ("caption", caption),
        ("access_token", tok),
    ];
    if !alt.is_empty() {
        form.push(("alt_text", alt));
    }
    let c = ig_post(&agent, &format!("{IG_VERSION}/{}/media", a.user_id), &form)
        .map_err(|e| format!("container: {e}"))?;
    let cid = c["id"].as_str().ok_or("container: no id")?.to_string();
    // Meta fetches the picture; wait until the container is ready.
    let mut ready = false;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_secs(3));
        let v = ig_get(
            &agent,
            &format!("{IG_VERSION}/{cid}"),
            &[("fields", "status_code,status"), ("access_token", tok)],
        )
        .map_err(|e| format!("container status: {e}"))?;
        match v["status_code"].as_str() {
            Some("FINISHED") => {
                ready = true;
                break;
            }
            Some("IN_PROGRESS") | None => {}
            Some(code) => {
                return Err(format!(
                    "container {code}: {}",
                    v["status"].as_str().unwrap_or("")
                ))
            }
        }
    }
    if !ready {
        return Err("container not ready after 2 minutes".into());
    }
    let p = ig_post(
        &agent,
        &format!("{IG_VERSION}/{}/media_publish", a.user_id),
        &[("creation_id", &cid), ("access_token", tok)],
    )
    .map_err(|e| format!("publish: {e}"))?;
    let mid = p["id"].as_str().ok_or("publish: no id")?.to_string();
    // Published; the link is nice to have.
    let link = ig_get(
        &agent,
        &format!("{IG_VERSION}/{mid}"),
        &[("fields", "permalink"), ("access_token", tok)],
    )
    .ok()
    .and_then(|v| v["permalink"].as_str().map(str::to_string));
    Ok(link.unwrap_or_else(|| match &a.username {
        Some(u) => format!("https://www.instagram.com/{u}/"),
        None => "https://www.instagram.com/".into(),
    }))
}

/// Renews the shard's Instagram tokens once a week (a token lives 60 days;
/// renewing needs it 24 hours old); stores a new token from the environment
/// first. A failure is kept in `error` and tried again an hour later.
async fn renew_accounts(s: &AppState) {
    let (st, shard) = (s.clone(), s.host.shard().to_string());
    let slugs = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            Ok(db
                .query("SELECT slug FROM geo.regions WHERE shard = $1", &[&shard])?
                .iter()
                .map(|r| r.get::<_, String>(0))
                .collect::<Vec<_>>())
        })
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    for slug in slugs.into_iter().filter(|sl| instagram_env(sl).is_some()) {
        let a = match instagram(s, &slug).await {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(region = %slug, error = %e, "posts: Instagram account");
                continue;
            }
        };
        let (st, sl) = (s.clone(), slug.clone());
        let due = tokio::task::spawn_blocking(move || {
            st.with_db(|db| {
                Ok(db
                    .query_opt(
                        "SELECT 1 FROM game.post_accounts WHERE region = $1 AND channel = 'instagram' \
                         AND refreshed_at < now() - interval '7 days'",
                        &[&sl],
                    )?
                    .is_some())
            })
        })
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
        if !due {
            continue;
        }
        let tok = a.token.clone();
        let got = tokio::task::spawn_blocking(move || {
            let agent = crate::media::agent().ok_or("no TLS")?;
            let v = ig_get(
                &agent,
                "refresh_access_token",
                &[("grant_type", "ig_refresh_token"), ("access_token", &tok)],
            )?;
            let t = v["access_token"].as_str().ok_or("no token in the answer")?;
            Ok::<_, String>((t.to_string(), v["expires_in"].as_i64().unwrap_or(0)))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let (st, sl) = (s.clone(), slug.clone());
        match &got {
            Ok(_) => tracing::info!(region = %slug, "posts: Instagram token renewed"),
            Err(e) => {
                tracing::warn!(region = %slug, error = %e, "posts: Instagram token not renewed")
            }
        }
        tokio::task::spawn_blocking(move || {
            st.with_db(|db| {
                match got {
                Ok((token, secs)) => db.execute(
                    "UPDATE game.post_accounts SET token = $2, refreshed_at = now(), error = NULL, \
                       expires_at = CASE WHEN $3 > 0 THEN now() + make_interval(secs => $3) END \
                     WHERE region = $1 AND channel = 'instagram'",
                    &[&sl, &token, &(secs as f64)],
                ),
                Err(e) => db.execute(
                    "UPDATE game.post_accounts SET error = $2 \
                     WHERE region = $1 AND channel = 'instagram'",
                    &[&sl, &e],
                ),
            }
            })
        })
        .await
        .ok();
    }
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn bad_url(msg: &str, url: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": msg, "url": url })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mastodon_accounts_from_the_env() {
        assert_eq!(env_name("konstanz-altstadt"), "KONSTANZ_ALTSTADT");
        let m = mastodon_of(" https://mastodon.social/  abc123 ").unwrap();
        assert_eq!(
            (m.base.as_str(), m.token.as_str()),
            ("https://mastodon.social", "abc123")
        );
        assert!(mastodon_of("http://mastodon.social abc").is_none());
        assert!(mastodon_of("https://mastodon.social").is_none());
        assert!(mastodon_of("https://mastodon.social a b").is_none());
        assert_eq!(seed_of("abc").len(), 64);
        assert_ne!(seed_of("abc"), seed_of("abd"));
    }

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

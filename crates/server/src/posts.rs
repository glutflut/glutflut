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
use crate::card::{self, Look, Sides, Spec};

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
            ..Default::default()
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
            let values = serde_json::from_value::<Vec<(i64, u8, u8)>>(v["v"].clone())
                .unwrap_or_default()
                .into_iter()
                .map(|(id, f, w)| (id, (f as f32 / 100.0, w as f32 / 100.0)))
                .collect();
            return Ok(Sides {
                fire: ids("fire"),
                water: ids("water"),
                front: ids("front"),
                values,
                sources: serde_json::from_value(v["src"].clone()).unwrap_or_default(),
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
    /// `plan` (default) or `game`.
    look: Option<String>,
}

/// `GET /api/ops/card.jpg?region=&chapter=&lat=&lon=&width_m=&title=&note=&look=`
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
    let Some(look) = Look::parse(q.look.as_deref().unwrap_or("plan")) else {
        return (StatusCode::BAD_REQUEST, "look: plan or game").into_response();
    };
    let clip = |t: Option<String>, n: usize| t.unwrap_or_default().chars().take(n).collect();
    let mut spec = Spec {
        look,
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
/// The most a field may have: the shared text, per channel (Instagram 2200,
/// Mastodon 500), the alt text.
const MAX: [(&str, usize); 4] = [
    ("text", 2200),
    ("instagram", 2200),
    ("mastodon", 500),
    ("alt", 1000),
];
/// Instagram takes at most 30 hashtags.
const TAGS_MAX: usize = 30;
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
    /// Hashtags, without "#".
    #[serde(default)]
    pub tags: Vec<String>,
    /// Instagram's place: `{id, name}` (a Facebook page with a location).
    #[serde(default)]
    pub location: Option<Value>,
}

/// The texts: known variants and channels only, within the lengths; at least one.
pub fn clean_texts(v: &Value) -> Result<Value, &'static str> {
    let mut out = serde_json::Map::new();
    for var in VARIANTS {
        let Some(e) = v.get(var) else { continue };
        let mut m = serde_json::Map::new();
        for (k, max) in MAX {
            if let Some(t) = clean_text(e.get(k), max).map_err(|_| match k {
                "text" => "text: at most 2200 characters",
                "instagram" => "Instagram: at most 2200 characters",
                "mastodon" => "Mastodon: at most 500 characters",
                _ => "alt text: at most 1000 characters",
            })? {
                m.insert(k.into(), t.into());
            }
        }
        if m.contains_key("text") || m.contains_key("instagram") || m.contains_key("mastodon") {
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
    let look = match v.get("look").and_then(Value::as_str) {
        None => Look::Plan,
        Some(l) => Look::parse(l).ok_or("picture: look plan or game")?,
    };
    let mut out = json!({ "width_m": width_m, "look": look.name() });
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

/// A post's place: `{id, name}`, id digits only; null for none.
pub fn clean_location(v: &Option<Value>) -> Result<Option<Value>, &'static str> {
    let Some(v) = v.as_ref().filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let id = match &v["id"] {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return Err("place: an id is needed"),
    };
    if id.is_empty() || id.len() > 30 || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err("place: the id is digits only");
    }
    let name = clean_text(v.get("name"), 80).map_err(|_| "place: name at most 80 characters")?;
    Ok(Some(json!({ "id": id, "name": name.unwrap_or_default() })))
}

/// A hashtag as stored: without "#", letters, digits and "_", not only digits.
pub fn clean_tag(t: &str) -> Option<String> {
    let t = t.trim().trim_start_matches('#');
    let n = t.chars().count();
    ((1..=60).contains(&n)
        && t.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !t.chars().all(|c| c.is_ascii_digit()))
    .then(|| t.to_string())
}

/// The hashtags as appended: a blank line, then `#a #b`; nothing without.
fn tag_line(tags: &[String]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let joined: Vec<String> = tags.iter().map(|t| format!("#{t}")).collect();
    format!("\n\n{}", joined.join(" "))
}

/// Hashtags written in the text itself.
fn tags_in(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.starts_with('#') && w.chars().count() > 1)
        .count()
}

#[allow(clippy::type_complexity)]
fn clean_post(
    p: &PostIn,
) -> Result<(String, Value, Option<Value>, Vec<String>, Vec<String>), &'static str> {
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
    let mut tags: Vec<String> = Vec::new();
    for t in &p.tags {
        let t = clean_tag(t).ok_or("hashtags: letters, digits and _ only")?;
        if !tags.iter().any(|x| x.to_lowercase() == t.to_lowercase()) {
            tags.push(t);
        }
    }
    if tags.len() > TAGS_MAX {
        return Err("at most 30 hashtags");
    }
    let texts = clean_texts(&p.texts)?;
    // Each channel's text with the hashtags must fit (the numbers change it a little).
    let extra = tag_line(&tags).chars().count();
    for v in texts.as_object().into_iter().flat_map(|m| m.values()) {
        for ch in &channels {
            let Some(t) = v
                .get(ch.as_str())
                .or_else(|| v.get("text"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let n = t.chars().count() + extra;
            match ch.as_str() {
                "mastodon" if n > 500 => {
                    return Err(
                        "Mastodon: at most 500 characters with the hashtags; write its own text",
                    )
                }
                "instagram" if n > 2200 => {
                    return Err("Instagram: at most 2200 characters with the hashtags")
                }
                "instagram" if tags_in(t) + tags.len() > TAGS_MAX => {
                    return Err("Instagram: at most 30 hashtags, with those in the text")
                }
                _ => {}
            }
        }
    }
    Ok((title.to_string(), texts, card, channels, tags))
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
pub fn fill(
    texts: &Value,
    outcome: Option<&str>,
    vars: &[(&str, String)],
    tags: &[String],
) -> Option<Value> {
    let v = outcome
        .and_then(|o| texts.get(o))
        .or_else(|| texts.get("any"))?;
    let put = |t: &str| {
        let mut t = t.to_string();
        for (name, val) in vars {
            t = t.replace(&format!("{{{name}}}"), val);
        }
        t
    };
    let mut out = serde_json::Map::new();
    // A channel's own text, else the shared one; the hashtags at the end.
    for ch in CHANNELS {
        if let Some(t) = v.get(ch).or_else(|| v.get("text")).and_then(Value::as_str) {
            out.insert(ch.into(), format!("{}{}", put(t), tag_line(tags)).into());
        }
    }
    if let Some(t) = v.get("alt").and_then(Value::as_str) {
        out.insert("alt".into(), put(t).into());
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
    tags: Vec<String>,
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
                "SELECT trigger, chapter, texts::text, card::text, tags FROM game.posts \
                 WHERE region = $1 AND id = $2",
                &[&sl, &id],
            )?
            else {
                return Ok(None);
            };
            let p = Planned {
                trigger: r.get(0),
                chapter: r.get(1),
                texts: serde_json::from_str(r.get(2)).unwrap_or(Value::Null),
                card: r
                    .get::<_, Option<String>>(3)
                    .and_then(|c| serde_json::from_str(&c).ok()),
                tags: r.get(4),
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
    let texts = fill(&p.texts, outcome, &vars, &p.tags).unwrap_or_else(|| {
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
                look: c["look"].as_str().and_then(Look::parse).unwrap_or_default(),
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
        // Accounts hourly (each checked daily), from the second minute on.
        n += 1;
        if n % 60 == 2 {
            check_accounts(&s).await;
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
                         channels, created_by, tags, location::text FROM game.posts \
                         WHERE region = $1 ORDER BY id DESC",
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
                        "tags": r.get::<_, Vec<String>>(10),
                        "location": r.get::<_, Option<String>>(11).and_then(|c| serde_json::from_str::<Value>(&c).ok()),
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
            let hashtags: Vec<Value> = db
                .query(
                    "SELECT tag, preset FROM game.post_tags WHERE region = $1 ORDER BY lower(tag)",
                    &[&slug],
                )?
                .iter()
                .map(|r| json!({ "tag": r.get::<_, String>(0), "preset": r.get::<_, bool>(1) }))
                .collect();
            Ok(json!({ "posts": posts, "runs": runs, "ready": ready(&slug), "instagram": instagram,
                       "hashtags": hashtags }))
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
    let (title, texts, card, channels, tags) = match clean_post(&req) {
        Ok(v) => v,
        Err(e) => return bad(e),
    };
    let location = match clean_location(&req.location) {
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
            let loc_s = location.as_ref().map(Value::to_string);
            let id: Option<i64> = match req.id {
                Some(id) => t
                    .query_opt(
                        "UPDATE game.posts SET title = $3, trigger = $4, at = to_timestamp($5), \
                         chapter = $6, offset_min = $7, texts = $8::text::jsonb, card = $9::text::jsonb, \
                         channels = $10, tags = $11, location = $12::text::jsonb \
                         WHERE region = $1 AND id = $2 RETURNING id",
                        &[&slug, &id, &title, &req.trigger, &at, &chapter, &req.offset_min,
                          &texts.to_string(), &card_s, &channels, &tags, &loc_s],
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
                             texts, card, channels, tags, location, created_by) VALUES ($1, $2, $3, \
                             to_timestamp($4), $5, $6, $7::text::jsonb, $8::text::jsonb, $9, $10, \
                             $11::text::jsonb, $12) RETURNING id",
                            &[&slug, &title, &req.trigger, &at, &chapter, &req.offset_min,
                              &texts.to_string(), &card_s, &channels, &tags, &loc_s, &by],
                        )?
                        .get(0),
                    )
                }
            };
            let Some(id) = id else {
                return Ok(Err("no such post"));
            };
            // A post's new hashtags join the region's list.
            t.execute(
                "INSERT INTO game.post_tags (region, tag) SELECT $1, unnest($2::text[]) \
                 ON CONFLICT DO NOTHING",
                &[&slug, &tags],
            )?;
            audit_entity(
                &mut t,
                &format!("post {slug}/{id}"),
                if req.id.is_some() { "change" } else { "add" },
                &by,
                json!({ "title": title, "trigger": req.trigger, "at": at, "chapter": chapter,
                        "offset_min": req.offset_min, "texts": texts, "card": card, "channels": channels,
                        "tags": tags, "location": location }),
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

#[derive(Deserialize)]
pub(super) struct TagIn {
    tag: String,
    #[serde(default)]
    preset: bool,
}

#[derive(Deserialize)]
pub(super) struct TagQ {
    region: Option<String>,
    tag: String,
}

/// `POST /api/ops/posts/tags?region=` (editor): a hashtag of the region's
/// list, new or with `preset` changed (ticked for new posts).
pub(super) async fn tag_save(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<TagIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(tag) = clean_tag(&req.tag) else {
        return bad("hashtags: letters, digits and _ only");
    };
    let (st, by) = (s.clone(), w.label);
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            let n: i64 = t
                .query_one(
                    "SELECT count(*) FROM game.post_tags WHERE region = $1",
                    &[&slug],
                )?
                .get(0);
            // Same tag in other case: that one.
            let known: Option<String> = t
                .query_opt(
                    "SELECT tag FROM game.post_tags WHERE region = $1 AND lower(tag) = lower($2)",
                    &[&slug, &tag],
                )?
                .map(|r| r.get(0));
            if known.is_none() && n >= 200 {
                return Ok(Err("at most 200 hashtags"));
            }
            let tag = known.unwrap_or(tag);
            t.execute(
                "INSERT INTO game.post_tags (region, tag, preset) VALUES ($1, $2, $3) \
                 ON CONFLICT (region, tag) DO UPDATE SET preset = $3",
                &[&slug, &tag, &req.preset],
            )?;
            audit_entity(
                &mut t,
                &format!("post tag {slug}/{tag}"),
                "set",
                &by,
                json!({ "preset": req.preset }),
            )?;
            t.commit()?;
            Ok(Ok(()))
        })
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Some(Err(e)) => bad(e),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `DELETE /api/ops/posts/tags?region=&tag=` (editor): off the region's list
/// (posts keep theirs).
pub(super) async fn tag_delete(
    State(s): State<AppState>,
    Query(q): Query<TagQ>,
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
    let (st, by, tag) = (
        s.clone(),
        w.label,
        q.tag.trim_start_matches('#').to_string(),
    );
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let mut t = db.transaction()?;
            let n = t.execute(
                "DELETE FROM game.post_tags WHERE region = $1 AND tag = $2",
                &[&slug, &tag],
            )?;
            if n > 0 {
                audit_entity(
                    &mut t,
                    &format!("post tag {slug}/{tag}"),
                    "delete",
                    &by,
                    json!({}),
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
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// What the editor typed for a place: a page id, or a Facebook link to the
/// page (`profile.php?id=`, `/pages/Name/123`, `/name`); the id or the name.
pub fn place_key(q: &str) -> Option<String> {
    let q = q.trim();
    if !q.is_empty() && q.chars().all(|c| c.is_ascii_digit()) {
        return Some(q.to_string());
    }
    let u = url::Url::parse(q).ok()?;
    if !u.host_str()?.ends_with("facebook.com") {
        return None;
    }
    if let Some((_, id)) = u.query_pairs().find(|(k, _)| k == "id") {
        return id
            .chars()
            .all(|c| c.is_ascii_digit())
            .then(|| id.to_string());
    }
    let parts: Vec<&str> = u.path_segments()?.filter(|p| !p.is_empty()).collect();
    if let Some(last) = parts
        .last()
        .filter(|l| l.chars().all(|c| c.is_ascii_digit()))
    {
        return Some(last.to_string());
    }
    // A page's own name: letters, digits, dots.
    let name = parts.first()?;
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.')
        .then(|| name.to_string())
}

#[derive(Deserialize)]
pub(super) struct PlaceQ {
    region: Option<String>,
    q: String,
}

/// `GET /api/ops/posts/place?region=&q=` (editor): is `q` a place Instagram
/// takes? First asks for the page (name, address; needs page rights the app
/// may lack), else tries it: a container with this `location_id` and a blank
/// picture, never published. `checked`: "page" or "container".
pub(super) async fn place(
    State(s): State<AppState>,
    Query(q): Query<PlaceQ>,
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
    let Some(key) = place_key(&q.q) else {
        return bad("place: a page id or a Facebook link to the page");
    };
    let a = match instagram(&s, &slug).await {
        Ok(a) => a,
        Err(e) => return bad(&e),
    };
    let sl = slug.clone();
    let got = tokio::task::spawn_blocking(move || {
        let agent = crate::media::agent().ok_or("no TLS")?;
        let page = ig_get(
            &agent,
            &key,
            &[("fields", "id,name,location"), ("access_token", &a.token)],
        );
        let page_err = match page {
            Ok(v) => {
                let loc = &v["location"];
                let city = loc["city"].as_str().unwrap_or("");
                return Ok(json!({
                    "id": id_of(&v["id"]),
                    "name": v["name"],
                    "city": city,
                    "has_place": loc.is_object() && (loc.get("latitude").is_some() || !city.is_empty()),
                    "checked": "page",
                }));
            }
            Err(e) => e,
        };
        if !key.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!(
                "{page_err}; with the page's number instead of its name it can be tried"
            ));
        }
        let store = crate::media::store().ok_or("no media bucket")?;
        let k = store.key(&format!("{sl}/posts/place-probe.jpg"));
        store
            .put(&k, &probe_jpeg()?)
            .map_err(|e| format!("picture: {e}"))?;
        ig_container(&agent, &a, &store.fetch_url(&k), "", "", Some(&key))
            .map_err(|e| format!("Instagram does not take it: {e}"))?;
        Ok(json!({ "id": key, "name": "", "city": "", "has_place": true, "checked": "container" }))
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match got {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("Meta: {e}") })),
        )
            .into_response(),
    }
}

/// A blank 4:5 picture in the paper's colour, for trying a place.
fn probe_jpeg() -> Result<Vec<u8>, String> {
    let img = image::RgbImage::from_pixel(400, 500, image::Rgb([0xea, 0xdc, 0xbc]));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode_image(&img)
        .map_err(|e| e.to_string())?;
    Ok(out)
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

/// A post variable for the region; quotes around it dropped (`docker
/// --env-file` keeps them).
fn post_var(channel: &str, slug: &str) -> Option<String> {
    let v = std::env::var(format!("ELEMENTS_POST_{channel}_{}", env_name(slug))).ok()?;
    let v = v.trim();
    let v = ['"', '\'']
        .iter()
        .find_map(|q| v.strip_prefix(*q).and_then(|x| x.strip_suffix(*q)))
        .unwrap_or(v);
    Some(v.trim().to_string())
}

/// A Mastodon account: `ELEMENTS_POST_MASTODON_<REGION>=https://instance token`
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
    mastodon_of(&post_var("MASTODON", slug)?)
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
                 RETURNING texts::text, image, \
                   (SELECT p.location ->> 'id' FROM game.posts p WHERE p.id = post_id)",
                &[&sl, &id, &texts.as_ref().map(Value::to_string), &ch],
            )? {
                return Ok(Ok((
                    serde_json::from_str::<Value>(r.get(0)).unwrap_or(Value::Null),
                    r.get::<_, Option<Vec<u8>>>(1),
                    r.get::<_, Option<String>>(2),
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
    let (texts, image, place, lang) = match got {
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
            Via::Mastodon(m) => {
                send_mastodon(&m, &key, &text, &alt, image.as_deref(), &lang).map(|u| (u, None))
            }
            Via::Instagram(a) => {
                let jpg = image.ok_or("Instagram needs a picture")?;
                let store = crate::media::store().ok_or("no media bucket")?;
                let k = store.key(&format!(
                    "{sl}/posts/{id}-{:08x}.jpg",
                    rand::random::<u32>()
                ));
                store.put(&k, &jpg).map_err(|e| format!("picture: {e}"))?;
                send_instagram(&a, &text, &alt, &store.fetch_url(&k), place.as_deref())
            }
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let result = match &sent {
        Ok((url, None)) => json!({ "url": url, "at": now_s() }),
        Ok((url, Some(n))) => json!({ "url": url, "note": n, "at": now_s() }),
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
        Ok((url, note)) => Json(json!({ "url": url, "note": note })).into_response(),
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

/// Instagram through the Facebook Graph API: new Meta apps only get "API setup
/// with Unified login", whose token is a system user's (expiry: never or 60
/// days, no renewal); the account comes from `/me/assigned_instagram_accounts`.
const IG: &str = "https://graph.facebook.com";
// Meta retires a version about two years after its release; bump then.
const IG_VERSION: &str = "v24.0";

/// An Instagram professional account as stored (`game.post_accounts`).
struct Insta {
    token: String,
    user_id: String,
    username: Option<String>,
}

/// The environment's `ELEMENTS_POST_INSTAGRAM_<REGION>`: `token`, or
/// `account-id token` when the token reaches several accounts.
fn instagram_of(v: &str) -> Option<(Option<String>, String)> {
    let mut it = v.split_whitespace();
    match (it.next(), it.next(), it.next()) {
        (Some(t), None, None) => Some((None, t.to_string())),
        (Some(id), Some(t), None) if id.chars().all(|c| c.is_ascii_digit()) => {
            Some((Some(id.to_string()), t.to_string()))
        }
        _ => None,
    }
}

fn instagram_env(slug: &str) -> Option<String> {
    post_var("INSTAGRAM", slug).filter(|v| instagram_of(v).is_some())
}

fn seed_of(token: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn ig_get(agent: &ureq::Agent, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let mut r = agent.get(&format!("{IG}/{IG_VERSION}/{path}"));
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
        .post(&format!("{IG}/{IG_VERSION}/{path}"))
        .send_form(form)
        .map_err(brief)?
        .into_json()
        .map_err(|e| e.to_string())
}

fn id_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The account the token posts to: the given one, else the only one assigned.
fn ig_account(id: Option<&str>, token: &str) -> Result<(String, Option<String>), String> {
    let agent = crate::media::agent().ok_or("no TLS")?;
    if let Some(id) = id {
        let v = ig_get(
            &agent,
            id,
            &[("fields", "id,username"), ("access_token", token)],
        )?;
        return Ok((id.to_string(), v["username"].as_str().map(str::to_string)));
    }
    let v = ig_get(
        &agent,
        "me/assigned_instagram_accounts",
        &[
            ("fields", "id,username"),
            ("limit", "100"),
            ("access_token", token),
        ],
    )?;
    let list: Vec<(String, Option<String>)> = v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some((id_of(&x["id"])?, x["username"].as_str().map(str::to_string)))
                })
                .collect()
        })
        .unwrap_or_default();
    match list.len() {
        0 => Err("no Instagram account assigned to this token".into()),
        1 => Ok(list.into_iter().next().unwrap()),
        _ => Err(format!(
            "several accounts, put `id token` in the environment: {}",
            list.iter()
                .map(|(id, u)| format!("{id} @{}", u.as_deref().unwrap_or("?")))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The region's account: the stored one, or the environment's token stored
/// first (with the account's id) when it is new there.
async fn instagram(s: &AppState, slug: &str) -> Result<Insta, String> {
    let env = instagram_env(slug).ok_or("Instagram is not set up for this region")?;
    let (id, token) = instagram_of(&env).ok_or("Instagram is not set up for this region")?;
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
    let tok = token.clone();
    let (user_id, username) = tokio::task::spawn_blocking(move || ig_account(id.as_deref(), &tok))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Instagram account: {e}"))?;
    let a = Insta {
        token,
        user_id,
        username,
    };
    let (st, sl) = (s.clone(), slug.to_string());
    let (tok, uid, un) = (a.token.clone(), a.user_id.clone(), a.username.clone());
    tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            // refreshed_at: last checked (`check_accounts`); long ago, so it checks soon.
            db.execute(
                "INSERT INTO game.post_accounts \
                   (region, channel, seed, token, user_id, username, refreshed_at) \
                 VALUES ($1, 'instagram', $2, $3, $4, $5, now() - interval '2 days') \
                 ON CONFLICT (region, channel) DO UPDATE SET seed = $2, token = $3, user_id = $4, \
                   username = $5, expires_at = NULL, refreshed_at = now() - interval '2 days', \
                   error = NULL",
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

/// Makes a media container and waits until Meta has fetched the picture;
/// its id. Nothing is published yet (an unused container expires in 24 h).
fn ig_container(
    agent: &ureq::Agent,
    a: &Insta,
    image_url: &str,
    caption: &str,
    alt: &str,
    place: Option<&str>,
) -> Result<String, String> {
    let tok = a.token.as_str();
    let mut form = vec![
        ("image_url", image_url),
        ("caption", caption),
        ("access_token", tok),
    ];
    if !alt.is_empty() {
        form.push(("alt_text", alt));
    }
    if let Some(p) = place {
        form.push(("location_id", p));
    }
    let c = ig_post(agent, &format!("{}/media", a.user_id), &form)
        .map_err(|e| format!("container: {e}"))?;
    let cid = id_of(&c["id"]).ok_or("container: no id")?;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_secs(3));
        let v = ig_get(
            agent,
            &cid,
            &[("fields", "status_code,status"), ("access_token", tok)],
        )
        .map_err(|e| format!("container status: {e}"))?;
        match v["status_code"].as_str() {
            Some("FINISHED") => return Ok(cid),
            Some("IN_PROGRESS") | None => {}
            Some(code) => {
                return Err(format!(
                    "container {code}: {}",
                    v["status"].as_str().unwrap_or("")
                ))
            }
        }
    }
    Err("container not ready after 2 minutes".into())
}

/// Publishes one picture with caption and alt text; the post's link, and a
/// note if Instagram refused the place (then it goes out without one).
fn send_instagram(
    a: &Insta,
    caption: &str,
    alt: &str,
    image_url: &str,
    place: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let agent = crate::media::agent().ok_or("no TLS")?;
    let tok = a.token.as_str();
    let mut note = None;
    let cid = match ig_container(&agent, a, image_url, caption, alt, place) {
        Ok(c) => c,
        Err(e) if place.is_some() => {
            tracing::warn!(error = %e, "posts: Instagram refused the place, sending without");
            note = Some(format!("place refused ({e}), sent without"));
            ig_container(&agent, a, image_url, caption, alt, None)?
        }
        Err(e) => return Err(e),
    };
    let p = ig_post(
        &agent,
        &format!("{}/media_publish", a.user_id),
        &[("creation_id", &cid), ("access_token", tok)],
    )
    .map_err(|e| format!("publish: {e}"))?;
    let mid = id_of(&p["id"]).ok_or("publish: no id")?;
    // Published; the link is nice to have.
    let link = ig_get(
        &agent,
        &mid,
        &[("fields", "permalink"), ("access_token", tok)],
    )
    .ok()
    .and_then(|v| v["permalink"].as_str().map(str::to_string));
    let link = link.unwrap_or_else(|| match &a.username {
        Some(u) => format!("https://www.instagram.com/{u}/"),
        None => "https://www.instagram.com/".into(),
    });
    Ok((link, note))
}

/// Checks the shard's Instagram tokens once a day (`debug_token`: valid,
/// expiry; a system user's token is not renewed); stores a new token from the
/// environment first. The editor warns from what is stored.
async fn check_accounts(s: &AppState) {
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
                         AND refreshed_at < now() - interval '1 day'",
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
        // Ok(expires: unix seconds, 0 never), Err(why not usable).
        let got = tokio::task::spawn_blocking(move || {
            let agent = crate::media::agent().ok_or("no TLS")?;
            let v = ig_get(
                &agent,
                "debug_token",
                &[("input_token", &tok), ("access_token", &tok)],
            )?;
            let d = &v["data"];
            if d["is_valid"].as_bool() != Some(true) {
                return Err(d["error"]["message"]
                    .as_str()
                    .unwrap_or("token not valid")
                    .to_string());
            }
            Ok::<_, String>(d["expires_at"].as_i64().unwrap_or(0))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        match &got {
            Ok(0) => tracing::info!(region = %slug, "posts: Instagram token valid, no expiry"),
            Ok(t) => tracing::info!(region = %slug, expires = t, "posts: Instagram token valid"),
            Err(e) => tracing::warn!(region = %slug, error = %e, "posts: Instagram token"),
        }
        let (st, sl) = (s.clone(), slug.clone());
        tokio::task::spawn_blocking(move || {
            st.with_db(|db| match got {
                Ok(t) => db.execute(
                    "UPDATE game.post_accounts SET refreshed_at = now(), error = NULL, \
                       expires_at = CASE WHEN $2 > 0 THEN to_timestamp($2) END \
                     WHERE region = $1 AND channel = 'instagram'",
                    &[&sl, &(t as f64)],
                ),
                Err(e) => db.execute(
                    "UPDATE game.post_accounts SET refreshed_at = now(), error = $2 \
                     WHERE region = $1 AND channel = 'instagram'",
                    &[&sl, &e],
                ),
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
        assert_eq!(instagram_of(" EAAx "), Some((None, "EAAx".into())));
        assert_eq!(
            instagram_of("1784 EAAx"),
            Some((Some("1784".into()), "EAAx".into()))
        );
        assert_eq!(instagram_of("abc EAAx"), None);
        assert_eq!(instagram_of(""), None);
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
        let f = fill(&t, Some("fire"), &vars, &[]).unwrap();
        assert_eq!(f["instagram"], "Glut 12:3");
        assert_eq!(f["alt"], "Plan");
        let w = fill(&t, Some("water"), &vars, &[]).unwrap();
        assert_eq!(w["mastodon"], "1. Advent vorbei");
        assert!(fill(&json!({ "fire": {} }), Some("water"), &vars, &[]).is_none());
        // One text for both channels, Mastodon its own; hashtags at the end.
        let t = clean_texts(&json!({ "any": { "text": "Stand {fire}", "mastodon": "M {water}" } }))
            .unwrap();
        let f = fill(&t, None, &vars, &["GlutUndFlut".into(), "Konstanz".into()]).unwrap();
        assert_eq!(f["instagram"], "Stand 12\n\n#GlutUndFlut #Konstanz");
        assert_eq!(f["mastodon"], "M 3\n\n#GlutUndFlut #Konstanz");
        assert!(f.get("alt").is_none());
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
            tags: vec!["#Konstanz".into(), "konstanz".into(), "GlutUndFlut".into()],
            location: None,
        };
        let (_, _, card, ch, tags) = clean_post(&ok).unwrap();
        assert_eq!(tags, vec!["Konstanz", "GlutUndFlut"]);
        assert_eq!(clean_tag("#Glut_2026"), Some("Glut_2026".into()));
        assert_eq!(clean_tag("2026"), None);
        assert_eq!(
            place_key(" 108229489200476 ").as_deref(),
            Some("108229489200476")
        );
        assert_eq!(
            place_key("https://www.facebook.com/profile.php?id=61550000000001").as_deref(),
            Some("61550000000001")
        );
        assert_eq!(
            place_key("https://de-de.facebook.com/pages/Konstanz/123456").as_deref(),
            Some("123456")
        );
        assert_eq!(
            place_key("https://www.facebook.com/konstanz.tourismus/").as_deref(),
            Some("konstanz.tourismus")
        );
        assert_eq!(place_key("https://example.com/x"), None);
        assert_eq!(
            clean_location(&Some(json!({ "id": "12a" }))),
            Err("place: the id is digits only")
        );
        assert_eq!(
            clean_location(&Some(json!({ "id": 123, "name": "Konstanz" })))
                .unwrap()
                .unwrap()["id"],
            "123"
        );
        assert_eq!(clean_tag("glut und flut"), None);

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
        // The hashtags count into Mastodon's 500.
        let long = PostIn {
            texts: json!({ "any": { "text": "x".repeat(495) } }),
            tags: vec!["Konstanz".into()],
            channels: vec!["mastodon".into()],
            ..too_long
        };
        assert!(clean_post(&long).unwrap_err().starts_with("Mastodon"));
    }
}

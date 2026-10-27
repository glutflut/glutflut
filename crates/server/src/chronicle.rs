//! The chronicle: what happened in a region's campaigns, public
//! (`GET /api/chronicle`, page `web/chronik.html`). Finished chapters with
//! their winner and the story's ending for it, the running one with its
//! progress, the next ones with their dates. Once all its chapters are over,
//! a campaign also carries its letters (finds with a text), so the story
//! stays readable; before that they would spoil the search. Nothing per player.

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use elements_shared::prelude::Faction;

use super::{AppState, RegionQ};

/// Chapters of a region's campaigns (published, not weather days), grouped
/// by campaign, newest campaign first, chapters in order.
pub(super) async fn chronicle(State(s): State<AppState>, Query(q): Query<RegionQ>) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || st.with_db(|db| load(db, &slug)))
        .await
        .ok()
        .flatten();
    match out {
        Some(v) => ([(header::CACHE_CONTROL, "public, max-age=120")], Json(v)).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

fn load(db: &mut postgres::Client, slug: &str) -> Result<Value, postgres::Error> {
    let city = db
        .query_opt(
            "SELECT name, coalesce(language, 'de') FROM geo.regions WHERE slug = $1",
            &[&slug],
        )?
        .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)));
    let now = crate::analytics::now_s();
    let rows = db.query(
        "SELECT ch.key, ch.campaign, ch.names::text, ch.story::text, extract(epoch FROM ch.starts)::float8, \
           extract(epoch FROM ch.ends)::float8, ch.goal, \
           CASE ch.goal WHEN 'hold' THEN greatest(1, cardinality(ch.nodes)) ELSE ch.target END, \
           CASE ch.goal WHEN 'hold' THEN greatest(1, cardinality(ch.nodes)) ELSE coalesce(ch.target_water, ch.target) END, \
           coalesce((SELECT value FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'fire'), 0), \
           coalesce((SELECT value FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'water'), 0), \
           (SELECT extract(epoch FROM reached_at)::float8 FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'fire'), \
           (SELECT extract(epoch FROM reached_at)::float8 FROM game.chapter_progress p WHERE p.region = ch.region AND p.key = ch.key AND p.faction = 'water'), \
           (SELECT min(starts) FROM game.chapters c2 WHERE c2.region = ch.region AND c2.campaign = ch.campaign AND c2.kind IS NULL AND c2.status = 'published') \
         FROM game.chapters ch WHERE ch.region = $1 AND ch.kind IS NULL AND ch.status = 'published' \
         ORDER BY 14 DESC, ch.campaign, ch.starts",
        &[&slug],
    )?;
    let letters = db.query(
        "SELECT id, campaign, name, text::text FROM geo.finds \
         WHERE region = $1 AND campaign IS NOT NULL AND text IS NOT NULL AND NOT hidden \
         ORDER BY starts NULLS FIRST, id",
        &[&slug],
    )?;
    let mut campaigns: Vec<Value> = Vec::new();
    for r in &rows {
        let campaign: String = r.get(1);
        let (starts, ends): (f64, f64) = (r.get(4), r.get(5));
        let targets = [i64::from(r.get::<_, i32>(7)), i64::from(r.get::<_, i32>(8))];
        let progress = [r.get::<_, i64>(9), r.get::<_, i64>(10)];
        let reached = [r.get::<_, Option<f64>>(11), r.get::<_, Option<f64>>(12)];
        let over = now >= ends;
        let winner = crate::campaign::decide(progress, reached, targets, over);
        let state = if over {
            "over"
        } else if now >= starts {
            "live"
        } else {
            "soon"
        };
        let chapter = json!({
            "key": r.get::<_, String>(0),
            "names": serde_json::from_str::<Value>(r.get(2)).unwrap_or(Value::Null),
            "story": r.get::<_, Option<String>>(3).and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null),
            "starts": starts,
            "ends": ends,
            "goal": r.get::<_, String>(6),
            "target": targets[0],
            "target_water": targets[1],
            "fire": progress[0],
            "water": progress[1],
            "state": state,
            "winner": winner.map(Faction::as_str),
        });
        match campaigns
            .last_mut()
            .filter(|c| c["campaign"].as_str() == Some(&campaign))
        {
            Some(c) => c["chapters"].as_array_mut().unwrap().push(chapter),
            None => campaigns.push(json!({ "campaign": campaign, "chapters": [chapter] })),
        }
    }
    for c in &mut campaigns {
        let over = c["chapters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|ch| ch["state"] == "over");
        if !over {
            continue;
        }
        let name = c["campaign"].as_str().unwrap_or_default().to_string();
        let list: Vec<Value> = letters
            .iter()
            .filter(|r| r.get::<_, String>(1) == name)
            .map(|r| {
                json!({
                    "id": r.get::<_, i64>(0),
                    "name": r.get::<_, String>(2),
                    "text": serde_json::from_str::<Value>(r.get(3)).unwrap_or(Value::Null),
                })
            })
            .collect();
        if !list.is_empty() {
            c["letters"] = Value::Array(list);
        }
    }
    Ok(json!({
        "city": city.as_ref().map(|c| c.0.clone()),
        "language": city.as_ref().map(|c| c.1.clone()),
        "campaigns": campaigns,
    }))
}

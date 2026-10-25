//! The world ranking: season points of the best players over all regions
//! and shards (`GET /api/world`, the globe page in the app, and
//! `glutflut/world.json` in the globe's bucket for the Weathermarble globe).
//! No new state: one query per shard (`scope=shard`), merged by the shard
//! asked and cached for a few minutes. People are the same across shards
//! by name and side (the passport carries the name); hidden players are
//! left out, as on the ranking page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// How long a merged list is served before it is built again (s).
const CACHE_S: f64 = 300.0;
/// Players in the merged list.
const TOP_N: usize = 50;
/// Players each shard hands over.
const SHARD_N: i64 = 100;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub faction: String,
    pub points: i64,
    /// Regions with points.
    pub cities: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Shard {
    pub shard: String,
    pub players: Vec<Entry>,
    /// Season points of each side over all regions.
    pub fire: i64,
    pub water: i64,
}

/// This shard's part: the best players over its regions, and the sides.
pub fn shard(c: &mut postgres::Client, shard: &str) -> Result<Shard, postgres::Error> {
    let players = c
        .query(
            "SELECT p.name, p.faction, sum(r.points_total)::int8, count(*)::int8 \
             FROM game.player_region r JOIN game.players p USING (id) \
             WHERE NOT p.hidden \
             GROUP BY p.id, p.name, p.faction HAVING sum(r.points_total) > 0 \
             ORDER BY 3 DESC, p.name LIMIT $1",
            &[&SHARD_N],
        )?
        .iter()
        .map(|r| Entry {
            name: r.get(0),
            faction: r.get(1),
            points: r.get(2),
            cities: r.get(3),
        })
        .collect();
    let mut sides = (0, 0);
    for r in c.query(
        "SELECT p.faction, sum(r.points_total)::int8 FROM game.player_region r \
         JOIN game.players p USING (id) GROUP BY p.faction",
        &[],
    )? {
        match r.get::<_, &str>(0) {
            "fire" => sides.0 = r.get(1),
            "water" => sides.1 = r.get(1),
            _ => {}
        }
    }
    Ok(Shard {
        shard: shard.to_string(),
        players,
        fire: sides.0,
        water: sides.1,
    })
}

/// Merge the shards' parts: one row per name and side, points and cities
/// added up, best first.
pub fn merge(parts: &[Shard], now: f64) -> serde_json::Value {
    let mut by: HashMap<(String, String), Entry> = HashMap::new();
    let (mut fire, mut water) = (0, 0);
    for p in parts {
        fire += p.fire;
        water += p.water;
        for e in &p.players {
            let k = (e.name.clone(), e.faction.clone());
            let m = by.entry(k).or_insert_with(|| Entry {
                points: 0,
                cities: 0,
                ..e.clone()
            });
            m.points += e.points;
            m.cities += e.cities;
        }
    }
    let mut players: Vec<Entry> = by.into_values().collect();
    players.sort_by(|a, b| b.points.cmp(&a.points).then_with(|| a.name.cmp(&b.name)));
    players.truncate(TOP_N);
    let mut shards: Vec<&str> = parts.iter().map(|p| p.shard.as_str()).collect();
    shards.sort_unstable();
    serde_json::json!({
        "format": "glutflut-world",
        "version": 1,
        "at": now as i64,
        "shards": shards,
        "players": players,
        "fire": fire,
        "water": water,
    })
}

/// The other shards' parts (`GET {url}/api/world?scope=shard`).
fn fetch_others() -> Vec<Shard> {
    let s = crate::shards::get();
    s.others()
        .into_iter()
        .filter_map(|name| {
            let url = s.urls.get(name)?;
            let res = crate::shards::agent()?
                .get(&format!("{url}/api/world?scope=shard"))
                .call()
                .map_err(|e| tracing::warn!(shard = name, error = %e, "world: shard list failed"))
                .ok()?;
            serde_json::from_reader(res.into_reader())
                .map_err(
                    |e| tracing::warn!(shard = name, error = %e, "world: shard list unreadable"),
                )
                .ok()
        })
        .collect()
}

/// The merged list, built at most every `CACHE_S` seconds (own part from
/// `db`, the others fetched).
pub fn merged(db: &str, own: &str, now: f64) -> Option<Arc<str>> {
    static CACHE: Mutex<Option<(f64, Arc<str>)>> = Mutex::new(None);
    if let Some((at, s)) = CACHE.lock().unwrap().as_ref() {
        if now - at < CACHE_S {
            return Some(s.clone());
        }
    }
    let mine = crate::db::connect(db)
        .ok()
        .and_then(|mut c| shard(&mut c, own).ok())?;
    let mut parts = fetch_others();
    parts.push(mine);
    let json: Arc<str> = merge(&parts, now).to_string().into();
    *CACHE.lock().unwrap() = Some((now, json.clone()));
    Some(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, f: &str, points: i64, cities: i64) -> Entry {
        Entry {
            name: name.into(),
            faction: f.into(),
            points,
            cities,
        }
    }

    #[test]
    fn shards_merge_by_name_and_side_best_first() {
        let eu = Shard {
            shard: "eu".into(),
            players: vec![e("Anna", "fire", 300, 2), e("Bo", "water", 500, 1)],
            fire: 300,
            water: 500,
        };
        let sa = Shard {
            shard: "sa".into(),
            players: vec![e("Anna", "fire", 250, 1), e("Anna", "water", 10, 1)],
            fire: 250,
            water: 10,
        };
        let v = merge(&[sa, eu], 1000.0);
        assert_eq!(v["shards"], serde_json::json!(["eu", "sa"]));
        assert_eq!(
            (v["fire"].as_i64(), v["water"].as_i64()),
            (Some(550), Some(510))
        );
        let p = v["players"].as_array().unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(
            (
                p[0]["name"].as_str(),
                p[0]["points"].as_i64(),
                p[0]["cities"].as_i64()
            ),
            (Some("Anna"), Some(550), Some(3))
        );
        assert_eq!(
            (p[1]["name"].as_str(), p[1]["points"].as_i64()),
            (Some("Bo"), Some(500))
        );
        assert_eq!(p[2]["faction"].as_str(), Some("water"));
    }
}

//! Found keys (`[keep] ember_*`, `bottle_*`): Funkenflug (fire, a claim on
//! a hilltop: a region nearby) and Flaschenpost (water, a
//! claim at a big river: a region downstream, `geo.river_regions`). The key
//! opens a permanent source of the player's side there (handlers/far.rs).

use std::collections::HashMap;

use bevy::prelude::Resource;

use crate::regions::Host;

/// The region's junctions at a big river and the regions downstream of each.
#[derive(Resource, Default)]
pub struct Rivers {
    /// Junction id -> river (index into `down`).
    at: HashMap<i64, usize>,
    /// Per river: the regions further downstream, nearest first.
    down: Vec<Vec<String>>,
}

impl Rivers {
    /// Regions downstream of the river at this junction, nearest first.
    pub fn downstream(&self, node: i64) -> &[String] {
        self.at.get(&node).map_or(&[], |&i| self.down[i].as_slice())
    }
}

pub fn load(c: &mut postgres::Client, slug: &str) -> Result<Rivers, postgres::Error> {
    let mut names: HashMap<String, usize> = HashMap::new();
    let mut down: Vec<Vec<String>> = Vec::new();
    for r in c.query(
        "SELECT x.river, d.region FROM geo.river_regions x \
         JOIN geo.river_regions d ON d.river = x.river AND d.pos > x.pos \
         WHERE x.region = $1 ORDER BY x.river, d.pos",
        &[&slug],
    )? {
        let river: String = r.get(0);
        let i = *names.entry(river).or_insert_with(|| {
            down.push(Vec::new());
            down.len() - 1
        });
        down[i].push(r.get(1));
    }
    let mut at = HashMap::new();
    for r in c.query(
        "SELECT nr.node_id, nr.river FROM geo.node_river nr JOIN geo.nodes n ON n.id = nr.node_id \
         JOIN geo.regions g ON g.id = n.region_id WHERE g.slug = $1 AND n.is_active",
        &[&slug],
    )? {
        if let Some(&i) = names.get(r.get::<_, &str>(1)) {
            at.insert(r.get::<_, i64>(0), i);
        }
    }
    Ok(Rivers { at, down })
}

/// Great-circle distance (km).
fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.sqrt().asin()
}

/// Regions of this server within `max_km` of (lat, lon), nearest first.
pub fn nearby(host: &Host, here: &str, lat: f64, lon: f64, max_km: f64) -> Vec<String> {
    let mut v: Vec<(f64, String)> = host
        .listings()
        .iter()
        .filter(|l| l.slug != here && host.here(l))
        .filter_map(|l| {
            let b = l.json.get("bounds")?.as_array()?;
            let f = |i: usize| b.get(i).and_then(|x| x.as_f64());
            let (clat, clon) = ((f(1)? + f(3)?) / 2.0, (f(0)? + f(2)?) / 2.0);
            let km = distance_km(lat, lon, clat, clon);
            (km >= 5.0 && km <= max_km).then(|| (km, l.slug.clone()))
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, s)| s).collect()
}

/// One of `nearest` (nearest first), the nearer the likelier (1, 1/2, 1/3, … of the first five).
pub fn pick(nearest: &[String]) -> Option<&String> {
    let n = nearest.len().min(5);
    let total: f32 = (1..=n).map(|i| 1.0 / i as f32).sum();
    let mut x = rand::random::<f32>() * total;
    for (i, s) in nearest.iter().take(n).enumerate() {
        x -= 1.0 / (i + 1) as f32;
        if x <= 0.0 {
            return Some(s);
        }
    }
    nearest.first()
}

/// The region's display name from the list.
pub fn name(host: &Host, slug: &str) -> String {
    host.listing(slug)
        .and_then(|l| {
            l.json
                .get("name")
                .and_then(|n| n.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| slug.to_string())
}

/// Write a key into the database for a sleeping region (it reads it on
/// waking): to a random permanent source of `faction` there. The source's
/// ticket hash, or `None` if the region has none of that side.
pub fn write_key(
    db: &str,
    region: &str,
    faction: &str,
    origin: &str,
    key: &[u8],
    uses: i32,
    days: f64,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let mut c = crate::db::connect(db)?;
    Ok(c.query_opt(
        "INSERT INTO game.keep_keys (key, region, keep, origin, uses, made, expires) \
         SELECT $1, $2, k.ticket, $4, $5::int4, now(), now() + make_interval(secs => $6::float8 * 86400) \
         FROM game.keeps k WHERE k.region = $2 AND k.faction = $3 ORDER BY random() LIMIT 1 \
         RETURNING keep",
        &[&key, &region, &faction, &origin, &uses, &days],
    )?
    .map(|r| r.get(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_konstanz_basel() {
        let km = distance_km(47.66, 9.17, 47.56, 7.59);
        assert!((100.0..130.0).contains(&km), "{km}");
    }

    #[test]
    fn pick_prefers_the_nearest() {
        let v: Vec<String> = (0..8).map(|i| i.to_string()).collect();
        let mut first = 0;
        for _ in 0..2000 {
            let p = pick(&v).unwrap();
            assert!(p.parse::<usize>().unwrap() < 5, "only the nearest five");
            first += usize::from(p == "0");
        }
        // 1 / (1 + 1/2 + 1/3 + 1/4 + 1/5) ≈ 44 %.
        assert!((700..1050).contains(&first), "{first}");
    }
}

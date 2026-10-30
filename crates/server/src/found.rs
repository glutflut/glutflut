//! Found keys (`[keep] ember_*`, `bottle_*`): Funkenflug (fire, a claim on
//! a hilltop on a windy day: a region downwind) and Flaschenpost (water, a
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

/// Great-circle distance (km) and initial bearing (degrees, 0 = north).
fn course(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> (f64, f64) {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    let km = 6371.0 * 2.0 * a.sqrt().asin();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    (km, (y.atan2(x).to_degrees() + 360.0) % 360.0)
}

/// Regions of this server downwind of (lat, lon): wind from `from_deg`,
/// within 40° of where it blows and `max_km`, nearest first.
pub fn downwind(
    host: &Host,
    here: &str,
    lat: f64,
    lon: f64,
    from_deg: f64,
    max_km: f64,
) -> Vec<String> {
    let to = (from_deg + 180.0) % 360.0;
    let mut v: Vec<(f64, String)> = host
        .listings()
        .iter()
        .filter(|l| l.slug != here && host.here(l))
        .filter_map(|l| {
            let b = l.json.get("bounds")?.as_array()?;
            let f = |i: usize| b.get(i).and_then(|x| x.as_f64());
            let (clat, clon) = ((f(1)? + f(3)?) / 2.0, (f(0)? + f(2)?) / 2.0);
            let (km, deg) = course(lat, lon, clat, clon);
            let off = ((deg - to + 540.0) % 360.0 - 180.0).abs();
            (km >= 5.0 && km <= max_km && off <= 40.0).then(|| (km, l.slug.clone()))
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, s)| s).collect()
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
         SELECT $1, $2, k.ticket, $4, $5, now(), now() + make_interval(secs => $6 * 86400) \
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
    fn course_konstanz_basel() {
        // Basel lies west of Konstanz, ~100 km.
        let (km, deg) = course(47.66, 9.17, 47.56, 7.59);
        assert!((100.0..130.0).contains(&km), "{km}");
        assert!((255.0..275.0).contains(&deg), "{deg}");
    }
}

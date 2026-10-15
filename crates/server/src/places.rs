//! Places a city sets in the editor:
//! zones (flows, Entdecken; focus places for measuring) with their own
//! radius, and hotspot candidates. Read when a region loads; after an edit
//! the gateway sends them to the loaded region (`FromWeb::Places`): zones
//! apply at once, hotspot sites from the next slot.

use bevy::prelude::*;
use elements_shared::geo::{project_enu, RegionInfo};

/// A zone as stored (`geo.zones`).
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneRow {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// `None`: the module's default radius.
    pub radius_m: Option<f32>,
    pub focus: bool,
}

/// A hotspot candidate as stored (`geo.hotspot_sites`), on a junction.
#[derive(Clone, Debug, PartialEq)]
pub struct SiteRow {
    pub node: i64,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

/// A planned hotspot (`game.hotspot_plans`): on during [starts, ends).
#[derive(Clone, Debug, PartialEq)]
pub struct PlanRow {
    pub id: i64,
    pub site: SiteRow,
    /// Unix seconds.
    pub starts: f64,
    pub ends: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Places {
    pub zones: Vec<ZoneRow>,
    pub sites: Vec<SiteRow>,
    /// Plans not over yet.
    pub plans: Vec<PlanRow>,
}

/// A zone in the region's ENU frame: id, name, position, radius.
pub type Zone = (String, String, Vec2, Option<f32>);

pub fn load(c: &mut postgres::Client, slug: &str) -> Result<Places, postgres::Error> {
    // Focus places first (Entdecken takes the first 64).
    let zones = c
        .query(
            "SELECT id, name, lat, lon, radius_m, focus FROM geo.zones WHERE region = $1 \
             ORDER BY focus DESC, id",
            &[&slug],
        )?
        .iter()
        .map(|r| ZoneRow {
            id: r.get(0),
            name: r.get(1),
            lat: r.get(2),
            lon: r.get(3),
            radius_m: r.get(4),
            focus: r.get(5),
        })
        .collect();
    let sites = c
        .query(
            "SELECT node_id, name, lat, lon FROM geo.hotspot_sites WHERE region = $1 ORDER BY id",
            &[&slug],
        )?
        .iter()
        .map(|r| SiteRow {
            node: r.get(0),
            name: r.get(1),
            lat: r.get(2),
            lon: r.get(3),
        })
        .collect();
    let plans = c
        .query(
            "SELECT id, node_id, name, lat, lon, extract(epoch FROM starts)::float8, \
             extract(epoch FROM ends)::float8 FROM game.hotspot_plans \
             WHERE region = $1 AND ends > now() ORDER BY starts",
            &[&slug],
        )?
        .iter()
        .map(|r| PlanRow {
            id: r.get(0),
            site: SiteRow {
                node: r.get(1),
                name: r.get(2),
                lat: r.get(3),
                lon: r.get(4),
            },
            starts: r.get(5),
            ends: r.get(6),
        })
        .collect();
    Ok(Places {
        zones,
        sites,
        plans,
    })
}

/// Zones in the region's frame.
pub fn project(zones: &[ZoneRow], region: &RegionInfo) -> Vec<Zone> {
    zones
        .iter()
        .map(|z| {
            let p = project_enu(z.lat, z.lon, region.centroid_lat, region.centroid_lon);
            (
                z.id.clone(),
                z.name.clone(),
                Vec2::new(p.x, p.z),
                z.radius_m,
            )
        })
        .collect()
}

/// Entdecken: names and positions.
pub fn for_challenges(zones: &[Zone]) -> Vec<(String, Vec2, Option<f32>)> {
    zones
        .iter()
        .map(|(_, n, p, r)| (n.clone(), *p, *r))
        .collect()
}

/// Focus places for visitors per hour: ids, positions, radius (default 400 m).
pub fn for_focus(rows: &[ZoneRow], region: &RegionInfo) -> Vec<(String, Vec2, f32)> {
    rows.iter()
        .filter(|z| z.focus)
        .map(|z| {
            let p = project_enu(z.lat, z.lon, region.centroid_lat, region.centroid_lon);
            (
                z.id.clone(),
                Vec2::new(p.x, p.z),
                z.radius_m.unwrap_or(crate::analytics::ZONE_R_M),
            )
        })
        .collect()
}

/// Flows: ids and positions.
pub fn for_flows(zones: &[Zone]) -> Vec<(String, Vec2, Option<f32>)> {
    zones
        .iter()
        .map(|(id, _, p, r)| (id.clone(), *p, *r))
        .collect()
}

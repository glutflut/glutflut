//! Static geospatial world — the region **road graph** (nodes + edges) loaded
//! from PostGIS (`geo.*`) into ECS by the server.
//!
//! These are the *geo* counterparts to [`crate::components`]: plain Bevy
//! components carrying WGS84 degrees (SRID 4326, exactly as stored in the
//! database's denormalized `lat`/`lon` columns). The server spawns them per
//! region; the web app gets the graph as tiles and GeoJSON from the gateway.
//!
//! [`project_enu`] maps lon/lat to local world-space meters (an ENU tangent
//! plane at the region centroid, [`RegionInfo::centroid_lat`]/`centroid_lon`).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A road-graph junction / control point (`geo.nodes`). `id` is the stable
/// `bigint` primary key from the database, reused as the in-game identity.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GeoNode {
    pub id: i64,
    /// WGS84 latitude / longitude in degrees (denormalized from `geom`).
    pub lat: f64,
    pub lon: f64,
    pub elevation_m: f64,
    /// Free text for now (`crossing`, `control_point`, …). Promoted to an enum
    /// once the simulation (S3) needs to branch on it.
    pub node_type: String,
    /// Runtime control marker as stored (`neutral`, `controlled`, `contested`).
    pub control_state: String,
    /// Surroundings: +1 fire-leaning .. -1 water-leaning (`geo.nodes.affinity`).
    pub affinity: f32,
}

/// A directed road segment (`geo.edges`) joining two [`GeoNode`]s by their `id`.
///
/// The full `LineString` geometry is intentionally omitted for S2 — a straight
/// line between the endpoint nodes is enough to render the graph (C3). The real
/// polyline can be carried later, when curved roads start to matter visually or
/// for snapping.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GeoEdge {
    pub id: i64,
    pub source: i64,
    pub target: i64,
    pub length_m: f64,
    pub road_type: Option<String>,
    pub one_way: bool,
    /// Directional element-spread costs from the terrain load (S3). Lower = the
    /// element spreads more easily that way (`forward` = `source` -> `target`);
    /// fire is cheaper uphill, water downhill. The server's diffusion reads these
    /// as conductances; they default to `length_m` when terrain hasn't been
    /// sampled.
    pub fire_cost_forward: f32,
    pub fire_cost_reverse: f32,
    pub water_cost_forward: f32,
    pub water_cost_reverse: f32,
}

/// Identity + georeference of the loaded region (`geo.regions`), stored as a
/// resource after the load. `centroid_lat`/`centroid_lon` are the origin for
/// C2's local ENU projection; the bounds frame the playable extent.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct RegionInfo {
    pub slug: String,
    pub name: String,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub bounds_min_lat: f64,
    pub bounds_min_lon: f64,
    pub bounds_max_lat: f64,
    pub bounds_max_lon: f64,
    /// Centre of the hotspot experiment ("lat, lon") and its radius, if set
    /// for the region (else the centroid and 700 m).
    pub hotspot_center: Option<(f64, f64)>,
    pub hotspot_area_m: Option<f32>,
    /// IANA time zone (`geo.regions.timezone`): days run in local time.
    pub timezone: Option<String>,
}

/// Plain (non-ECS) aggregate the server's loader returns before it spawns
/// entities — keeps the database layer from depending on a `World`. `region` is
/// `None` when no active region matched the requested slug.
#[derive(Clone, Debug, Default)]
pub struct RegionGraph {
    pub region: Option<RegionInfo>,
    pub nodes: Vec<GeoNode>,
    pub edges: Vec<GeoEdge>,
}

/// WGS84 semi-major axis (meters) — the sphere radius used for the local
/// tangent-plane projection. Over a city-sized region the spherical vs.
/// ellipsoidal difference is well under a meter, which is irrelevant here.
const EARTH_RADIUS_M: f64 = 6_378_137.0;

/// Project a WGS84 lat/lon to local world-space meters on an **ENU tangent
/// plane** at `(origin_lat, origin_lon)` — the geo→world step (C2).
///
/// Returns Bevy world coordinates with the map laid flat on the XZ plane:
/// `x = +east`, `z = -north` (so north points toward `-Z`, i.e. "up" on a
/// top-down camera whose up hint is `-Z`), `y = 0`. Equirectangular (small-angle
/// equidistant) projection: exact at the origin and within centimeters across a
/// kilometers-wide region, which is all the playable area needs.
pub fn project_enu(lat: f64, lon: f64, origin_lat: f64, origin_lon: f64) -> Vec3 {
    let east = EARTH_RADIUS_M * (lon - origin_lon).to_radians() * origin_lat.to_radians().cos();
    let north = EARTH_RADIUS_M * (lat - origin_lat).to_radians();
    Vec3::new(east as f32, 0.0, -north as f32)
}

/// Inverse of [`project_enu`]: local ENU metres back to `(lat, lon)`.
pub fn unproject_enu(p: Vec3, origin_lat: f64, origin_lon: f64) -> (f64, f64) {
    let lat = origin_lat + (-p.z as f64 / EARTH_RADIUS_M).to_degrees();
    let lon =
        origin_lon + (p.x as f64 / (EARTH_RADIUS_M * origin_lat.to_radians().cos())).to_degrees();
    (lat, lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unproject_inverts_project() {
        let (lat0, lon0) = (47.6635, 9.1755);
        let p = project_enu(47.6601, 9.1790, lat0, lon0);
        let (lat, lon) = unproject_enu(p, lat0, lon0);
        assert!((lat - 47.6601).abs() < 1e-6 && (lon - 9.1790).abs() < 1e-6);
    }

    #[test]
    fn origin_projects_to_zero() {
        let p = project_enu(47.6635, 9.1755, 47.6635, 9.1755);
        assert!(p.length() < 1e-3, "origin should map to ~0, got {p:?}");
    }

    #[test]
    fn north_is_negative_z_and_east_is_positive_x() {
        let (lat0, lon0) = (47.6635, 9.1755);
        // ~0.001° north ≈ 111 m; cos(47.66°)≈0.674 so ~0.001° east ≈ 75 m.
        let north = project_enu(lat0 + 0.001, lon0, lat0, lon0);
        let east = project_enu(lat0, lon0 + 0.001, lat0, lon0);
        assert!((north.z - -111.3).abs() < 1.0, "north.z = {}", north.z);
        assert!(north.x.abs() < 1e-3 && north.y.abs() < 1e-3);
        assert!((east.x - 75.0).abs() < 1.0, "east.x = {}", east.x);
        assert!(east.z.abs() < 1e-3 && east.y.abs() < 1e-3);
    }
}

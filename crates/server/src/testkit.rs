//! Small hand-built worlds for unit tests: nodes on a line running east from
//! the region centroid, edges with plain costs, and `setup_elements` run.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use elements_shared::prelude::*;
use elements_shared::rules::Rules;

use crate::sim::{
    element_tick, setup_elements, GameRules, NodeById, PendingEdges, ELEMENT_TICK_EVERY,
};

pub const LAT: f64 = 47.66;
pub const LON: f64 = 9.17;
const R: f64 = 6_378_137.0;

pub fn region() -> RegionInfo {
    RegionInfo {
        slug: "test".into(),
        name: "Test".into(),
        centroid_lat: LAT,
        centroid_lon: LON,
        bounds_min_lat: LAT - 0.01,
        bounds_min_lon: LON - 0.01,
        bounds_max_lat: LAT + 0.01,
        bounds_max_lon: LON + 0.01,
        hotspot_center: None,
        hotspot_area_m: None,
        timezone: None,
    }
}

/// A node `x` m east of the centroid.
pub fn node(id: i64, x: f64, elev: f64, kind: &str) -> GeoNode {
    GeoNode {
        id,
        lat: LAT,
        lon: LON + (x / (R * LAT.to_radians().cos())).to_degrees(),
        elevation_m: elev,
        node_type: kind.into(),
        control_state: "neutral".into(),
        affinity: 0.0,
    }
}

/// An edge with all element costs equal to its length.
pub fn edge(id: i64, a: i64, b: i64, len: f64) -> GeoEdge {
    let c = len as f32;
    GeoEdge {
        id,
        source: a,
        target: b,
        length_m: len,
        road_type: None,
        one_way: false,
        fire_cost_forward: c,
        fire_cost_reverse: c,
        water_cost_forward: c,
        water_cost_reverse: c,
    }
}

/// Nodes `1..=n` every `gap` m, flat, joined in a line; `kinds` marks some
/// (`(id, "fire_source")`).
pub fn line(n: i64, gap: f64, kinds: &[(i64, &str)]) -> (Vec<GeoNode>, Vec<GeoEdge>) {
    let nodes = (1..=n)
        .map(|id| {
            let k = kinds.iter().find(|k| k.0 == id).map_or("crossing", |k| k.1);
            node(id, (id - 1) as f64 * gap, 0.0, k)
        })
        .collect();
    let edges = (1..n).map(|id| edge(id, id, id + 1, gap)).collect();
    (nodes, edges)
}

/// A world with the nodes spawned and the element simulation set up.
pub fn world(rules: Rules, nodes: Vec<GeoNode>, edges: Vec<GeoEdge>) -> World {
    let mut w = World::new();
    w.insert_resource(GameRules(rules));
    w.insert_resource(region());
    w.insert_resource(PendingEdges(edges));
    w.insert_resource(Time::<()>::default());
    for n in nodes {
        w.spawn((n, ElementState::default()));
    }
    w.run_system_once(setup_elements).unwrap();
    w
}

/// Node entity by graph id.
pub fn ent(w: &World, id: i64) -> Entity {
    *w.resource::<NodeById>().get(&id).expect("node")
}

pub fn state(w: &World, id: i64) -> ElementState {
    *w.get::<ElementState>(ent(w, id)).unwrap()
}

/// World position `x` m east of the centroid.
pub fn at(x: f32) -> Vec3 {
    Vec3::new(x, 0.0, 0.0)
}

/// Let `secs` pass on the virtual clock for the next system run.
pub fn advance(w: &mut World, secs: f32) {
    w.resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(secs));
}

/// `k` element steps (each `ELEMENT_TICK_EVERY` frames).
pub fn steps(w: &mut World, k: u64) {
    let id = w.register_system(element_tick);
    for _ in 0..k * ELEMENT_TICK_EVERY {
        w.run_system(id).unwrap();
    }
}

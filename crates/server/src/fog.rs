//! Fog at the front (`[fog]`, shared `fog.rs`): every 10 s the clock runs
//! over the field; the fogged junctions go to every app when they change.

use bevy::prelude::*;
use elements_shared::fog::FogClock;
use elements_shared::geo::GeoNode;

use crate::analytics::now_s;
use crate::sim::{ElementSim, GameRules};

const EVERY_S: f64 = 10.0;

#[derive(Resource, Default)]
pub struct LiveFog {
    clock: FogClock,
    /// Fogged junction ids, sorted.
    pub ids: Vec<i64>,
    /// Changes whenever `ids` does.
    pub version: u32,
    next: f64,
}

pub fn tick(
    mut fog: ResMut<LiveFog>,
    rules: Res<GameRules>,
    sim: Res<ElementSim>,
    geo: Query<&GeoNode>,
) {
    let r = &rules.0.fog;
    let now = now_s();
    if now < fog.next {
        return;
    }
    let first = fog.next == 0.0;
    fog.next = now + EVERY_S;
    if !r.enabled {
        if !fog.ids.is_empty() {
            *fog = LiveFog {
                next: fog.next,
                version: fog.version.wrapping_add(1),
                ..Default::default()
            };
        }
        return;
    }
    let dt = if first { 0.0 } else { EVERY_S as f32 };
    if !fog.clock.step(r, sim.state(), dt) {
        return;
    }
    let nodes = sim.nodes();
    let mut ids: Vec<i64> = fog
        .clock
        .fogged()
        .filter_map(|i| geo.get(*nodes.get(i)?).ok().map(|g| g.id))
        .collect();
    ids.sort_unstable();
    if ids != fog.ids {
        fog.ids = ids;
        fog.version = fog.version.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use bevy::ecs::system::RunSystemOnce;
    use elements_shared::rules::Rules;

    #[test]
    fn fog_over_a_long_front() {
        let mut rules = Rules::default();
        rules.fog.enabled = true;
        rules.fog.after_s = 60.0;
        // Fire at 1, water at 5: they meet at 3.
        let (nodes, edges) = line(5, 50.0, &[(1, "fire_source"), (5, "water_source")]);
        let mut w = world(rules, nodes, edges);
        steps(&mut w, 50);
        w.insert_resource(LiveFog::default());
        for _ in 0..8 {
            w.run_system_once(tick).unwrap();
            w.resource_mut::<LiveFog>().next = 1.0;
        }
        let s = state(&w, 3);
        assert!(
            !w.resource::<LiveFog>().ids.is_empty(),
            "no fog; middle {:?}",
            (s.fire, s.water)
        );
    }
}

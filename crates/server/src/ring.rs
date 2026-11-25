//! Walked rings (`[ring]`, shared `shape.rs`): each connection's route; a
//! closed loop holds the junctions on and inside it for its side.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use elements_shared::elements::Faction;
use elements_shared::geo::unproject_enu;
use elements_shared::shape::{Loop, Walk};

use crate::analytics::now_s;

/// A ring in force.
pub struct Active {
    pub side: Faction,
    pub nodes: HashSet<i64>,
    /// Outline as [lon, lat].
    pub line: Vec<[f64; 2]>,
    pub until: f64,
}

/// What the app gets (`ServerMsg::Rings`).
#[derive(serde::Serialize, Clone, Debug, ts_rs::TS)]
#[ts(rename = "RingView")]
pub struct View {
    #[ts(as = "crate::protocol::Faction")]
    pub side: &'static str,
    pub line: Vec<[f64; 2]>,
    #[ts(type = "number")]
    pub until: i64,
}

#[derive(Resource, Default)]
pub struct Rings {
    walks: HashMap<u64, Walk>,
    pub active: Vec<Active>,
    /// Changes whenever `active` does.
    pub version: u32,
}

impl Rings {
    /// A fix of connection `conn` (local metres); a loop if it closed one.
    pub fn walk(
        &mut self,
        conn: u64,
        rules: &elements_shared::shape::Ring,
        p: Vec2,
        t: f64,
    ) -> Option<Loop> {
        self.walks.entry(conn).or_default().push(rules, p, t)
    }

    pub fn add(
        &mut self,
        side: Faction,
        nodes: HashSet<i64>,
        l: &Loop,
        origin: (f64, f64),
        until: f64,
    ) {
        let line = l
            .outline
            .iter()
            .map(|v| {
                let (lat, lon) = unproject_enu(Vec3::new(v.x, 0.0, v.y), origin.0, origin.1);
                [lon, lat]
            })
            .collect();
        self.active.push(Active {
            side,
            nodes,
            line,
            until,
        });
        self.version = self.version.wrapping_add(1);
    }

    /// Strength factor for a claim of `f` at `node`: `weaken` inside another side's ring.
    pub fn factor(&self, node: i64, f: Faction, now: f64, weaken: f32) -> f32 {
        let hit = self
            .active
            .iter()
            .any(|r| r.side != f && r.until > now && r.nodes.contains(&node));
        if hit {
            weaken
        } else {
            1.0
        }
    }

    pub fn views(&self) -> Vec<View> {
        self.active
            .iter()
            .map(|r| View {
                side: r.side.as_str(),
                line: r.line.clone(),
                until: r.until as i64,
            })
            .collect()
    }
}

/// Every 5 s: rings run out, idle routes are dropped.
pub fn tick(mut rings: ResMut<Rings>, rules: Res<crate::sim::GameRules>, mut next: Local<f64>) {
    let now = now_s();
    if now < *next {
        return;
    }
    *next = now + 5.0;
    let before = rings.active.len();
    rings.active.retain(|r| r.until > now);
    if rings.active.len() != before {
        rings.version = rings.version.wrapping_add(1);
    }
    let max = rules.0.ring.max_s;
    rings.walks.retain(|_, w| !w.idle(now, max));
}

#[cfg(test)]
mod tests {
    use super::*;
    use elements_shared::shape::Ring;

    #[test]
    fn a_closed_ring_weakens_the_other_side_inside() {
        let rules = Ring::default();
        let mut rings = Rings::default();
        let mut t = 0.0;
        let mut closed = None;
        // Round an 80 m block, then a step past the start.
        let corners = [
            (0.0, 0.0),
            (80.0, 0.0),
            (80.0, 80.0),
            (0.0, 80.0),
            (0.0, -10.0),
        ];
        for w in corners.windows(2) {
            let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
            for k in 0..8 {
                t += 10.0;
                let p =
                    a.lerp(b, k as f32 / 8.0) + Vec2::new(5.0, 0.0) * (w[1].1 < 0.0) as u8 as f32;
                closed = rings.walk(7, &rules, p, t).or(closed);
            }
        }
        closed = rings
            .walk(7, &rules, Vec2::new(5.0, -10.0), t + 10.0)
            .or(closed);
        let l = closed.expect("ring");
        rings.add(Faction::Fire, [1, 2].into(), &l, (47.66, 9.17), t + 1800.0);
        assert_eq!(rings.factor(1, Faction::Water, t, 0.5), 0.5);
        assert_eq!(rings.factor(1, Faction::Fire, t, 0.5), 1.0);
        assert_eq!(rings.factor(3, Faction::Water, t, 0.5), 1.0);
        assert_eq!(rings.factor(1, Faction::Water, t + 1801.0, 0.5), 1.0);
        assert_eq!(rings.views()[0].side, "fire");
    }
}

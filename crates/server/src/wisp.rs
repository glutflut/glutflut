//! The will-o'-the-wisp (`[wisp]`, vault `wilde-ideen.md`): one light per
//! region wanders the streets from the hotspot. Claims nearby feed it towards
//! their side; leaning, it is a source of that side (`ElementSim::extra`) and
//! walks into that side's ground. Herding instead of fighting.

use bevy::prelude::*;
use elements_shared::elements::{ElementState, Faction};
use elements_shared::geo::{project_enu, GeoNode, RegionInfo};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::analytics::now_s;
use crate::sim::{ElementSim, GameRules};

/// Below this lean it belongs to nobody (no source, no pull).
const NEUTRAL: f32 = 0.15;

/// What the app gets (`ServerMsg::Wisp`).
#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, ts_rs::TS)]
#[ts(rename = "WispShown")]
pub struct Shown {
    pub lon: f64,
    pub lat: f64,
    /// -1 water .. +1 fire.
    pub lean: f32,
}

#[derive(Resource)]
pub struct Wisp {
    idx: Option<usize>,
    prev: Option<usize>,
    home: Option<usize>,
    pub lean: f32,
    pub shown: Option<Shown>,
    /// Changes whenever `shown` does.
    pub version: u32,
    next: f64,
    rng: StdRng,
}

impl Default for Wisp {
    fn default() -> Self {
        Self {
            idx: None,
            prev: None,
            home: None,
            lean: 0.0,
            shown: None,
            version: 0,
            next: 0.0,
            rng: StdRng::from_os_rng(),
        }
    }
}

impl Wisp {
    /// A claim of `f` at `at` (lat, lon): within `feed_m` it feeds the wisp.
    pub fn feed(&mut self, rules: &elements_shared::rules::Wisp, f: Faction, at: (f64, f64)) {
        let Some(s) = self.shown else { return };
        let p = project_enu(at.0, at.1, s.lat, s.lon);
        if Vec2::new(p.x, p.z).length() > rules.feed_m {
            return;
        }
        let sign = if f == Faction::Fire { 1.0 } else { -1.0 };
        self.lean = (self.lean + sign * rules.feed).clamp(-1.0, 1.0);
        self.shown = Some(Shown {
            lean: self.lean,
            ..s
        });
        self.version = self.version.wrapping_add(1);
    }

    /// The side it leans to, if any.
    fn side(&self) -> Option<Faction> {
        if self.lean >= NEUTRAL {
            Some(Faction::Fire)
        } else if self.lean <= -NEUTRAL {
            Some(Faction::Water)
        } else {
            None
        }
    }
}

/// Every `step_s`: one junction on; the lean fades; the field gets its source.
pub fn tick(
    mut wisp: ResMut<Wisp>,
    rules: Res<GameRules>,
    mut sim: ResMut<ElementSim>,
    region: Option<Res<RegionInfo>>,
    geo: Query<(&GeoNode, &ElementState)>,
) {
    let r = &rules.0.wisp;
    let now = now_s();
    if !r.enabled {
        if wisp.idx.is_some() {
            *wisp = Wisp::default();
            sim.extra.clear();
            wisp.version = 1;
        }
        return;
    }
    if now < wisp.next {
        return;
    }
    wisp.next = now + f64::from(r.step_s);
    let Some(region) = region else { return };
    let nodes = sim.nodes().to_vec();
    if nodes.is_empty() {
        return;
    }
    let origin = (region.centroid_lat, region.centroid_lon);
    let pos = |i: usize| -> Option<Vec2> {
        let (g, _) = geo.get(nodes[i]).ok()?;
        let p = project_enu(g.lat, g.lon, origin.0, origin.1);
        Some(Vec2::new(p.x, p.z))
    };
    // Home: the junction nearest the hotspot (or the centre).
    if wisp.home.is_none() {
        let (lat, lon) = region.hotspot_center.unwrap_or(origin);
        let h = project_enu(lat, lon, origin.0, origin.1);
        let h = Vec2::new(h.x, h.z);
        wisp.home = (0..nodes.len())
            .filter_map(|i| Some((i, pos(i)?.distance(h))))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        wisp.idx = wisp.home;
    }
    let (Some(at), Some(home)) = (wisp.idx, wisp.home) else {
        return;
    };
    let (Some(here), Some(home_p)) = (pos(at), pos(home)) else {
        return;
    };
    // On: never straight back; into the leaning side's ground; home when far.
    let side = wisp.side();
    let far = here.distance(home_p) > r.roam_m;
    let prev = wisp.prev;
    let choices: Vec<(usize, f32)> = sim
        .neighbour_idx(at)
        .into_iter()
        .filter(|&j| Some(j) != prev)
        .filter_map(|j| {
            let p = pos(j)?;
            let (_, st) = geo.get(nodes[j]).ok()?;
            let mut w = 1.0;
            if side.is_some() && st.owner() == side {
                w *= 3.0;
            }
            if far && p.distance(home_p) < here.distance(home_p) {
                w *= 6.0;
            }
            Some((j, w))
        })
        .collect();
    let next = if choices.is_empty() {
        prev
    } else {
        let total: f32 = choices.iter().map(|c| c.1).sum();
        let mut x = wisp.rng.random::<f32>() * total;
        choices
            .iter()
            .find(|c| {
                x -= c.1;
                x <= 0.0
            })
            .or(choices.last())
            .map(|c| c.0)
    };
    if let Some(n) = next {
        wisp.prev = Some(at);
        wisp.idx = Some(n);
    }
    let fade = r.fade;
    wisp.lean -= wisp.lean.signum() * fade.min(wisp.lean.abs());
    let i = wisp.idx.unwrap_or(at);
    sim.extra = match wisp.side() {
        Some(f) => vec![(i, f, r.strength * wisp.lean.abs())],
        None => Vec::new(),
    };
    if let Ok((g, _)) = geo.get(nodes[i]) {
        wisp.shown = Some(Shown {
            lon: g.lon,
            lat: g.lat,
            lean: wisp.lean,
        });
        wisp.version = wisp.version.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use bevy::ecs::system::RunSystemOnce;
    use elements_shared::rules::Rules;

    #[test]
    fn wanders_and_follows_its_feeders() {
        let mut rules = Rules::default();
        rules.wisp.enabled = true;
        let (nodes, edges) = line(9, 40.0, &[]);
        let mut w = world(rules.clone(), nodes, edges);
        w.insert_resource(Wisp::default());
        w.run_system_once(tick).unwrap();
        let start = w.resource::<Wisp>().shown.expect("placed");
        // Fed by fire right where it stands: it leans to fire and is a fire source.
        for _ in 0..3 {
            w.resource_mut::<Wisp>()
                .feed(&rules.wisp, Faction::Fire, (start.lat, start.lon));
        }
        assert!((w.resource::<Wisp>().lean - 0.6).abs() < 1e-5);
        w.resource_mut::<Wisp>().next = 0.0;
        w.run_system_once(tick).unwrap();
        let moved = w.resource::<Wisp>().shown.unwrap();
        assert_ne!(
            (moved.lon, moved.lat),
            (start.lon, start.lat),
            "it walked on"
        );
        let extra = &w.resource::<ElementSim>().extra;
        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0].1, Faction::Fire);
        // Water far away doesn't feed it.
        w.resource_mut::<Wisp>()
            .feed(&rules.wisp, Faction::Water, (start.lat + 0.01, start.lon));
        assert!(w.resource::<Wisp>().lean > 0.5);
    }
}

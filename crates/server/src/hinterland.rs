//! The hinterland: supply distances from the anchors
//! to the sources at the front, and sparks near players who have nothing to
//! claim close by.

use std::collections::HashMap;

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::analytics::now_s;
use crate::sim::{ElemIdx, ElementSim, ElementSource, GameRules, SourceKind};

/// Street distance to each side's nearest anchor per dense node (fire, water).
#[derive(Resource, Default)]
pub struct Supply {
    dist: [Vec<f32>; 2],
}

impl Supply {
    pub fn at(&self, f: Faction, idx: Option<usize>) -> f32 {
        idx.and_then(|i| self.dist[f.index()].get(i).copied())
            .unwrap_or(f32::INFINITY)
    }
}

/// A spark just appeared for a player (sent by `web::broadcast`).
#[derive(Resource, Default)]
pub struct SparkNews(pub Vec<(Entity, Spark)>);

#[derive(Clone, Copy, Debug)]
pub struct Spark {
    pub node: i64,
    pub lat: f64,
    pub lon: f64,
    pub m: u32,
}

const CELL_M: f32 = 100.0;

/// Node positions (local metres) in 100 m cells, for radius queries.
pub struct Grid {
    pos: Vec<Vec2>,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl Grid {
    fn new(pos: Vec<Vec2>) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, p) in pos.iter().enumerate() {
            cells.entry(Self::key(*p)).or_default().push(i as u32);
        }
        Self { pos, cells }
    }

    fn key(p: Vec2) -> (i32, i32) {
        ((p.x / CELL_M).floor() as i32, (p.y / CELL_M).floor() as i32)
    }

    /// Nodes within `r` of `p`, with their distance.
    fn within(&self, p: Vec2, r: f32) -> impl Iterator<Item = (usize, f32)> + '_ {
        let (cx, cy) = Self::key(p);
        let k = (r / CELL_M).ceil() as i32;
        (cx - k..=cx + k)
            .flat_map(move |x| (cy - k..=cy + k).map(move |y| (x, y)))
            .filter_map(|c| self.cells.get(&c))
            .flatten()
            .map(move |&i| (i as usize, self.pos[i as usize].distance(p)))
            .filter(move |&(_, d)| d <= r)
    }
}

const SUPPLY_EVERY_S: f64 = 5.0;
/// `Sparks::due` assumes this.
const SPARK_EVERY_S: f64 = 10.0;

/// Every 5 s: supply distances and the ageing factor of claimed sources.
/// Every 10 s: sparks near idle players (rule sets with `[sparks]`).
#[allow(clippy::too_many_arguments)]
pub fn tick(
    mut commands: Commands,
    rules: Res<GameRules>,
    sim: Res<ElementSim>,
    mut supply: ResMut<Supply>,
    mut news: ResMut<SparkNews>,
    mut sources: Query<(&ElemIdx, &mut ElementSource)>,
    geo: Query<&GeoNode>,
    players: Query<(Entity, &Faction, &Position), With<Player>>,
    region: Option<Res<RegionInfo>>,
    tide: Option<Res<crate::tide::LiveTide>>,
    mut grid: Local<Option<Grid>>,
    mut next: Local<(f64, f64)>,
) {
    let rules = &rules.0;
    let now = now_s();
    let owner = |i: usize| {
        let (fire, water) = sim.state()[i];
        ElementState { fire, water }.owner()
    };
    if rules.supply.enabled && now >= next.0 {
        next.0 = now + SUPPLY_EVERY_S;
        if let Some(streets) = sim.streets() {
            for f in [Faction::Fire, Faction::Water] {
                let seeds: Vec<usize> = sources
                    .iter()
                    .filter(|(_, s)| s.supplies(f, &rules.supply))
                    .map(|(i, _)| i.0)
                    .collect();
                supply.dist[f.index()] =
                    streets.reach(seeds, |i| owner(i) == Some(f), rules.supply.range_m);
            }
            for (i, mut s) in &mut sources {
                if s.supplied() {
                    let v = rules.supply.speed(supply.at(s.faction, Some(i.0)));
                    if s.supply != v {
                        s.supply = v;
                    }
                }
            }
        }
    }
    let sp = &rules.sparks;
    if !sp.enabled || now < next.1 {
        return;
    }
    next.1 = now + SPARK_EVERY_S;
    let Some(region) = region else { return };
    let grid = grid.get_or_insert_with(|| {
        let pos = sim
            .nodes()
            .iter()
            .map(|&e| {
                geo.get(e).map_or(Vec2::splat(f32::MAX), |g| {
                    let w = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
                    Vec2::new(w.x, w.z)
                })
            })
            .collect();
        Grid::new(pos)
    });
    let (mut fire_n, mut water_n) = (0.0f32, 0.0f32);
    for i in 0..sim.nodes().len() {
        match owner(i) {
            Some(Faction::Fire) => fire_n += 1.0,
            Some(Faction::Water) => water_n += 1.0,
            None => {}
        }
    }
    let mut against = [0u32; 2];
    for (_, s) in &sources {
        if s.kind == SourceKind::Spark {
            against[s.faction.other().index()] += 1;
        }
    }
    for (e, &f, pos) in &players {
        if pos.0 == Vec3::ZERO {
            continue; // no fix yet
        }
        let p = Vec2::new(pos.0.x, pos.0.z);
        let mine = if f == Faction::Fire { fire_n } else { water_n };
        let busy = || grid.within(p, sp.idle_m).any(|(i, _)| owner(i) != Some(f));
        if !sp.due(
            mine,
            fire_n + water_n,
            against[f.index()],
            busy,
            rand::random,
        ) {
            continue;
        }
        let spots: Vec<(usize, f32)> = grid
            .within(p, sp.max_m)
            .filter(|&(i, d)| {
                d >= sp.min_m
                    && owner(i) == Some(f)
                    && sources.get(sim.nodes()[i]).is_err()
                    // Never at the waterline (`tide.rs`).
                    && !tide.as_ref().is_some_and(|t| t.shore(i))
            })
            .collect();
        if spots.is_empty() {
            continue;
        }
        let (i, d) = spots[rand::random_range(0..spots.len())];
        let node = sim.nodes()[i];
        let Ok(g) = geo.get(node) else { continue };
        commands
            .entity(node)
            .insert(ElementSource::spark(f.other(), sp));
        against[f.index()] += 1;
        debug!(node = g.id, m = d as u32, faction = ?f.other(), "spark");
        news.0.push((
            e,
            Spark {
                node: g.id,
                lat: g.lat,
                lon: g.lon,
                m: d as u32,
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_finds_nodes_within_radius() {
        let g = Grid::new(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(150.0, 0.0),
            Vec2::new(-250.0, 30.0),
        ]);
        let mut near: Vec<usize> = g.within(Vec2::ZERO, 200.0).map(|x| x.0).collect();
        near.sort();
        assert_eq!(near, vec![0, 1]);
        assert_eq!(g.within(Vec2::new(-260.0, 0.0), 50.0).count(), 1);
    }
}

#[cfg(test)]
mod world_tests {
    use super::*;
    use crate::testkit::*;
    use elements_shared::rules::Rules;

    fn sys(w: &mut World) -> bevy::ecs::system::SystemId {
        w.insert_resource(Supply::default());
        w.insert_resource(SparkNews::default());
        w.register_system(tick)
    }

    #[test]
    fn supply_runs_through_own_ground_from_anchors() {
        let mut r = Rules::default();
        r.supply.enabled = true;
        r.supply.range_m = 250.0;
        // 100 m hops: fire owns 1..=3, water 4..=6
        let (n, e) = line(6, 100.0, &[(1, "fire_source"), (6, "water_source")]);
        let mut w = world(r.clone(), n, e);
        steps(&mut w, 20);
        let owners: Vec<_> = (1..=6).map(|id| state(&w, id).owner()).collect();
        assert_eq!(owners[..3], [Some(Faction::Fire); 3]);
        assert_eq!(owners[3..], [Some(Faction::Water); 3]);
        let claim =
            |kind| ElementSource::claim(&r, Faction::Fire, kind, 100.0, None, 1.0, false, 0.0);
        let (a, front, deep) = (ent(&w, 2), ent(&w, 3), ent(&w, 5));
        w.entity_mut(a).insert(claim(SourceKind::Anchor));
        w.entity_mut(front).insert(claim(SourceKind::Claim));
        w.entity_mut(deep).insert(claim(SourceKind::Claim));
        let id = sys(&mut w);
        w.run_system(id).unwrap();
        let s = w.resource::<Supply>();
        let d = |id: i64| s.at(Faction::Fire, Some(id as usize - 1));
        assert_eq!((d(1), d(2), d(3)), (100.0, 0.0, 100.0));
        // one step out of own ground still arrives, no farther
        assert_eq!(d(4), 200.0);
        assert_eq!(d(5), f32::INFINITY);
        // natural sources don't count here: no water supply at all
        assert_eq!(s.at(Faction::Water, Some(5)), f32::INFINITY);
        assert_eq!(s.at(Faction::Fire, None), f32::INFINITY);
        let speed = |e| w.get::<ElementSource>(e).unwrap().supply;
        assert_eq!(speed(front), r.supply.supplied);
        assert_eq!(speed(deep), r.supply.cut);
        // anchors age normally
        assert_eq!(speed(a), 1.0);
    }

    /// Fire owns nodes 1..=7 (50 m apart), water only far-off node 8.
    fn fire_land(r: &Rules) -> World {
        let (mut n, mut e) = line(7, 50.0, &[(1, "fire_source")]);
        n.push(node(8, 5000.0, 0.0, "water_source"));
        e.push(edge(7, 7, 8, 5000.0));
        let mut w = world(r.clone(), n, e);
        steps(&mut w, 20);
        for id in 1..=7 {
            assert_eq!(state(&w, id).owner(), Some(Faction::Fire), "node {id}");
        }
        w
    }

    fn spark_rules() -> Rules {
        let mut r = Rules::default();
        r.sparks.enabled = true;
        r.sparks.per_min = 6.0; // the roll always passes
        r
    }

    fn sparks(w: &mut World) -> Vec<i64> {
        let mut q = w.query::<(&GeoNode, &ElementSource)>();
        let mut v: Vec<i64> = q
            .iter(w)
            .filter(|(_, s)| s.kind == SourceKind::Spark)
            .map(|(g, _)| g.id)
            .collect();
        v.sort();
        v
    }

    #[test]
    fn spark_appears_near_an_idle_player_once_per_round() {
        let r = spark_rules();
        let mut w = fire_land(&r);
        let p = w.spawn((Player, Faction::Fire, Position(at(1.0)))).id();
        let id = sys(&mut w);
        w.run_system(id).unwrap();
        let got = sparks(&mut w);
        assert_eq!(got.len(), 1);
        // 120..300 m away, on own ground without a source: nodes 4..=7
        assert!((4..=7).contains(&got[0]), "{got:?}");
        let s = *w.get::<ElementSource>(ent(&w, got[0])).unwrap();
        assert_eq!((s.faction, s.strength), (Faction::Water, r.sparks.start));
        let news = &w.resource::<SparkNews>().0;
        assert_eq!(news.len(), 1);
        assert_eq!((news[0].0, news[0].1.node), (p, got[0]));
        assert!((120..=300).contains(&news[0].1.m));
        // next round only after 10 s
        w.run_system(id).unwrap();
        assert_eq!(sparks(&mut w).len(), 1);
    }

    #[test]
    fn no_spark_with_enemy_ground_near_or_without_a_fix() {
        let r = spark_rules();
        let mut w = fire_land(&r);
        // a water player here has fire all around to claim
        w.spawn((Player, Faction::Water, Position(at(1.0))));
        w.spawn((Player, Faction::Fire, Position(Vec3::ZERO)));
        let id = sys(&mut w);
        w.run_system(id).unwrap();
        assert!(sparks(&mut w).is_empty());
        assert!(w.resource::<SparkNews>().0.is_empty());
    }
}

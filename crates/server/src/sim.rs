//! Server-authoritative element simulation (S3): fire and water spread across
//! the region road graph, biased by the terrain-derived directional edge costs,
//! and the resulting per-node [`ElementState`] goes to the web clients.
//!
//! Model — a **max-reach (bottleneck) field** stepped at a coarse element-tick
//! rate (the realtime-arch doc's `element_tick`, ~2 Hz):
//!
//! - Each node holds `fire` and `water` intensities in `[0, 1]`.
//! - Each step a node takes the strongest signal arriving from a neighbour:
//!   `intensity[i] = max_j(intensity[j] * t(j -> i))`, with transmission
//!   `t = REACH_SCALE / (REACH_SCALE + cost)` (high on cheap edges). Because the
//!   costs are directional (fire cheap uphill, water downhill), fire reaches a
//!   node strongly only via *uphill* edges and water via *downhill* ones — so
//!   fire climbs and water descends. The field is monotone and bounded, so it
//!   converges (no oscillation/blow-up); per-hop attenuation `< 1` limits reach.
//! - Nodes carrying [`ElementSource`] are pinned to full intensity each step:
//!   elevation seeds at startup (fire low, water high) plus nodes claimed by
//!   player attacks (S5). Ownership is derived, not stored: see
//!   [`ElementState::owner`].

use std::collections::HashMap;

use bevy::prelude::*;

use elements_shared::field::{EdgeCosts, Field, Quiet, NO_ORIGIN};
use elements_shared::hinterland::Streets;
use elements_shared::prelude::*;
use elements_shared::rules::Rules;
pub use elements_shared::source::{Kind as SourceKind, Source as ElementSource};

/// Element steps per second target: `FixedUpdate` is 64 Hz, so step every 32.
pub(crate) const ELEMENT_TICK_EVERY: u64 = 32;

/// Active game rules (`ELEMENTS_RULES` TOML, else defaults).
#[derive(Resource, Clone)]
pub struct GameRules(pub Rules);

/// Dense per-node index into [`ElementSim`]'s arrays.
#[derive(Component, Clone, Copy)]
pub struct ElemIdx(pub usize);

/// A node's natural source, kept so it comes back after a claim on it expires.
#[derive(Component, Clone, Copy)]
pub struct NaturalSource(pub ElementSource);

/// A natural source at a fountain (`spring_source`): water, weaker than the
/// shore (`Rules::natural_spring_strength`).
#[derive(Component, Clone, Copy)]
pub struct Spring;

/// Age claimed sources (per element and surroundings), weaken them along the
/// rules' decay curve, and drop them at the end of their lifetime; the node
/// then follows its neighbours, or gets its natural source back.
pub fn expire_sources(
    mut commands: Commands,
    time: Res<Time>,
    rules: Res<GameRules>,
    blocks: Option<Res<crate::area::Blocks>>,
    live: Option<Res<crate::live_rain::LiveRain>>,
    sun: Option<Res<crate::live_rain::LiveSun>>,
    tide: Option<Res<crate::tide::LiveTide>>,
    mut sources: Query<(
        Entity,
        &mut ElementSource,
        &GeoNode,
        Option<&NaturalSource>,
        Option<&crate::keep::KeepInfo>,
        Option<&ElemIdx>,
    )>,
) {
    let dt = time.delta_secs();
    let rules = &rules.0;
    for (e, mut src, geo, natural, keep, idx) in &mut sources {
        if src.life.is_none() {
            continue;
        }
        let slow = match (idx, blocks.as_deref()) {
            (Some(i), Some(b)) => b.slow_at(i.0),
            _ => 1.0,
        };
        // Inside a ring of the other side's players (`siege.rs`).
        let slow = slow * siege_factor(rules, &src);
        // Rain here now (`live_rain.rs`): fire ages faster, water slower.
        let rain = match (&live, idx) {
            (Some(l), Some(i)) => l.at(i.0),
            _ => 0.0,
        };
        let shine = match (&sun, idx) {
            (Some(l), Some(i)) => l.at(i.0),
            _ => 0.0,
        };
        // Tide at the waterfront (`tide.rs`): high water, fire ages faster.
        let flood = match (&tide, idx) {
            (Some(t), Some(i)) => t.at(i.0),
            _ => 0.0,
        };
        let slow = slow
            * rules.weather.live_age(src.faction, rain)
            * rules.weather.sun_age(src.faction, shine)
            * rules.tide.age(src.faction, flood);
        src.tick_slowed(rules, dt, geo.affinity, slow);
        // Permanent sources go without a top-up for long (lost tickets).
        if src.kind == SourceKind::Keep && src.built >= rules.keep.expire_s {
            commands.entity(e).remove::<ElementSource>();
            continue;
        }
        match src.current(rules, keep.map_or(0.0, |k| k.wear)) {
            Some(s) => src.strength = s,
            None => match natural {
                Some(n) => *src = n.0,
                None => {
                    commands.entity(e).remove::<ElementSource>();
                }
            },
        }
    }
}

/// Ageing factor of a source in a ring (`[siege]`; 1 outside one).
pub fn siege_factor(rules: &elements_shared::rules::Rules, s: &ElementSource) -> f32 {
    match (s.sieged, s.kind) {
        (false, _) => 1.0,
        (true, SourceKind::Keep) => rules.siege.keep_factor,
        (true, _) => rules.siege.factor,
    }
}

/// Edges not spawned as entities: kept only until
/// [`setup_elements`] has built the field from them.
#[derive(Resource, Default)]
pub struct PendingEdges(pub Vec<GeoEdge>);

/// Node entity by stable graph id — lets the attack handler resolve a target.
/// Sorted ids and the entities in the same (dense sim) order: no hash map for
/// a million nodes.
#[derive(Resource, Default)]
pub struct NodeById {
    ids: Vec<i64>,
    ents: Vec<Entity>,
}

impl NodeById {
    pub fn index(&self, id: i64) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }

    pub fn get(&self, id: &i64) -> Option<&Entity> {
        self.index(*id).map(|i| &self.ents[i])
    }
}

/// Precomputed reach graph. Built once in [`setup_elements`].
#[derive(Resource, Default)]
pub struct ElementSim {
    /// Node entities in dense-index order.
    nodes: Vec<Entity>,
    /// Street each node carries (m), for holding points.
    metres: Vec<f32>,
    field: Option<Field>,
    /// Last step's values and the source node each comes from; recomputes
    /// only what changed (`Field::step_quiet`).
    quiet: Quiet,
    /// Input buffer for the step, kept to avoid allocating each time.
    cur: Vec<(f32, f32)>,
    /// Hilltops and sinks (dense indices), for tunnels (`Rules::tunnel`).
    peaks: Vec<u32>,
    sinks: Vec<u32>,
    /// Where a source may rise (`Rules::rise`): junctions `dh_m` above
    /// (fire) or below (water) their neighbours' mean, dense indices.
    rise: [Vec<u32>; 2],
    /// Street lengths, for supply (`Rules::supply`).
    streets: Option<Streets>,
    /// What the field is built from, to build it again (weather: frost).
    costs: Vec<EdgeCosts>,
    affinity: Vec<f32>,
    /// Node ids on or within `keep.gap_m` of a natural source: no permanent
    /// source there (the app offers none).
    no_keep: std::collections::HashSet<i64>,
}

impl ElementSim {
    /// New field parameters (the day's weather), same graph.
    pub fn rebuild_field(&mut self, p: &elements_shared::field::FieldParams) {
        self.field = Some(Field::new(self.nodes.len(), &self.costs, &self.affinity, p));
        self.quiet.reset();
    }

    pub fn no_keep(&self) -> &std::collections::HashSet<i64> {
        &self.no_keep
    }

    pub fn nodes(&self) -> &[Entity] {
        &self.nodes
    }

    pub fn state(&self) -> &[(f32, f32)] {
        &self.quiet.next
    }

    pub fn metres(&self) -> &[f32] {
        &self.metres
    }

    pub fn origin(&self) -> &[(u32, u32)] {
        &self.quiet.origin
    }

    pub fn streets(&self) -> Option<&Streets> {
        self.streets.as_ref()
    }

    /// Where `f`'s element gathers: hilltops for fire, sinks for water.
    pub fn gathers(&self, f: Faction) -> impl Iterator<Item = Entity> + '_ {
        let v = match f {
            Faction::Fire => &self.peaks,
            Faction::Water => &self.sinks,
        };
        v.iter().map(|&i| self.nodes[i as usize])
    }

    /// Where a source of `f` may rise (`Rules::rise`).
    pub fn rise_spots(&self, f: Faction) -> impl Iterator<Item = Entity> + '_ {
        self.rise[f.index()].iter().map(|&i| self.nodes[i as usize])
    }

    /// Dense indices next to node `i` on the street graph.
    pub fn neighbour_idx(&self, i: usize) -> Vec<usize> {
        self.field
            .as_ref()
            .map_or_else(Vec::new, |f| f.neighbours(i).collect())
    }

    /// Node entities next to node `i` (dense index) on the street graph.
    pub fn neighbours(&self, i: usize) -> Vec<Entity> {
        let Some(field) = &self.field else {
            return Vec::new();
        };
        field.neighbours(i).map(|j| self.nodes[j]).collect()
    }
}

/// Build the reach graph from the already-spawned nodes/edges and seed the
/// element sources. Runs in `PostStartup` so the `Startup` geo spawns are
/// applied and queryable.
pub fn setup_elements(
    mut commands: Commands,
    mut nodes: Query<(Entity, &GeoNode, &mut ElementState)>,
    has_idx: Query<&ElemIdx>,
    edges: Query<&GeoEdge>,
    pending: Option<Res<PendingEdges>>,
    rules: Res<GameRules>,
) {
    // Stable dense order (by node id) so indices are reproducible.
    let mut list: Vec<(Entity, i64, f64)> = nodes
        .iter()
        .map(|(e, n, _)| (e, n.id, n.elevation_m))
        .collect();
    if list.is_empty() {
        return; // empty world (no DB) — nothing to simulate.
    }
    list.sort_by_key(|&(_, id, _)| id);
    let n = list.len();

    let mut id_to_idx = HashMap::with_capacity(n);
    let mut node_entities = Vec::with_capacity(n);
    let mut elev = Vec::with_capacity(n);
    let mut affinity = vec![0.0f32; n];
    for (idx, &(e, id, el)) in list.iter().enumerate() {
        id_to_idx.insert(id, idx);
        node_entities.push(e);
        elev.push(el);
        if let Ok((_, g, _)) = nodes.get(e) {
            affinity[idx] = g.affinity;
        }
        // Spawned with it (main.rs `load_geo`); others (tests) get it here.
        if has_idx.get(e).map_or(true, |i| i.0 != idx) {
            commands.entity(e).insert(ElemIdx(idx));
        }
    }

    // Undirected for spread (elements aren't traffic): each edge feeds both
    // directions at that direction's element cost; `forward` is source -> target.
    let costs: Vec<EdgeCosts> = edges
        .iter()
        .chain(pending.iter().flat_map(|p| p.0.iter()))
        .filter_map(|e| {
            let a = *id_to_idx.get(&e.source)?;
            let b = *id_to_idx.get(&e.target)?;
            Some(EdgeCosts {
                a,
                b,
                fire_fwd: e.fire_cost_forward,
                fire_rev: e.fire_cost_reverse,
                water_fwd: e.water_cost_forward,
                water_rev: e.water_cost_reverse,
                len: e.length_m as f32,
                dh: (elev[b] - elev[a]) as f32,
            })
        })
        .collect();
    let field = Field::new(n, &costs, &affinity, &rules.0.field);
    let metres = elements_shared::scoring::node_metres(n, &costs);
    let streets = rules.0.supply.enabled.then(|| Streets::new(n, &costs));
    let tunnel = &rules.0.tunnel;
    let (peaks, sinks) = if tunnel.chance > 0.0 {
        let h: Vec<f32> = elev.iter().map(|&e| e as f32).collect();
        field.extrema(&h, tunnel.hops, tunnel.margin_m)
    } else {
        Default::default()
    };

    let rise = Faction::ALL.map(|f| {
        (0..n)
            .filter(|&i| {
                let nb: Vec<f64> = field.neighbours(i).map(|j| elev[j]).collect();
                !nb.is_empty() && rules.0.rise.fits(f, (elev[i] - nb.iter().sum::<f64>() / nb.len() as f64) as f32)
            })
            .map(|i| i as u32)
            .collect::<Vec<u32>>()
    });

    // Natural sources: nodes marked by the data import (e.g.
    // db/import/simple-sources.sql); otherwise by elevation, lowest -> fire,
    // highest -> water. Players add more sources by attacking (S5).
    let marked: Vec<(usize, Faction)> = list
        .iter()
        .enumerate()
        .filter_map(|(i, &(e, ..))| {
            let f = match nodes.get(e).ok()?.1.node_type.as_str() {
                "fire_source" => Faction::Fire,
                "water_source" | "spring_source" => Faction::Water,
                _ => return None,
            };
            Some((i, f))
        })
        .collect();
    let spring: Vec<bool> = list
        .iter()
        .map(|&(e, ..)| {
            nodes
                .get(e)
                .is_ok_and(|(_, g, _)| g.node_type == "spring_source")
        })
        .collect();
    let any_marked = !marked.is_empty();
    let seeds = elements_shared::source::natural_seeds(marked, &elev);
    // Too close to a natural source for a permanent source (as in web.rs
    // `FromWeb::Keep`; a little wider, so the app never offers one it refuses).
    let no_keep = {
        let gap = rules.0.keep.gap_m as f64 + 2.0;
        let pos: Vec<(i64, f64, f64)> = list
            .iter()
            .filter_map(|&(e, id, _)| nodes.get(e).ok().map(|(_, g, _)| (id, g.lat, g.lon)))
            .collect();
        let nat: Vec<(f64, f64)> = seeds.iter().map(|&(i, _)| (pos[i].1, pos[i].2)).collect();
        let k = (list
            .first()
            .map_or(0.0, |&(e, ..)| nodes.get(e).map_or(0.0, |(_, g, _)| g.lat)))
        .to_radians()
        .cos();
        pos.iter()
            .filter(|&&(_, la, lo)| {
                nat.iter().any(|&(nla, nlo)| {
                    let dy = (la - nla) * 110_540.0;
                    let dx = (lo - nlo) * 111_320.0 * k;
                    dx * dx + dy * dy < gap * gap
                })
            })
            .map(|&(id, ..)| id)
            .collect()
    };
    let (fire_n, water_n) = seeds.iter().fold((0, 0), |(f, w), (_, x)| match x {
        Faction::Fire => (f + 1, w),
        Faction::Water => (f, w + 1),
    });
    for (idx, fac) in seeds {
        let mut natural = ElementSource::natural(fac, &rules.0);
        natural.strength = rules.0.natural_strength_at(fac, spring[idx]);
        if let Ok((_, _, mut es)) = nodes.get_mut(node_entities[idx]) {
            match fac {
                Faction::Fire => es.fire = natural.strength,
                Faction::Water => es.water = natural.strength,
            }
        }
        commands
            .entity(node_entities[idx])
            .insert((natural, NaturalSource(natural)));
        if spring[idx] {
            commands.entity(node_entities[idx]).insert(Spring);
        }
    }

    info!(
        nodes = n,
        fire_sources = fire_n,
        water_sources = water_n,
        hilltops = peaks.len(),
        sinks = sinks.len(),
        marked = any_marked,
        "element simulation initialised"
    );
    commands.remove_resource::<PendingEdges>();
    commands.insert_resource(NodeById {
        ids: list.iter().map(|&(_, id, _)| id).collect(),
        ents: node_entities.clone(),
    });
    commands.insert_resource(ElementSim {
        nodes: node_entities,
        metres,
        field: Some(field),
        quiet: Quiet::new(vec![(0.0, 0.0); n], vec![(NO_ORIGIN, NO_ORIGIN); n]),
        cur: vec![(0.0, 0.0); n],
        peaks,
        sinks,
        rise,
        streets,
        costs,
        affinity,
        no_keep,
    });
    crate::trim_memory("region loaded");
}

/// One reach step, gated to [`ELEMENT_TICK_EVERY`]. Each node takes the strongest
/// inbound signal; [`ElementSource`] nodes are pinned; only meaningful changes
/// are written back (fewer diffs to send).
pub fn element_tick(
    mut sim: ResMut<ElementSim>,
    mut q: Query<(
        &ElemIdx,
        &GeoNode,
        Option<&ElementSource>,
        &mut ElementState,
    )>,
    mut step: Local<u64>,
) {
    *step += 1;
    if *step % ELEMENT_TICK_EVERY != 0 {
        return;
    }

    let sim = &mut *sim;
    let Some(field) = sim.field.as_ref() else {
        return;
    };
    let n = sim.nodes.len();
    let mut sources: Vec<(usize, Faction, f32)> = Vec::new();
    for (idx, _geo, source, es) in &q {
        sim.cur[idx.0] = (es.fire, es.water);
        if let Some(s) = source {
            sources.push((idx.0, s.faction, s.strength));
        }
    }

    field.step_quiet(&mut sim.quiet, &sim.cur, sources);
    let next = &sim.quiet.next;

    for (idx, _geo, _source, mut es) in &mut q {
        let (nf, nw) = next[idx.0];
        if (es.fire - nf).abs() > 1e-3 || (es.water - nw).abs() > 1e-3 {
            es.fire = nf;
            es.water = nw;
        }
    }

    // Periodic territory summary (~every 4 s) so the spread is observable without
    // a client.
    if (*step / ELEMENT_TICK_EVERY) % 8 == 0 {
        let (mut fire, mut water, mut contested) = (0u32, 0u32, 0u32);
        for &(f, w) in next {
            let es = ElementState { fire: f, water: w };
            match es.owner() {
                Some(Faction::Fire) => fire += 1,
                Some(Faction::Water) => water += 1,
                None if es.contested() => contested += 1,
                None => {}
            }
        }
        info!(
            fire,
            water,
            contested,
            neutral = n as u32 - fire - water - contested,
            recomputed = sim.quiet.recomputed,
            "element territory"
        );
        // Dev: dump node states for offline rendering.
        if let Ok(path) = std::env::var("ELEMENTS_DUMP") {
            let mut ids = vec![0i64; n];
            for (idx, geo, _, _) in &q {
                ids[idx.0] = geo.id;
            }
            let mut csv = String::from("node_id,fire,water\n");
            for (i, &(f, w)) in next.iter().enumerate() {
                csv.push_str(&format!("{},{f:.4},{w:.4}\n", ids[i]));
            }
            if let Err(e) = std::fs::write(&path, csv) {
                warn!(error = %e, path, "state dump failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use elements_shared::rules::Decay;

    /// Five nodes 200 m apart (transmission 0.5 per hop), fire marked at 1,
    /// water at 5.
    fn five() -> World {
        let (n, e) = line(5, 200.0, &[(1, "fire_source"), (5, "water_source")]);
        world(Rules::default(), n, e)
    }

    #[test]
    fn setup_seeds_marked_nodes_in_id_order() {
        let w = five();
        let src = |id| w.get::<ElementSource>(ent(&w, id)).copied();
        assert_eq!(src(1).unwrap().faction, Faction::Fire);
        assert_eq!(src(5).unwrap().faction, Faction::Water);
        assert!(src(1).unwrap().life.is_none());
        assert!(src(3).is_none());
        assert!(w.get::<NaturalSource>(ent(&w, 1)).is_some());
        // pinned at once, before the first step
        assert_eq!(state(&w, 1).fire, 1.0);
        for id in 1..=5 {
            assert_eq!(w.get::<ElemIdx>(ent(&w, id)).unwrap().0, id as usize - 1);
        }
        assert!(w.resource::<NodeById>().get(&99).is_none());
    }

    #[test]
    fn springs_are_weaker_water_sources() {
        let (n, e) = line(
            5,
            200.0,
            &[
                (1, "fire_source"),
                (3, "spring_source"),
                (5, "water_source"),
            ],
        );
        let rules = Rules::default();
        let w = world(rules.clone(), n, e);
        let src = |id| *w.get::<ElementSource>(ent(&w, id)).unwrap();
        assert_eq!(src(3).faction, Faction::Water);
        assert_eq!(src(3).strength, rules.natural_spring_strength);
        assert_eq!(src(5).strength, rules.natural_water_strength);
        assert!(w.get::<Spring>(ent(&w, 3)).is_some());
        assert!(w.get::<Spring>(ent(&w, 5)).is_none());
        // What comes back after a claim on it ends: the spring's strength.
        assert_eq!(
            w.get::<NaturalSource>(ent(&w, 3)).unwrap().0.strength,
            rules.natural_spring_strength
        );
        assert_eq!(state(&w, 3).water, rules.natural_spring_strength);
    }

    #[test]
    fn setup_without_marks_seeds_by_elevation() {
        let nodes: Vec<GeoNode> = (1..=6)
            .map(|i| node(i, i as f64 * 50.0, i as f64, ""))
            .collect();
        let edges = (1..6).map(|i| edge(i, i, i + 1, 50.0)).collect();
        let w = world(Rules::default(), nodes, edges);
        let f = |id| w.get::<ElementSource>(ent(&w, id)).map(|s| s.faction);
        // 6 nodes: 3 lowest fire, 3 highest water
        assert_eq!(
            (1..=6).map(f).collect::<Vec<_>>(),
            [Faction::Fire, Faction::Fire, Faction::Fire]
                .into_iter()
                .chain([Faction::Water; 3])
                .map(Some)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_keep_near_natural_sources() {
        let (n, e) = line(5, 50.0, &[(1, "fire_source"), (5, "water_source")]);
        let w = world(Rules::default(), n, e);
        let nk = w.resource::<ElementSim>().no_keep();
        // gap 60 m (+2): the sources and their 50 m neighbours
        let mut got: Vec<i64> = nk.iter().copied().collect();
        got.sort();
        assert_eq!(got, vec![1, 2, 4, 5]);
    }

    #[test]
    fn spread_halves_per_hop_and_stays_pinned() {
        let mut w = five();
        steps(&mut w, 10);
        let s: Vec<ElementState> = (1..=5).map(|id| state(&w, id)).collect();
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(near(s[0].fire, 1.0) && near(s[4].water, 1.0));
        assert!(near(s[1].fire, 0.5) && near(s[2].fire, 0.25) && near(s[3].fire, 0.125));
        assert!(near(s[3].water, 0.5) && near(s[1].water, 0.125));
        for x in &s {
            assert!((0.0..=1.0).contains(&x.fire) && (0.0..=1.0).contains(&x.water));
        }
        assert_eq!(s[1].owner(), Some(Faction::Fire));
        assert_eq!(s[2].owner(), None); // 0.25 vs 0.25: contested front
        assert!(s[2].contested());
        assert_eq!(s[3].owner(), Some(Faction::Water));
        // owned nodes trace back to their source (dense index)
        let o = w.resource::<ElementSim>().origin().to_vec();
        assert_eq!((o[1].0, o[3].1), (0, 4));
    }

    #[test]
    fn settled_field_is_stable_and_fades_without_sources() {
        let mut w = five();
        steps(&mut w, 10);
        let before: Vec<ElementState> = (1..=5).map(|id| state(&w, id)).collect();
        steps(&mut w, 5);
        assert_eq!(before, (1..=5).map(|id| state(&w, id)).collect::<Vec<_>>());
        for id in [1, 5] {
            let e = ent(&w, id);
            w.entity_mut(e).remove::<ElementSource>();
        }
        steps(&mut w, 10);
        for id in 1..=5 {
            assert_eq!(state(&w, id).owner(), None, "node {id}");
        }
    }

    #[test]
    fn claims_fade_then_fall_or_give_the_natural_source_back() {
        let rules = Rules {
            claim_ttl_s: 100.0,
            decay: Decay::Linear,
            ..Default::default()
        };
        let (n, e) = line(5, 200.0, &[(1, "fire_source"), (5, "water_source")]);
        let mut w = world(rules.clone(), n, e);
        let claim = ElementSource::claim(
            &rules,
            Faction::Water,
            SourceKind::Claim,
            100.0,
            Some(7),
            1.0,
            false,
            0.0,
        );
        let (on_nat, plain) = (ent(&w, 1), ent(&w, 3));
        w.entity_mut(on_nat).insert(claim);
        w.entity_mut(plain).insert(claim);
        let id = w.register_system(expire_sources);
        advance(&mut w, 50.0);
        w.run_system(id).unwrap();
        let s = w.get::<ElementSource>(plain).unwrap();
        assert!((s.strength - 0.5).abs() < 1e-5);
        // natural sources don't age
        let nat = *w.get::<ElementSource>(ent(&w, 5)).unwrap();
        assert_eq!((nat.age, nat.strength), (0.0, 1.0));
        advance(&mut w, 50.0);
        w.run_system(id).unwrap();
        assert!(w.get::<ElementSource>(plain).is_none());
        let back = w.get::<ElementSource>(on_nat).unwrap();
        assert_eq!(*back, ElementSource::natural(Faction::Fire, &rules));
    }
}

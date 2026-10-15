//! Encirclement (`[siege]`): a player's source whose
//! junction has at least `min_neighbours` neighbours, all held by the other
//! side's players, for `delay_s`, is marked `sieged`: it ages `factor` times
//! as fast (permanent sources `keep_factor`, `sim::expire_sources`) until the
//! ring breaks, and the ring's holders share `points` (`keep_points`) once per
//! surrounded source. Checked every 10 s. The app shows the mark (sources row)
//! and tells the owner; the holders hear of their points (`Encircled`).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use elements_shared::elements::ElementState;
use elements_shared::prelude::Faction;
use elements_shared::source::Kind as SourceKind;

use crate::sim::{ElemIdx, ElementSim, ElementSource, GameRules};

const EVERY_S: f64 = 10.0;

#[derive(Default)]
pub struct State {
    next: f64,
    /// Surrounded since (unix s), by side of the source.
    since: HashMap<Entity, (f64, Faction)>,
    /// Ring points paid for the source now on that node.
    paid: HashSet<Entity>,
}

pub fn tick(
    rules: Res<GameRules>,
    sim: Res<ElementSim>,
    mut sources: Query<(Entity, &mut ElementSource, &ElemIdx)>,
    mut scores: ResMut<crate::score::Scores>,
    mut st: Local<State>,
) {
    let r = &rules.0.siege;
    if !r.enabled {
        return;
    }
    let now = crate::analytics::now_s();
    if now < st.next {
        return;
    }
    st.next = now + EVERY_S;
    // Ordinary sources only when the rules give them something.
    let all_kinds = r.factor != 1.0 || r.points > 0.0;
    // Player sources by dense index: side and slot (keeps have none).
    let mut player: HashMap<usize, (Faction, Option<u32>)> = HashMap::new();
    for (_, s, i) in &sources {
        if s.life.is_some() && s.kind != SourceKind::Spark {
            player.insert(i.0, (s.faction, s.by));
        }
    }
    let state = sim.state();
    let origin = sim.origin();
    let code = |f: Faction| if f == Faction::Fire { 1u8 } else { 2 };
    // A neighbour's side (1 fire, 2 water, 0 none); with `by_players` only
    // when a player's source of that side holds it.
    let side = |j: usize| -> u8 {
        let Some(&(fire, water)) = state.get(j) else {
            return 0;
        };
        let Some(f) = (ElementState { fire, water }).owner() else {
            return 0;
        };
        if r.by_players {
            let (of, ow) = origin[j];
            let src = if f == Faction::Fire { of } else { ow } as usize;
            if !player.get(&src).is_some_and(|p| p.0 == f) {
                return 0;
            }
        }
        code(f)
    };
    let mut live: HashSet<Entity> = HashSet::new();
    for (e, mut s, i) in &mut sources {
        let candidate = s.life.is_some()
            && match s.kind {
                SourceKind::Keep => true,
                SourceKind::Spark => false,
                _ => all_kinds && s.by.is_some(),
            };
        if !candidate {
            if s.sieged {
                s.sieged = false;
            }
            continue;
        }
        live.insert(e);
        let around = sim.neighbour_idx(i.0);
        let ring = r.surrounded(code(s.faction), around.iter().map(|&j| side(j)));
        let since = match st.since.get(&e) {
            Some(&(t, f)) if ring && f == s.faction => t,
            Some(&(_, f)) if f != s.faction => {
                // Taken over: a new source, unpaid.
                st.paid.remove(&e);
                if !ring {
                    st.since.remove(&e);
                    s.sieged = false;
                    continue;
                }
                now
            }
            _ if ring => now,
            _ => {
                st.since.remove(&e);
                if s.sieged {
                    s.sieged = false;
                }
                continue;
            }
        };
        st.since.insert(e, (since, s.faction));
        let sieged = now - since >= f64::from(r.delay_s);
        if sieged && !s.sieged {
            s.sieged = true;
            let pts = if s.kind == SourceKind::Keep {
                r.keep_points
            } else {
                r.points
            };
            if pts > 0.0 && st.paid.insert(e) {
                // The ring's holders with a slot share the points.
                let other = s.faction.other();
                let by: Vec<u32> = around
                    .iter()
                    .filter_map(|&j| {
                        let (of, ow) = origin[j];
                        let src = if other == Faction::Fire { of } else { ow } as usize;
                        player.get(&src).filter(|p| p.0 == other).and_then(|p| p.1)
                    })
                    .collect();
                if !by.is_empty() {
                    let each = (pts / by.len() as f32).round() as i64;
                    for slot in by {
                        let got = scores.award(slot, each);
                        if got > 0 {
                            scores.sieges.push((slot, got, s.faction));
                        }
                    }
                }
            }
        } else if !sieged && s.sieged {
            s.sieged = false;
        }
    }
    // Sources that went: forget them (a new one on the node starts unpaid).
    st.since.retain(|e, _| live.contains(e));
    st.paid.retain(|e| live.contains(e));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::{Global, Scores};
    use crate::testkit::*;
    use bevy::ecs::system::RunSystemOnce;
    use elements_shared::rules::{Rules, Siege};
    use elements_shared::source::Source;

    fn player(s: &mut Scores, token: &str, f: Faction) -> u32 {
        s.adopt(
            token,
            Global {
                faction: f,
                name: token.into(),
                hidden: false,
                recovery: None,
                xp_prev: 0.0,
                day_xp: Default::default(),
                xp_day: 0,
                xp_since: 0.0,
                collect: false,
                finds: Vec::new(),
                claims: 0,
                badges: Vec::new(),
                regions: Vec::new(),
                presence: Vec::new(),
                streak: Default::default(),
            },
        )
    }

    #[test]
    fn a_ring_of_players_surrounds_a_keep_pays_its_holders_and_lets_go() {
        let rules = Rules {
            siege: Siege {
                enabled: true,
                min_neighbours: 3,
                delay_s: 0.0,
                factor: 1.0,
                keep_factor: 4.0,
                keep_points: 30.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // A star: junction 1 with three roads to 2, 3, 4.
        let nodes = vec![
            node(1, 0.0, 0.0, "crossing"),
            node(2, 60.0, 0.0, "crossing"),
            node(3, -60.0, 0.0, "crossing"),
            node(4, 120.0, 0.0, "crossing"),
        ];
        let edges = vec![
            edge(1, 1, 2, 60.0),
            edge(2, 1, 3, 60.0),
            edge(3, 1, 4, 60.0),
        ];
        let mut w = world(rules.clone(), nodes, edges);
        let mut scores = Scores::new(None, "t".into(), Default::default());
        let slots: Vec<u32> = ["b", "c", "d"]
            .iter()
            .map(|t| player(&mut scores, t, Faction::Water))
            .collect();
        w.insert_resource(scores);
        let mut keep = Source::claim(
            &rules,
            Faction::Fire,
            SourceKind::Keep,
            f32::INFINITY,
            None,
            1.0,
            false,
            0.0,
        );
        keep.strength = 1.0;
        let e1 = ent(&w, 1);
        w.entity_mut(e1).insert(keep);
        for (id, &slot) in [2, 3, 4].iter().zip(&slots) {
            let c = Source::claim(
                &rules,
                Faction::Water,
                SourceKind::Claim,
                600.0,
                Some(slot),
                1.0,
                false,
                0.0,
            );
            let e = ent(&w, *id);
            w.entity_mut(e).insert(c);
        }
        steps(&mut w, 20);
        w.run_system_once(tick).unwrap();
        assert!(w.get::<ElementSource>(e1).unwrap().sieged, "surrounded");
        let paid = w.resource::<Scores>().sieges.clone();
        assert_eq!(paid.len(), 3, "each holder gets a share");
        assert!(paid
            .iter()
            .all(|&(_, p, side)| p == 10 && side == Faction::Fire));
        assert_eq!(
            crate::sim::siege_factor(&rules, w.get::<ElementSource>(e1).unwrap()),
            4.0
        );
        // One holder's claim goes: the ring is broken.
        let e2 = ent(&w, 2);
        w.entity_mut(e2).remove::<ElementSource>();
        steps(&mut w, 40);
        w.run_system_once(tick).unwrap();
        assert!(!w.get::<ElementSource>(e1).unwrap().sieged, "ring broken");
    }
}

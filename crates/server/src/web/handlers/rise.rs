//! A source rises where a player waits (`Rules::rise`, lab round 28): a
//! minute within `radius_m` of a sink (water) or a peak (fire) raises a source
//! of the player's element there. It is a claim the server makes: `strength`,
//! `life_s`, an anchor on own ground, no cooldown used. Once per player per
//! `player_gap_s`, once per junction per `place_gap_s`.

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::actions;
use crate::sim::NodeById;
use crate::web::{send, ServerMsg};

use super::{Ctx, Game};

/// After a position fix of player `e` on connection `conn`.
pub(super) fn on_fix(game: &mut Game, c: &Ctx, conn: u64, e: Entity) {
    let (now, rules) = (c.now, c.rules);
    let r = &rules.rise;
    if !r.enabled {
        return;
    }
    let (Some(sim), Some((_, out))) = (game.sim.as_deref(), game.clients.0.get(&conn)) else {
        return;
    };
    let Ok((&faction, pos, mut cd, _, slot, mut motion)) = game.players.get_mut(e) else {
        return;
    };
    let Some(slot) = slot.map(|s| s.0) else {
        return;
    };
    if now < motion.rise_next || game.scores.shadowed(slot, now) {
        return;
    }
    let me = Vec2::new(pos.0.x, pos.0.z);
    let power = actions::Power {
        mult: r.strength / rules.claim_strength.max(0.01),
        newcomer: false,
        reach_m: r.radius_m + 5.0,
    };
    // The nearest fitting junction in reach: free to rise again, no natural
    // source, no source of the own side, none of the other's too strong for it.
    let spot = sim
        .rise_spots(faction)
        .filter(|&n| !game.naturals.contains(n))
        .filter(|&n| {
            game.sources
                .get(n)
                .ok()
                .is_none_or(|(_, s)| s.faction != faction && !actions::holds(s, rules, power))
        })
        .filter_map(|n| {
            let (g, _) = game.nodes.get(n).ok()?;
            if game.rise_places.get(&g.id).is_some_and(|&t| now < t) {
                return None;
            }
            let p = project_enu(g.lat, g.lon, c.region.centroid_lat, c.region.centroid_lon);
            let d = me.distance(Vec2::new(p.x, p.z));
            (d <= r.radius_m).then_some((g.id, g.lat, g.lon, d))
        })
        .min_by(|a, b| a.3.total_cmp(&b.3));
    let msg = |(node, lat, lon, _): (i64, f64, f64, f32), state, secs, took| ServerMsg::Rise {
        node,
        lat,
        lon,
        state,
        secs,
        took,
    };
    match (spot, motion.at_rise) {
        (Some(s), Some((was, since))) if s.0 == was => {
            if now - since < r.dwell_s as f64 {
                return;
            }
            motion.at_rise = None;
            // The spot first; with `fill_m` then its hollow / hilltop, nearest first.
            let fill = if r.fill_m > 0.0 {
                fill_around(
                    &game.nodes,
                    &game.node_by_id,
                    &game.sources,
                    &game.naturals,
                    c,
                    s.0,
                    faction,
                    r,
                )
            } else {
                Vec::new()
            };
            let spot_pos = spot_at(&game.nodes, &game.node_by_id, c, s.0).unwrap_or(pos.0);
            // The player's own claim timer stays as it was.
            let saved = cd.0;
            let mut took = 0u32;
            let mut spot_res = None;
            for node in std::iter::once(s.0).chain(fill) {
                let ent = game.node_by_id.get(&node).copied();
                let existing = ent.and_then(|n| game.sources.get(n).ok()).map(|(_, x)| *x);
                let prior = ent.and_then(|n| game.nodes.get(n).ok()).map(|(_, st)| *st);
                cd.0 = 0.0;
                let res = actions::claim(
                    &mut game.commands,
                    &game.node_by_id,
                    &mut game.nodes,
                    c.region,
                    // The hollow counts from the spot, not from the player.
                    if node == s.0 { pos.0 } else { spot_pos },
                    faction,
                    &mut cd,
                    node,
                    rules,
                    r.life_s,
                    Some(slot),
                    existing,
                    actions::Power {
                        reach_m: if node == s.0 {
                            power.reach_m
                        } else {
                            r.fill_m + 1.0
                        },
                        ..power
                    },
                    f32::INFINITY, // the anchor gap doesn't apply
                    false,
                );
                if node == s.0 {
                    spot_res = Some(res);
                    if res.is_err() {
                        break;
                    }
                }
                if let (Ok(_), Some(p)) = (res, prior) {
                    took += 1;
                    let pts = rules.score.claim_points(p, faction, false);
                    game.scores.award(slot, pts);
                }
            }
            cd.0 = saved;
            game.rise_places.retain(|_, t| *t > now);
            game.rise_places.insert(s.0, now + r.place_gap_s as f64);
            match spot_res {
                Some(Err(actions::Reject::Strong | actions::Reject::Worn)) => {
                    // Grew too strong while the player waited.
                    send(out, &msg(s, "strong", 0, 0));
                }
                Some(Ok(_)) => {
                    motion.rise_next = now + r.player_gap_s as f64;
                    debug!(node = s.0, ?faction, took, "source rose");
                    send(out, &msg(s, "done", 0, took));
                }
                _ => send(out, &msg(s, "left", 0, 0)),
            }
        }
        (Some(s), _) => {
            motion.at_rise = Some((s.0, now));
            send(out, &msg(s, "wait", r.dwell_s.round() as u32, 0));
        }
        (None, Some((was, _))) => {
            motion.at_rise = None;
            if let Some((g, _)) = game
                .node_by_id
                .get(&was)
                .and_then(|&n| game.nodes.get(n).ok())
            {
                send(out, &msg((was, g.lat, g.lon, 0.0), "left", 0, 0));
            }
        }
        (None, None) => {}
    }
}

/// The spot's position (ENU), for the reach of the fill claims.
fn spot_at(
    nodes: &Query<(&GeoNode, &mut ElementState), Without<Player>>,
    by_id: &NodeById,
    c: &Ctx,
    spot: i64,
) -> Option<Vec3> {
    let (g, _) = nodes.get(*by_id.get(&spot)?).ok()?;
    Some(project_enu(
        g.lat,
        g.lon,
        c.region.centroid_lat,
        c.region.centroid_lon,
    ))
}

/// The hollow (water) / hilltop (fire) around `spot`: up to `fill_max`
/// junctions within `fill_m` no higher / no lower than it (`Rise::fills`),
/// nearest first; no natural sources, no permanent sources, none of the own side.
#[allow(clippy::too_many_arguments)]
fn fill_around(
    nodes: &Query<(&GeoNode, &mut ElementState), Without<Player>>,
    by_id: &NodeById,
    sources: &Query<(&GeoNode, &mut crate::sim::ElementSource)>,
    naturals: &Query<&GeoNode, With<crate::sim::NaturalSource>>,
    c: &Ctx,
    spot: i64,
    faction: Faction,
    r: &elements_shared::hinterland::Rise,
) -> Vec<i64> {
    let Some((sg, _)) = by_id.get(&spot).and_then(|&e| nodes.get(e).ok()) else {
        return Vec::new();
    };
    let at = |g: &GeoNode| {
        let p = project_enu(g.lat, g.lon, c.region.centroid_lat, c.region.centroid_lon);
        Vec2::new(p.x, p.z)
    };
    let (centre, elev) = (at(sg), sg.elevation_m);
    let mut v: Vec<(i64, f32)> = nodes
        .iter()
        .filter(|(g, _)| g.id != spot && r.fills(faction, elev, g.elevation_m))
        .map(|(g, _)| (g.id, at(g).distance(centre)))
        .filter(|&(_, d)| d <= r.fill_m)
        .filter(|&(id, _)| {
            by_id.get(&id).is_some_and(|&e| {
                !naturals.contains(e)
                    && sources.get(e).ok().is_none_or(|(_, s)| {
                        s.faction != faction && s.kind != crate::sim::SourceKind::Keep
                    })
            })
        })
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v.into_iter()
        .take(r.fill_max as usize)
        .map(|(id, _)| id)
        .collect()
}

//! A source rises where a player waits (`Rules::rise`, lab round 28): a
//! minute within `radius_m` of a sink (water) or a peak (fire) raises a source
//! of the player's element there. It is a claim the server makes: `strength`,
//! `life_s`, an anchor on own ground, no cooldown used. Once per player per
//! `player_gap_s`, once per junction per `place_gap_s`.

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::actions;
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
    let msg = |(node, lat, lon, _): (i64, f64, f64, f32), state, secs| ServerMsg::Rise {
        node,
        lat,
        lon,
        state,
        secs,
    };
    match (spot, motion.at_rise) {
        (Some(s), Some((was, since))) if s.0 == was => {
            if now - since < r.dwell_s as f64 {
                return;
            }
            motion.at_rise = None;
            let existing = game
                .node_by_id
                .get(&s.0)
                .and_then(|&n| game.sources.get(n).ok())
                .map(|(_, x)| *x);
            let prior = game
                .node_by_id
                .get(&s.0)
                .and_then(|&n| game.nodes.get(n).ok())
                .map(|(_, st)| *st);
            // The player's own claim timer stays as it was.
            let saved = cd.0;
            cd.0 = 0.0;
            let res = actions::claim(
                &mut game.commands,
                &game.node_by_id,
                &mut game.nodes,
                c.region,
                pos.0,
                faction,
                &mut cd,
                s.0,
                rules,
                r.life_s,
                Some(slot),
                existing,
                power,
                f32::INFINITY, // the anchor gap doesn't apply
                false,
            );
            cd.0 = saved;
            game.rise_places.retain(|_, t| *t > now);
            game.rise_places.insert(s.0, now + r.place_gap_s as f64);
            if let Err(actions::Reject::Strong | actions::Reject::Worn) = res {
                // Grew too strong while the player waited.
                send(out, &msg(s, "strong", 0));
            } else if res.is_ok() {
                motion.rise_next = now + r.player_gap_s as f64;
                if let Some(p) = prior {
                    let pts = rules.score.claim_points(p, faction, false);
                    game.scores.award(slot, pts);
                }
                debug!(node = s.0, ?faction, "source rose");
                send(out, &msg(s, "done", 0));
            } else {
                send(out, &msg(s, "left", 0));
            }
        }
        (Some(s), _) => {
            motion.at_rise = Some((s.0, now));
            send(out, &msg(s, "wait", r.dwell_s.round() as u32));
        }
        (None, Some((was, _))) => {
            motion.at_rise = None;
            if let Some((g, _)) = game.node_by_id.get(&was).and_then(|&n| game.nodes.get(n).ok()) {
                send(out, &msg((was, g.lat, g.lon, 0.0), "left", 0));
            }
        }
        (None, None) => {}
    }
}

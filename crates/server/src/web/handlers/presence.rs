//! Being there and being counted: presence codes, place ratings, the
//! analytics objection, the voluntary profile.

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::analytics::{Profile, Tracked};
use crate::sim::NodeById;
use crate::web::{send, ServerMsg, CATEGORIES, RATE_COOLDOWN_S, RATE_RADIUS_M};

use super::{Ctx, Game};

pub(super) fn presence(
    game: &mut Game,
    c: &Ctx,
    conn: u64,
    code: String,
    lat: f64,
    lon: f64,
    radius_m: f32,
) {
    let (now, region) = (c.now, c.region);
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok((_, _, _, tracked, slot, mut motion)) = game.players.get_mut(*e) else {
        return;
    };
    let Some(slot) = slot.map(|s| s.0) else {
        return;
    };
    let at = project_enu(lat, lon, region.centroid_lat, region.centroid_lon);
    // On site now: a fix of the last minute within the radius,
    // not locked by the speed check, not flagged.
    let reason = match motion.last_fix {
        _ if game.scores.shadowed(slot, now) => Some("flagged"),
        _ if motion.locked(now) => Some("stale"),
        Some((p, t)) if now - t <= 60.0 => {
            let d = Vec2::new(p.x, p.z).distance(Vec2::new(at.x, at.z));
            (d > radius_m).then_some("far")
        }
        _ => Some("stale"),
    };
    if reason.is_none() {
        if game.scores.confirm_presence(slot, &code).is_some() {
            info!(conn, "presence confirmed");
        }
        // Trusted from now on: full strength, counted at once.
        motion.trusted = true;
        if let Some(t) = tracked {
            game.agg.standing.establish(t.pseudo, now);
        }
    }
    send(
        out,
        &ServerMsg::Presence {
            ok: reason.is_none(),
            reason,
            n: Some(game.scores.presence(slot)),
        },
    );
}

pub(super) fn rate(game: &mut Game, c: &Ctx, conn: u64, node: i64, cat: &str) {
    let (now, rules) = (c.now, c.rules);
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok((_, pos, _, Some(mut t), slot, motion)) = game.players.get_mut(*e) else {
        return;
    };
    if motion.locked(now) {
        send(
            out,
            &ServerMsg::Rate {
                ok: false,
                reason: Some("moving"),
                points: 0,
            },
        );
        return;
    }
    let reason = rate_check(
        cat,
        node,
        pos.0,
        &t,
        now,
        &game.node_by_id,
        &game.nodes,
        c.region,
    );
    let mut points = 0;
    if let Ok(category) = reason {
        t.last_rate = now;
        if t.counted {
            game.agg.rate(node, category, t.pseudo, t.synthetic, now);
        }
        if let Some(s) = slot {
            points = game.scores.rated(s.0);
            if rules.progression.enabled && points > 0 {
                game.scores
                    .add_xp(s.0, elements_shared::progression::Goal::Rating);
            }
        }
    }
    send(
        out,
        &ServerMsg::Rate {
            ok: reason.is_ok(),
            reason: reason.err(),
            points,
        },
    );
}

fn rate_check(
    cat: &str,
    node: i64,
    pos: Vec3,
    t: &Tracked,
    now: f64,
    node_by_id: &NodeById,
    nodes: &Query<(&GeoNode, &mut ElementState), Without<Player>>,
    region: &RegionInfo,
) -> Result<&'static str, &'static str> {
    let category = *CATEGORIES.iter().find(|c| **c == cat).ok_or("category")?;
    if now - t.last_rate < RATE_COOLDOWN_S {
        return Err("cooldown");
    }
    let (g, _) = node_by_id
        .get(&node)
        .and_then(|&e| nodes.get(e).ok())
        .ok_or("unknown")?;
    let p = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
    if p.distance(pos) > RATE_RADIUS_M {
        return Err("range");
    }
    Ok(category)
}

/// Count in the analytics for the city, or not (objection, Art. 21 GDPR).
pub(super) fn stats(game: &mut Game, c: &Ctx, conn: u64, on: bool) {
    let Some(&(e, _)) = game.clients.0.get(&conn) else {
        return;
    };
    if let Ok((.., Some(mut t), slot, _)) = game.players.get_mut(e) {
        if t.counted && !on {
            game.agg.withdraw(t.pseudo);
            game.hotspots.withdraw(t.pseudo);
        }
        // Flagged players stay out (shadow ban).
        t.counted = on && !slot.is_some_and(|s| game.scores.shadowed(s.0, c.now));
    }
}

pub(super) fn profile(game: &mut Game, conn: u64, age: Option<String>, local: Option<bool>) {
    let Some(&(e, _)) = game.clients.0.get(&conn) else {
        return;
    };
    if let Ok((.., Some(mut t), _, _)) = game.players.get_mut(e) {
        t.profile = Profile::parse(age.as_deref(), local);
    }
}

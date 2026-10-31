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

pub(super) fn rate(game: &mut Game, c: &Ctx, conn: u64, node: i64, cats: &[String]) {
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
        cats,
        node,
        pos.0,
        &t,
        now,
        &game.node_by_id,
        &game.nodes,
        c.region,
        &game.campaign.questions,
    );
    let mut points = 0;
    if let Ok((categories, cell)) = &reason {
        t.last_rate = now;
        if t.counted {
            for category in categories {
                game.agg
                    .rate(node, *cell, category, t.pseudo, t.synthetic, now);
            }
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

/// `q<id>.<n>`: answer `n` of the running question `id`, as an analytics
/// category, with where the question is asked (lon, lat, radius).
fn question_answer(
    cat: &str,
    questions: &[crate::campaign::Question],
) -> Option<(&'static str, Option<[f64; 2]>, f32)> {
    let (id, n) = cat.strip_prefix('q')?.split_once('.')?;
    let (id, n): (i64, usize) = (id.parse().ok()?, n.parse().ok()?);
    let q = questions.iter().find(|q| q.id == id)?;
    (n < q.answers.as_array()?.len())
        .then(|| (crate::analytics::intern(cat), q.at, q.radius_m as f32))
}

/// At most this many answers in one card (three pairs, the tags, the city's question).
const ANSWERS_MAX: usize = 9;

/// One card's answers: each a known category or an answer to a running city
/// question (one per question), the player near the junction (and in the
/// question's area), not too soon after the last card.
#[allow(clippy::too_many_arguments)]
fn rate_check(
    cats: &[String],
    node: i64,
    pos: Vec3,
    t: &Tracked,
    now: f64,
    node_by_id: &NodeById,
    nodes: &Query<(&GeoNode, &mut ElementState), Without<Player>>,
    region: &RegionInfo,
    questions: &[crate::campaign::Question],
) -> Result<(Vec<&'static str>, u64), &'static str> {
    if cats.is_empty() || cats.len() > ANSWERS_MAX {
        return Err("category");
    }
    let mut out: Vec<&'static str> = Vec::new();
    let mut asked: Vec<Option<[f64; 2]>> = Vec::new();
    let mut seen_q: Vec<&str> = Vec::new();
    for cat in cats {
        let category = match CATEGORIES.iter().find(|c| **c == cat) {
            Some(c) => *c,
            None => {
                let (c, at, r) = question_answer(cat, questions).ok_or("category")?;
                // One answer per question.
                let q = cat.split_once('.').map_or("", |(q, _)| q);
                if seen_q.contains(&q) {
                    return Err("category");
                }
                seen_q.push(q);
                if let Some([lon, lat]) = at {
                    let p = project_enu(lat, lon, region.centroid_lat, region.centroid_lon);
                    if p.distance(pos) > r {
                        return Err("range");
                    }
                }
                asked.push(at);
                c
            }
        };
        if !out.contains(&category) {
            out.push(category);
        }
    }
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
    Ok((out, crate::analytics::mood_cell(g.lat, g.lon)))
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

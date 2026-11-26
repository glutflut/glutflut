//! Taking and putting down loads (`carry.rs`), at the server's own idea of
//! where the player is (the last accepted fix, not older than `FRESH_S`).

use bevy::prelude::*;

use elements_shared::geo::project_enu;
use elements_shared::prelude::*;

use crate::carry::{CarryView, Laid, Taken};
use crate::sim::SourceKind;
use crate::web::{send, ServerMsg};

use super::{Ctx, Game};

/// A fix older than this doesn't place the player.
const FRESH_S: f64 = 30.0;

/// Who and where: (slot's id, game name, side, position), or why not.
fn who(
    game: &Game,
    c: &Ctx,
    conn: u64,
) -> Result<([u8; 32], Option<String>, Faction, Vec2), &'static str> {
    if !c.rules.carry.enabled {
        return Err("off");
    }
    let (e, _) = game.clients.0.get(&conn).ok_or("no position")?;
    let (&faction, _, _, _, slot, motion) = game.players.get(*e).map_err(|_| "no position")?;
    let slot = slot.ok_or("no position")?.0;
    let rec = game.scores.get(slot).ok_or("no position")?;
    let (p, at) = motion.last_fix.ok_or("no position")?;
    if motion.locked(c.now) || c.now - at > FRESH_S {
        return Err("no position");
    }
    let name = (!rec.hidden).then(|| rec.name.clone());
    Ok((rec.id, name, faction, Vec2::new(p.x, p.z)))
}

/// How far taking at a source reaches for player `e`, and whether that is
/// an armed Weitblick's reach.
pub(super) fn reach(game: &Game, rules: &elements_shared::rules::Rules, e: Entity) -> (f32, bool) {
    let base = rules.carry.reach_m;
    let far = rules.progression.enabled
        && game
            .abilities
            .get(e)
            .is_ok_and(|a| a.armed[super::ability::REACH]);
    if far && rules.progression.reach_m > base {
        (rules.progression.reach_m, true)
    } else {
        (base, false)
    }
}

fn reply(game: &Game, conn: u64, msg: &ServerMsg) {
    if let Some((_, out)) = game.clients.0.get(&conn) {
        send(out, msg);
    }
}

fn own_load(game: &Game, c: &Ctx, conn: u64, id: &[u8; 32]) {
    let load = game
        .carry
        .held(id)
        .map(|h| CarryView::of(h, &c.rules.carry, c.now));
    reply(game, conn, &ServerMsg::Carry { load });
}

pub(super) fn take(game: &mut Game, c: &Ctx, conn: u64) {
    let (id, name, side, p) = match who(game, c, conn) {
        Ok(w) => w,
        Err(reason) => return reply(game, conn, &ServerMsg::CarryFail { reason }),
    };
    let r = &c.rules.carry;
    let o = (c.region.centroid_lat, c.region.centroid_lon);
    // The nearest own natural source or building, or permanent source
    // (farther with Weitblick armed: used up when it was needed).
    let e = game.clients.0.get(&conn).map(|(e, _)| *e);
    let (reach_m, far) = e.map_or((r.reach_m, false), |e| reach(game, c.rules, e));
    let source = game
        .sources
        .iter()
        .filter(|(_, s)| s.faction == side && (s.life.is_none() || s.kind == SourceKind::Keep))
        .map(|(g, _)| {
            let q = project_enu(g.lat, g.lon, o.0, o.1);
            (g.id, Vec2::new(q.x, q.z).distance(p))
        })
        .filter(|&(_, d)| d <= reach_m)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let needed = far && source.is_some_and(|(_, d)| d > r.reach_m);
    match game
        .carry
        .take(r, id, name, side, p, source.map(|(id, _)| id), c.now)
    {
        Ok(t) => {
            if needed && t == Taken::Fresh {
                use_reach(game, c, conn);
            }
            let what = match t {
                Taken::Fresh => "fresh",
                Taken::Handed => "handed",
            };
            reply(
                game,
                conn,
                &ServerMsg::CarryDone {
                    what,
                    filled: None,
                    need: r.loads,
                },
            );
            own_load(game, c, conn, &id);
        }
        Err(reason) => reply(game, conn, &ServerMsg::CarryFail { reason }),
    }
}

/// Weitblick did it: used up, as after a claim.
fn use_reach(game: &mut Game, c: &Ctx, conn: u64) {
    use super::ability::{ABILITIES, REACH};
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok(mut ab) = game.abilities.get_mut(*e) else {
        return;
    };
    ab.use_up(REACH, c.now, &c.rules.progression);
    send(
        out,
        &ServerMsg::Ability {
            name: ABILITIES[REACH].0,
            ok: true,
            reason: None,
            armed: false,
            ready_in: (ab.ready[REACH] - c.now).max(0.0) as u32,
        },
    );
}

pub(super) fn lay(game: &mut Game, c: &Ctx, conn: u64, fill: f32) {
    let (id, _, _, p) = match who(game, c, conn) {
        Ok(w) => w,
        Err(reason) => return reply(game, conn, &ServerMsg::CarryFail { reason }),
    };
    let r = &c.rules.carry;
    let fill = if fill.is_finite() { fill } else { 1.0 };
    let spot = game.carry.spot_near(r, p);
    match game.carry.lay(r, &id, p, fill, spot, c.now) {
        Ok(l) => {
            let (what, filled) = match &l {
                Laid::Waiting => ("waiting", None),
                Laid::Delivered(f) => ("delivered", Some(*f)),
                Laid::Built(_) => ("built", Some(r.loads)),
                Laid::Back => ("back", None),
            };
            reply(
                game,
                conn,
                &ServerMsg::CarryDone {
                    what,
                    filled,
                    need: r.loads,
                },
            );
            own_load(game, c, conn, &id);
        }
        Err(reason) => reply(game, conn, &ServerMsg::CarryFail { reason }),
    }
}

/// On connect: the own load, the side's loads and site, all buildings.
pub(super) fn hello(
    game: &Game,
    c: &Ctx,
    out: &crate::gateway::Outbox,
    id: Option<[u8; 32]>,
    side: Faction,
) {
    if !game.carry.built.is_empty() {
        send(
            out,
            &ServerMsg::Builds {
                items: game.carry.builds(),
            },
        );
    }
    if !c.rules.carry.enabled {
        return;
    }
    let load = id
        .and_then(|id| game.carry.held(&id))
        .map(|h| CarryView::of(h, &c.rules.carry, c.now));
    send(out, &ServerMsg::Carry { load });
    send(
        out,
        &ServerMsg::Loads {
            view: game.carry.view(&c.rules.carry, side, c.now),
        },
    );
}

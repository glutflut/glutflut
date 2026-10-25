//! Permanent sources (`keep.rs`): place, move or top up, redeem a ticket,
//! free slots without their tickets.

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::sim::{ElementSource, SourceKind};
use crate::web::{send, ServerMsg};

use super::{Ctx, Game};

pub(super) fn keep(game: &mut Game, c: &Ctx, conn: u64, node: i64, ticket: Option<String>) {
    let (now, region, rules) = (c.now, c.region, c.rules);
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok((&faction, pos, mut cd, _, slot, motion)) = game.players.get_mut(*e) else {
        return;
    };
    let reply = |ok: bool, reason: Option<&'static str>, ticket, topup, wait| {
        send(
            out,
            &ServerMsg::Keep {
                node,
                ok,
                reason,
                ticket,
                topup,
                wait,
            },
        )
    };
    let player = slot
        .and_then(|s| game.scores.get(s.0))
        .map(|r| r.id)
        .filter(|_| game.keeps.enabled());
    let Some(player) = player else {
        reply(false, Some("off"), None, false, 0.0);
        return;
    };
    // The player's strength now (level; new identities half): placing
    // and topping up set it, not the claim it came from.
    let mult = slot.filter(|_| rules.progression.enabled).map_or(1.0, |s| {
        rules.progression.strength_factor(game.scores.xp(s.0).1)
    }) * if motion.full_strength() {
        1.0
    } else {
        crate::motion::NEW_STRENGTH
    };
    if motion.locked(now) {
        reply(false, Some("moving"), None, false, motion.lock_left(now));
        return;
    }
    let at = |g: &GeoNode| {
        let w = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
        Vec2::new(w.x, w.z)
    };
    let Some(&ent) = game.node_by_id.get(&node) else {
        reply(false, Some("unknown"), None, false, 0.0);
        return;
    };
    let Ok((g, src)) = game.sources.get(ent) else {
        reply(false, Some("notclaim"), None, false, 0.0);
        return;
    };
    let here = at(g);
    if here.distance(Vec2::new(pos.0.x, pos.0.z)) > rules.claim_radius_m {
        reply(false, Some("range"), None, false, 0.0);
        return;
    }
    let hashes = ticket.as_deref().and_then(crate::keep::hashes);
    if src.kind == SourceKind::Keep {
        // Top-up: only with this source's ticket.
        let Some(mut k) = game
            .keep_info
            .get_mut(ent)
            .ok()
            .filter(|k| hashes.is_some_and(|h| h.0 == k.1.ticket))
        else {
            reply(false, Some("notyours"), None, false, 0.0);
            return;
        };
        if cd.0 > 0.0 {
            reply(false, Some("cooldown"), None, false, cd.0);
            return;
        }
        k.1.wear = 0.0;
        if let Ok((_, mut s)) = game.sources.get_mut(ent) {
            s.top_up(mult);
        }
        cd.0 = rules.claim_cooldown_s;
        game.keeps.dirty = true;
        reply(true, None, None, true, cd.0);
        return;
    }
    // Placing: the player's own claim, of their side, not natural.
    if src.kind != SourceKind::Claim
        || src.life.is_none()
        || src.faction != faction
        || src.by != slot.map(|s| s.0)
        || game.naturals.contains(ent)
    {
        reply(false, Some("notclaim"), None, false, 0.0);
        return;
    }
    let gap = rules.keep.gap_m;
    // Natural sources count even while claimed.
    let crowded = game.naturals.iter().any(|ng| at(ng).distance(here) < gap)
        || game.sources.iter().any(|(og, os)| {
            og.id != node && os.kind == SourceKind::Keep && at(og).distance(here) < gap
        });
    if crowded {
        reply(false, Some("near"), None, false, 0.0);
        return;
    }
    // Moving: give up the source of the ticket sent (one of the player's).
    // Placing: only with a free slot (more by level).
    let level = slot
        .filter(|_| rules.progression.enabled)
        .map_or(1, |s| game.scores.xp(s.0).1);
    let mine = game.keeps.slots(&player);
    let moving = hashes.filter(|h| mine.contains(&h.1));
    if moving.is_none() && mine.len() as u32 >= rules.keep.allowed(level) {
        reply(false, Some("slot"), None, false, 0.0);
        return;
    }
    // Moving takes the tag line along.
    let mut motto = 0u16;
    if let Some((h1, h2)) = moving {
        let old = game
            .keep_info
            .iter()
            .find(|(_, k)| k.ticket == h1)
            .map(|(oe, k)| (oe, *k));
        let pts = match old {
            Some((oe, k)) => {
                motto = game.sources.get(oe).map_or(0, |(_, s)| s.motto);
                if let Some(p) = k.link {
                    unlink(&mut game.keep_info, &mut game.sources, &mut game.keeps, &p);
                }
                game.commands
                    .entity(oe)
                    .remove::<crate::keep::KeepInfo>()
                    .remove::<ElementSource>();
                crate::keep::payable(k.points)
            }
            None => game.keeps.pot_take(&h1).unwrap_or(0),
        };
        if pts > 0 {
            if let Some(s) = slot {
                game.scores.award(s.0, pts);
            }
        }
        game.keeps.clear_slot_if(&player, &h2);
    }
    let (t, h1, h2) = crate::keep::new_ticket();
    if let Ok((_, mut s)) = game.sources.get_mut(ent) {
        // Still building up: the rest of it, then full from now on.
        let rest = (s.ramp - s.built).max(0.0);
        s.kind = SourceKind::Keep;
        s.life = Some(f32::INFINITY);
        s.by = None;
        s.newcomer = false;
        s.mult = mult;
        s.age = 0.0;
        s.ramp = rest;
        s.built = 0.0;
        s.motto = motto;
    }
    game.commands.entity(ent).insert(crate::keep::KeepInfo {
        ticket: h1,
        wear: 0.0,
        points: 0.0,
        link: None,
    });
    game.keeps.add_slot(player, h2);
    // Debug only: a permanent source is a place (log privacy).
    debug!(node, ?faction, "permanent source placed");
    reply(true, None, Some(t), false, cd.0);
}

/// Clear the link of the source with ticket hash `h` (its partner went).
fn unlink(
    keep_info: &mut Query<(Entity, &mut crate::keep::KeepInfo)>,
    sources: &mut Query<(&GeoNode, &mut ElementSource)>,
    keeps: &mut crate::keep::Keeps,
    h: &crate::keep::Hash,
) {
    if let Some((e, mut k)) = keep_info.iter_mut().find(|(_, k)| k.ticket == *h) {
        k.link = None;
        if let Ok((_, mut s)) = sources.get_mut(e) {
            s.linked = false;
        }
        keeps.dirty = true;
    }
}

/// Link two own permanent sources (`[keep] link`), or unlink one (`to`
/// none). Both tickets prove ownership; a source has one partner at most.
pub(super) fn link(
    game: &mut Game,
    c: &Ctx,
    conn: u64,
    node: i64,
    ticket: &str,
    to: Option<i64>,
    to_ticket: Option<&str>,
) {
    let Some((_, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let out = out.clone();
    let reply = |ok: bool, reason: Option<&'static str>| {
        send(&out, &ServerMsg::Link { node, ok, reason });
    };
    if !game.keeps.enabled() || !c.rules.keep.link {
        return reply(false, Some("off"));
    }
    // An own permanent source: its ticket's hash on the node's KeepInfo.
    let own = |game: &Game, node: i64, ticket: &str| -> Option<(Entity, crate::keep::Hash)> {
        let h = crate::keep::hashes(ticket)?.0;
        let &e = game.node_by_id.get(&node)?;
        let (_, k) = game.keep_info.get(e).ok()?;
        (k.ticket == h
            && game
                .sources
                .get(e)
                .is_ok_and(|(_, s)| s.kind == SourceKind::Keep))
        .then_some((e, h))
    };
    let Some((a, ha)) = own(game, node, ticket) else {
        return reply(false, Some("notyours"));
    };
    let Some(to) = to else {
        // Unlink: both ends.
        let partner = game.keep_info.get(a).ok().and_then(|(_, k)| k.link);
        if let Some(p) = partner {
            unlink(&mut game.keep_info, &mut game.sources, &mut game.keeps, &p);
        }
        unlink(&mut game.keep_info, &mut game.sources, &mut game.keeps, &ha);
        return reply(true, None);
    };
    if to == node {
        return reply(false, Some("same"));
    }
    let Some((b, hb)) = to_ticket.and_then(|t| own(game, to, t)) else {
        return reply(false, Some("notyours"));
    };
    // Earlier partners of either end lose their link.
    for e in [a, b] {
        if let Some(p) = game.keep_info.get(e).ok().and_then(|(_, k)| k.link) {
            if p != ha && p != hb {
                unlink(&mut game.keep_info, &mut game.sources, &mut game.keeps, &p);
            }
        }
    }
    for (e, other) in [(a, hb), (b, ha)] {
        if let Ok((_, mut k)) = game.keep_info.get_mut(e) {
            k.link = Some(other);
        }
        if let Ok((_, mut s)) = game.sources.get_mut(e) {
            s.linked = true;
        }
    }
    game.keeps.dirty = true;
    reply(true, None);
}

pub(super) fn redeem(game: &mut Game, c: &Ctx, conn: u64, ticket: &str) {
    let now = c.now;
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let slot = game
        .players
        .get(*e)
        .ok()
        .and_then(|(.., s, _)| s.map(|s| s.0));
    let (Some(slot), Some((h1, h2))) = (slot, crate::keep::hashes(ticket)) else {
        return;
    };
    let Some(player) = game.scores.get(slot).map(|r| r.id) else {
        return;
    };
    if !game.keeps.enabled() || !game.keeps.redeem_ok(player, now) {
        return;
    }
    let (state, node, pts) = if let Some(p) = game.keeps.pot_take(&h1) {
        game.keeps.clear_slot_if(&player, &h2);
        ("fallen", None, p)
    } else if let Some((ke, mut k)) = game.keep_info.iter_mut().find(|(_, k)| k.ticket == h1) {
        let p = crate::keep::payable(k.points);
        k.points -= p as f32;
        let id = game.sources.get(ke).ok().map(|(g, _)| g.id);
        ("standing", id, p)
    } else {
        game.keeps.clear_slot_if(&player, &h2);
        ("gone", None, 0)
    };
    let points = if pts > 0 {
        game.scores.award(slot, pts)
    } else {
        0
    };
    send(
        out,
        &ServerMsg::Kept {
            ticket: ticket.to_string(),
            state,
            node,
            points,
        },
    );
}

pub(super) fn release(game: &mut Game, c: &Ctx, conn: u64, tickets: &[String]) {
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let slot = game
        .players
        .get(*e)
        .ok()
        .and_then(|(.., s, _)| s.map(|s| s.0));
    let Some(player) = slot.and_then(|s| game.scores.get(s)).map(|r| r.id) else {
        return;
    };
    let known: Vec<crate::keep::Hash> = tickets
        .iter()
        .filter_map(|t| crate::keep::hashes(t).map(|h| h.1))
        .collect();
    let day = game.keeps.clock.day(c.now);
    let ok = game.keeps.enabled() && game.keeps.release(player, day, &known);
    send(out, &ServerMsg::Released { ok });
}

/// A tag line for an own permanent source (its ticket), unlocked by the
/// player's level (without levels: those of level 2) and badges.
pub(super) fn motto(
    game: &mut Game,
    c: &Ctx,
    conn: u64,
    node: i64,
    ticket: &str,
    motto: Option<&str>,
) {
    let rules = c.rules;
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok((&faction, _, _, _, slot, _)) = game.players.get(*e) else {
        return;
    };
    let reply = |ok: bool, reason: Option<&'static str>| {
        send(out, &ServerMsg::Motto { node, ok, reason });
    };
    if !game.keeps.enabled() {
        return reply(false, Some("off"));
    }
    let Some(&ent) = game.node_by_id.get(&node) else {
        return reply(false, Some("notyours"));
    };
    let h = crate::keep::hashes(ticket);
    let mine = game
        .keep_info
        .get(ent)
        .is_ok_and(|(_, k)| h.is_some_and(|h| h.0 == k.ticket));
    if !mine {
        return reply(false, Some("notyours"));
    }
    let index = match motto {
        None => 0,
        Some(key) if key.starts_with("c-") => {
            // The city's phrases: anyone with a permanent source while the campaign runs.
            let n = elements_shared::mottos::parse(key);
            let ok = n.filter(|&i| {
                game.campaign.mottos.iter().any(|m| {
                    elements_shared::mottos::CITY_BASE + m.num == i
                        && m.side.is_none_or(|s| s == faction)
                })
            });
            let Some(i) = ok else {
                return reply(false, Some("locked"));
            };
            i
        }
        Some(key) => {
            let level = slot
                .filter(|_| rules.progression.enabled)
                .map_or(2, |s| game.scores.xp(s.0).1);
            let badges = slot.and_then(|s| game.scores.get(s.0)).map(|r| &r.badges);
            let ok = elements_shared::mottos::index(key).filter(|&i| {
                elements_shared::mottos::allowed(
                    i,
                    faction,
                    level,
                    badges.into_iter().flatten().map(String::as_str),
                )
            });
            let Some(i) = ok else {
                return reply(false, Some("locked"));
            };
            i
        }
    };
    if let Ok((_, mut s)) = game.sources.get_mut(ent) {
        if s.kind == SourceKind::Keep {
            s.motto = index;
            game.keeps.dirty = true;
            return reply(true, None);
        }
    }
    reply(false, Some("notyours"));
}

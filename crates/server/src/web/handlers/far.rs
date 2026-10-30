//! Claims through a link into another region (`crate::remote`): here as X
//! (the player stands at own source A, the app sends the far key) and as Y
//! (the key opens a permanent source B here; claim around it).

use std::collections::HashMap;

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::actions;
use crate::gateway::FromWeb;
use crate::protocol::Far;
use crate::remote::{deliver, AddKey, Answer, Ask, Mint, Minted};
use crate::sim::SourceKind;
use crate::web::{send, ServerMsg};

use super::claim::{at_front, link_target, power_of};
use super::{Ctx, Game};

/// No answer from Y after this long: `linkdown`.
const WAIT_S: f64 = 3.0;
/// A claim into a sleeping region waits for it to load (Berlin: ~45 s).
const WAKE_WAIT_S: f64 = 90.0;

/// What waits on other regions.
#[derive(Default)]
pub struct Pending {
    /// Asks of this region waiting for Y, by id.
    asks: HashMap<u64, Waiting>,
    /// As Y: asks that came before the permanent sources were back on the
    /// map, for the next frame.
    later: Vec<Ask>,
    /// As Y: found keys asked for before the permanent sources were back.
    later_mint: Vec<Mint>,
    /// Found keys being made in Y, by id.
    found: HashMap<u64, FoundKey>,
    /// Day (unix days) of each player's last found key: one a day.
    found_day: HashMap<u32, i64>,
}

struct FoundKey {
    conn: u64,
    slot: u32,
    until: f64,
    origin: &'static str,
    region: String,
    key: String,
}

/// A found key that never got made (region gone, database down) is dropped.
const FOUND_WAIT_S: f64 = 120.0;

struct Waiting {
    conn: u64,
    until: f64,
    /// A peek (answered as `peek`), else a claim.
    via: Option<String>,
}

fn refused(out: &crate::gateway::Outbox, reason: &'static str, wait: f32) {
    send(
        out,
        &ServerMsg::Claim {
            node: 0,
            ok: false,
            reason: Some(reason),
            bonus: false,
            points: 0,
            wait,
            build: 0.0,
            doused: false,
            anchor: false,
            far: None,
            near: None,
            streets: None,
            from: None,
        },
    );
}

/// The player of `conn` stands within reach of own permanent source A
/// (ticket `via`) of their side.
fn at_own_source(game: &Game, c: &Ctx, pos: Vec3, faction: Faction, via: &str) -> bool {
    let Some((ha, _)) = crate::keep::hashes(via) else {
        return false;
    };
    let Some((ea, _)) = game.keep_info.iter().find(|(_, k)| k.ticket == ha) else {
        return false;
    };
    game.sources.get(ea).is_ok_and(|(g, s)| {
        let w = project_enu(g.lat, g.lon, c.region.centroid_lat, c.region.centroid_lon);
        s.kind == SourceKind::Keep
            && s.faction == faction
            && Vec2::new(w.x, w.z).distance(Vec2::new(pos.x, pos.z)) <= c.rules.claim_radius_m
    })
}

/// Send an ask to Y for the player of `conn` at own source `via`; false if
/// not at it or Y can't be reached.
fn ask_far(game: &mut Game, c: &Ctx, conn: u64, far: &Far, via: &str, look: bool) -> bool {
    let Some(e) = game.clients.0.get(&conn).map(|(e, _)| *e) else {
        return false;
    };
    let Ok((&faction, pos, _, _, slot, motion)) = game.players.get(e) else {
        return false;
    };
    if !at_own_source(game, c, pos.0, faction, via) {
        return false;
    }
    let power = power_of(
        &game.scores,
        &game.abilities,
        c.rules,
        e,
        slot.map(|s| s.0),
        motion.full_strength(),
    );
    let id = rand::random::<u64>();
    // A peek never wakes Y (`peek` asks only loaded regions); a claim does.
    let asleep = !crate::remote::awake(&game.peers.0, &far.region);
    let sent = deliver(
        &game.peers.0,
        &far.region,
        FromWeb::FarAsk(Ask {
            from: c.region.slug.clone(),
            id,
            key: far.key.clone(),
            faction,
            mult: power.mult,
            factor: c.rules.keep.link_factor,
            newcomer: power.newcomer,
            look,
            tries: 0,
        }),
    );
    if sent {
        game.pending.asks.insert(
            id,
            Waiting {
                conn,
                until: c.now + if asleep { WAKE_WAIT_S } else { WAIT_S },
                via: look.then(|| via.to_string()),
            },
        );
    }
    sent
}

/// X: a claim through the link of `via` into `far.region`.
pub(super) fn claim(game: &mut Game, c: &Ctx, conn: u64, via: &str, far: Far) {
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let (e, out) = (*e, out.clone());
    let Ok((&faction, pos, cd, _, _, motion)) = game.players.get(e) else {
        return;
    };
    if motion.locked(c.now) {
        return refused(&out, "moving", motion.lock_left(c.now));
    }
    if cd.0 > 0.0 {
        return refused(&out, "cooldown", cd.0);
    }
    if !c.rules.keep.link || far.region == c.region.slug {
        return refused(&out, "linked", 0.0);
    }
    if !at_own_source(game, c, pos.0, faction, via) {
        return refused(&out, "linkfar", 0.0);
    }
    if ask_far(game, c, conn, &far, via, false) {
        // The wait starts now; given back if nothing falls.
        if let Ok((_, _, mut cd, ..)) = game.players.get_mut(e) {
            cd.0 = c.rules.claim_cooldown_s;
        }
    } else {
        refused(&out, "linkdown", 0.0);
    }
}

/// X: what a claim through the link of `via` into `far.region` would take.
pub(super) fn peek(game: &mut Game, c: &Ctx, conn: u64, via: String, far: Far) {
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let (e, out) = (*e, out.clone());
    let at_a = game
        .players
        .get(e)
        .is_ok_and(|(&f, pos, ..)| at_own_source(game, c, pos.0, f, &via));
    let reason = if !c.rules.keep.link || far.region == c.region.slug {
        Some("linked")
    } else if !at_a {
        Some("linkfar")
    } else if !crate::remote::awake(&game.peers.0, &far.region) {
        // Sleeping or still loading (or another cell): don't wait for a
        // look; a claim wakes it and waits.
        Some("asleep")
    } else if !ask_far(game, c, conn, &far, &via, true) {
        Some("linkdown")
    } else {
        None
    };
    if reason.is_some() {
        send(
            &out,
            &ServerMsg::Peek {
                via: &via,
                node: None,
                reason,
                lon: None,
                lat: None,
            },
        );
    }
}

/// Y: the key opens a permanent source of the asker's side here; claim (or
/// look) around it with the smaller link factor of both regions.
pub(super) fn ask(game: &mut Game, c: &Ctx, a: Ask) {
    let rules = c.rules;
    // Just woken: the permanent sources come onto the map after this frame's
    // inbox; take it up next frame rather than answer `nokey`.
    if !game.keeps.restored() && a.tries < 100 {
        game.pending.later.push(Ask {
            tries: a.tries + 1,
            ..a
        });
        return;
    }
    let mut ans = Answer {
        id: a.id,
        ..Default::default()
    };
    // The key: the source's own ticket, or a key made for others (a gift).
    let hk = rules
        .keep
        .link
        .then(|| crate::keep::hashes(&a.key))
        .flatten()
        .map(|h| h.0);
    let gift = hk.filter(|h| !game.keep_info.iter().any(|(_, k)| k.ticket == *h));
    let keep = match gift {
        Some(g) => game.keeps.key(&g, c.now).map(|k| k.keep),
        None => hk,
    };
    debug!(
        tries = a.tries,
        restored = game.keeps.restored(),
        keeps = game.keep_info.iter().count(),
        gift = gift.is_some(),
        key_ok = keep.is_some(),
        "far ask"
    );
    let b = keep
        .and_then(|hb| game.keep_info.iter().find(|(_, k)| k.ticket == hb))
        .and_then(|(eb, _)| game.sources.get(eb).ok())
        .filter(|(_, s)| s.kind == SourceKind::Keep && s.faction == a.faction)
        .map(|(g, _)| {
            let w = project_enu(g.lat, g.lon, c.region.centroid_lat, c.region.centroid_lon);
            (Vec2::new(w.x, w.z), g.id)
        });
    let power = actions::Power {
        mult: a.mult * a.factor.min(rules.keep.link_factor),
        newcomer: a.newcomer,
        reach_m: rules.claim_radius_m,
    };
    let target = b.and_then(|(b, b_node)| {
        link_target(
            &game.nodes,
            &game.sources,
            &game.node_by_id,
            c.region,
            rules,
            power,
            b,
            b_node,
            a.faction,
        )
        .map(|t| (b, t))
    });
    match (b, target) {
        (None, _) => ans.reason = Some("nokey"),
        (Some(_), None) => ans.reason = Some("linked"),
        (Some(_), Some((_, (node, lon, lat)))) if a.look => {
            ans.ok = true;
            ans.node = Some(node);
            ans.at = Some([lon, lat]);
        }
        (Some((_, b_node)), Some((b, (node, lon, lat)))) => {
            take(game, c, &a, power, b, node, &mut ans);
            if ans.ok {
                (ans.near, ans.streets) = near(game, c, node, a.faction);
                ans.from = beam(game, b_node, node, a.faction);
            }
            // A gift: one use less; its owner gets a share of the points.
            if let (true, Some(g)) = (ans.ok, gift) {
                game.keeps.use_key(&g);
                let (fire, water) = ans.prior.unwrap_or_default();
                let pts = rules
                    .score
                    .claim_points(ElementState { fire, water }, a.faction, false);
                if let Some(mut k) = game
                    .node_by_id
                    .get(&b_node)
                    .and_then(|&n| game.keep_info.get_mut(n).ok())
                {
                    k.1.points += pts as f32 * rules.keep.gift_share;
                }
            }
            ans.node = Some(node);
            ans.at = Some([lon, lat]);
        }
    }
    if !deliver(&game.peers.0, &a.from, FromWeb::FarAnswer(ans)) {
        warn!(from = %a.from, "far answer not delivered");
    }
}

/// Everyone here sees the claim reach from the permanent source `b_node`
/// to `node`; its (lon, lat).
pub(super) fn beam(game: &Game, b_node: i64, node: i64, faction: Faction) -> Option<[f64; 2]> {
    let ll = |id: i64| {
        game.node_by_id
            .get(&id)
            .and_then(|&e| game.nodes.get(e).ok())
            .map(|(g, _)| [g.lon, g.lat])
    };
    let (from, to) = (ll(b_node)?, ll(node)?);
    crate::web::to_all(
        &game.clients,
        &ServerMsg::Beam {
            from,
            to,
            side: faction.as_str(),
        },
    );
    Some(from)
}

/// Y: the junctions around `node` as (lon, lat, fire, water) and the
/// streets between them (index pairs; `node` itself counts as `faction`'s:
/// its claim may still be building up).
fn near(game: &Game, c: &Ctx, node: i64, faction: Faction) -> (Vec<[f64; 4]>, Vec<[u16; 2]>) {
    let at = |g: &GeoNode| {
        let p = project_enu(g.lat, g.lon, c.region.centroid_lat, c.region.centroid_lon);
        Vec2::new(p.x, p.z)
    };
    let Some(here) = game
        .node_by_id
        .get(&node)
        .and_then(|&e| game.nodes.get(e).ok())
        .map(|(g, _)| at(g))
    else {
        return Default::default();
    };
    let round = |x: f32| (f64::from(x) * 100.0).round() / 100.0;
    // Dense sim index -> position in the list.
    let mut idx: HashMap<usize, u16> = HashMap::new();
    let mut pts = Vec::new();
    for (g, s) in game.nodes.iter() {
        if pts.len() >= 400 || at(g).distance(here) > crate::remote::NEAR_M {
            continue;
        }
        let Some(i) = game.node_by_id.index(g.id) else {
            continue;
        };
        let (f, w) = match (g.id == node, faction) {
            (true, Faction::Fire) => (1.0, 0.0),
            (true, Faction::Water) => (0.0, 1.0),
            _ => (round(s.fire), round(s.water)),
        };
        idx.insert(i, pts.len() as u16);
        pts.push([g.lon, g.lat, f, w]);
    }
    let mut streets = Vec::new();
    if let Some(sim) = &game.sim {
        for (&i, &a) in &idx {
            for j in sim.neighbour_idx(i) {
                if let Some(&b) = idx.get(&j).filter(|_| j > i) {
                    streets.push([a, b]);
                }
            }
        }
    }
    (pts, streets)
}

/// Y: the claim itself, from B; a too strong permanent source wears down.
fn take(
    game: &mut Game,
    c: &Ctx,
    a: &Ask,
    power: actions::Power,
    b: Vec2,
    node: i64,
    ans: &mut Answer,
) {
    let rules = c.rules;
    ans.prior = game
        .node_by_id
        .get(&node)
        .and_then(|&e| game.nodes.get(e).ok())
        .map(|(_, s)| (s.fire, s.water));
    let existing = game
        .node_by_id
        .get(&node)
        .and_then(|&e| game.sources.get(e).ok())
        .map(|(_, s)| *s);
    let ttl = rules.claim_ttl_s
        * if a.newcomer {
            rules.progression.newcomer_ttl_factor
        } else {
            1.0
        };
    let front = rules.reinforce.enabled
        && at_front(
            &game.node_by_id,
            game.sim.as_deref(),
            &game.nodes,
            node,
            a.faction,
        );
    // X keeps the player's wait; this one is only for the call.
    let mut cd = actions::AttackCooldown(0.0);
    let res = actions::claim(
        &mut game.commands,
        &game.node_by_id,
        &mut game.nodes,
        c.region,
        Vec3::new(b.x, 0.0, b.y),
        a.faction,
        &mut cd,
        node,
        rules,
        ttl,
        None,
        existing,
        power,
        game.supply.at(a.faction, game.node_by_id.index(node)),
        front,
    );
    match (res, existing) {
        (Ok(done), _) => {
            ans.ok = true;
            ans.build = done.ramp;
            ans.doused = done.doused;
            // The campaign here counts it; the points go to the player in X.
            let p = ans.prior.unwrap_or_default();
            let prior = ElementState {
                fire: p.0,
                water: p.1,
            };
            let taken = prior.contested() || prior.owner().is_some_and(|o| o != a.faction);
            game.campaign
                .add(a.faction, crate::campaign::Goal::Claims, 1, c.now);
            game.campaign.add(
                a.faction,
                crate::campaign::Goal::Fronts,
                i64::from(taken),
                c.now,
            );
            game.campaign.hold_claim(node, c.now);
        }
        (Err(actions::Reject::Strong), Some(ex))
            if ex.kind == SourceKind::Keep && rules.keep.hit > 0.0 =>
        {
            if let Some(mut k) = game
                .node_by_id
                .get(&node)
                .and_then(|&n| game.keep_info.get_mut(n).ok())
            {
                k.1.wear += rules.keep.hit;
            }
            ans.reason = Some("worn");
        }
        (Err(r), _) => ans.reason = Some(r.as_str()),
    }
}

/// X: Y's answer, to the app that asked.
pub(super) fn answer(game: &mut Game, c: &Ctx, ans: Answer) {
    let Some(w) = game.pending.asks.remove(&ans.id) else {
        return;
    };
    let Some((e, out)) = game.clients.0.get(&w.conn) else {
        return;
    };
    let (e, out) = (*e, out.clone());
    if let Some(via) = w.via {
        let at = ans.at.filter(|_| ans.ok);
        return send(
            &out,
            &ServerMsg::Peek {
                via: &via,
                node: ans.node.filter(|_| ans.ok),
                reason: (!ans.ok).then(|| ans.reason.unwrap_or("linked")),
                lon: at.map(|a| a[0]),
                lat: at.map(|a| a[1]),
            },
        );
    }
    let Ok((&faction, _, mut cd, _, slot, _)) = game.players.get_mut(e) else {
        return;
    };
    if !ans.ok {
        cd.0 = 0.0;
        return refused(&out, ans.reason.unwrap_or("linked"), 0.0);
    }
    let wait = cd.0;
    let points = match (slot.map(|s| s.0), ans.prior) {
        (Some(s), Some((fire, water))) => {
            let pts = c
                .rules
                .score
                .claim_points(ElementState { fire, water }, faction, false);
            let pts = game.scores.award(s, pts);
            game.scores.claimed(s);
            pts
        }
        _ => 0,
    };
    send(
        &out,
        &ServerMsg::Claim {
            node: ans.node.unwrap_or(0),
            ok: true,
            reason: None,
            bonus: false,
            points,
            wait,
            build: ans.build,
            doused: ans.doused,
            anchor: false,
            far: ans.at,
            near: Some(ans.near),
            streets: Some(ans.streets),
            from: ans.from,
        },
    );
}

/// Y: asks held back last frame (sources not yet on the map) come up again.
pub(super) fn later(game: &mut Game, c: &Ctx) {
    for a in std::mem::take(&mut game.pending.later) {
        ask(game, c, a);
    }
    for m in std::mem::take(&mut game.pending.later_mint) {
        mint(game, c, m);
    }
}

/// X: after an ordinary claim, rarely a found key: Funkenflug (fire near a
/// hilltop: a region nearby) or Flaschenpost (water at a big river: a region
/// downstream); at most one a day per player.
pub(super) fn found(
    game: &mut Game,
    c: &Ctx,
    conn: u64,
    slot: Option<u32>,
    node: i64,
    faction: Faction,
) {
    let r = &c.rules.keep;
    let Some(slot) = slot.filter(|_| r.link) else {
        return;
    };
    let day = (c.now / 86_400.0).floor() as i64;
    if game.pending.found_day.get(&slot) == Some(&day) {
        return;
    }
    let host = game.peers.0.clone();
    let (origin, chance, cands) = match faction {
        Faction::Fire => {
            // Near a hilltop (the hilltop itself holds fire's natural source).
            let at = |e: Entity| {
                game.nodes.get(e).ok().map(|(g, _)| {
                    let p = project_enu(g.lat, g.lon, c.region.centroid_lat, c.region.centroid_lon);
                    Vec2::new(p.x, p.z)
                })
            };
            let here = game.node_by_id.get(&node).and_then(|&e| at(e));
            let peak = here.zip(game.sim.as_deref()).is_some_and(|(h, sim)| {
                sim.gathers(Faction::Fire)
                    .filter_map(at)
                    .any(|p| p.distance(h) <= c.rules.claim_radius_m)
            });
            if !peak {
                debug!("found: no hilltop near");
                return;
            }
            let near = crate::found::nearby(
                &host,
                &c.region.slug,
                c.region.centroid_lat,
                c.region.centroid_lon,
                f64::from(r.ember_km),
            );
            ("ember", r.ember_chance, near)
        }
        Faction::Water => {
            let Some(rv) = game.rivers.as_deref() else {
                return;
            };
            ("bottle", r.bottle_chance, rv.downstream(node).to_vec())
        }
    };
    debug!(origin, cands = cands.len(), "found: candidates");
    if cands.is_empty() || rand::random::<f32>() >= chance {
        return;
    }
    // Nearer regions likelier (downstream: the next ones along the river).
    let Some(y) = crate::found::pick(&cands).cloned() else {
        return;
    };
    game.pending.found_day.insert(slot, day);
    let (key, kh, _) = crate::keep::new_ticket();
    let id = rand::random::<u64>();
    game.pending.found.insert(
        id,
        FoundKey {
            conn,
            slot,
            until: c.now + FOUND_WAIT_S,
            origin,
            region: y.clone(),
            key: key.clone(),
        },
    );
    let from = c.region.slug.clone();
    if host.peek(&y).is_some() {
        // Running or loading: Y makes it (messages wait for the load).
        deliver(
            &host,
            &y,
            FromWeb::FarMint(Mint {
                from,
                id,
                key,
                faction,
                origin,
            }),
        );
        return;
    }
    // Asleep: straight into the database, without waking it.
    let Some(db) = host.db().map(String::from) else {
        return;
    };
    let (uses, days, now) = (r.found_uses, f64::from(r.found_days), c.now);
    std::thread::spawn(move || {
        let res =
            crate::found::write_key(&db, &y, faction.as_str(), origin, &kh, uses as i32, days);
        if let Err(e) = &res {
            warn!(error = %e, "found key: write failed");
        }
        let keep = res.ok().flatten();
        // Woke meanwhile: it read its keys before this one.
        if let (Some(k), Some(_)) = (&keep, host.peek(&y)) {
            if let Ok(keep) = crate::keep::Hash::try_from(k.as_slice()) {
                let key = crate::keep::Key {
                    keep,
                    origin,
                    uses,
                    made: now,
                    expires: now + days * 86_400.0,
                };
                deliver(&host, &y, FromWeb::FarKey(AddKey { hash: kh, key }));
            }
        }
        let ok = keep.is_some();
        deliver(&host, &from, FromWeb::FarMinted(Minted { id, ok }));
    });
}

/// Y: make a found key to a random permanent source of the asker's side.
pub(super) fn mint(game: &mut Game, c: &Ctx, m: Mint) {
    if !game.keeps.restored() {
        game.pending.later_mint.push(m);
        return;
    }
    let r = &c.rules.keep;
    let mine: Vec<crate::keep::Hash> = game
        .keep_info
        .iter()
        .filter(|(e, _)| {
            game.sources
                .get(*e)
                .is_ok_and(|(_, s)| s.kind == SourceKind::Keep && s.faction == m.faction)
        })
        .map(|(_, k)| k.ticket)
        .collect();
    let ok = match crate::keep::hashes(&m.key) {
        Some((kh, _)) if !mine.is_empty() => {
            let keep = mine[rand::random::<u32>() as usize % mine.len()];
            game.keeps.add_key(
                kh,
                crate::keep::Key {
                    keep,
                    origin: m.origin,
                    uses: r.found_uses,
                    made: c.now,
                    expires: c.now + f64::from(r.found_days) * 86_400.0,
                },
            );
            true
        }
        _ => false,
    };
    deliver(
        &game.peers.0,
        &m.from,
        FromWeb::FarMinted(Minted { id: m.id, ok }),
    );
}

/// X: the found key exists; to the player (if still here).
pub(super) fn minted(game: &mut Game, m: Minted) {
    let Some(f) = game.pending.found.remove(&m.id) else {
        return;
    };
    if !m.ok {
        // No permanent source of that side there: a later claim may try again.
        game.pending.found_day.remove(&f.slot);
        return;
    }
    let Some((_, out)) = game.clients.0.get(&f.conn) else {
        return;
    };
    let name = crate::found::name(&game.peers.0, &f.region);
    send(
        out,
        &ServerMsg::Found {
            origin: f.origin,
            region: f.region,
            name,
            key: f.key,
        },
    );
}

/// X: asks Y never answered: `linkdown` (a claim gets its wait back).
pub(super) fn expire(game: &mut Game, c: &Ctx) {
    game.pending.found.retain(|_, f| f.until > c.now);
    if game.pending.asks.is_empty() {
        return;
    }
    let late: Vec<u64> = game
        .pending
        .asks
        .iter()
        .filter(|(_, w)| w.until < c.now)
        .map(|(&id, _)| id)
        .collect();
    for id in late {
        let Some(w) = game.pending.asks.remove(&id) else {
            continue;
        };
        let Some((e, out)) = game.clients.0.get(&w.conn) else {
            continue;
        };
        let (e, out) = (*e, out.clone());
        match w.via {
            Some(via) => send(
                &out,
                &ServerMsg::Peek {
                    via: &via,
                    node: None,
                    reason: Some("linkdown"),
                    lon: None,
                    lat: None,
                },
            ),
            None => {
                if let Ok((_, _, mut cd, ..)) = game.players.get_mut(e) {
                    cd.0 = 0.0;
                }
                refused(&out, "linkdown", 0.0);
            }
        }
    }
}

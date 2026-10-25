//! A claim: strength, tunnels, points, finds, armed abilities, the challenge.

use std::collections::HashMap;

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::actions;
use crate::challenge::ClaimInfo;
use crate::sim::SourceKind;
use crate::web::{send, send_challenge, uncover_near, ServerMsg};

use super::ability::{Abilities, ABILITIES, DOUBLE, REACH};
use super::{Ctx, Game};

/// Open tunnels by connection (far node, open until) and when each score
/// slot last got one.
#[derive(Default)]
pub struct Tunnels {
    /// By player (`tunnel_key`): survives a reconnect (phones drop sockets).
    pub(super) open: HashMap<u64, (i64, f64)>,
    last: HashMap<u32, f64>,
}

/// Open tunnels by score slot, else (bots) by connection.
fn tunnel_key(conn: u64, slot: Option<u32>) -> u64 {
    slot.map_or(conn, |s| (1 << 48) | u64::from(s))
}

/// A street neighbour of `node` isn't `faction`'s (`[reinforce]`).
fn at_front(
    by_id: &crate::sim::NodeById,
    sim: Option<&crate::sim::ElementSim>,
    nodes: &Query<(&GeoNode, &mut ElementState), Without<Player>>,
    node: i64,
    faction: Faction,
) -> bool {
    by_id
        .index(node)
        .zip(sim)
        .map(|(t, sim)| sim.neighbours(t))
        .unwrap_or_default()
        .into_iter()
        .any(|n| {
            nodes
                .get(n)
                .is_ok_and(|(_, st)| st.owner() != Some(faction))
        })
}

/// Where a claim on own ground at `node` would be refused (anchor too
/// close, not at the front): the nearest own junction without a source at
/// the front within the claim radius, along the streets. `None`: keep `node`.
#[allow(clippy::too_many_arguments)]
fn front_spot(
    by_id: &crate::sim::NodeById,
    sim: Option<&crate::sim::ElementSim>,
    nodes: &Query<(&GeoNode, &mut ElementState), Without<Player>>,
    sources: &Query<(&GeoNode, &mut crate::sim::ElementSource)>,
    supply: &crate::hinterland::Supply,
    rules: &elements_shared::rules::Rules,
    region: &RegionInfo,
    pos: Vec3,
    faction: Faction,
    node: i64,
) -> Option<i64> {
    let sim = sim?;
    let mine = |e: Entity| {
        nodes
            .get(e)
            .is_ok_and(|(_, st)| st.owner() == Some(faction))
    };
    let free = |e: Entity| sources.get(e).is_err();
    let start = *by_id.get(&node)?;
    let idx = by_id.index(node)?;
    let needs = mine(start)
        && free(start)
        && supply.at(faction, Some(idx)) <= rules.supply.anchor_gap_m
        && !at_front(by_id, Some(sim), nodes, node, faction);
    if !needs {
        return None;
    }
    let near = |e: Entity| {
        nodes.get(e).is_ok_and(|(g, _)| {
            let p = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
            Vec2::new(pos.x, pos.z).distance(Vec2::new(p.x, p.z)) <= rules.claim_radius_m
        })
    };
    let mut seen = std::collections::HashSet::from([start]);
    let mut looked = 0usize;
    let mut queue = std::collections::VecDeque::from([(start, idx)]);
    while let Some((_, i)) = queue.pop_front() {
        if seen.len() > 2000 {
            break;
        }
        for n in sim.neighbours(i) {
            if !seen.insert(n) || !near(n) {
                continue;
            }
            let Ok((g, _)) = nodes.get(n) else { continue };
            let Some(ni) = by_id.index(g.id) else {
                continue;
            };
            looked += 1;
            if mine(n) && free(n) && at_front(by_id, Some(sim), nodes, g.id, faction) {
                return Some(g.id);
            }
            queue.push_back((n, ni));
        }
    }
    debug!(looked, "reinforce: no front in reach");
    None
}

/// The far end of a link (`[keep] link`): standing within reach of own
/// permanent source A (its ticket `via`), its partner B as (position, node).
fn link_partner(
    keep_info: &Query<(Entity, &mut crate::keep::KeepInfo)>,
    sources: &Query<(&GeoNode, &mut crate::sim::ElementSource)>,
    region: &RegionInfo,
    rules: &elements_shared::rules::Rules,
    here: Vec2,
    via: &str,
) -> Option<(Vec2, i64)> {
    let (ha, _) = crate::keep::hashes(via)?;
    let at = |g: &GeoNode| {
        let w = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
        Vec2::new(w.x, w.z)
    };
    let (ea, ka) = keep_info.iter().find(|(_, k)| k.ticket == ha)?;
    let (ga, _) = sources.get(ea).ok()?;
    if at(ga).distance(here) > rules.claim_radius_m {
        return None;
    }
    let hb = ka.link?;
    let (eb, _) = keep_info.iter().find(|(_, k)| k.ticket == hb)?;
    let (gb, sb) = sources.get(eb).ok()?;
    (sb.kind == SourceKind::Keep).then(|| (at(gb), gb.id))
}

/// The nearest junction within `radius` of B that isn't `faction`'s (never
/// B itself): (id, lon, lat).
fn link_target(
    nodes: &Query<(&GeoNode, &mut ElementState), Without<Player>>,
    region: &RegionInfo,
    radius: f32,
    b: Vec2,
    b_node: i64,
    faction: Faction,
) -> Option<(i64, f64, f64)> {
    nodes
        .iter()
        .filter(|(g, _)| g.id != b_node)
        .filter(|(_, s)| s.owner() != Some(faction))
        .map(|(g, _)| {
            let w = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
            (g, Vec2::new(w.x, w.z).distance(b))
        })
        .filter(|&(_, d)| d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(g, _)| (g.id, g.lon, g.lat))
}

/// What a claim through the link of `via` would take right now (the app's
/// second button and its camera move): the target around the partner, if any.
pub(super) fn peek(game: &mut Game, c: &Ctx, conn: u64, via: &str) {
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok((&faction, pos, ..)) = game.players.get(*e) else {
        return;
    };
    let target = c
        .rules
        .keep
        .link
        .then_some(())
        .and_then(|_| {
            link_partner(
                &game.keep_info,
                &game.sources,
                c.region,
                c.rules,
                Vec2::new(pos.0.x, pos.0.z),
                via,
            )
        })
        .and_then(|(b, b_node)| {
            link_target(
                &game.nodes,
                c.region,
                c.rules.claim_radius_m,
                b,
                b_node,
                faction,
            )
        });
    send(
        out,
        &ServerMsg::Peek {
            via,
            node: target.map(|t| t.0),
            lon: target.map(|t| t.1),
            lat: target.map(|t| t.2),
        },
    );
}

pub(super) fn claim(game: &mut Game, c: &Ctx, conn: u64, node: i64, via: Option<&str>) {
    let (now, region, rules) = (c.now, c.region, c.rules);
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok((&faction, pos, mut cd, tracked, slot, motion)) = game.players.get_mut(*e) else {
        return;
    };
    let pseudo = tracked.as_ref().map(|t| t.pseudo);
    if motion.locked(now) {
        send(
            out,
            &ServerMsg::Claim {
                node,
                ok: false,
                reason: Some("moving"),
                bonus: false,
                points: 0,
                wait: motion.lock_left(now),
                build: 0.0,
                doused: false,
                anchor: false,
            },
        );
        return;
    }
    let slot = slot.map(|s| s.0);
    // Through a link (`[keep] link`): standing at own permanent source A
    // (its ticket in `via`), the claim reaches around its partner B: the
    // range counts from B, the strength is `link_factor` of it.
    let link = via.filter(|_| rules.keep.link).and_then(|v| {
        link_partner(
            &game.keep_info,
            &game.sources,
            region,
            rules,
            Vec2::new(pos.0.x, pos.0.z),
            v,
        )
    });
    // The app may not know the junctions around B (not in view): asked for
    // B itself, the server picks the nearest junction around it that isn't
    // the player's side's. Nothing there: `linked`.
    let node = match link {
        Some((b, b_node)) if node == b_node => {
            let pick = link_target(
                &game.nodes,
                region,
                rules.claim_radius_m,
                b,
                b_node,
                faction,
            );
            match pick {
                Some((id, _, _)) => id,
                None => {
                    send(
                        out,
                        &ServerMsg::Claim {
                            node,
                            ok: false,
                            reason: Some("linked"),
                            bonus: false,
                            points: 0,
                            wait: 0.0,
                            build: 0.0,
                            doused: false,
                            anchor: false,
                        },
                    );
                    return;
                }
            }
        }
        _ => node,
    };
    let link = link.map(|(b, _)| b);
    // Reinforcing: own ground where no anchor fits but not at the front goes
    // to the nearest own junction without a source at the front, in reach.
    let node = if link.is_none() && rules.reinforce.enabled && rules.supply.enabled {
        front_spot(
            &game.node_by_id,
            game.sim.as_deref(),
            &game.nodes,
            &game.sources,
            &game.supply,
            rules,
            region,
            pos.0,
            faction,
            node,
        )
        .unwrap_or(node)
    } else {
        node
    };
    let prior = game
        .node_by_id
        .get(&node)
        .and_then(|&e| game.nodes.get(e).ok())
        .map(|(_, s)| ElementState {
            fire: s.fire,
            water: s.water,
        });
    let existing = game
        .node_by_id
        .get(&node)
        .and_then(|&e| game.sources.get(e).ok())
        .map(|(_, s)| *s);
    let bonus = game.hotspots.bonus(pos.0, rules.hotspot_radius_m);
    // Levels: strength by level, Rückenwind on the first day.
    let power = match slot.filter(|_| rules.progression.enabled) {
        Some(s) => actions::Power {
            mult: rules.progression.strength_factor(game.scores.xp(s).1),
            newcomer: game.scores.newcomer(s),
            reach_m: if game.abilities.get(*e).is_ok_and(|a| a.armed[REACH]) {
                rules.progression.reach_m
            } else {
                rules.claim_radius_m
            },
        },
        None => actions::Power::plain(rules),
    };
    // New identities claim weaker until they played a while.
    let power = if motion.full_strength() {
        power
    } else {
        actions::Power {
            mult: power.mult * crate::motion::NEW_STRENGTH,
            ..power
        }
    };
    // Through an open tunnel: any distance, no cooldown.
    let tunnel = game
        .tunnels
        .open
        .get(&tunnel_key(conn, slot))
        .is_some_and(|&(n, until)| n == node && now < until);
    let power = if tunnel {
        actions::Power {
            reach_m: f32::MAX,
            ..power
        }
    } else if link.is_some() {
        actions::Power {
            mult: power.mult * rules.keep.link_factor,
            ..power
        }
    } else {
        power
    };
    let from = link.map_or(pos.0, |b| Vec3::new(b.x, pos.0.y, b.y));
    let saved_cd = cd.0;
    if tunnel {
        cd.0 = 0.0;
    }
    let ttl = rules.claim_ttl_s
        * if bonus { rules.hotspot_ttl_factor } else { 1.0 }
        * if power.newcomer {
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
            faction,
        );
    let res = actions::claim(
        &mut game.commands,
        &game.node_by_id,
        &mut game.nodes,
        region,
        from,
        faction,
        &mut cd,
        node,
        rules,
        ttl,
        slot,
        existing,
        power,
        game.supply.at(faction, game.node_by_id.index(node)),
        front,
    );
    // Too strong to take: a permanent source still wears down.
    let res = match (res, existing) {
        (Err(actions::Reject::Strong), Some(ex))
            if ex.kind == SourceKind::Keep && rules.keep.hit > 0.0 && cd.0 <= 0.0 =>
        {
            if let Some(mut k) = game
                .node_by_id
                .get(&node)
                .and_then(|&n| game.keep_info.get_mut(n).ok())
            {
                k.1.wear += rules.keep.hit;
                cd.0 = rules.claim_cooldown_s;
            }
            Err(actions::Reject::Worn)
        }
        (r, _) => r,
    };
    if tunnel {
        if res.is_ok() {
            game.tunnels.open.remove(&tunnel_key(conn, slot));
        } else {
            cd.0 = saved_cd;
        }
    }
    let points = match (res, slot, prior) {
        (Ok(c), Some(s), Some(p)) => {
            // Hinterland: putting out a spark, placing an anchor.
            let pts = rules.score.claim_points(p, faction, bonus)
                + if c.doused { rules.sparks.points } else { 0 }
                + if c.anchor {
                    rules.supply.anchor_points
                } else {
                    0
                };
            let pts = game.scores.award(s, pts);
            game.scores.claimed(s);
            game.marble.add(now, node, s);
            // Campaign: claims, and those taken from the other side.
            let front = p.contested() || p.owner().is_some_and(|o| o != faction);
            let a = game
                .campaign
                .add(faction, crate::campaign::Goal::Claims, 1, now);
            let b = game.campaign.add(
                faction,
                crate::campaign::Goal::Fronts,
                i64::from(front),
                now,
            );
            let c = game.campaign.hold_claim(node, now);
            if let Some(key) = a.or(b).or(c) {
                game.scores.grant(s, &format!("chapter:{key}"));
            }
            pts
        }
        _ => 0,
    };
    // Finds near the claimed junction.
    let at = game
        .node_by_id
        .get(&node)
        .and_then(|&n| game.nodes.get(n).ok())
        .map(|(g, _)| {
            let p = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
            Vec2::new(p.x, p.z)
        });
    if let (true, false, Some(s), Some(at), Some(f)) =
        (res.is_ok(), tunnel, slot, at, game.finds.as_deref())
    {
        uncover_near(
            f,
            rules.finds.uncover_m,
            at,
            s,
            faction,
            &mut game.scores,
            &mut game.campaign,
            out,
            now,
        );
    }
    // Rarely a tunnel opens (not from a tunnel claim).
    if let (true, false, Some(s), Some(sim)) = (res.is_ok(), tunnel, slot, game.sim.as_deref()) {
        let t = &rules.tunnel;
        let last = game.tunnels.last.get(&s).copied();
        if t.chance > 0.0
            && last.is_none_or(|l| now - l > t.cooldown_s as f64)
            && rand::random::<f32>() < t.chance
        {
            let enemy = match faction {
                Faction::Fire => Faction::Water,
                Faction::Water => Faction::Fire,
            };
            let me = Vec2::new(pos.0.x, pos.0.z);
            let far: Vec<(i64, f64, f64, f32)> = sim
                .gathers(enemy)
                .filter_map(|e| {
                    let (g, st) = game.nodes.get(e).ok()?;
                    // The enemy's field there must be weaker than the claim
                    // will be, or it never turns (sinks near the shore).
                    let held = match enemy {
                        Faction::Fire => st.fire,
                        Faction::Water => st.water,
                    };
                    // Its peak: full once built up, but fading meanwhile.
                    let peak = rules
                        .source_strength(rules.claim_ramp(held), rules.claim_ttl_s)
                        .unwrap_or(0.0)
                        * power.mult;
                    if st.owner() != Some(enemy)
                        || held >= t.enemy_below * peak
                        || game
                            .sources
                            .get(e)
                            .is_ok_and(|(_, ex)| actions::holds(ex, rules, power))
                    {
                        return None;
                    }
                    let p = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
                    let d = me.distance(Vec2::new(p.x, p.z));
                    (t.min_m..=t.max_m)
                        .contains(&d)
                        .then_some((g.id, g.lat, g.lon, d))
                })
                .collect();
            if !far.is_empty() {
                let (id, lat, lon, d) = far[rand::random_range(0..far.len())];
                game.tunnels.last.insert(s, now);
                game.tunnels.open.retain(|_, v| v.1 > now);
                game.tunnels
                    .open
                    .insert(tunnel_key(conn, Some(s)), (id, now + t.window_s as f64));
                debug!(node = id, m = d as u32, ?faction, "tunnel opened");
                send(
                    out,
                    &ServerMsg::Tunnel {
                        node: id,
                        lat,
                        lon,
                        kind: if enemy == Faction::Fire {
                            "peak"
                        } else {
                            "sink"
                        },
                        m: d as u32,
                        secs: t.window_s as u32,
                    },
                );
            }
        }
    }
    if let (true, Some(p)) = (res.is_ok(), pseudo) {
        game.agg.claimed(p, now);
    }
    // Armed abilities take effect with a successful claim.
    if res.is_ok() && !tunnel {
        if let Ok(mut ab) = game.abilities.get_mut(*e) {
            let used = |ab: &Abilities, i: usize| ServerMsg::Ability {
                name: ABILITIES[i].0,
                ok: true,
                reason: None,
                armed: false,
                ready_in: (ab.ready[i] - now).max(0.0) as u32,
            };
            if ab.armed[REACH] {
                ab.use_up(REACH, now, &rules.progression);
                send(out, &used(&ab, REACH));
            }
            if ab.armed[DOUBLE] {
                ab.use_up(DOUBLE, now, &rules.progression);
                send(out, &used(&ab, DOUBLE));
                // Funkenflug / Welle: a neighbouring junction too.
                let target = game.node_by_id.index(node);
                let next = target
                    .zip(game.sim.as_deref())
                    .map(|(t, sim)| sim.neighbours(t))
                    .unwrap_or_default();
                let saved = cd.0;
                for n in next {
                    let Ok((g, _)) = game.nodes.get(n) else {
                        continue;
                    };
                    let id = g.id;
                    let ex = game.sources.get(n).ok().map(|(_, s)| *s);
                    cd.0 = 0.0;
                    let r = actions::claim(
                        &mut game.commands,
                        &game.node_by_id,
                        &mut game.nodes,
                        region,
                        pos.0,
                        faction,
                        &mut cd,
                        id,
                        rules,
                        ttl,
                        slot,
                        ex,
                        actions::Power {
                            reach_m: f32::MAX,
                            ..power
                        },
                        f32::INFINITY,
                        false,
                    );
                    if r.is_ok() {
                        break;
                    }
                }
                cd.0 = saved;
            }
        }
    }
    // XP and the personal challenge (rule sets with progression).
    if let (true, Some(s), Some(ch)) = (
        res.is_ok() && rules.progression.enabled,
        slot,
        game.challenges.as_deref_mut(),
    ) {
        let info = ClaimInfo {
            pos: pos.0,
            front: prior.is_some_and(|p| p.contested()),
            retake: existing.is_some_and(|ex| ex.faction != faction),
        };
        for ev in ch.on_claim(conn, s, info, now, &rules.progression, &mut game.scores) {
            send_challenge(out, ev);
        }
    }
    send(
        out,
        &ServerMsg::Claim {
            node,
            ok: res.is_ok(),
            reason: res.as_ref().err().map(|r| r.as_str()),
            bonus: bonus && res.is_ok(),
            points,
            wait: cd.0,
            build: res.map_or(0.0, |c| c.ramp),
            doused: res.is_ok_and(|c| c.doused),
            anchor: res.is_ok_and(|c| c.anchor),
        },
    );
}

//! A browser's connection: hello, the tiles it holds, positions, goodbye.

use std::collections::HashSet;

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::actions::AttackCooldown;
use crate::analytics::{Profile, Tracked};
use crate::gateway::Outbox;
use crate::motion::Motion;
use crate::score::{Global, ScoreSlot};
use crate::tiles::TileKey;
use crate::web::{
    chapter_msg, q, send, send_finds, source_item, source_list, totals, uncover_near, ServerMsg,
    WebConn,
};
use crate::NextPlayerId;

use super::{Abilities, Ctx, Game};

/// Shadow ban after a made-up track (`plausible.rs`). Anti-cheat, not game
/// balance, so not a rule a rule file could weaken.
const SHADOW_BAN_S: f64 = 86_400.0;

pub(super) fn hello(
    game: &mut Game,
    c: &Ctx,
    conn: u64,
    token: Option<String>,
    global: Option<Global>,
    bot: bool,
    by_tiles: bool,
    pow_ok: bool,
    watch: bool,
    out: Outbox,
) {
    let (now, rules) = (c.now, c.rules);
    // One player per socket: a second hello would leave the first
    // entity behind and replay the whole state.
    if game.clients.0.contains_key(&conn) {
        debug!(conn, "hello again, ignored");
        return;
    }
    // Bots (synthetic, no points) only on test servers.
    let bot = bot && game.allow_sim.0;
    // A watcher: an entity for the socket, but no token, row or count.
    let token = token.filter(|_| !watch);
    // Played on an earlier day: counts in the analytics at once.
    let today = game.scores.clock.day(now);
    let returning = global
        .as_ref()
        .is_some_and(|g| g.xp_prev > 0.0 || (g.xp_day < today && g.claims > 0));
    // Bots stay in memory; players keep a score row (see score.rs),
    // as stored if they played before (here or elsewhere).
    let known = match (token.as_deref(), global) {
        (Some(t), Some(g)) if !bot => Some(game.scores.adopt(t, g)),
        (t, _) => t.and_then(|t| game.scores.find(t)),
    };
    let fresh = !token
        .as_deref()
        .is_some_and(|t| known.is_some() || game.sessions.knows(t));
    // A new identity costs a proof of work first (pow.rs); the
    // client solves it and says hello again.
    if fresh && !bot && !watch && !pow_ok {
        send(
            &out,
            &ServerMsg::Pow {
                c: &crate::pow::challenge(),
                bits: crate::pow::bits(),
            },
        );
        return;
    }
    let token = token
        .filter(|t| known.is_some() || game.sessions.knows(t))
        .unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
    let faction = match (
        known.and_then(|s| game.scores.get(s)),
        game.sessions.0.get(&token).map(|(f, _)| f),
    ) {
        (Some(r), _) => r.faction,
        (None, Some(&f)) => f,
        (None, None) => {
            let land = game.sim.as_deref().map(|sim| match totals(sim) {
                ServerMsg::Totals { fire, water, .. } => [fire, water],
                _ => [0, 0],
            });
            join_side(
                game.scores.active_today(),
                land.unwrap_or([0, 0]),
                rules.join_weaker,
                rand::random::<f32>(),
            )
        }
    };
    if !watch {
        game.sessions.remember(token.clone(), faction, now);
    }
    let slot =
        (!bot && !watch).then(|| known.unwrap_or_else(|| game.scores.create(&token, faction)));
    let tracked = Tracked {
        pseudo: game.agg.pseudonym(&token, now),
        synthetic: bot || watch,
        counted: !watch && !slot.is_some_and(|s| game.scores.shadowed(s, now)),
        last_rate: 0.0,
        profile: Profile::default(),
    };
    if (returning || slot.is_some_and(|s| game.scores.presence(s) > 0)) && !bot {
        game.agg.standing.establish(tracked.pseudo, now);
    }
    // A brand-new identity: time to its first claim (analytics `joined`/`quick`).
    if known.is_none() && slot.is_some() {
        game.agg.joined(tracked.pseudo, now);
    }
    // Full strength at once for returning players, those confirmed
    // on site, and on test servers.
    let trusted =
        returning || slot.is_some_and(|s| game.scores.presence(s) > 0) || game.allow_sim.0;
    let state = game.parked.resume(slot, now, trusted);
    let entity = spawn_player(
        &mut game.commands,
        &mut game.next_id,
        faction,
        conn,
        tracked,
        state,
    );
    if let Some(slot) = slot {
        game.scores.touch(slot);
        game.commands.entity(entity).insert(ScoreSlot(slot));
    }
    send(
        &out,
        &ServerMsg::Welcome {
            token: if watch { "" } else { &token },
            faction: faction.as_str(),
            sim: game.allow_sim.0,
            levels: rules.progression.enabled,
            abilities: rules.progression.enabled && rules.progression.abilities,
            reach_m: rules.progression.reach_m,
            supply: rules.supply.enabled,
            anchor_gap_m: rules.supply.anchor_gap_m,
            reinforce: rules.supply.enabled && rules.reinforce.enabled,
            keep: game.keeps.enabled(),
            keep_used: slot
                .and_then(|s| game.scores.get(s))
                .map_or(0, |r| game.keeps.slots(&r.id).len() as u32),
            keep_base: rules.keep.per_player,
            keep_per_level: if rules.progression.enabled {
                rules.keep.per_level
            } else {
                0
            },
            keep_gap_m: rules.keep.gap_m,
            mottos: crate::protocol::MottoItem::all(&game.campaign.mottos),
            hotspot_m: rules.hotspot_radius_m,
            version: crate::gateway::version(),
            watch,
        },
    );
    // By tiles: state follows with the first `view`.
    if by_tiles && game.tiles.is_some() {
        game.views.0.insert(conn, Some(HashSet::new()));
    } else {
        game.views.0.insert(conn, None);
        let snapshot = game
            .nodes
            .iter()
            .map(|(g, s)| (g.id, q(s.fire), q(s.water)))
            .collect();
        send(&out, &ServerMsg::State { nodes: snapshot });
    }
    if let Some(t) = game.sim.as_deref().map(totals) {
        send(&out, &t);
    }
    send(
        &out,
        &ServerMsg::Hotspots {
            items: game.hotspots.list(now),
        },
    );
    if let Some(mine) = slot.and_then(|s| game.scores.mine(s)) {
        send(&out, &ServerMsg::Score(mine));
    }
    if let (Some(f), Some(s)) = (game.finds.as_deref(), slot) {
        let running = game.campaign.running(now);
        send_finds(&out, f, &game.scores, s, &running, now);
    }
    if let Some(s) = slot {
        game.scores.catch_up(s, &game.campaign.past);
    }
    if let Some(m) = chapter_msg(&game.campaign, now, rules.progression.enabled) {
        send(&out, &m);
    }
    if !game.campaign.notices.is_empty() {
        send(
            &out,
            &ServerMsg::Notices {
                items: &game.campaign.notices,
            },
        );
    }
    let live = game.live_rain.as_ref().is_some_and(|l| l.on);
    let sun = game.live_sun.as_ref().is_some_and(|l| l.on);
    if let Some(w) = crate::weather::shown(rules, game.campaign.weather, live, sun) {
        send(&out, &ServerMsg::Weather(w));
    }
    if let Some(t) = game.tide.as_ref().and_then(|t| t.shown()) {
        send(&out, &ServerMsg::Tide(t));
    }
    // By tiles: the sources follow with each `view`, like the state.
    if !(by_tiles && game.tiles.is_some()) {
        send(
            &out,
            &ServerMsg::Sources {
                items: source_list(game.sources.iter(), slot, rules, game.sim.as_deref()),
            },
        );
    }
    crate::cheer::greet(&game.cheers, &out);
    game.clients.0.insert(conn, (entity, out));
    info!(conn, ?faction, bot, watch, "web player connected");
}

pub(super) fn overview(game: &mut Game, conn: u64, on: bool) {
    let Some((_, out)) = game.clients.0.get(&conn) else {
        return;
    };
    if !on {
        game.overview.subs.remove(&conn);
    } else if game.overview.subs.insert(conn) && !game.overview.rows.is_empty() {
        let tiles = game.overview.rows.clone();
        send(out, &ServerMsg::Overview { tiles, full: true });
    }
}

pub(super) fn view(game: &mut Game, c: &Ctx, conn: u64, keys: Vec<TileKey>) {
    let (Some(&(e, ref out)), Some(tiles)) = (game.clients.0.get(&conn), game.tiles.as_deref())
    else {
        return;
    };
    let Some(Some(view)) = game.views.0.get_mut(&conn) else {
        return;
    };
    let keys: HashSet<TileKey> = keys.into_iter().collect();
    // State for the nodes of newly held tiles; the rest follows as it changes.
    let mut ids: Vec<i64> = keys
        .difference(view)
        .filter_map(|k| tiles.0.tiles.get(k))
        .flat_map(|t| t.nodes.iter().copied())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let state: Vec<_> = ids
        .iter()
        .filter_map(|id| game.node_by_id.get(id))
        .filter_map(|&e| game.nodes.get(e).ok())
        .map(|(g, s)| (g.id, q(s.fire), q(s.water)))
        .collect();
    *view = keys;
    if !state.is_empty() {
        send(out, &ServerMsg::State { nodes: state });
    }
    // And their claimed sources (the app drops those of tiles it let go).
    let me = game.players.get(e).ok().and_then(|p| p.4).map(|s| s.0);
    let set: Vec<_> = game
        .sources
        .iter()
        .filter(|(g, _)| ids.binary_search(&g.id).is_ok())
        .filter_map(|(g, s)| source_item(g, s, me, c.rules, game.sim.as_deref()))
        .collect();
    if !set.is_empty() {
        send(
            out,
            &ServerMsg::Sourcesdiff {
                set,
                gone: Vec::new(),
            },
        );
    }
}

/// How often the speed survey is logged.
const SPEED_LOG_S: f64 = 600.0;
/// Players with fewer good moving fixes than this aren't counted.
const SPEED_MIN_FIXES: u32 = 20;

/// Survey: do moving players' devices report a speed? Totals only, no ids
/// (would a "no speed while moving" signal for plausible.rs hit real phones?).
fn log_speed_seen(game: &Game) {
    let (mut players, mut never, mut partly, mut fixes, mut with) = (0u32, 0u32, 0u32, 0u64, 0u64);
    for (.., motion) in game.players.iter() {
        let s = motion.track.speed;
        if s.moving < SPEED_MIN_FIXES {
            continue;
        }
        players += 1;
        never += (s.with_speed == 0) as u32;
        partly += (s.with_speed > 0 && s.with_speed * 2 < s.moving) as u32;
        fixes += u64::from(s.moving);
        with += u64::from(s.with_speed);
    }
    if players > 0 {
        info!(
            players,
            never,
            partly,
            fixes,
            with_speed = with,
            "speed while moving (connected players, good fixes)"
        );
    }
}

pub(super) fn pos(
    game: &mut Game,
    c: &Ctx,
    conn: u64,
    lat: f64,
    lon: f64,
    speed: Option<f32>,
    acc: Option<f32>,
) {
    let now = c.now;
    if now >= *game.speed_log_at {
        if *game.speed_log_at > 0.0 {
            log_speed_seen(game);
        }
        *game.speed_log_at = now + SPEED_LOG_S;
    }
    let Some(&(e, _)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok((&faction, mut pos, _, mut tracked, slot, mut motion)) = game.players.get_mut(e) else {
        return;
    };
    let slot = slot.map(|s| s.0);
    pos.0 = project_enu(lat, lon, c.region.centroid_lat, c.region.centroid_lon);
    if !game.allow_sim.0 && (!motion.fix(pos.0, now, speed) || motion.locked(now)) {
        return; // implausible: no analytics while locked
    }
    motion.last_fix = Some((pos.0, now));
    // A made-up track at walking speed: shadow ban for a day (memory).
    if !game.allow_sim.0 {
        let axis = game.agg.street_distance(pos.0);
        let sig = motion.track.fix(pos.0, acc, axis).filter(|s| s.flag());
        motion.track.speed.fix(pos.0, acc, speed);
        if let (Some(sig), Some(s)) = (sig, slot) {
            if !game.scores.shadowed(s, now) {
                game.scores.shadow(s, now + SHADOW_BAN_S);
                info!(conn, ?sig, "implausible track: shadow ban for a day");
            }
        }
    }
    if let Some(t) = tracked.as_mut().filter(|t| t.counted) {
        if slot.is_some_and(|s| game.scores.shadowed(s, now)) {
            game.agg.flagged(t.pseudo, now);
            game.agg.withdraw(t.pseudo);
            game.hotspots.withdraw(t.pseudo);
            t.counted = false;
        }
    }
    if let Some(t) = tracked.filter(|t| t.counted) {
        game.agg.observe(
            &t,
            pos.0,
            now,
            game.hotspots.bonus(pos.0, c.rules.hotspot_radius_m),
        );
        game.hotspots.observe(&t, pos.0);
    }
    // A hint for the nearest find not known yet, once per find and `hint_every_s`.
    if let (Some(f), Some(slot), Some((_, out))) =
        (game.finds.as_deref(), slot, game.clients.0.get(&conn))
    {
        let known = game.scores.finds(slot).map(|k| k.1).unwrap_or_default();
        let p = Vec2::new(pos.0.x, pos.0.z);
        let running = game.campaign.running(now);
        if let Some(near) = f
            .near(p, c.rules.finds.hint_m, &running, now)
            .into_iter()
            .find(|x| !known.contains(&x.id))
        {
            let last = game.hinted.get(&conn).copied();
            if last.is_none_or(|(id, at)| {
                id != near.id || now - at > c.rules.finds.hint_every_s as f64
            }) {
                game.hinted.insert(conn, (near.id, now));
                send(
                    out,
                    &ServerMsg::Near {
                        c: &near.collection,
                    },
                );
            }
        }
        // Being there uncovers too, also where all is one's own:
        // `dwell_s` near a find not known yet (not when flagged).
        let here = f
            .near(p, c.rules.finds.uncover_m, &running, now)
            .into_iter()
            .find(|x| !known.contains(&x.id))
            .map(|x| x.id)
            .filter(|_| !game.scores.shadowed(slot, now));
        match (here, motion.at_find) {
            (Some(id), Some((was, since))) if id == was => {
                if now - since >= c.rules.finds.dwell_s as f64 {
                    motion.at_find = None;
                    uncover_near(
                        f,
                        c.rules.finds.uncover_m,
                        p,
                        slot,
                        faction,
                        &mut game.scores,
                        &mut game.campaign,
                        out,
                        now,
                    );
                }
            }
            (here, _) => motion.at_find = here.map(|id| (id, now)),
        }
    }
}

pub(super) fn bye(game: &mut Game, c: &Ctx, conn: u64) {
    game.views.0.remove(&conn);
    game.hinted.remove(&conn);
    game.tunnels.open.remove(&conn);
    game.overview.subs.remove(&conn);
    if let Some(ch) = game.challenges.as_deref_mut() {
        ch.forget(conn);
    }
    if let Some((e, _)) = game.clients.0.remove(&conn) {
        if let Ok((_, _, cd, t, slot, mut motion)) = game.players.get_mut(e) {
            if let Some(t) = t {
                game.agg.forget(t.pseudo);
            }
            if let Some(slot) = slot {
                let ab = game
                    .abilities
                    .get_mut(e)
                    .map(|mut a| std::mem::take(&mut *a));
                game.parked.park(
                    slot.0,
                    c.now,
                    std::mem::take(&mut *motion),
                    cd.0,
                    ab.unwrap_or_default(),
                );
            }
        }
        game.commands.entity(e).despawn();
        info!(conn, "web player left");
    }
}

fn spawn_player(
    commands: &mut Commands,
    next_id: &mut NextPlayerId,
    faction: Faction,
    conn: u64,
    tracked: Tracked,
    (motion, cooldown, abilities): (Motion, AttackCooldown, Abilities),
) -> Entity {
    commands
        .spawn((
            Name::new(format!("Web player {conn}")),
            Player,
            PlayerId(next_id.take()),
            faction,
            cooldown,
            Position::default(),
            WebConn,
            tracked,
            motion,
            abilities,
        ))
        .id()
}

/// The side a new player joins: the weaker one (fewer players today; on a
/// tie, fewer junctions) with chance `weaker`, else the other; a full tie is
/// a coin. `u` is a uniform draw in [0, 1). Friends who start together can
/// end up on the same side, and the balance still leans the right way.
fn join_side(players: [u32; 2], land: [u32; 2], weaker: f32, u: f32) -> Faction {
    let [fire, water] = if players[0] == players[1] {
        land
    } else {
        players
    };
    let (weak, strong) = match fire.cmp(&water) {
        std::cmp::Ordering::Less => (Faction::Fire, Faction::Water),
        std::cmp::Ordering::Greater => (Faction::Water, Faction::Fire),
        std::cmp::Ordering::Equal if u < 0.5 => return Faction::Fire,
        std::cmp::Ordering::Equal => return Faction::Water,
    };
    if u < weaker.clamp(0.0, 1.0) {
        weak
    } else {
        strong
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weaker_side_is_likelier_not_certain() {
        // Always weaker: the old rule.
        assert_eq!(join_side([3, 5], [900, 100], 1.0, 0.99), Faction::Fire);
        assert_eq!(join_side([5, 3], [100, 900], 1.0, 0.99), Faction::Water);
        // Same number of players: less land is the weaker side.
        assert_eq!(join_side([0, 0], [900, 100], 1.0, 0.99), Faction::Water);
        assert_eq!(join_side([2, 2], [100, 900], 1.0, 0.99), Faction::Fire);
        // With a bias the stronger side still gets some.
        assert_eq!(join_side([3, 5], [0, 0], 0.67, 0.5), Faction::Fire);
        assert_eq!(join_side([3, 5], [0, 0], 0.67, 0.8), Faction::Water);
        let fire = (0..1000)
            .filter(|i| join_side([3, 5], [0, 0], 0.67, *i as f32 / 1000.0) == Faction::Fire)
            .count();
        assert_eq!(fire, 670);
        // A full tie is a coin.
        assert_eq!(join_side([0, 0], [0, 0], 0.67, 0.2), Faction::Fire);
        assert_eq!(join_side([0, 0], [0, 0], 0.67, 0.7), Faction::Water);
    }
}

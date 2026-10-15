//! The card of a tapped junction (`info.rs`), and the ops hotspot.

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::web::{q, send, ServerMsg};

use super::{Ctx, Game};

pub(super) fn info(game: &mut Game, c: &Ctx, conn: u64, node: i64) {
    let (region, rules) = (c.region, c.rules);
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Ok((&faction, pos, _, _, slot, _)) = game.players.get(*e) else {
        return;
    };
    let slot = slot.map(|s| s.0);
    let Some(&ent) = game.node_by_id.get(&node) else {
        return;
    };
    let Ok((g, st)) = game.nodes.get(ent) else {
        return;
    };
    let w = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
    let dist = Vec2::new(w.x, w.z).distance(Vec2::new(pos.0.x, pos.0.z));
    let existing = game.sources.get(ent).ok().map(|(_, s)| *s);
    let natural = game.naturals.contains(ent);
    let wear = game.keep_info.get(ent).map_or(0.0, |(_, k)| k.wear);
    let mult = match slot.filter(|_| rules.progression.enabled) {
        Some(s) => rules.progression.strength_factor(game.scores.xp(s).1),
        None => 1.0,
    };
    let attack = rules.claim_strength * mult;
    // Weitblick armed: the next claim reaches farther (as in claim.rs).
    let reach = if rules.progression.enabled
        && game
            .abilities
            .get(*e)
            .is_ok_and(|a| a.armed[super::ability::REACH])
    {
        rules.progression.reach_m
    } else {
        rules.claim_radius_m
    };
    let (take, wait) = match existing {
        // Watchers (no score slot): no side, nothing to take.
        _ if slot.is_none() => ("watch", None),
        _ if dist > reach => ("range", None),
        Some(ex) if ex.faction == faction => ("own", None),
        Some(ex) => match crate::info::weak_in(&ex, rules, g.affinity, wear, attack) {
            Some(0) => ("ok", None),
            t => ("strong", t),
        },
        None if st.owner() == Some(faction) && rules.supply.enabled => ("anchor", None),
        None if st.owner() == Some(faction) => ("mine", None),
        None => ("ok", None),
    };
    let around: Vec<f64> = game
        .node_by_id
        .index(node)
        .zip(game.sim.as_deref())
        .map(|(i, sim)| sim.neighbours(i))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| game.nodes.get(n).ok().map(|(ng, _)| ng.elevation_m))
        .collect();
    let (terrain, dh) = crate::info::terrain(g.elevation_m, &around);
    let radar = crate::radar::frame();
    let rain = radar.as_ref().and_then(|f| f.at(g.lat, g.lon));
    send(
        out,
        &ServerMsg::Info {
            node,
            fire: q(st.fire),
            water: q(st.water),
            source: existing.map(|s| crate::info::source(&s, natural, rules, g.affinity, slot)),
            take,
            wait,
            dist: dist.round() as u32,
            reach: reach.round() as u32,
            elev: (g.elevation_m * 10.0).round() as f32 / 10.0,
            terrain,
            dh,
            lean: crate::info::lean(g.affinity),
            rain: rain.map(|r| (r * 10.0).round() / 10.0),
            rain_at: rain.and(radar.map(|f| f.time.clone())),
        },
    );
}

pub(super) fn ops_hotspot(game: &mut Game, c: &Ctx, node: i64, minutes: f64) {
    let geo = game
        .node_by_id
        .get(&node)
        .and_then(|&e| game.nodes.get(e).ok());
    match geo {
        Some((g, _)) => {
            game.hotspots.add_manual(g, c.region, minutes, c.now);
            info!(node, minutes, "manual hotspot");
        }
        None => warn!(node, "manual hotspot: unknown node"),
    }
}

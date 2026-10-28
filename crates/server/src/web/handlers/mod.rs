//! The browser messages (`FromWeb`), one module per family. `Game` holds
//! everything the handlers share (one `SystemParam`, past Bevy's 16-tuple
//! limit); `inbox` only dispatches.

mod ability;
mod account;
mod claim;
mod info;
mod keep;
mod presence;
mod rise;
mod session;

use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use elements_shared::prelude::*;
use elements_shared::rules::Rules;

use crate::analytics::{now_s, Aggregator};
use crate::challenge::Challenges;
use crate::finds::Finds;
use crate::gateway::FromWeb;
use crate::hotspot::Hotspots;
use crate::motion::AllowSim;
use crate::score::Scores;
use crate::sim::{ElementSim, ElementSource, NodeById};
use crate::NextPlayerId;

use super::{Overview, Parked, PlayerQuery, Sessions, Tiles, Views, WebClients, WebInbox};
pub use ability::Abilities;
use claim::Tunnels;

/// What every handler may touch.
#[derive(SystemParam)]
pub struct Game<'w, 's> {
    commands: Commands<'w, 's>,
    clients: ResMut<'w, WebClients>,
    sessions: ResMut<'w, Sessions>,
    next_id: ResMut<'w, NextPlayerId>,
    agg: ResMut<'w, Aggregator>,
    hotspots: ResMut<'w, Hotspots>,
    scores: ResMut<'w, Scores>,
    allow_sim: Res<'w, AllowSim>,
    players: PlayerQuery<'w, 's>,
    nodes: Query<'w, 's, (&'static GeoNode, &'static mut ElementState), Without<Player>>,
    node_by_id: Res<'w, NodeById>,
    sources: Query<'w, 's, (&'static GeoNode, &'static mut ElementSource)>,
    peers: Res<'w, crate::regions::Peers>,
    challenges: Option<ResMut<'w, Challenges>>,
    sim: Option<Res<'w, ElementSim>>,
    abilities: Query<'w, 's, &'static mut Abilities>,
    views: ResMut<'w, Views>,
    tiles: Option<Res<'w, Tiles>>,
    overview: ResMut<'w, Overview>,
    finds: Option<ResMut<'w, Finds>>,
    /// Last find hinted per connection, and when.
    hinted: Local<'s, HashMap<u64, (i64, f64)>>,
    /// Next speed survey log line (unix s; `session::pos`).
    speed_log_at: Local<'s, f64>,
    campaign: ResMut<'w, crate::campaign::Campaign>,
    live_rain: Option<Res<'w, crate::live_rain::LiveRain>>,
    live_sun: Option<Res<'w, crate::live_rain::LiveSun>>,
    tide: Option<Res<'w, crate::tide::LiveTide>>,
    tunnels: Local<'s, Tunnels>,
    supply: Res<'w, crate::hinterland::Supply>,
    keeps: ResMut<'w, crate::keep::Keeps>,
    marble: ResMut<'w, crate::marble::Claims>,
    keep_info: Query<'w, 's, (Entity, &'static mut crate::keep::KeepInfo)>,
    naturals: Query<'w, 's, &'static GeoNode, With<crate::sim::NaturalSource>>,
    parked: ResMut<'w, Parked>,
    cheers: ResMut<'w, crate::cheer::Cheers>,
    /// `Rules::rise`: when a junction may rise again (node id -> unix s).
    rise_places: Local<'s, HashMap<i64, f64>>,
}

/// The frame's time, region and rules.
pub struct Ctx<'a> {
    pub now: f64,
    pub region: &'a RegionInfo,
    pub rules: &'a Rules,
}

pub fn inbox(
    mut game: Game,
    inbox: Res<WebInbox>,
    region: Option<Res<RegionInfo>>,
    rules: Res<crate::sim::GameRules>,
) {
    let Some(region) = region.as_deref() else {
        return;
    };
    let c = Ctx {
        now: now_s(),
        region,
        rules: &rules.0,
    };
    let g = &mut game;
    for ev in inbox.0.try_iter() {
        match ev {
            FromWeb::Hello {
                conn,
                token,
                global,
                bot,
                tiles,
                pow_ok,
                watch,
                invite,
                out,
            } => session::hello(
                g, &c, conn, token, global, bot, tiles, pow_ok, watch, invite, out,
            ),
            FromWeb::Overview { conn, on } => session::overview(g, conn, on),
            FromWeb::View { conn, tiles } => session::view(g, &c, conn, tiles),
            FromWeb::Pos {
                conn,
                lat,
                lon,
                speed,
                acc,
            } => session::pos(g, &c, conn, lat, lon, speed, acc),
            FromWeb::Bye { conn } => session::bye(g, &c, conn),
            FromWeb::Claim { conn, node, via } => claim::claim(g, &c, conn, node, via.as_deref()),
            FromWeb::Keep { conn, node, ticket } => keep::keep(g, &c, conn, node, ticket),
            FromWeb::Redeem { conn, ticket } => keep::redeem(g, &c, conn, &ticket),
            FromWeb::Release { conn, tickets } => keep::release(g, &c, conn, &tickets),
            FromWeb::Ability { conn, name } => ability::ability(g, &c, conn, &name),
            FromWeb::Presence {
                conn,
                code,
                lat,
                lon,
                radius_m,
            } => presence::presence(g, &c, conn, code, lat, lon, radius_m),
            FromWeb::Rate { conn, node, cat } => presence::rate(g, &c, conn, node, &cat),
            FromWeb::Stats { conn, on } => presence::stats(g, &c, conn, on),
            FromWeb::Profile { conn, age, local } => presence::profile(g, conn, age, local),
            FromWeb::Rename { conn } => account::rename(g, conn),
            FromWeb::Hide { conn, on } => account::hide(g, conn, on),
            FromWeb::Forget { conn } => account::forget(g, &c, conn),
            FromWeb::Code { conn } => account::code(g, conn),
            FromWeb::Side { conn, faction } => account::side(g, conn, &faction),
            FromWeb::Level { conn, level } => account::level(g, conn, level),
            FromWeb::Cheer { conn, side } => {
                // Players (with a score slot) cheer only for their own side,
                // watchers for either; nothing before the entity is spawned.
                let ok = g
                    .clients
                    .0
                    .get(&conn)
                    .and_then(|(e, _)| g.players.get(*e).ok())
                    .is_some_and(|p| p.4.is_none() || Faction::parse(&side) == Some(*p.0));
                if ok {
                    crate::cheer::cheer(&mut g.cheers, conn, &side, c.now);
                }
            }
            FromWeb::Motto {
                conn,
                node,
                ticket,
                motto,
            } => keep::motto(g, &c, conn, node, &ticket, motto.as_deref()),
            FromWeb::Peek { conn, via } => claim::peek(g, &c, conn, &via),
            FromWeb::Link {
                conn,
                node,
                ticket,
                to,
                to_ticket,
            } => keep::link(g, &c, conn, node, &ticket, to, to_ticket.as_deref()),
            FromWeb::Recovered { conn, token } => account::recovered(g, &c, conn, &token),
            FromWeb::Collect { conn, on } => account::collect(g, conn, on),
            FromWeb::Rekey { old, new } => account::rekey(g, &old, new),
            FromWeb::Evict { id } => account::evict(g, &id),
            FromWeb::Flush { ack } => g.scores.flush_then(ack),
            FromWeb::Info { conn, node } => info::info(g, &c, conn, node),
            FromWeb::OpsHotspot { node, minutes } => info::ops_hotspot(g, &c, node, minutes),
            FromWeb::Finds(rows) => {
                let f = crate::finds::Finds::from_rows(rows, c.region);
                info!(finds = f.len(), "finds changed");
                if let Some(old) = g.finds.as_mut() {
                    **old = f;
                }
            }
            FromWeb::Places(p) => {
                let zones = crate::places::project(&p.zones, c.region);
                g.agg.set_zones(crate::places::for_flows(&zones));
                g.agg
                    .set_focus(crate::places::for_focus(&p.zones, c.region));
                if let Some(ch) = g.challenges.as_mut() {
                    ch.set_zones(crate::places::for_challenges(&zones));
                }
                g.hotspots.set_sites(&p.sites, c.region);
                g.hotspots.set_plans(&p.plans, c.region);
                info!(zones = zones.len(), sites = p.sites.len(), "places changed");
            }
        }
    }
}

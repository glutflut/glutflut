//! Bevy side of the web gateway: browser connections become player entities,
//! claims go through `actions::claim`, positions feed the
//! aggregator and hotspot experiment, and state changes go to every browser.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::prelude::*;
use crossbeam_channel::Receiver;

use elements_shared::prelude::*;
use elements_shared::rules::Rules;

use crate::actions::AttackCooldown;
use crate::analytics::{now_s, Tracked};
use crate::challenge::{self, Challenges};
use crate::finds::Finds;
use crate::gateway::{FromWeb, Outbox, SharedJson};
use crate::hotspot::Hotspots;
use crate::motion::Motion;
use crate::protocol::{ServerMsg, SourceItem};
use crate::score::{ScoreSlot, Scores};
use crate::sim::{ElementSim, ElementSource, SourceKind};
use crate::tiles::{SharedTiles, TileKey, TileSet};

/// Place-rating categories the client may send.
/// Ratings: three pairs, then what stands out ("warum?").
pub const CATEGORIES: [&str; 10] = [
    "voll",
    "leer",
    "laut",
    "ruhig",
    "unangenehm",
    "einladend",
    "dunkel",
    "kalt",
    "verkehr",
    "schmutzig",
];
/// Ratings: max distance to the node and min seconds between two.
pub const RATE_RADIUS_M: f32 = 80.0;
pub const RATE_COOLDOWN_S: f64 = 15.0;

#[derive(Resource)]
pub struct WebInbox(pub Receiver<FromWeb>);

/// Where the gateway looks for the tiles (filled at load).
#[derive(Resource)]
pub struct SharedTilesRes(pub SharedTiles);

/// Territory per tile for `/api/overview`.
#[derive(Resource)]
pub struct OverviewJson(pub SharedJson);

/// Side of every junction on request (`gateway::NodesShare`).
#[derive(Resource)]
pub struct NodesJson(pub crate::gateway::NodesShare);

/// The region's tiles, once loaded.
#[derive(Resource, Clone)]
pub struct Tiles(pub Arc<TileSet>);

/// Territory per tile ([x, y, fire, water, front, nodes]) as last sent, and
/// the browsers that show the overview (zoomed out) and get its changes.
#[derive(Resource, Default)]
pub struct Overview {
    rows: Vec<[u32; 6]>,
    subs: HashSet<u64>,
}

/// Tiles each browser holds; `None` = the whole region (bots, old clients).
#[derive(Resource, Default)]
pub struct Views(HashMap<u64, Option<HashSet<TileKey>>>);

/// Open browser connections: conn id -> (player entity, outbox).
#[derive(Resource, Default)]
pub struct WebClients(HashMap<u64, (Entity, Outbox)>);

/// Session token -> faction and last hello, so a returning browser keeps
/// its side. Players have a score row anyway; this matters for bots and is
/// forgotten after a day.
#[derive(Resource, Default)]
pub struct Sessions(HashMap<String, (Faction, f64)>);

impl Sessions {
    const KEEP_S: f64 = 86_400.0;

    fn knows(&self, token: &str) -> bool {
        self.0.contains_key(token)
    }

    fn remember(&mut self, token: String, faction: Faction, now: f64) {
        self.0.retain(|_, (_, at)| now - *at < Self::KEEP_S);
        self.0.insert(token, (faction, now));
    }
}

/// Per-player state that outlives the socket, by score slot: the speed lock,
/// the track, the claim and ability cooldowns. Without it a reconnect (phones
/// drop sockets anyway) would start clean, and a script could claim anywhere
/// at once by reconnecting. Memory only, dropped after an hour.
#[derive(Resource, Default)]
pub struct Parked(HashMap<u32, ParkedState>);

pub struct ParkedState {
    at: f64,
    motion: Motion,
    cooldown: f32,
    abilities: Abilities,
}

const PARK_S: f64 = 3600.0;

impl Parked {
    fn park(&mut self, slot: u32, now: f64, motion: Motion, cooldown: f32, abilities: Abilities) {
        self.0.retain(|_, p| now - p.at < PARK_S);
        let p = ParkedState {
            at: now,
            motion,
            cooldown,
            abilities,
        };
        self.0.insert(slot, p);
    }

    /// The state for a new connection: parked if there is one, else fresh.
    fn resume(
        &mut self,
        slot: Option<u32>,
        now: f64,
        trusted: bool,
    ) -> (Motion, AttackCooldown, Abilities) {
        match slot
            .and_then(|s| self.0.remove(&s))
            .filter(|p| now - p.at < PARK_S)
        {
            Some(mut p) => {
                p.motion.trusted |= trusted;
                let left = (p.cooldown as f64 - (now - p.at)).max(0.0) as f32;
                (p.motion, AttackCooldown(left), p.abilities)
            }
            None => (
                Motion::with_trust(trusted),
                AttackCooldown::default(),
                Abilities::default(),
            ),
        }
    }
}

/// Marks a player that plays via the browser.
#[derive(Component)]
pub struct WebConn;

fn source_kind(s: &ElementSource, rules: &Rules) -> &'static str {
    match s.kind {
        SourceKind::Spark => "spark",
        SourceKind::Anchor => "anchor",
        SourceKind::Keep => "keep",
        SourceKind::Claim if rules.supply.enabled && s.supply > rules.supply.supplied => "cut",
        SourceKind::Claim => "",
    }
}

pub(crate) fn source_list<'a>(
    q: impl Iterator<Item = (&'a GeoNode, &'a ElementSource)>,
    me: Option<u32>,
    rules: &Rules,
    sim: Option<&crate::sim::ElementSim>,
) -> Vec<SourceItem> {
    q.filter_map(|(g, s)| source_item(g, s, me, rules, sim))
        .collect()
}

/// One claimed source as the app gets it (`None`: natural, not sent).
pub(crate) fn source_item(
    g: &GeoNode,
    s: &ElementSource,
    me: Option<u32>,
    rules: &Rules,
    sim: Option<&crate::sim::ElementSim>,
) -> Option<SourceItem> {
    {
        let life = s.life?;
        // Permanent: the time until it has faded to its floor.
        let life = if s.kind == SourceKind::Keep {
            rules.keep.fade_s
        } else {
            life
        };
        let speed = if s.kind == SourceKind::Spark {
            1.0
        } else {
            (rules.source_age(s.faction, 1.0, g.affinity)
                * s.supply
                * crate::sim::siege_factor(rules, s))
            .max(1e-3)
        };
        let build = (s.ramp - s.built).max(0.0).ceil() as u32;
        Some(SourceItem {
            node: g.id,
            faction: s.faction.as_str(),
            left: ((life - s.age).max(0.0) / speed).round() as u32,
            life: (life / speed).round() as u32,
            mine: me.is_some() && s.by == me,
            lon: g.lon,
            lat: g.lat,
            build,
            ramp: if build > 0 { s.ramp.ceil() as u32 } else { 0 },
            kind: source_kind(s, rules),
            no_keep: s.kind == SourceKind::Claim
                && sim.is_some_and(|m| m.no_keep().contains(&g.id)),
            motto: (s.kind == SourceKind::Keep)
                .then(|| elements_shared::mottos::key_of(s.motto))
                .flatten(),
            sieged: s.sieged,
            linked: s.linked,
        })
    }
}

/// Uncover the finds within `uncover_m` of `at` for the player in `slot`
/// (after a claim, or after being there for `[finds] dwell_s`).
#[allow(clippy::too_many_arguments)]
fn uncover_near(
    f: &Finds,
    uncover_m: f32,
    at: Vec2,
    s: u32,
    faction: Faction,
    scores: &mut Scores,
    campaign: &mut crate::campaign::Campaign,
    out: &Outbox,
    now: f64,
) {
    let running = campaign.running(now);
    for find in f.near(at, uncover_m, &running, now) {
        let Some(stored) = scores.uncover(s, find.id) else {
            continue;
        };
        if let Some(key) = campaign.add(faction, crate::campaign::Goal::Finds, 1, now) {
            scores.grant(s, &format!("chapter:{key}"));
        }
        let known = scores.finds(s).map(|k| k.1).unwrap_or_default();
        let n = f.count(&find.collection, known.iter().copied(), &running, now);
        let of = f
            .totals(&running, now)
            .get(&find.collection)
            .copied()
            .unwrap_or(0);
        send(
            out,
            &ServerMsg::Find {
                find,
                n,
                of,
                stored,
            },
        );
        if stored && n == of {
            scores.completed(s, &find.collection);
        }
    }
}

pub(crate) fn send(out: &Outbox, msg: &ServerMsg) {
    if let Ok(json) = serde_json::to_string(msg) {
        let _ = out.send(json);
    }
}

/// Round to 3 decimals to keep frames small.
fn q(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

type PlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Faction,
        &'static mut Position,
        &'static mut AttackCooldown,
        Option<&'static mut Tracked>,
        Option<&'static ScoreSlot>,
        &'static mut Motion,
    ),
    With<Player>,
>;

/// The browser messages, by family; `Game` is the shared system param.
mod handlers;
pub use handlers::inbox;
use handlers::Abilities;

fn send_challenge(out: &Outbox, ev: challenge::Event) {
    match ev {
        challenge::Event::Update(v) => send(out, &ServerMsg::Challenge(v)),
        challenge::Event::Done { kind, xp } => send(out, &ServerMsg::ChallengeDone { kind, xp }),
        challenge::Event::Paused => send(out, &ServerMsg::ChallengePaused),
    }
}

/// Round ends and "Halten" for the players' challenges.
pub fn challenge_tick(
    clients: Res<WebClients>,
    mut challenges: ResMut<Challenges>,
    mut scores: ResMut<Scores>,
    rules: Res<crate::sim::GameRules>,
    players: Query<&Position, With<Player>>,
    mut next: Local<f64>,
) {
    let now = now_s();
    if now < *next || !rules.0.progression.enabled {
        return;
    }
    *next = now + 1.0;
    let pos = |conn: u64| {
        let (e, _) = clients.0.get(&conn)?;
        players.get(*e).ok().map(|p| p.0)
    };
    for (conn, ev) in challenges.tick(now, &rules.0.progression, &mut scores, pos) {
        if let Some((_, out)) = clients.0.get(&conn) {
            send_challenge(out, ev);
        }
    }
}

/// The chapter and the race, for the apps.
fn chapter_msg(c: &crate::campaign::Campaign, now: f64, levels: bool) -> Option<ServerMsg<'_>> {
    let ch = c.chapter.as_ref()?;
    Some(ServerMsg::Chapter {
        chapter: ch,
        starts_in: (ch.starts - now).max(0.0),
        ends_in: (ch.ends - now).max(0.0),
        fire: c.progress[0],
        water: c.progress[1],
        winner: c.winner(now).map(Faction::as_str),
        resets: levels && c.resets(ch),
    })
}

/// Campaign: new chapters from the database, points into the race, the end of
/// a chapter (winner badges), and the state to every app when it changed.
pub fn campaign_tick(
    mut campaign: ResMut<crate::campaign::Campaign>,
    mut scores: ResMut<Scores>,
    clients: Res<WebClients>,
    rules: Res<crate::sim::GameRules>,
    live: Option<Res<crate::live_rain::LiveRain>>,
    sun: Option<Res<crate::live_rain::LiveSun>>,
    node_by_id: Res<crate::sim::NodeById>,
    nodes: Query<&ElementState, Without<Player>>,
    mut next: Local<f64>,
    mut weather: Local<(Option<elements_shared::weather::WeatherDay>, bool, bool)>,
) {
    let now = now_s();
    campaign.poll();
    // Hold: who holds the chapter's junctions now.
    let held = campaign.hold_nodes(now).map(|ids| {
        let mut n = [0i64; 2];
        for id in ids {
            let owner = node_by_id
                .get(id)
                .and_then(|&e| nodes.get(e).ok())
                .and_then(|st| st.owner());
            if let Some(f) = owner {
                n[f.index()] += 1;
            }
        }
        n
    });
    if let Some(n) = held {
        campaign.held(n, now);
    }
    let earned = std::mem::take(&mut scores.earned);
    for (i, f) in [Faction::Fire, Faction::Water].into_iter().enumerate() {
        campaign.add(f, crate::campaign::Goal::Points, earned[i], now);
    }
    if let Some((key, Some(w))) = campaign.take_end(now) {
        info!(key, winner = ?w, "chapter ended");
        scores.grant_winners(&key, w);
    }
    campaign.save(now, false);
    let live_on = live.is_some_and(|l| l.on);
    let sun_on = sun.is_some_and(|l| l.on);
    if (campaign.weather, live_on, sun_on) != *weather {
        *weather = (campaign.weather, live_on, sun_on);
        if let Some(w) = crate::weather::shown(&rules.0, campaign.weather, live_on, sun_on) {
            to_all(&clients, &ServerMsg::Weather(w));
        }
    }
    if now >= *next || (campaign.changed && now >= *next - 8.0) {
        *next = now + 10.0;
        if std::mem::take(&mut campaign.changed) {
            if let Some(m) = chapter_msg(&campaign, now, rules.0.progression.enabled) {
                to_all(&clients, &m);
            }
        }
    }
    if std::mem::take(&mut campaign.questions_changed) {
        to_all(
            &clients,
            &ServerMsg::Questions {
                items: &campaign.questions,
            },
        );
    }
    if std::mem::take(&mut campaign.notices_changed) {
        to_all(
            &clients,
            &ServerMsg::Notices {
                items: &campaign.notices,
            },
        );
    }
}

/// The tide to every app when it changed (at most once a minute; off at once).
pub fn tide_tick(
    tide: Option<Res<crate::tide::LiveTide>>,
    clients: Res<WebClients>,
    mut seen: Local<(u32, f64, bool)>,
) {
    let Some(shown) = tide.and_then(|t| t.shown().map(|s| (t.version, s))) else {
        return;
    };
    let (version, s) = shown;
    let now = now_s();
    if version == seen.0 || (s.on == seen.2 && now - seen.1 < 60.0) {
        return;
    }
    *seen = (version, now, s.on);
    to_all(&clients, &ServerMsg::Tide(s));
}

/// The player's finds in this region and the collection sizes (`running`:
/// the campaigns running at `now`, `Campaign::running`).
fn send_finds(out: &Outbox, f: &Finds, scores: &Scores, slot: u32, running: &[String], now: f64) {
    let Some((collect, known)) = scores.finds(slot) else {
        return;
    };
    send(
        out,
        &ServerMsg::Finds {
            collect,
            items: f.of(&known).collect(),
            totals: f.album_totals(running, now, &known),
        },
    );
}

/// Own score to each player: right after it changed, and every 10 s for ranks.
pub fn send_scores(
    clients: Res<WebClients>,
    mut scores: ResMut<Scores>,
    slots: Query<&ScoreSlot>,
    mut next: Local<f64>,
) {
    let now = now_s();
    let all = now >= *next;
    if all {
        *next = now + 10.0;
    }
    if !all
        && scores.areas.is_none()
        && scores.changed.is_empty()
        && scores.news.is_empty()
        && scores.rings.is_empty()
        && scores.invites.is_empty()
        && scores.sieges.is_empty()
    {
        return;
    }
    let mut changed: std::collections::HashSet<u32> = scores.changed.drain(..).collect();
    let news = std::mem::take(&mut scores.news);
    let rings = std::mem::take(&mut scores.rings);
    let invites = std::mem::take(&mut scores.invites);
    let sieges = std::mem::take(&mut scores.sieges);
    let areas = scores.areas.take();
    let area_ids: Vec<i32> = areas.iter().flatten().map(|(id, _)| *id).collect();
    changed.extend(news.iter().map(|(s, _)| *s));
    changed.extend(rings.iter().map(|(s, _)| *s));
    changed.extend(invites.iter().map(|(s, _)| *s));
    changed.extend(sieges.iter().map(|(s, _, _)| *s));
    for (e, out) in clients.0.values() {
        let Ok(slot) = slots.get(*e) else { continue };
        for (_, badge) in news.iter().filter(|(s, _)| *s == slot.0) {
            send(out, &ServerMsg::Badge { badge });
        }
        if let Some(areas) = &areas {
            let mine = areas
                .iter()
                .filter(|(_, by)| by.contains(&slot.0))
                .map(|(id, _)| *id)
                .collect();
            send(
                out,
                &ServerMsg::Areas {
                    player: area_ids.clone(),
                    mine,
                },
            );
        }
        for &(_, points) in rings.iter().filter(|(s, _)| *s == slot.0) {
            send(out, &ServerMsg::RingBroken { points });
        }
        for &(_, points) in invites.iter().filter(|(s, _)| *s == slot.0) {
            send(out, &ServerMsg::Invited { points });
        }
        for &(_, points, side) in sieges.iter().filter(|(s, _, _)| *s == slot.0) {
            send(
                out,
                &ServerMsg::Encircled {
                    points,
                    side: side.as_str(),
                },
            );
        }
        if all || changed.contains(&slot.0) {
            if let Some(mine) = scores.mine(slot.0) {
                send(out, &ServerMsg::Score(mine));
            }
        }
    }
}

pub(crate) fn to_all(clients: &WebClients, msg: &ServerMsg) {
    if clients.0.is_empty() {
        return;
    }
    let Ok(json) = serde_json::to_string(msg) else {
        return;
    };
    for (_, out) in clients.0.values() {
        let _ = out.send(json.clone());
    }
}

/// Push changed node states (to each browser for the tiles it holds) and
/// hotspot and source changes to all browsers.
#[allow(clippy::too_many_arguments)]
pub fn broadcast(
    clients: Res<WebClients>,
    views: Res<Views>,
    tiles: Option<Res<Tiles>>,
    mut hotspots: ResMut<Hotspots>,
    changed: Query<(&GeoNode, &ElementState), Changed<ElementState>>,
    sources: Query<(&GeoNode, &ElementSource)>,
    slots: Query<&ScoreSlot>,
    rules: Res<crate::sim::GameRules>,
    mut last: Local<HashMap<i64, (Faction, Option<u32>, f32, &'static str, u16, bool)>>,
    mut sparks: ResMut<crate::hinterland::SparkNews>,
    sim: Option<Res<crate::sim::ElementSim>>,
) {
    for (e, s) in sparks.0.drain(..) {
        if let Some((_, out)) = clients.0.values().find(|(pe, _)| *pe == e) {
            send(
                out,
                &ServerMsg::Spark {
                    node: s.node,
                    lat: s.lat,
                    lon: s.lon,
                    m: s.m,
                },
            );
        }
    }
    // Full list whenever the claimed sources change: one appears, fades or
    // is taken over, or its lifetime restarts. Not just Added/Removed: a claim
    // on a natural or an enemy source replaces the component in place.
    // Also when supply comes or goes (the arc runs at another speed).
    // Compared in place: no allocation unless something changed.
    let claimed = || {
        sources
            .iter()
            .filter(|(_, s)| s.life.is_some())
            .map(|(g, s)| {
                (
                    g.id,
                    (
                        s.faction,
                        s.by,
                        s.age,
                        source_kind(s, &rules.0),
                        s.motto,
                        s.sieged,
                    ),
                )
            })
    };
    let mut set: Vec<i64> = Vec::new();
    let mut n = 0;
    for (id, now) in claimed() {
        n += 1;
        match last.get_mut(&id) {
            Some(l) => {
                let (f, by, age, k, mo, sg) = now;
                if f != l.0 || by != l.1 || age + 0.5 < l.2 || k != l.3 || mo != l.4 || sg != l.5 {
                    set.push(id);
                }
                *l = now;
            }
            None => {
                last.insert(id, now);
                set.push(id);
            }
        }
    }
    let mut gone: Vec<i64> = Vec::new();
    if last.len() > n {
        let live: HashSet<i64> = claimed().map(|(id, _)| id).collect();
        gone = last
            .keys()
            .filter(|id| !live.contains(id))
            .copied()
            .collect();
        last.retain(|id, _| live.contains(id));
    }
    // Only the changes, once per frame; each app gets those in its tiles.
    if !(set.is_empty() && gone.is_empty()) && !clients.0.is_empty() {
        set.sort_unstable();
        let changed: Vec<_> = sources
            .iter()
            .filter(|(g, _)| set.binary_search(&g.id).is_ok())
            .collect();
        let held = |view: Option<&HashSet<TileKey>>, id: i64| match (view, tiles.as_deref()) {
            (Some(v), Some(t)) => t.0.tiles_of(id).any(|k| v.contains(&k)),
            _ => true,
        };
        for (conn, (e, out)) in &clients.0 {
            let view = views.0.get(conn).and_then(|v| v.as_ref());
            let me = slots.get(*e).ok().map(|s| s.0);
            let set: Vec<_> = changed
                .iter()
                .filter(|(g, _)| held(view, g.id))
                .filter_map(|(g, s)| source_item(g, s, me, &rules.0, sim.as_deref()))
                .collect();
            let gone: Vec<i64> = gone.iter().copied().filter(|&id| held(view, id)).collect();
            if !(set.is_empty() && gone.is_empty()) {
                send(out, &ServerMsg::Sourcesdiff { set, gone });
            }
        }
    }
    if hotspots.changed {
        hotspots.changed = false;
        to_all(
            &clients,
            &ServerMsg::Hotspots {
                items: hotspots.list(now_s()),
            },
        );
    }
    if clients.0.is_empty() {
        return;
    }
    let nodes: Vec<_> = changed
        .iter()
        .map(|(g, s)| (g.id, q(s.fire), q(s.water)))
        .collect();
    if nodes.is_empty() {
        return;
    }
    let whole = || {
        serde_json::to_string(&ServerMsg::State {
            nodes: nodes.clone(),
        })
        .ok()
    };
    let mut whole_json: Option<String> = None;
    // Changes grouped by tile, for browsers that hold only some.
    let mut by_tile: HashMap<TileKey, Vec<(i64, f32, f32)>> = HashMap::new();
    if let Some(t) = tiles.as_deref() {
        for &n in &nodes {
            for k in t.0.tiles_of(n.0) {
                by_tile.entry(k).or_default().push(n);
            }
        }
    }
    for (conn, (_, out)) in &clients.0 {
        match views.0.get(conn) {
            Some(Some(view)) => {
                let mut mine: Vec<_> = view
                    .iter()
                    .filter_map(|k| by_tile.get(k))
                    .flatten()
                    .copied()
                    .collect();
                if mine.is_empty() {
                    continue;
                }
                // Border nodes sit in several tiles.
                mine.sort_unstable_by_key(|n| n.0);
                mine.dedup_by_key(|n| n.0);
                send(out, &ServerMsg::State { nodes: mine });
            }
            _ => {
                if whole_json.is_none() {
                    whole_json = whole();
                }
                if let Some(json) = &whole_json {
                    let _ = out.send(json.clone());
                }
            }
        }
    }
}

/// Every few seconds: nodes per side in the whole region to all browsers
/// (when it changed), the tiles whose territory changed to the browsers that
/// show the overview, and all of it for `/api/overview`.
#[allow(clippy::too_many_arguments)]
pub fn overview_tick(
    clients: Res<WebClients>,
    sim: Res<ElementSim>,
    tiles: Option<Res<Tiles>>,
    overview: Res<OverviewJson>,
    mut state: ResMut<Overview>,
    geo: Query<&GeoNode>,
    mut home: Local<Vec<u32>>,
    mut last: Local<(f64, [u32; 3])>,
) {
    let now = now_s();
    if now < last.0 + OVERVIEW_EVERY_S {
        return;
    }
    last.0 = now;
    let msg = totals(&sim);
    if let ServerMsg::Totals { fire, water, front } = msg {
        if [fire, water, front] != last.1 {
            last.1 = [fire, water, front];
            to_all(&clients, &msg);
        }
    }
    let Some(tiles) = tiles.as_deref() else {
        return;
    };
    // Tile per dense sim index, and the tile list, once.
    let keys = &tiles.0.home_keys;
    if home.len() != sim.nodes().len() {
        let pos: HashMap<TileKey, u32> = keys
            .iter()
            .enumerate()
            .map(|(i, &k)| (k, i as u32))
            .collect();
        *home = sim
            .nodes()
            .iter()
            .map(|&e| {
                geo.get(e)
                    .ok()
                    .and_then(|g| tiles.0.home_of(g.id))
                    .and_then(|k| pos.get(&k).copied())
                    .unwrap_or(u32::MAX)
            })
            .collect();
    }
    // [fire, water, front, all] per tile.
    let mut count = vec![[0u32; 4]; keys.len()];
    for (&(f, w), &t) in sim.state().iter().zip(home.iter()) {
        let Some(c) = count.get_mut(t as usize) else {
            continue;
        };
        let es = ElementState { fire: f, water: w };
        match es.owner() {
            Some(Faction::Fire) => c[0] += 1,
            Some(Faction::Water) => c[1] += 1,
            None if es.contested() => c[2] += 1,
            None => {}
        }
        c[3] += 1;
    }
    let rows: Vec<_> = keys
        .iter()
        .zip(&count)
        .filter(|(_, c)| c[3] > 0)
        .map(|(&(x, y), c)| [x, y, c[0], c[1], c[2], c[3]])
        .collect();
    // Same tiles in the same order each time: compare row by row.
    let t: Vec<[u32; 6]> = if state.rows.len() == rows.len() {
        rows.iter()
            .zip(&state.rows)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| *a)
            .collect()
    } else {
        rows.clone()
    };
    if !t.is_empty() && !state.subs.is_empty() {
        let full = t.len() == rows.len();
        if let Ok(json) = serde_json::to_string(&ServerMsg::Overview { tiles: t, full }) {
            for conn in &state.subs {
                if let Some((_, out)) = clients.0.get(conn) {
                    let _ = out.send(json.clone());
                }
            }
        }
    }
    state.rows.clone_from(&rows);
    let json = serde_json::json!({ "z": crate::tiles::TILE_Z, "t": rows }).to_string();
    *overview.0.write().unwrap() = Some(json.into());
}

const OVERVIEW_EVERY_S: f64 = 4.0;

/// When asked: `{t, fire, water, front}` with the node ids per side (unix s).
pub fn nodes_tick(share: Res<NodesJson>, sim: Res<ElementSim>, geo: Query<&GeoNode>) {
    if !share
        .0
        .wanted
        .swap(false, std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }
    let (mut fire, mut water, mut front) = (vec![], vec![], vec![]);
    for (&(f, w), &e) in sim.state().iter().zip(sim.nodes()) {
        let Ok(g) = geo.get(e) else { continue };
        let es = ElementState { fire: f, water: w };
        match es.owner() {
            Some(Faction::Fire) => fire.push(g.id),
            Some(Faction::Water) => water.push(g.id),
            None if es.contested() => front.push(g.id),
            None => {}
        }
    }
    let json = serde_json::json!({ "t": now_s(), "fire": fire, "water": water, "front": front });
    *share.0.json.write().unwrap() = Some(json.to_string().into());
}

fn totals(sim: &ElementSim) -> ServerMsg<'static> {
    let (mut fire, mut water, mut front) = (0, 0, 0);
    for &(f, w) in sim.state() {
        let es = ElementState { fire: f, water: w };
        match es.owner() {
            Some(Faction::Fire) => fire += 1,
            Some(Faction::Water) => water += 1,
            None if es.contested() => front += 1,
            None => {}
        }
    }
    ServerMsg::Totals { fire, water, front }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_and_cooldown_survive_a_reconnect() {
        let mut parked = Parked::default();
        let mut motion = Motion::with_trust(false);
        motion.fix(Vec3::ZERO, 100.0, None);
        assert!(!motion.fix(Vec3::new(5000.0, 0.0, 0.0), 101.0, None));
        parked.park(7, 110.0, motion, 30.0, Abilities::default());
        // other slots and bots start fresh
        assert_eq!(parked.resume(Some(8), 111.0, false).1 .0, 0.0);
        assert_eq!(parked.resume(None, 111.0, false).1 .0, 0.0);
        let (m, cd, _) = parked.resume(Some(7), 120.0, false);
        assert!(m.locked(120.0));
        assert!((cd.0 - 20.0).abs() < 1e-3);
        // taken once, trust is only ever added
        let (m, cd, _) = parked.resume(Some(7), 121.0, true);
        assert!(!m.locked(121.0) && cd.0 == 0.0 && m.trusted);
        // forgotten after an hour
        parked.park(
            7,
            200.0,
            Motion::with_trust(true),
            30.0,
            Abilities::default(),
        );
        assert!(!parked.resume(Some(7), 200.0 + PARK_S, false).0.trusted);
    }
}

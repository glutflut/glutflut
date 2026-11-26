//! Carrying (`[carry]`, vault `tragen-bauen-kipppunkt.md`): take a load at
//! an own source, carry it at most `hand_s`, put it down at a junction where
//! it waits `lying_s` for another hand of the side (bucket chain, torch relay),
//! deliver it at the side's open site. Enough loads there make a building:
//! a source of the side for good, with the builders' names (well for water,
//! forge for fire). Held and lying loads live in memory only (hours); sites
//! and buildings are stored (`game.carry_sites`, `game.carry_builds`).

use std::collections::HashMap;

use bevy::prelude::*;
use rand::seq::IndexedRandom;
use serde::Serialize;

use elements_shared::prelude::Faction;
use elements_shared::rules::Carry as Rules;

/// A junction a load can lie on or a site can stand at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub id: i64,
    pub p: Vec2,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Load {
    pub side: Faction,
    /// 0..1: what is left after spilling (`web/src/spill.ts`).
    pub fill: f32,
    /// Names of the hands so far (shown on the building).
    pub hands: Vec<String>,
    /// The last hand: may not take it up again.
    pub last: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct Held {
    pub load: Load,
    /// Taken at (the hand's time runs from here).
    pub since: f64,
}

#[derive(Clone, Debug)]
pub struct Lying {
    pub at: Spot,
    pub until: f64,
    pub load: Load,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub at: f64,
    pub fill: f32,
    pub names: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Site {
    pub at: Spot,
    pub opened: f64,
    pub got: Vec<Delivery>,
}

impl Site {
    pub fn filled(&self) -> f32 {
        // An empty f32 sum is -0.0, shown as "-0".
        self.got.iter().map(|d| d.fill).sum::<f32>() + 0.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    pub side: Faction,
    pub at: Spot,
    pub names: Vec<String>,
    pub when: f64,
}

/// What a take did.
#[derive(Debug, PartialEq)]
pub enum Taken {
    /// A fresh load from a source.
    Fresh,
    /// A lying load, carried on.
    Handed,
}

/// What putting a load down did.
#[derive(Debug, PartialEq)]
pub enum Laid {
    /// At a junction, waiting for another hand.
    Waiting,
    /// At the site: filled so far.
    Delivered(f32),
    /// At the site, and that finished it.
    Built(Built),
}

/// Stored changes (`Writer`).
pub enum Out {
    Site(Faction, Option<Site>),
    Build(Built),
    /// All put-down loads of the region (replaces what is stored).
    Lying(Vec<Lying>),
}

#[derive(Resource, Default)]
pub struct Carry {
    held: HashMap<[u8; 32], Held>,
    pub lying: Vec<Lying>,
    /// Open site per side (fire, water) and when the next may open.
    pub sites: [Option<Site>; 2],
    reopen: [f64; 2],
    pub built: Vec<Built>,
    /// Built but not yet a source in the world (`apply_builds`).
    pub pending: Vec<Built>,
    /// Just built, not announced yet (`web::carry_tick`).
    pub news: Vec<Built>,
    /// Where sites may stand (the region's junctions) and where people are.
    spots: Vec<Spot>,
    attract: Vec<Vec2>,
    /// The streets between the junctions (by index into `spots`, metres);
    /// empty: distances as the crow flies.
    adj: Vec<Vec<(usize, f32)>>,
    index: HashMap<i64, usize>,
    /// Bumped per side when its lying loads or site change; and for buildings.
    pub version: [u32; 2],
    pub built_version: u32,
    tx: Option<crate::writers::LaneTx<Out>>,
    /// Database and region, for the last write at shutdown (`save_on_stop`).
    db: Option<(String, String)>,
}

pub fn idx(f: Faction) -> usize {
    match f {
        Faction::Fire => 0,
        Faction::Water => 1,
    }
}

impl Carry {
    /// Stored sites and buildings (all of them become pending sources).
    pub fn new(
        spots: Vec<Spot>,
        attract: Vec<Vec2>,
        sites: [Option<Site>; 2],
        built: Vec<Built>,
        tx: Option<crate::writers::LaneTx<Out>>,
    ) -> Self {
        // Stored ones come with lat/lon only: take the junction's position.
        let fix = |at: &mut Spot| {
            if let Some(s) = spots.iter().find(|s| s.id == at.id) {
                *at = *s;
            }
        };
        let mut sites = sites;
        for s in sites.iter_mut().flatten() {
            fix(&mut s.at);
        }
        let mut built = built;
        for b in &mut built {
            fix(&mut b.at);
        }
        Self {
            pending: built.clone(),
            built,
            sites,
            spots,
            attract,
            tx,
            version: [1, 1],
            built_version: 1,
            ..Default::default()
        }
    }

    /// The street graph (junction ids and length), for site distances.
    pub fn with_streets(mut self, edges: impl Iterator<Item = (i64, i64, f32)>) -> Self {
        self.index = self
            .spots
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        self.adj = vec![Vec::new(); self.spots.len()];
        for (a, b, m) in edges {
            if let (Some(&i), Some(&j)) = (self.index.get(&a), self.index.get(&b)) {
                self.adj[i].push((j, m));
                self.adj[j].push((i, m));
            }
        }
        self
    }

    /// Distance from the nearest of `from` (junction ids) to every spot: along
    /// the streets if known, else as the crow flies.
    fn distances(&self, from: &[i64]) -> Vec<f32> {
        let start: Vec<usize> = from.iter().filter_map(|id| self.spot_index(*id)).collect();
        if self.adj.is_empty() {
            return self
                .spots
                .iter()
                .map(|s| {
                    start
                        .iter()
                        .map(|&i| self.spots[i].p.distance(s.p))
                        .fold(f32::INFINITY, f32::min)
                })
                .collect();
        }
        // Dijkstra in whole metres.
        let mut d = vec![u32::MAX; self.spots.len()];
        let mut heap = std::collections::BinaryHeap::new();
        for &i in &start {
            d[i] = 0;
            heap.push(std::cmp::Reverse((0u32, i)));
        }
        while let Some(std::cmp::Reverse((di, i))) = heap.pop() {
            if di > d[i] {
                continue;
            }
            for &(j, m) in &self.adj[i] {
                let dj = di.saturating_add(m.round() as u32);
                if dj < d[j] {
                    d[j] = dj;
                    heap.push(std::cmp::Reverse((dj, j)));
                }
            }
        }
        d.into_iter()
            .map(|v| {
                if v == u32::MAX {
                    f32::INFINITY
                } else {
                    v as f32
                }
            })
            .collect()
    }

    fn spot_index(&self, id: i64) -> Option<usize> {
        self.index
            .get(&id)
            .copied()
            .or_else(|| self.spots.iter().position(|s| s.id == id))
    }

    /// Stored put-down loads (after `new`, which knows the junctions).
    pub fn with_lying(mut self, lying: Vec<Lying>, db: Option<(String, String)>) -> Self {
        self.lying = lying
            .into_iter()
            .filter_map(|mut l| {
                l.at = *self.spots.iter().find(|s| s.id == l.at.id)?;
                Some(l)
            })
            .collect();
        self.db = db;
        self
    }

    fn save_lying(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Out::Lying(self.lying.clone()));
        }
    }

    /// The app stops: loads still carried are put down where their carriers
    /// are (`here`; lost if they are away), then all is written at once.
    pub fn stop(&mut self, r: &Rules, now: f64, here: impl Fn(&[u8; 32]) -> Option<Vec2>) {
        for (id, h) in std::mem::take(&mut self.held) {
            let Some(at) = here(&id).and_then(|p| self.spot_near(r, p)) else {
                continue;
            };
            let mut load = h.load;
            load.last = id;
            self.lying.push(Lying {
                at,
                until: now + f64::from(r.lying_s),
                load,
            });
        }
        let Some((db, region)) = &self.db else { return };
        let res = crate::db::connect(db)
            .map_err(|e| e.to_string())
            .and_then(|mut c| write_lying(&mut c, region, &self.lying).map_err(|e| e.to_string()));
        match res {
            Ok(()) => info!(loads = self.lying.len(), "carried loads saved"),
            Err(e) => warn!(error = %e, "carried loads not saved"),
        }
    }

    pub fn held(&self, id: &[u8; 32]) -> Option<&Held> {
        self.held.get(id)
    }

    fn bump(&mut self, side: Faction) {
        self.version[idx(side)] = self.version[idx(side)].wrapping_add(1);
    }

    fn save_site(&self, side: Faction) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Out::Site(side, self.sites[idx(side)].clone()));
        }
    }

    /// Take a load at `p`: a lying one of the side within `reach_m` (not
    /// last carried by `id`), else a fresh one if `at_source`.
    #[allow(clippy::too_many_arguments)]
    pub fn take(
        &mut self,
        r: &Rules,
        id: [u8; 32],
        name: Option<String>,
        side: Faction,
        p: Vec2,
        at_source: bool,
        now: f64,
    ) -> Result<Taken, &'static str> {
        if self.held.contains_key(&id) {
            return Err("carrying");
        }
        let near = self
            .lying
            .iter()
            .enumerate()
            .filter(|(_, l)| l.load.side == side && l.load.last != id)
            .map(|(i, l)| (i, l.at.p.distance(p)))
            .filter(|&(_, d)| d <= r.reach_m)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        let (mut load, taken) = match near {
            Some(i) => {
                let l = self.lying.swap_remove(i);
                self.bump(side);
                self.save_lying();
                (l.load, Taken::Handed)
            }
            None if at_source => (
                Load {
                    side,
                    fill: 1.0,
                    hands: Vec::new(),
                    last: id,
                },
                Taken::Fresh,
            ),
            None => return Err("nothing here"),
        };
        load.last = id;
        if let Some(n) = name.filter(|n| !load.hands.contains(n)) {
            load.hands.push(n);
        }
        self.held.insert(id, Held { load, since: now });
        Ok(taken)
    }

    /// Put `id`'s load down at `p` with `fill` left: at the side's site
    /// (within `deliver_m`) it is delivered, else it lies at `spot`.
    pub fn lay(
        &mut self,
        r: &Rules,
        id: &[u8; 32],
        p: Vec2,
        fill: f32,
        spot: Option<Spot>,
        now: f64,
    ) -> Result<Laid, &'static str> {
        let Some(h) = self.held.get(id) else {
            return Err("not carrying");
        };
        let side = h.load.side;
        let at_site = self.sites[idx(side)]
            .as_ref()
            .is_some_and(|s| s.at.p.distance(p) <= r.deliver_m);
        if !at_site && spot.is_none() {
            return Err("no street here");
        }
        let mut load = self.held.remove(id).map(|h| h.load).ok_or("not carrying")?;
        load.fill = fill.clamp(r.min_fill, 1.0).min(load.fill.max(r.min_fill));
        self.bump(side);
        if !at_site {
            self.lying.push(Lying {
                at: spot.ok_or("no street here")?,
                until: now + f64::from(r.lying_s),
                load,
            });
            self.save_lying();
            return Ok(Laid::Waiting);
        }
        let site = self.sites[idx(side)].as_mut().ok_or("no site")?;
        site.got.push(Delivery {
            at: now,
            fill: load.fill,
            names: load.hands,
        });
        let filled = site.filled();
        if filled + 1e-4 < r.loads {
            self.save_site(side);
            return Ok(Laid::Delivered(filled));
        }
        let site = self.sites[idx(side)].take().ok_or("no site")?;
        let mut names: Vec<String> = Vec::new();
        for n in site.got.iter().flat_map(|d| d.names.iter()) {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
        let b = Built {
            side,
            at: site.at,
            names,
            when: now,
        };
        self.built.push(b.clone());
        self.pending.push(b.clone());
        self.news.push(b.clone());
        self.built_version = self.built_version.wrapping_add(1);
        self.reopen[idx(side)] = now + f64::from(r.reopen_s);
        self.save_site(side);
        if let Some(tx) = &self.tx {
            let _ = tx.send(Out::Build(b.clone()));
        }
        info!(side = side.as_str(), node = b.at.id, "built");
        Ok(Laid::Built(b))
    }

    /// Hands whose time is up (`hand_s`): the caller puts their loads down.
    pub fn tired(&self, r: &Rules, now: f64) -> Vec<[u8; 32]> {
        self.held
            .iter()
            .filter(|(_, h)| now - h.since >= f64::from(r.hand_s))
            .map(|(id, _)| *id)
            .collect()
    }

    /// A tired hand with nowhere to put it: the load is gone.
    pub fn drop_held(&mut self, id: &[u8; 32]) {
        self.held.remove(id);
    }

    /// Once a second or so: loads that waited too long go, sites nobody
    /// finished move, due sites open (`sources`: the side's natural sources
    /// and buildings, to keep `site_min_m` from).
    pub fn tick(
        &mut self,
        r: &Rules,
        now: f64,
        sources: [&[i64]; 2],
        own: &dyn Fn(i64) -> Option<Faction>,
    ) {
        let before: Vec<Faction> = self
            .lying
            .iter()
            .filter(|l| l.until <= now)
            .map(|l| l.load.side)
            .collect();
        self.lying.retain(|l| l.until > now);
        if !before.is_empty() {
            self.save_lying();
        }
        for s in before {
            self.bump(s);
        }
        for side in [Faction::Fire, Faction::Water] {
            let i = idx(side);
            if self.sites[i]
                .as_ref()
                .is_some_and(|s| now - s.opened >= f64::from(r.site_s))
            {
                self.sites[i] = None;
                self.reopen[i] = now;
                self.bump(side);
                self.save_site(side);
            }
            let built = self.built.iter().filter(|b| b.side == side).count() as u32;
            if self.sites[i].is_none() && now >= self.reopen[i] && built < r.max_built {
                if let Some(at) = self.new_site(r, side, sources[i], own) {
                    self.sites[i] = Some(Site {
                        at,
                        opened: now,
                        got: Vec::new(),
                    });
                    self.bump(side);
                    self.save_site(side);
                } else {
                    // Nowhere fits now: try again in an hour.
                    self.reopen[i] = now + 3600.0;
                }
            }
        }
    }

    /// A site for `side`: `site_min_m`..`site_max_m` along the streets from
    /// its natural sources and buildings (`sources`), not on a building or the
    /// other site; nearest to a random place people go, own ground (`own`)
    /// first among those within 150 m of the nearest.
    fn new_site(
        &self,
        r: &Rules,
        side: Faction,
        sources: &[i64],
        own: &dyn Fn(i64) -> Option<Faction>,
    ) -> Option<Spot> {
        let taken = |s: &Spot| {
            self.built.iter().any(|b| b.at.id == s.id)
                || self.sites.iter().flatten().any(|x| x.at.id == s.id)
        };
        let d = self.distances(sources);
        let band: Vec<&Spot> = self
            .spots
            .iter()
            .zip(&d)
            .filter(|(s, _)| !taken(s))
            .filter(|(_, &d)| sources.is_empty() || (d >= r.site_min_m && d <= r.site_max_m))
            .map(|(s, _)| s)
            .collect();
        let near = *self.attract.choose(&mut rand::rng()).unwrap_or(&Vec2::ZERO);
        let dist = |s: &Spot| s.p.distance(near);
        let best = band.iter().map(|s| dist(s)).fold(f32::INFINITY, f32::min);
        let mine = |s: &Spot| own(s.id) == Some(side);
        band.into_iter()
            .filter(|s| dist(s) <= best + 150.0)
            .min_by(|a, b| mine(b).cmp(&mine(a)).then(dist(a).total_cmp(&dist(b))))
            .copied()
    }

    /// The nearest junction within `reach_m` of `p`.
    pub fn spot_near(&self, r: &Rules, p: Vec2) -> Option<Spot> {
        self.spots
            .iter()
            .map(|s| (s, s.p.distance(p)))
            .filter(|(_, d)| *d <= r.reach_m)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(s, _)| *s)
    }

    /// What `side` sees: its lying loads and its site.
    pub fn view(&self, r: &Rules, side: Faction, now: f64) -> LoadsView {
        LoadsView {
            lying: self
                .lying
                .iter()
                .filter(|l| l.load.side == side)
                .map(|l| LyingView {
                    lat: l.at.lat,
                    lon: l.at.lon,
                    left: (l.until - now).max(0.0) as u32,
                    fill: q2(l.load.fill),
                    hands: l.load.hands.len() as u32,
                })
                .collect(),
            site: self.sites[idx(side)].as_ref().map(|s| SiteView {
                lat: s.at.lat,
                lon: s.at.lon,
                filled: q2(s.filled()),
                need: r.loads,
                left: (s.opened + f64::from(r.site_s) - now).max(0.0) as u32,
            }),
        }
    }

    pub fn builds(&self) -> Vec<BuiltView> {
        self.built.iter().map(BuiltView::of).collect()
    }
}

/// Once a second: loads that waited too long go, sites move and open.
pub fn tick(
    mut carry: ResMut<Carry>,
    rules: Res<crate::sim::GameRules>,
    region: Option<Res<elements_shared::geo::RegionInfo>>,
    naturals: Query<(&elements_shared::geo::GeoNode, &crate::sim::NaturalSource)>,
    by_id: Res<crate::sim::NodeById>,
    states: Query<&elements_shared::elements::ElementState>,
    mut next: Local<f64>,
) {
    let r = &rules.0.carry;
    let now = crate::analytics::now_s();
    if !r.enabled || now < *next {
        return;
    }
    *next = now + 1.0;
    let _ = region;
    let mut src: [Vec<i64>; 2] = [Vec::new(), Vec::new()];
    for (g, n) in &naturals {
        src[idx(n.0.faction)].push(g.id);
    }
    let own = |id: i64| {
        by_id
            .get(&id)
            .and_then(|&e| states.get(e).ok())
            .and_then(|st| st.owner())
    };
    carry.tick(r, now, [&src[0], &src[1]], &own);
}

/// When the app stops: put carried loads down and write them (once).
pub fn save_on_stop(
    mut carry: ResMut<Carry>,
    rules: Res<crate::sim::GameRules>,
    stop: Res<crate::Stop>,
    clients: Res<crate::web::WebClients>,
    scores: Res<crate::score::Scores>,
    players: Query<(
        &elements_shared::components::Position,
        Option<&crate::score::ScoreSlot>,
    )>,
    mut done: Local<bool>,
) {
    use std::sync::atomic::Ordering::Relaxed;
    if *done || !(crate::gateway::SHUTDOWN.load(Relaxed) || stop.0.load(Relaxed)) {
        return;
    }
    *done = true;
    let here = |id: &[u8; 32]| {
        clients.0.values().find_map(|(e, _)| {
            let (pos, slot) = players.get(*e).ok()?;
            let rec = scores.get(slot?.0)?;
            (rec.id == *id).then(|| Vec2::new(pos.0.x, pos.0.z))
        })
    };
    carry.stop(&rules.0.carry, crate::analytics::now_s(), here);
}

/// Buildings become sources of their side for good: they come back after a
/// claim on them runs out (as natural sources do), the weather leaves them.
#[derive(Component, Clone, Copy)]
pub struct Building;

pub fn apply_builds(
    mut commands: Commands,
    mut carry: ResMut<Carry>,
    rules: Res<crate::sim::GameRules>,
    by_id: Res<crate::sim::NodeById>,
    mut states: Query<&mut elements_shared::elements::ElementState>,
) {
    if carry.pending.is_empty() {
        return;
    }
    for b in std::mem::take(&mut carry.pending) {
        let Some(&e) = by_id.get(&b.at.id) else {
            continue;
        };
        let mut src = crate::sim::ElementSource::natural(b.side, &rules.0);
        src.strength = rules.0.carry.built_strength;
        commands
            .entity(e)
            .insert((src, crate::sim::NaturalSource(src), Building));
        if let Ok(mut es) = states.get_mut(e) {
            match b.side {
                Faction::Fire => es.fire = es.fire.max(src.strength),
                Faction::Water => es.water = es.water.max(src.strength),
            }
        }
    }
}

fn q2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// One's own load (`ServerMsg::Carry`).
#[derive(Serialize, Clone, Debug, ts_rs::TS)]
#[ts(rename = "CarryView")]
pub struct CarryView {
    #[ts(as = "crate::protocol::Faction")]
    pub side: &'static str,
    pub fill: f32,
    /// Seconds this hand can still carry.
    pub left: u32,
    pub hands: u32,
}

impl CarryView {
    pub fn of(h: &Held, r: &Rules, now: f64) -> Self {
        Self {
            side: h.load.side.as_str(),
            fill: q2(h.load.fill),
            left: (h.since + f64::from(r.hand_s) - now).max(0.0) as u32,
            hands: h.load.hands.len() as u32,
        }
    }
}

#[derive(Serialize, Clone, Debug, ts_rs::TS)]
#[ts(rename = "LyingLoad")]
pub struct LyingView {
    pub lat: f64,
    pub lon: f64,
    pub left: u32,
    pub fill: f32,
    pub hands: u32,
}

#[derive(Serialize, Clone, Debug, ts_rs::TS)]
#[ts(rename = "SiteView")]
pub struct SiteView {
    pub lat: f64,
    pub lon: f64,
    pub filled: f32,
    pub need: f32,
    /// Seconds until it moves.
    pub left: u32,
}

#[derive(Serialize, Clone, Debug, ts_rs::TS)]
#[ts(rename = "LoadsView")]
pub struct LoadsView {
    pub lying: Vec<LyingView>,
    pub site: Option<SiteView>,
}

#[derive(Serialize, Clone, Debug, ts_rs::TS)]
#[ts(rename = "BuiltView")]
pub struct BuiltView {
    #[ts(as = "crate::protocol::Faction")]
    pub side: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub names: Vec<String>,
    /// Unix seconds.
    pub when: i64,
}

impl BuiltView {
    pub fn of(b: &Built) -> Self {
        Self {
            side: b.side.as_str(),
            lat: b.at.lat,
            lon: b.at.lon,
            names: b.names.clone(),
            when: b.when as i64,
        }
    }
}

/// The region's carrying from the database (sites, buildings, writer).
pub fn setup(
    conn_str: &str,
    graph: &[elements_shared::geo::GeoNode],
    edges: &[elements_shared::geo::GeoEdge],
    region: &elements_shared::geo::RegionInfo,
    places: &crate::places::Places,
) -> Carry {
    let o = (region.centroid_lat, region.centroid_lon);
    let at = |lat: f64, lon: f64| {
        let p = elements_shared::geo::project_enu(lat, lon, o.0, o.1);
        Vec2::new(p.x, p.z)
    };
    let spots = graph
        .iter()
        .map(|n| Spot {
            id: n.id,
            p: at(n.lat, n.lon),
            lat: n.lat,
            lon: n.lon,
        })
        .collect();
    // Where people go: hotspot sites, zones, else the hotspot centre.
    let mut attract: Vec<Vec2> = places
        .sites
        .iter()
        .map(|s| at(s.lat, s.lon))
        .chain(places.zones.iter().map(|z| at(z.lat, z.lon)))
        .collect();
    if attract.is_empty() {
        let (lat, lon) = region.hotspot_center.unwrap_or(o);
        attract.push(at(lat, lon));
    }
    let (sites, built, lying) = load(conn_str, &region.slug).unwrap_or_else(|e| {
        warn!(error = %e, "no carry sites or buildings");
        Default::default()
    });
    info!(built = built.len(), lying = lying.len(), "carry");
    Carry::new(
        spots,
        attract,
        sites,
        built,
        Some(writer(conn_str.to_string(), region.slug.clone())),
    )
    .with_streets(
        edges
            .iter()
            .map(|e| (e.source, e.target, e.length_m as f32)),
    )
    .with_lying(lying, Some((conn_str.to_string(), region.slug.clone())))
}

/// Stored sites and buildings of the region.
#[allow(clippy::type_complexity)]
pub fn load(
    db: &str,
    slug: &str,
) -> Result<([Option<Site>; 2], Vec<Built>, Vec<Lying>), Box<dyn std::error::Error>> {
    let mut c = crate::db::connect(db)?;
    let mut sites: [Option<Site>; 2] = [None, None];
    for r in c.query(
        "SELECT s.faction, s.node, n.lat, n.lon, extract(epoch FROM s.opened)::float8, s.got::text \
         FROM game.carry_sites s JOIN geo.nodes n ON n.id = s.node WHERE s.region = $1",
        &[&slug],
    )? {
        let Some(side) = Faction::parse(r.get(0)) else {
            continue;
        };
        let got: Vec<serde_json::Value> = serde_json::from_str(r.get(5)).unwrap_or_default();
        sites[idx(side)] = Some(Site {
            at: Spot {
                id: r.get(1),
                p: Vec2::ZERO,
                lat: r.get(2),
                lon: r.get(3),
            },
            opened: r.get(4),
            got: got
                .iter()
                .map(|d| Delivery {
                    at: d["at"].as_f64().unwrap_or(0.0),
                    fill: d["fill"].as_f64().unwrap_or(0.0) as f32,
                    names: d["names"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|n| n.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect(),
        });
    }
    let built = c
        .query(
            "SELECT b.faction, b.node, n.lat, n.lon, b.names, extract(epoch FROM b.built_at)::float8 \
             FROM game.carry_builds b JOIN geo.nodes n ON n.id = b.node WHERE b.region = $1 ORDER BY b.built_at",
            &[&slug],
        )?
        .iter()
        .filter_map(|r| {
            Some(Built {
                side: Faction::parse(r.get(0))?,
                at: Spot {
                    id: r.get(1),
                    p: Vec2::ZERO,
                    lat: r.get(2),
                    lon: r.get(3),
                },
                names: r.get(4),
                when: r.get(5),
            })
        })
        .collect();
    // Put-down loads apart: without their table (V33) the rest still loads.
    let lying = load_lying(&mut c, slug).unwrap_or_else(|e| {
        warn!(error = %e, "no put-down loads");
        Vec::new()
    });
    Ok((sites, built, lying))
}

fn load_lying(c: &mut postgres::Client, slug: &str) -> Result<Vec<Lying>, postgres::Error> {
    let now = crate::analytics::now_s();
    Ok(c
        .query(
            "SELECT l.node, n.lat, n.lon, l.faction, l.fill, l.hands, l.last, extract(epoch FROM l.until)::float8 \
             FROM game.carry_lying l JOIN geo.nodes n ON n.id = l.node WHERE l.region = $1",
            &[&slug],
        )?
        .iter()
        .filter_map(|r| {
            let until: f64 = r.get(7);
            (until > now).then_some(())?;
            Some(Lying {
                at: Spot {
                    id: r.get(0),
                    p: Vec2::ZERO,
                    lat: r.get(1),
                    lon: r.get(2),
                },
                until,
                load: Load {
                    side: Faction::parse(r.get(3))?,
                    fill: r.get(4),
                    hands: r.get(5),
                    last: <[u8; 32]>::try_from(r.get::<_, &[u8]>(6)).ok()?,
                },
            })
        })
        .collect())
}

/// Replace the region's stored put-down loads with `lying`.
fn write_lying(
    c: &mut postgres::Client,
    region: &str,
    lying: &[Lying],
) -> Result<(), postgres::Error> {
    let mut t = c.transaction()?;
    t.execute("DELETE FROM game.carry_lying WHERE region = $1", &[&region])?;
    for l in lying {
        t.execute(
            "INSERT INTO game.carry_lying (region, node, faction, fill, hands, last, until) \
             VALUES ($1, $2, $3, $4, $5, $6, to_timestamp($7))",
            &[
                &region,
                &l.at.id,
                &l.load.side.as_str(),
                &l.load.fill,
                &l.load.hands,
                &&l.load.last[..],
                &l.until,
            ],
        )?;
    }
    t.commit()
}

pub fn writer(db: String, region: String) -> crate::writers::LaneTx<Out> {
    crate::writers::lane(&format!("carry {region}"), |rx| Writer { db, region, rx })
}

struct Writer {
    db: String,
    region: String,
    rx: crossbeam_channel::Receiver<Out>,
}

impl crate::writers::Lane for Writer {
    fn run(&mut self, pg: &mut crate::writers::Db) -> bool {
        let db = Some(self.db.as_str());
        loop {
            let o = match self.rx.try_recv() {
                Ok(o) => o,
                Err(crossbeam_channel::TryRecvError::Empty) => return true,
                Err(crossbeam_channel::TryRecvError::Disconnected) => return false,
            };
            let Some(c) = pg.get(db) else { continue };
            let res = match &o {
                Out::Site(side, None) => c
                    .execute(
                        "DELETE FROM game.carry_sites WHERE region = $1 AND faction = $2",
                        &[&self.region, &side.as_str()],
                    )
                    .map(|_| ()),
                Out::Site(side, Some(s)) => {
                    let got: Vec<serde_json::Value> = s
                        .got
                        .iter()
                        .map(|d| serde_json::json!({"at": d.at, "fill": d.fill, "names": d.names}))
                        .collect();
                    let got = serde_json::Value::Array(got).to_string();
                    c.execute(
                        "INSERT INTO game.carry_sites (region, faction, node, opened, got) \
                         VALUES ($1, $2, $3, to_timestamp($4), $5::text::jsonb) \
                         ON CONFLICT (region, faction) DO UPDATE SET node = $3, opened = to_timestamp($4), got = $5::text::jsonb",
                        &[&self.region, &side.as_str(), &s.at.id, &s.opened, &got],
                    )
                    .map(|_| ())
                }
                Out::Lying(l) => write_lying(c, &self.region, l),
                Out::Build(b) => c
                    .execute(
                        "INSERT INTO game.carry_builds (region, node, faction, names, built_at) \
                         VALUES ($1, $2, $3, $4, to_timestamp($5)) ON CONFLICT DO NOTHING",
                        &[&self.region, &b.at.id, &b.side.as_str(), &b.names, &b.when],
                    )
                    .map(|_| ()),
            };
            if let Err(e) = res {
                pg.failed(db, "carry", &crate::db::detail(&e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_site_is_plus_zero() {
        let s = Site {
            at: spots()[0],
            opened: 0.0,
            got: vec![],
        };
        assert!(s.filled().is_sign_positive());
    }

    fn spots() -> Vec<Spot> {
        (0..20)
            .map(|i| Spot {
                id: i,
                p: Vec2::new(i as f32 * 100.0, 0.0),
                lat: 0.0,
                lon: 0.0,
            })
            .collect()
    }

    fn carry() -> Carry {
        Carry::new(
            spots(),
            vec![Vec2::new(900.0, 0.0)],
            [None, None],
            vec![],
            None,
        )
    }

    const A: [u8; 32] = [1; 32];
    const B: [u8; 32] = [2; 32];

    #[test]
    fn take_carry_hand_over_and_build() {
        let r = Rules::default();
        let mut c = carry();
        let fire_src = [0];
        c.tick(&r, 0.0, [&fire_src, &[19]], &|_| None);
        let site = c.sites[0].clone().expect("a fire site");
        assert!(
            site.at.p.x >= 400.0 && site.at.p.x <= 1000.0,
            "{:?}",
            site.at.p
        );
        assert_eq!(site.at.p.x, 900.0, "nearest to where people are");
        // Nothing at a bare street.
        assert_eq!(
            c.take(
                &r,
                A,
                None,
                Faction::Fire,
                Vec2::new(300.0, 0.0),
                false,
                0.0
            ),
            Err("nothing here")
        );
        assert_eq!(
            c.take(
                &r,
                A,
                Some("Ada".into()),
                Faction::Fire,
                Vec2::ZERO,
                true,
                0.0
            ),
            Ok(Taken::Fresh)
        );
        assert_eq!(
            c.take(&r, A, None, Faction::Fire, Vec2::ZERO, true, 0.0),
            Err("carrying")
        );
        // Half way: put down; A may not take it up again, B may.
        let mid = c.spot_near(&r, Vec2::new(405.0, 0.0));
        assert_eq!(
            c.lay(&r, &A, Vec2::new(405.0, 0.0), 0.9, mid, 10.0),
            Ok(Laid::Waiting)
        );
        assert_eq!(c.view(&r, Faction::Fire, 10.0).lying.len(), 1);
        assert!(
            c.view(&r, Faction::Water, 10.0).lying.is_empty(),
            "the other side doesn't see it"
        );
        assert_eq!(
            c.take(
                &r,
                A,
                None,
                Faction::Fire,
                Vec2::new(400.0, 0.0),
                false,
                20.0
            ),
            Err("nothing here")
        );
        assert_eq!(
            c.take(
                &r,
                B,
                None,
                Faction::Water,
                Vec2::new(400.0, 0.0),
                false,
                20.0
            ),
            Err("nothing here"),
            "not the other side's"
        );
        assert_eq!(
            c.take(
                &r,
                B,
                Some("Bo".into()),
                Faction::Fire,
                Vec2::new(400.0, 0.0),
                false,
                20.0
            ),
            Ok(Taken::Handed)
        );
        assert_eq!(c.held(&B).unwrap().load.hands, ["Ada", "Bo"]);
        // Delivered: 0.9 (spilled before) of 4.
        assert_eq!(
            c.lay(&r, &B, Vec2::new(890.0, 0.0), 1.0, None, 30.0),
            Ok(Laid::Delivered(0.9))
        );
        // 0.9 + 3 is not 4: the fourth full one finishes it.
        for k in 0..4 {
            c.take(
                &r,
                A,
                Some("Ada".into()),
                Faction::Fire,
                Vec2::ZERO,
                true,
                40.0,
            )
            .unwrap();
            let out = c.lay(&r, &A, Vec2::new(900.0, 0.0), 1.0, None, 50.0 + k as f64);
            if k < 3 {
                assert!(matches!(out, Ok(Laid::Delivered(_))), "{out:?}");
            } else {
                let Ok(Laid::Built(b)) = out else {
                    panic!("{out:?}")
                };
                assert_eq!(b.names, ["Ada", "Bo"]);
                assert_eq!(b.at.id, 9);
            }
        }
        assert!(c.sites[0].is_none());
        assert_eq!(c.pending.len(), 1);
        // The next site only after reopen_s.
        c.tick(&r, 100.0, [&fire_src, &[]], &|_| None);
        assert!(c.sites[0].is_none());
        c.tick(&r, 100.0 + f64::from(r.reopen_s), [&fire_src, &[]], &|_| {
            None
        });
        assert_ne!(c.sites[0].as_ref().unwrap().at.id, 9, "not on the building");
    }

    #[test]
    fn spill_floor_tired_hands_and_expiry() {
        let r = Rules::default();
        let mut c = carry();
        c.take(&r, A, None, Faction::Water, Vec2::ZERO, true, 0.0)
            .unwrap();
        assert!(c.tired(&r, 599.0).is_empty());
        assert_eq!(c.tired(&r, 600.0), [A]);
        let s = c.spot_near(&r, Vec2::new(10.0, 0.0));
        c.lay(&r, &A, Vec2::new(10.0, 0.0), 0.05, s, 600.0).unwrap();
        assert_eq!(c.lying[0].load.fill, r.min_fill, "never below the floor");
        c.take(&r, B, None, Faction::Water, Vec2::ZERO, false, 700.0)
            .unwrap();
        c.lay(&r, &B, Vec2::ZERO, 1.0, s, 800.0).unwrap();
        assert_eq!(c.lying[0].load.fill, r.min_fill, "spilled stays spilled");
        c.tick(&r, 800.0 + f64::from(r.lying_s), [&[], &[]], &|_| None);
        assert!(c.lying.is_empty(), "waited too long");
        assert_eq!(
            c.lay(&r, &B, Vec2::ZERO, 1.0, s, 900.0),
            Err("not carrying")
        );
    }

    #[test]
    fn unfinished_site_moves_and_cap() {
        let r = Rules {
            max_built: 0,
            ..Rules::default()
        };
        let mut c = carry();
        c.tick(&r, 0.0, [&[0], &[]], &|_| None);
        assert!(c.sites[0].is_none(), "cap reached");
        let r = Rules::default();
        c.tick(&r, 0.0, [&[0], &[]], &|_| None);
        assert!(c.sites[0].is_some());
        c.tick(&r, f64::from(r.site_s), [&[0], &[]], &|_| None);
        assert!(c.sites[0].is_some(), "moved: a new one at once");
        assert_eq!(c.sites[0].as_ref().unwrap().opened, f64::from(r.site_s));
    }

    #[test]
    fn a_building_is_a_source_for_good_and_the_weather_leaves_it() {
        use crate::sim::{expire_sources, ElementSource, NaturalSource, SourceKind};
        use crate::testkit::{advance, ent, line, world};
        use bevy::ecs::system::RunSystemOnce;
        let mut r = elements_shared::rules::Rules::default();
        r.weather.enabled = true;
        r.weather.live_natural = 0.5;
        let (n, e) = line(5, 200.0, &[]);
        let mut w = world(r.clone(), n, e);
        let node = 2;
        let b = Built {
            side: Faction::Fire,
            at: Spot {
                id: node,
                p: Vec2::ZERO,
                lat: 0.0,
                lon: 0.0,
            },
            names: vec!["Ada".into()],
            when: 0.0,
        };
        w.insert_resource(Carry::new(vec![], vec![], [None, None], vec![b], None));
        w.run_system_once(apply_builds).unwrap();
        let e2 = ent(&w, node);
        let s = *w.get::<ElementSource>(e2).expect("a source");
        assert_eq!((s.faction, s.strength, s.life), (Faction::Fire, 0.8, None));
        assert!(w.get::<Building>(e2).is_some());
        // A water claim on it runs out: the building is back.
        let claim = ElementSource::claim(
            &r,
            Faction::Water,
            SourceKind::Claim,
            10.0,
            Some(1),
            1.0,
            false,
            0.0,
        );
        w.entity_mut(e2).insert(claim);
        let id = w.register_system(expire_sources);
        advance(&mut w, 1000.0);
        w.run_system(id).unwrap();
        let s = *w.get::<ElementSource>(e2).unwrap();
        assert_eq!((s.faction, s.strength), (Faction::Fire, 0.8));
        // Rain everywhere: natural fire weakens, the building doesn't.
        w.insert_resource(crate::weather::BaseRules(r.clone()));
        w.insert_resource(crate::campaign::Campaign::spawn(None, "test".into()));
        w.insert_resource(crate::live_rain::LiveRain(
            crate::live_rain::LiveField::with(vec![1.0; 5]),
        ));
        w.run_system_once(crate::weather::apply).unwrap();
        assert_eq!(w.get::<NaturalSource>(e2).unwrap().0.strength, 0.8);
    }

    #[test]
    fn sites_by_street_distance_and_own_ground_first() {
        let r = Rules::default();
        // A row of junctions 100 m apart, the source at 0; the street from 0
        // to 1..3 goes round a block (1 km), the rest follows the row.
        let edges: Vec<(i64, i64, f32)> = (0..19)
            .map(|i| (i, i + 1, if i == 0 { 1000.0 } else { 100.0 }))
            .collect();
        let c = carry().with_streets(edges.clone().into_iter());
        let d = c.distances(&[0]);
        assert_eq!(d[1], 1000.0);
        assert_eq!(d[5], 1400.0);
        // People go to 300 m: as the crow flies 4..10 would fit, along the
        // streets only 1 (1000 m; 2 is 1100).
        let mut c = Carry::new(
            spots(),
            vec![Vec2::new(300.0, 0.0)],
            [None, None],
            vec![],
            None,
        )
        .with_streets(edges.clone().into_iter());
        c.tick(&r, 0.0, [&[0], &[]], &|_| None);
        assert_eq!(c.sites[0].as_ref().unwrap().at.id, 1);
        // A wider band (1..3 at 1000-1200 m), people at 250 m: 2 and 3 are
        // nearest, 1 is within 150 m of them and fire's own ground: 1 wins.
        let wide = Rules {
            site_max_m: 1300.0,
            ..Rules::default()
        };
        let mk = || {
            Carry::new(
                spots(),
                vec![Vec2::new(250.0, 0.0)],
                [None, None],
                vec![],
                None,
            )
            .with_streets(edges.clone().into_iter())
        };
        let mut c = mk();
        c.tick(&wide, 0.0, [&[0], &[]], &|_| None);
        assert!([2, 3].contains(&c.sites[0].as_ref().unwrap().at.id));
        let mut c = mk();
        c.tick(&wide, 0.0, [&[0], &[]], &|id| {
            (id == 1).then_some(Faction::Fire)
        });
        assert_eq!(c.sites[0].as_ref().unwrap().at.id, 1, "own ground first");
        let mut c = mk();
        c.tick(&wide, 0.0, [&[0], &[]], &|id| {
            (id == 1).then_some(Faction::Water)
        });
        assert_ne!(
            c.sites[0].as_ref().unwrap().at.id,
            1,
            "not the other side's"
        );
    }

    #[test]
    fn stopping_puts_carried_loads_down() {
        let r = Rules::default();
        let mut c = carry();
        c.take(
            &r,
            A,
            Some("Ada".into()),
            Faction::Fire,
            Vec2::ZERO,
            true,
            0.0,
        )
        .unwrap();
        c.take(&r, B, None, Faction::Water, Vec2::ZERO, true, 0.0)
            .unwrap();
        // A is still here (at 500 m), B has gone.
        c.stop(&r, 10.0, |id| (*id == A).then_some(Vec2::new(505.0, 0.0)));
        assert!(c.held(&A).is_none() && c.held(&B).is_none());
        assert_eq!(c.lying.len(), 1);
        assert_eq!(c.lying[0].at.id, 5);
        assert_eq!(c.lying[0].load.hands, ["Ada"]);
        assert_eq!(c.lying[0].until, 10.0 + f64::from(r.lying_s));
    }
}

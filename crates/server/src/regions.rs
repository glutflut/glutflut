//! Many regions in one server:
//! the active regions come from the database; each one the players open runs
//! as its own Bevy app on a worker of `pool.rs`, loaded on first use and
//! unloaded after a while without players. The gateway routes by `?region=`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tracing::{error, info, warn};

use crate::db::Listing;
use crate::gateway::{FromWeb, SharedJson};
use crate::tiles::SharedTiles;

/// How often the region list is read again (new regions without a restart).
const LIST_EVERY: Duration = Duration::from_secs(300);

pub struct HostConfig {
    pub db: Option<String>,
    /// Regions of this shard run here (`geo.regions.shard`).
    pub shard: String,
    /// Cell within the shard: only regions with this `geo.regions.cell` load
    /// here; `main` also runs the shard-wide jobs.
    pub cell: String,
    /// Other shards' base URLs (`ELEMENTS_SHARDS="sa=https://sa.example"`).
    pub shard_urls: HashMap<String, String>,
    /// For requests without `?region=` (old app versions, tools); kept loaded.
    pub default_region: Option<String>,
    /// Kept loaded from the start, never unloaded.
    pub preload: Vec<String>,
    /// Unload after this long without connections or requests.
    pub idle: Duration,
    /// Most regions loaded at once; beyond it the least recently used idle one goes.
    pub max_loaded: usize,
    /// Most junctions loaded at once (0: no limit); idle ones go to make room.
    pub max_junctions: u64,
    pub demo_day: Option<String>,
}

/// What a region app gets from the host.
pub struct RegionStart {
    pub db: Option<String>,
    pub slug: String,
    pub rules: Option<String>,
    /// Bench only: the graph from this file instead of the database.
    pub graph_file: Option<std::path::PathBuf>,
    pub rx: crossbeam_channel::Receiver<FromWeb>,
    pub tiles: SharedTiles,
    pub overview: SharedJson,
    pub nodes: crate::gateway::NodesShare,
    pub board: SharedJson,
    /// Set to unload: the app writes what it holds and exits.
    pub stop: Arc<AtomicBool>,
    /// To tell the other regions (a player deleted).
    pub host: Arc<Host>,
}

/// A loaded (or loading) region as the gateway sees it.
pub struct Region {
    pub slug: String,
    pub tx: crate::pool::RegionTx,
    pub tiles: SharedTiles,
    pub overview: SharedJson,
    pub nodes: crate::gateway::NodesShare,
    pub board: SharedJson,
    /// Whole graph as GeoJSON, built on first request.
    pub full: tokio::sync::OnceCell<Arc<str>>,
    stop: Arc<AtomicBool>,
    conns: AtomicUsize,
    /// Unix seconds of the last use.
    used: AtomicU64,
    /// Set once the app has written what it held and stopped.
    pub done: crate::pool::Done,
}

impl Region {
    /// Tiles are there once the app has loaded the graph.
    pub fn ready(&self) -> bool {
        self.tiles.read().unwrap().is_some()
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Ask the app to write what it holds and stop (`done` once it has).
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Open WebSockets.
    pub fn conns(&self) -> usize {
        self.conns.load(Ordering::Relaxed)
    }
}

/// Region loads: 6 at once, then one a minute.
static LAUNCHES: crate::limits::Limiter = crate::limits::Limiter::new(1.0 / 60.0, 6.0);

pub enum Unavailable {
    /// Not an active region (or none given and no default).
    Unknown,
    /// Runs on another shard: its base URL.
    Elsewhere(String),
    /// Being unloaded, or no room to load it; ask again in a moment.
    Busy,
    /// This shard, another cell: the proxy sent it to the wrong server.
    OtherCell,
}

/// A loaded region as the junction budget sees it.
struct Held {
    size: u64,
    stopping: bool,
    /// No players and not pinned: may go.
    idle: bool,
    used: u64,
}

#[derive(Debug, PartialEq)]
enum Room {
    Fits,
    /// Not now: stop these (least recently used idle first); `enough` if that
    /// frees what is needed once they are gone.
    Wait {
        stop: Vec<usize>,
        enough: bool,
    },
}

/// Room for `want` junctions under `budget` (0: none). Stopping regions
/// count as held (their memory isn't free yet) and as freeing. A region alone
/// always loads, even beyond the budget.
fn room(budget: u64, want: u64, held: &[Held]) -> Room {
    let total: u64 = held.iter().map(|h| h.size).sum();
    if budget == 0 || total == 0 || total + want <= budget {
        return Room::Fits;
    }
    let need = total + want - budget;
    let mut freeing: u64 = held.iter().filter(|h| h.stopping).map(|h| h.size).sum();
    let mut idle: Vec<usize> = (0..held.len())
        .filter(|&i| !held[i].stopping && held[i].idle)
        .collect();
    idle.sort_by_key(|&i| held[i].used);
    let mut stop = Vec::new();
    for i in idle {
        if freeing >= need {
            break;
        }
        freeing += held[i].size;
        stop.push(i);
    }
    Room::Wait {
        stop,
        enough: freeing >= need,
    }
}

pub struct Host {
    cfg: HostConfig,
    list: RwLock<Arc<Vec<Listing>>>,
    /// `GET /api/regions` (GeoJSON outlines) and `?v=2` (packed), each with
    /// its ETag.
    list_json: RwLock<[(Arc<str>, Arc<str>); 2]>,
    loaded: Mutex<HashMap<String, Arc<Region>>>,
    /// Active junctions per region of this shard (budget), counted once.
    sizes: Mutex<HashMap<String, u64>>,
    /// Last extra list read for a region of another cell ([`Host::reread`]).
    reread_at: Mutex<Option<Instant>>,
    /// Last request that used the list or a region (unix s): the list is
    /// read again only while someone uses this server (scale to zero).
    touched: AtomicU64,
    /// Other cells of this shard registered in `ops.cells` (read with the list).
    registered: RwLock<Vec<String>>,
    /// Globe history of this shard's regions (`marble.rs`).
    pub marble: crate::marble::Collector,
}

impl Host {
    pub fn new(cfg: HostConfig) -> Arc<Self> {
        let host = Arc::new(Self {
            cfg,
            list: RwLock::default(),
            list_json: RwLock::new([("[]".into(), "\"0\"".into()), ("[]".into(), "\"0\"".into())]),
            loaded: Mutex::default(),
            sizes: Mutex::default(),
            reread_at: Mutex::default(),
            touched: AtomicU64::new(now()),
            registered: RwLock::default(),
            marble: crate::marble::Collector::default(),
        });
        host.refresh_list();
        host
    }

    pub fn demo_day(&self) -> Option<&str> {
        self.cfg.demo_day.as_deref()
    }

    pub fn db(&self) -> Option<&str> {
        self.cfg.db.as_deref()
    }

    /// Read the active regions from the database; keeps the last list on error.
    pub fn refresh_list(&self) {
        let list = match self.cfg.db.as_deref().map(crate::db::list_regions) {
            Some(Ok(l)) => l,
            Some(Err(e)) => {
                error!(error = %e, "region list: query failed");
                return;
            }
            None => Vec::new(),
        };
        // Cells registered before their first region (`ops.cells`).
        if let Some(db) = self.cfg.db.as_deref() {
            match crate::db::registered_cells(db, &self.cfg.shard, &self.cfg.cell) {
                Ok(c) => *self.registered.write().unwrap() = c,
                Err(e) => warn!(error = %e, "region list: cells query failed"),
            }
        }
        let json: Vec<serde_json::Value> = list
            .iter()
            .map(|l| {
                let mut v = l.json.clone();
                if l.shard != self.cfg.shard {
                    v["url"] = self.cfg.shard_urls.get(&l.shard).cloned().into();
                }
                v
            })
            .collect();
        let packed: Vec<serde_json::Value> = json
            .iter()
            .map(|v| {
                let mut v = v.clone();
                if let Some(o) = v.as_object_mut().and_then(|o| o.remove("outline")) {
                    v["ol"] = pack_outline(&o);
                }
                v
            })
            .collect();
        let tagged = |v: &[serde_json::Value]| {
            let body = serde_json::to_string(v).unwrap();
            let tag = etag(&body);
            (Arc::<str>::from(body), Arc::<str>::from(tag))
        };
        *self.list_json.write().unwrap() = [tagged(&json), tagged(&packed)];
        // A missing per-region set shows at start, not when someone plays there.
        for l in list.iter().filter(|l| self.here(l)) {
            if let Some(set) = l.rules.as_deref() {
                let path = crate::actions::rules_path(set);
                if !path.is_file() {
                    error!(region = %l.slug, set, path = %path.display(), "rules file missing; the region would play ELEMENTS_RULES");
                }
            }
        }
        self.count_sizes(&list);
        let here = list.iter().filter(|l| self.here(l)).count();
        info!(regions = list.len(), here, shard = %self.cfg.shard, cell = %self.cfg.cell, "region list");
        *self.list.write().unwrap() = Arc::new(list);
    }

    /// Junctions of this shard's regions not counted yet (new or reloaded).
    fn count_sizes(&self, list: &[Listing]) {
        let (Some(db), true) = (self.cfg.db.as_deref(), self.cfg.max_junctions > 0) else {
            return;
        };
        let todo: Vec<String> = {
            let sizes = self.sizes.lock().unwrap();
            list.iter()
                .filter(|l| self.here(l) && !sizes.contains_key(&l.slug))
                .map(|l| l.slug.clone())
                .collect()
        };
        if todo.is_empty() {
            return;
        }
        match crate::db::junction_counts(db, &todo) {
            Ok(rows) => self.sizes.lock().unwrap().extend(rows),
            Err(e) => error!(error = %e, "region sizes: query failed"),
        }
    }

    /// Active junctions of a region (0 if not counted).
    fn size(&self, slug: &str) -> u64 {
        self.sizes.lock().unwrap().get(slug).copied().unwrap_or(0)
    }

    /// Whether `slug` fits the junction budget now; if not, idle regions go
    /// to make room ([`room`]).
    fn fits(self: &Arc<Self>, slug: &str, loaded: &HashMap<String, Arc<Region>>) -> bool {
        let rs: Vec<&Arc<Region>> = loaded.values().collect();
        let held: Vec<Held> = rs
            .iter()
            .map(|r| Held {
                size: self.size(&r.slug),
                stopping: r.stopping(),
                idle: r.conns() == 0 && !self.pinned(&r.slug),
                used: r.used.load(Ordering::Relaxed),
            })
            .collect();
        let want = self.size(slug);
        match room(self.cfg.max_junctions, want, &held) {
            Room::Fits => true,
            Room::Wait { stop, enough } => {
                // Stuck only if nothing goes at all (else: alone once they're gone).
                let stuck = !enough && stop.is_empty() && !held.iter().any(|h| h.stopping);
                for i in stop {
                    self.stop_region(rs[i].clone(), "room in the junction budget");
                }
                if stuck {
                    let held: u64 = held.iter().map(|h| h.size).sum();
                    let budget = self.cfg.max_junctions;
                    warn!(
                        slug,
                        want, held, budget, "junction budget full; not loading"
                    );
                }
                false
            }
        }
    }

    /// The list body and its ETag; `packed`: outlines as [`pack_outline`].
    pub fn list_json(&self, packed: bool) -> (Arc<str>, Arc<str>) {
        self.touch();
        self.list_json.read().unwrap()[usize::from(packed)].clone()
    }

    /// The region's list entry (any shard).
    /// All active regions (every shard).
    pub fn listings(&self) -> Arc<Vec<Listing>> {
        self.list.read().unwrap().clone()
    }

    pub fn shard(&self) -> &str {
        &self.cfg.shard
    }

    pub fn cell(&self) -> &str {
        &self.cfg.cell
    }

    /// Runs the shard-wide jobs (globe manifest, shard index, cleanup): with
    /// other cells the holder of the main lease, alone the `main` cell.
    pub fn is_main(&self) -> bool {
        crate::cells::main_here().unwrap_or(self.cfg.cell == "main")
    }

    /// The shard's other cells (from the region list).
    pub fn other_cells(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .listings()
            .iter()
            .filter(|l| l.shard == self.cfg.shard && l.cell != self.cfg.cell)
            .map(|l| l.cell.clone())
            .chain(self.registered.read().unwrap().iter().cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// A region of this server: this shard and this cell.
    pub fn here(&self, l: &Listing) -> bool {
        l.shard == self.cfg.shard && l.cell == self.cfg.cell
    }

    pub fn listing(&self, slug: &str) -> Option<Listing> {
        self.list
            .read()
            .unwrap()
            .iter()
            .find(|l| l.slug == slug)
            .cloned()
    }

    /// `?region=` or the default: a slug on the list.
    pub fn resolve(&self, asked: Option<&str>) -> Option<String> {
        let slug = asked
            .filter(|s| !s.is_empty())
            .or(self.cfg.default_region.as_deref())?;
        self.listing(slug).map(|l| l.slug)
    }

    /// The region if it runs, without starting it or counting as use.
    pub fn peek(&self, slug: &str) -> Option<Arc<Region>> {
        self.loaded
            .lock()
            .unwrap()
            .get(slug)
            .filter(|r| !r.stopping())
            .cloned()
    }

    /// The running region, started if needed.
    /// A request used this server.
    pub fn touch(&self) {
        self.touched.store(now(), Ordering::Relaxed);
    }

    /// Nobody played here for a while and no region is loaded: the database
    /// may sleep (health checks don't wake it).
    pub fn quiet(&self) -> bool {
        now().saturating_sub(self.touched.load(Ordering::Relaxed)) > LIST_EVERY.as_secs()
            && self.loaded.lock().unwrap().is_empty()
    }

    pub fn get(self: &Arc<Self>, slug: &str) -> Result<Arc<Region>, Unavailable> {
        self.touch();
        let listing = self.listing(slug).ok_or(Unavailable::Unknown)?;
        if listing.shard != self.cfg.shard {
            return Err(self
                .cfg
                .shard_urls
                .get(&listing.shard)
                .map_or(Unavailable::Unknown, |u| Unavailable::Elsewhere(u.clone())));
        }
        // Never a second app for another cell's region (same database). Just
        // moved here? The list may be older than the move: read it again.
        let listing = if listing.cell != self.cfg.cell && self.reread() {
            self.listing(slug).ok_or(Unavailable::Unknown)?
        } else {
            listing
        };
        if listing.cell != self.cfg.cell {
            warn!(slug, cell = %listing.cell, "region of another cell asked here; check the proxy");
            return Err(Unavailable::OtherCell);
        }
        let mut loaded = self.loaded.lock().unwrap();
        if let Some(r) = loaded.get(slug) {
            if r.stopping() {
                return Err(Unavailable::Busy);
            }
            r.used.store(now(), Ordering::Relaxed);
            return Ok(r.clone());
        }
        // Busy while the regions making room write and stop; the app asks again.
        if !self.fits(slug, &loaded) {
            return Err(Unavailable::Busy);
        }
        // Loading costs memory and database time: never beyond the limit,
        // and only so many a minute (pinned ones always).
        if !self.pinned(slug) && !LAUNCHES.allow(crate::limits::Client::ANY) {
            warn!(slug, "too many region loads; try again later");
            return Err(Unavailable::Busy);
        }
        let busy = loaded.values().filter(|r| !r.stopping()).count();
        if busy >= self.cfg.max_loaded {
            match self.least_used_idle(&loaded) {
                Some(old) => self.stop_region(old, "room for another region"),
                None => {
                    warn!(slug, busy, "all loaded regions in use; not loading");
                    return Err(Unavailable::Busy);
                }
            }
        }
        // Another cell still has it (moved, not let go yet): ask again soon.
        if !crate::cells::claim(slug) {
            return Err(Unavailable::Busy);
        }
        let r = self.launch(&listing);
        loaded.insert(slug.to_string(), r.clone());
        Ok(r)
    }

    fn launch(self: &Arc<Self>, l: &Listing) -> Arc<Region> {
        self.launch_with(l, None)
    }

    fn launch_with(
        self: &Arc<Self>,
        l: &Listing,
        graph_file: Option<std::path::PathBuf>,
    ) -> Arc<Region> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let region = Arc::new(Region {
            slug: l.slug.clone(),
            tx: crate::pool::RegionTx::new(tx),
            tiles: Arc::default(),
            overview: Arc::default(),
            nodes: Default::default(),
            board: Arc::default(),
            full: Default::default(),
            stop: Arc::default(),
            conns: AtomicUsize::new(0),
            used: AtomicU64::new(now()),
            done: Default::default(),
        });
        let start = RegionStart {
            db: self.cfg.db.clone(),
            slug: l.slug.clone(),
            rules: l.rules.clone(),
            graph_file,
            rx,
            tiles: region.tiles.clone(),
            overview: region.overview.clone(),
            nodes: region.nodes.clone(),
            board: region.board.clone(),
            stop: region.stop.clone(),
            host: self.clone(),
        };
        info!(slug = %l.slug, rules = ?l.rules, "region: loading");
        crate::pool::start(start, region.clone());
        region
    }

    /// Kept loaded: `ELEMENTS_PRELOAD` only. The default region loads at
    /// start but may sleep like any other (an idle database scales to zero).
    fn pinned(&self, slug: &str) -> bool {
        self.cfg.preload.iter().any(|p| p == slug)
    }

    /// `ELEMENTS_REGION`, if it is an active region of this server.
    pub fn default_known(&self) -> bool {
        self.cfg
            .default_region
            .as_deref()
            .and_then(|d| self.listing(d))
            .is_some_and(|l| self.here(&l))
    }

    fn least_used_idle(&self, loaded: &HashMap<String, Arc<Region>>) -> Option<Arc<Region>> {
        loaded
            .values()
            .filter(|r| {
                !r.stopping() && r.conns.load(Ordering::Relaxed) == 0 && !self.pinned(&r.slug)
            })
            .min_by_key(|r| r.used.load(Ordering::Relaxed))
            .cloned()
    }

    /// Stop a region; a helper thread waits for it to write and exit, then
    /// drops it from the map (so this never takes the `loaded` lock itself).
    fn stop_region(self: &Arc<Self>, r: Arc<Region>, why: &str) {
        if r.stop.swap(true, Ordering::Relaxed) {
            return;
        }
        info!(slug = %r.slug, why, "region: unloading");
        let host = self.clone();
        std::thread::spawn(move || {
            r.done.wait();
            host.loaded.lock().unwrap().remove(&r.slug);
            crate::cells::release(&r.slug);
            drop(r);
            crate::trim_memory("region unloaded");
        });
    }

    /// Ops: unload a region and load it again from the database (new graph,
    /// outline, settings or rules without a restart). Loaded again right away
    /// if it was loaded or is pinned; otherwise the next player loads it.
    /// `None`: not a region of this shard; else whether it was loaded.
    pub fn reload(self: &Arc<Self>, slug: &str) -> Option<bool> {
        self.sizes.lock().unwrap().remove(slug);
        self.refresh_list();
        let l = self.listing(slug).filter(|l| self.here(l))?;
        let old = self.loaded.lock().unwrap().get(slug).cloned();
        let was = old.is_some();
        if let Some(r) = old {
            self.stop_region(r, "ops reload");
        }
        if was || self.pinned(slug) {
            let host = self.clone();
            std::thread::spawn(move || {
                let t0 = Instant::now();
                while host
                    .loaded
                    .lock()
                    .unwrap()
                    .get(&l.slug)
                    .is_some_and(|r| r.stopping())
                {
                    if t0.elapsed() > Duration::from_secs(300) {
                        warn!(slug = %l.slug, "reload: old region did not stop");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                if host.get(&l.slug).is_err() {
                    warn!(slug = %l.slug, "reload: could not load again");
                }
            });
        }
        Some(was)
    }

    /// Send an event to every loaded region (but `except`).
    /// Returns how many got it.
    pub fn broadcast(&self, except: Option<&str>, ev: impl Fn() -> FromWeb) -> usize {
        let mut n = 0;
        for r in self.loaded.lock().unwrap().values() {
            if except != Some(r.slug.as_str()) && r.tx.send(ev()).is_ok() {
                n += 1;
            }
        }
        n
    }

    /// Unload regions idle for longer than `idle`.
    pub fn sweep(self: &Arc<Self>) {
        let now = now();
        let idle: Vec<Arc<Region>> = self
            .loaded
            .lock()
            .unwrap()
            .values()
            .filter(|r| {
                !r.stopping()
                    && !self.pinned(&r.slug)
                    && r.conns.load(Ordering::Relaxed) == 0
                    && now.saturating_sub(r.used.load(Ordering::Relaxed)) >= self.cfg.idle.as_secs()
            })
            .cloned()
            .collect();
        for r in idle {
            self.stop_region(r, "idle");
        }
        // Moved to another cell or shard: write and let go (pinned ones too).
        let moved: Vec<Arc<Region>> = self
            .loaded
            .lock()
            .unwrap()
            .values()
            .filter(|r| !r.stopping() && self.listing(&r.slug).is_some_and(|l| !self.here(&l)))
            .cloned()
            .collect();
        for r in moved {
            self.stop_region(r, "moved away");
        }
    }

    /// Read the region list again unless that happened in the last 5 s.
    pub fn reread(&self) -> bool {
        let mut last = self.reread_at.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return false;
        }
        *last = Some(Instant::now());
        drop(last);
        // Called from the web handlers too: blocking database work.
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.refresh_list());
        } else {
            self.refresh_list();
        }
        true
    }

    /// Someone is connected to a region of this server.
    pub fn connected(&self) -> bool {
        self.loaded.lock().unwrap().values().any(|r| r.conns() > 0)
    }

    /// Loaded regions that aren't stopping (lease renewal).
    pub fn loaded_slugs(&self) -> Vec<String> {
        self.loaded
            .lock()
            .unwrap()
            .values()
            .filter(|r| !r.stopping())
            .map(|r| r.slug.clone())
            .collect()
    }

    /// Stop a region now (its lease went to another cell).
    pub fn unload(self: &Arc<Self>, slug: &str, why: &str) {
        let r = self.loaded.lock().unwrap().get(slug).cloned();
        if let Some(r) = r {
            self.stop_region(r, why);
        }
    }

    /// Load the pinned regions.
    pub fn preload(self: &Arc<Self>) {
        let pinned: Vec<String> = self
            .cfg
            .default_region
            .iter()
            .chain(&self.cfg.preload)
            .cloned()
            .collect();
        for slug in pinned {
            if self.get(&slug).is_err() {
                warn!(slug, "preload: not an active region of this shard");
            }
        }
    }

    /// Wait for every region app to write and exit (on SIGTERM they stop by
    /// themselves).
    pub fn join_all(&self) {
        let regions: Vec<_> = self.loaded.lock().unwrap().values().cloned().collect();
        for r in regions {
            r.done.wait();
        }
    }

    /// Bench: run a region from a graph file, outside the region list.
    pub fn launch_file(self: &Arc<Self>, slug: &str, file: std::path::PathBuf) -> Arc<Region> {
        let l = Listing {
            slug: slug.into(),
            shard: self.cfg.shard.clone(),
            cell: self.cfg.cell.clone(),
            rules: None,
            demo_day: None,
            json: serde_json::Value::Null,
        };
        let r = self.launch_with(&l, Some(file));
        self.loaded.lock().unwrap().insert(slug.into(), r.clone());
        r
    }

    /// Main thread: refresh the list, unload idle regions, until shutdown.
    pub fn supervise(self: &Arc<Self>) {
        let mut listed = Instant::now();
        while !crate::gateway::SHUTDOWN.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            // Only while someone uses this server: an idle database sleeps.
            let used =
                now().saturating_sub(self.touched.load(Ordering::Relaxed)) < LIST_EVERY.as_secs();
            if listed.elapsed() >= LIST_EVERY && used {
                listed = Instant::now();
                self.refresh_list();
            }
            self.sweep();
        }
        self.join_all();
    }
}

/// The host, for a region app to reach the others.
#[derive(bevy::prelude::Resource, Clone)]
pub struct Peers(pub Arc<Host>);

/// Counts an open WebSocket against its region (no unload while connected).
pub struct ConnGuard(Arc<Region>);

impl ConnGuard {
    pub fn new(r: Arc<Region>) -> Self {
        r.conns.fetch_add(1, Ordering::Relaxed);
        Self(r)
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.used.store(now(), Ordering::Relaxed);
        self.0.conns.fetch_sub(1, Ordering::Relaxed);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A GeoJSON (Multi)Polygon as rings of integer steps at 1e-4° (~10 m; the
/// outlines are simplified to ~50 m anyway): `[[[x0, y0, dx, dy, ...], ...], ...]`,
/// polygons of rings, the first value pair absolute, then differences. A
/// third of the GeoJSON's size; the app adds them up again (`region.ts`).
pub fn pack_outline(o: &serde_json::Value) -> serde_json::Value {
    let polys = match o["type"].as_str() {
        Some("Polygon") => vec![&o["coordinates"]],
        Some("MultiPolygon") => o["coordinates"]
            .as_array()
            .map_or(vec![], |a| a.iter().collect()),
        _ => vec![],
    };
    let q = |v: &serde_json::Value| (v.as_f64().unwrap_or(0.0) * 1e4).round() as i64;
    let ring = |r: &serde_json::Value| {
        let (mut px, mut py) = (0, 0);
        let mut out = Vec::new();
        for p in r.as_array().into_iter().flatten() {
            let (x, y) = (q(&p[0]), q(&p[1]));
            out.extend([x - px, y - py]);
            (px, py) = (x, y);
        }
        out
    };
    polys
        .into_iter()
        .map(|p| {
            p.as_array()
                .into_iter()
                .flatten()
                .map(ring)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .into()
}

/// A strong ETag of a body.
fn etag(body: &str) -> String {
    use sha2::Digest;
    let h = sha2::Sha256::digest(body.as_bytes());
    format!(
        "\"{}\"",
        h[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::{room, Held, Room};

    fn h(size: u64, stopping: bool, idle: bool, used: u64) -> Held {
        Held {
            size,
            stopping,
            idle,
            used,
        }
    }

    #[test]
    fn junction_budget_makes_room_with_the_oldest_idle() {
        // Fits, or no limit, or nothing else loaded.
        assert_eq!(room(1000, 300, &[h(600, false, false, 1)]), Room::Fits);
        assert_eq!(room(0, 5000, &[h(600, false, false, 1)]), Room::Fits);
        assert_eq!(room(1000, 5000, &[]), Room::Fits);
        // Over: the least recently used idle one goes, not the one in use.
        let held = [
            h(400, false, true, 5),
            h(400, false, false, 1),
            h(300, false, true, 2),
        ];
        assert_eq!(
            room(1000, 200, &held),
            Room::Wait {
                stop: vec![2],
                enough: true
            }
        );
        assert_eq!(
            room(1000, 500, &held),
            Room::Wait {
                stop: vec![2, 0],
                enough: true
            }
        );
        // A stopping one already frees its share: wait, stop nothing more.
        let held = [h(600, true, true, 1), h(300, false, true, 2)];
        assert_eq!(
            room(1000, 400, &held),
            Room::Wait {
                stop: vec![],
                enough: true
            }
        );
        // Only regions in use: nothing to stop.
        assert_eq!(
            room(1000, 500, &[h(800, false, false, 1)]),
            Room::Wait {
                stop: vec![],
                enough: false
            }
        );
    }

    #[test]
    fn packed_outlines_add_up_again() {
        let o = serde_json::json!({"type": "Polygon", "coordinates": [[[9.17501, 47.66], [9.18, 47.661], [9.1749, 47.6599], [9.17501, 47.66]]]});
        let p = super::pack_outline(&o);
        assert_eq!(
            p,
            serde_json::json!([[[91750, 476600, 50, 10, -51, -11, 1, 1]]])
        );
        let m = serde_json::json!({"type": "MultiPolygon", "coordinates": [o["coordinates"], o["coordinates"]]});
        assert_eq!(super::pack_outline(&m).as_array().unwrap().len(), 2);
        assert_eq!(super::etag("[]"), super::etag("[]"));
        assert_ne!(super::etag("[]"), super::etag("[1]"));
    }
}

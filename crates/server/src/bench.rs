//! `elements-server bench`: many regions in one process without a database.
//! Loads graph files (JSON, see `GraphFile`) as N regions, measures load
//! time, threads, memory and CPU at rest, then with walking players in some
//! regions (claim → answer latency). Writers drop their rows.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use elements_shared::geo::{GeoEdge, GeoNode, RegionGraph, RegionInfo};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::gateway::FromWeb;
use crate::regions::{Host, HostConfig};
use crate::tiles::{tile_of, TileKey};

#[derive(clap::Args, Debug, Clone)]
pub struct BenchArgs {
    /// Graph files (JSON, see the module docs), used in turn for the regions.
    #[arg(long, required = true)]
    graph: Vec<PathBuf>,
    /// Regions to load.
    #[arg(long, default_value_t = 100)]
    regions: usize,
    /// Regions with players (the first ones).
    #[arg(long, default_value_t = 1)]
    active: usize,
    /// Walking players per active region (0: rest only).
    #[arg(long, default_value_t = 50)]
    players: usize,
    /// Seconds before each measurement.
    #[arg(long, default_value_t = 20)]
    warmup: u64,
    /// Seconds per measurement.
    #[arg(long, default_value_t = 30)]
    secs: u64,
}

#[derive(Deserialize)]
struct FileNode {
    id: i64,
    lat: f64,
    lon: f64,
    elev: f64,
    node_type: String,
    affinity: f32,
}

#[derive(Deserialize)]
struct FileEdge {
    id: i64,
    source: i64,
    target: i64,
    len: f32,
    fcf: f32,
    fcr: f32,
    wcf: f32,
    wcr: f32,
    wkt: String,
}

/// A region's street graph as JSON: `centroid` (lat, lon), `nodes` (id,
/// lat, lon, elev in m, node_type, affinity), `edges` (id, source, target,
/// len in m, fire/water costs forward and reverse, WKT line) and optional
/// `blocks` (id, area in m², border node ids).
#[derive(Deserialize)]
struct GraphFile {
    centroid: (f64, f64),
    nodes: Vec<FileNode>,
    edges: Vec<FileEdge>,
    #[serde(default)]
    blocks: Vec<FileBlock>,
}

/// A block of the graph file (no outline).
#[derive(Deserialize)]
struct FileBlock {
    id: i32,
    area: f32,
    nodes: Vec<i64>,
}

/// A region from a graph file: no zones, finds or hotspot area.
pub fn load_file(path: &Path, slug: &str) -> Result<crate::RegionData, Box<dyn std::error::Error>> {
    let f: GraphFile = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let (mut lo_lat, mut lo_lon, mut hi_lat, mut hi_lon) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
    for n in &f.nodes {
        (lo_lat, lo_lon) = (lo_lat.min(n.lat), lo_lon.min(n.lon));
        (hi_lat, hi_lon) = (hi_lat.max(n.lat), hi_lon.max(n.lon));
    }
    let region = RegionInfo {
        slug: slug.into(),
        name: slug.into(),
        centroid_lat: f.centroid.0,
        centroid_lon: f.centroid.1,
        bounds_min_lat: lo_lat,
        bounds_min_lon: lo_lon,
        bounds_max_lat: hi_lat,
        bounds_max_lon: hi_lon,
        hotspot_center: None,
        hotspot_area_m: None,
        timezone: None,
    };
    let meta = serde_json::json!({
        "slug": slug, "name": slug, "demo_label": null, "demo_day": null, "language": "de",
        "centroid": [f.centroid.1, f.centroid.0],
        "bounds": [lo_lon, lo_lat, hi_lon, hi_lat],
    });
    let lines = f.edges.iter().map(|e| (e.id, wkt_line(&e.wkt))).collect();
    let mut nodes: Vec<GeoNode> = f
        .nodes
        .into_iter()
        .map(|n| GeoNode {
            id: n.id,
            lat: n.lat,
            lon: n.lon,
            elevation_m: n.elev,
            node_type: n.node_type,
            control_state: String::new(),
            affinity: n.affinity,
        })
        .collect();
    nodes.sort_by_key(|n| n.id);
    let mut edges: Vec<GeoEdge> = f
        .edges
        .into_iter()
        .map(|e| GeoEdge {
            id: e.id,
            source: e.source,
            target: e.target,
            length_m: e.len as f64,
            road_type: None,
            one_way: false,
            fire_cost_forward: e.fcf,
            fire_cost_reverse: e.fcr,
            water_cost_forward: e.wcf,
            water_cost_reverse: e.wcr,
        })
        .collect();
    edges.sort_by_key(|e| e.id);
    Ok(crate::RegionData {
        graph: RegionGraph {
            region: None,
            nodes,
            edges,
        },
        region,
        lines,
        meta: Some(meta),
        places: Default::default(),
        finds: Default::default(),
        blocks: f
            .blocks
            .into_iter()
            .map(|b| crate::area::Block {
                id: b.id,
                area: b.area,
                nodes: b.nodes,
                rings: Vec::new(),
            })
            .collect(),
        tiles: None,
        tide: None,
    })
}

/// `LINESTRING(lon lat, …)` as (lat, lon).
fn wkt_line(wkt: &str) -> Vec<(f64, f64)> {
    let inner = wkt
        .split_once('(')
        .and_then(|(_, r)| r.rsplit_once(')'))
        .map_or("", |(i, _)| i);
    inner
        .split(',')
        .filter_map(|p| {
            let mut it = p.split_whitespace().map(str::parse::<f64>);
            let lon = it.next()?.ok()?;
            let lat = it.next()?.ok()?;
            Some((lat, lon))
        })
        .collect()
}

/// Nodes and neighbours of a graph file, for the walkers.
struct Streets {
    at: Vec<(i64, f64, f64)>,
    adj: Vec<Vec<usize>>,
}

impl Streets {
    fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let f: GraphFile = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        let at: Vec<_> = f.nodes.iter().map(|n| (n.id, n.lat, n.lon)).collect();
        let idx: std::collections::HashMap<i64, usize> =
            at.iter().enumerate().map(|(i, n)| (n.0, i)).collect();
        let mut adj = vec![Vec::new(); at.len()];
        for e in &f.edges {
            if let (Some(&a), Some(&b)) = (idx.get(&e.source), idx.get(&e.target)) {
                if a != b {
                    adj[a].push(b);
                    adj[b].push(a);
                }
            }
        }
        Ok(Self { at, adj })
    }

    fn dist(&self, a: usize, b: usize) -> f32 {
        let (_, la, oa) = self.at[a];
        let (_, lb, ob) = self.at[b];
        let k = la.to_radians().cos();
        (((ob - oa) * 111_320.0 * k).hypot((lb - la) * 110_540.0)) as f32
    }
}

const SPEED_MS: f32 = 1.3;

/// A player walking from junction to junction, claiming each one.
struct Walker {
    region: usize,
    conn: u64,
    rx: mpsc::UnboundedReceiver<String>,
    from: usize,
    to: usize,
    walked: f32,
    next_fix: Instant,
    asked: Option<Instant>,
    tile: TileKey,
    view: Vec<TileKey>,
    /// Counts as an open socket (the region updates at the connected rate).
    _guard: crate::regions::ConnGuard,
    /// First walker only: the sources as the app would hold them (node → side).
    seen: Option<HashMap<i64, String>>,
}

/// Apply a `sources`/`sourcesdiff` frame to `seen` (as `web/src/sources.ts`).
fn track(seen: &mut HashMap<i64, String>, m: &str) {
    let full = m.starts_with(r#"{"t":"sources""#);
    if !full && !m.starts_with(r#"{"t":"sourcesdiff""#) {
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(m) else {
        return;
    };
    if full {
        seen.clear();
    }
    for id in v["gone"].as_array().into_iter().flatten() {
        seen.remove(&id.as_i64().unwrap_or_default());
    }
    let rows = if full { &v["items"] } else { &v["set"] };
    for r in rows.as_array().into_iter().flatten() {
        if let (Some(id), Some(f)) = (r[0].as_i64(), r[1].as_str()) {
            seen.insert(id, f.to_string());
        }
    }
}

/// The 3 × 3 tiles around `t`.
fn around(t: TileKey) -> Vec<TileKey> {
    (0..9).map(|i| (t.0 + i % 3 - 1, t.1 + i / 3 - 1)).collect()
}

fn next_hop(s: &Streets, from: usize, came: usize) -> usize {
    let n: Vec<usize> = s.adj[from].iter().copied().filter(|&j| j != came).collect();
    match (n.is_empty(), s.adj[from].is_empty()) {
        (false, _) => n[rand::random_range(0..n.len())],
        // Dead end: back where we came from (or stay on an isolated junction).
        (true, false) => came,
        (true, true) => from,
    }
}

/// CPU seconds so far from a `stat` file (USER_HZ 100).
fn cpu_of(path: &str) -> f64 {
    let stat = std::fs::read_to_string(path).unwrap_or_default();
    let rest = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<f64> = rest
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    // After the name: state is field 3; utime/stime are 14/15 → 11/12 here.
    f.get(10)
        .zip(f.get(11))
        .map_or(0.0, |(u, s)| (u + s) / 100.0)
}

/// CPU seconds of the regions: the process minus this (driving) thread.
fn cpu_s() -> f64 {
    cpu_of("/proc/self/stat") - cpu_of("/proc/thread-self/stat")
}

fn threads() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("Threads:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Driver loop period: walkers move and answers are read this often.
const POLL: Duration = Duration::from_millis(5);

/// CPU share of one core over `secs` (without this thread), while `during`
/// runs every `POLL`.
fn measure(secs: u64, mut during: impl FnMut(bool)) -> f64 {
    let (t0, c0) = (Instant::now(), cpu_s());
    while t0.elapsed() < Duration::from_secs(secs) {
        during(true);
        std::thread::sleep(POLL);
    }
    (cpu_s() - c0) / t0.elapsed().as_secs_f64()
}

pub fn run(a: &BenchArgs) -> Result<(), Box<dyn std::error::Error>> {
    let streets: Vec<Streets> = a
        .graph
        .iter()
        .map(|p| Streets::load(p))
        .collect::<Result<_, _>>()?;
    let host = Host::new(HostConfig {
        db: None,
        shard: "bench".into(),
        shard_urls: Default::default(),
        default_region: None,
        preload: Vec::new(),
        idle: Duration::from_secs(86_400),
        max_loaded: a.regions,
        max_junctions: 0,
        cell: "main".into(),
        demo_day: None,
    });
    let base_threads = threads();
    println!(
        "bench: {} regions from {} graph file(s), {} active with {} players each",
        a.regions,
        a.graph.len(),
        a.active.min(a.regions),
        a.players
    );

    // Load, as `regions::Host::launch` does, from the files.
    let t0 = Instant::now();
    let regions: Vec<Arc<crate::regions::Region>> = (0..a.regions)
        .map(|i| host.launch_file(&format!("bench-{i}"), a.graph[i % a.graph.len()].clone()))
        .collect();
    while !regions.iter().all(|r| r.ready()) {
        if t0.elapsed() > Duration::from_secs(600) {
            return Err("regions not loaded after 10 min".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let load_s = t0.elapsed().as_secs_f64();
    crate::trim_memory("bench: all regions loaded");
    let junctions: usize = (0..a.regions)
        .map(|i| streets[i % streets.len()].at.len())
        .sum();
    println!(
        "load: {load_s:.1} s, {junctions} junctions, {} threads (+{} for regions), RSS {} MB",
        threads(),
        threads().saturating_sub(base_threads),
        crate::rss_mb()
    );

    // At rest.
    std::thread::sleep(Duration::from_secs(a.warmup));
    let rest = measure(a.secs, |_| {});
    println!(
        "rest: {:.1} % of a core in all, {:.3} % per region, RSS {} MB",
        rest * 100.0,
        rest * 100.0 / a.regions as f64,
        crate::rss_mb()
    );

    if a.players > 0 && a.active > 0 {
        let mut walkers = Vec::new();
        let mut conn = 1_000_000u64;
        for r in 0..a.active.min(a.regions) {
            let s = &streets[r % streets.len()];
            for _ in 0..a.players {
                conn += 1;
                let (out, rx) = mpsc::unbounded_channel();
                regions[r].tx.send(FromWeb::Hello {
                    conn,
                    token: None,
                    global: None,
                    bot: false,
                    tiles: true,
                    pow_ok: true,
                    watch: false,
                    invite: Default::default(),
                    out,
                })?;
                let from = rand::random_range(0..s.at.len());
                walkers.push(Walker {
                    region: r,
                    conn,
                    rx,
                    from,
                    to: next_hop(s, from, from),
                    walked: 0.0,
                    next_fix: Instant::now(),
                    asked: None,
                    tile: (u32::MAX, u32::MAX),
                    view: Vec::new(),
                    _guard: crate::regions::ConnGuard::new(regions[r].clone()),
                    seen: walkers.is_empty().then(HashMap::new),
                });
            }
        }
        let mut lat_ms: Vec<f64> = Vec::new();
        let (mut claims, mut ok) = (0usize, 0usize);
        let mut down = 0usize;
        let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
        let mut kinds: std::collections::HashMap<String, usize> = Default::default();
        let mut last = Instant::now();
        let mut step = |measuring: bool| {
            let dt = last.elapsed().as_secs_f32();
            last = Instant::now();
            for w in &mut walkers {
                let s = &streets[w.region % streets.len()];
                let tx = &regions[w.region].tx;
                while let Ok(m) = w.rx.try_recv() {
                    if let Some(seen) = w.seen.as_mut() {
                        track(seen, &m);
                    }
                    if measuring {
                        down += m.len();
                        let t = m.get(6..).and_then(|r| r.split('"').next()).unwrap_or("?");
                        *kinds.entry(t.to_string()).or_default() += m.len();
                    }
                    if m.starts_with(r#"{"t":"claim""#) {
                        if let Some(t) = w.asked.take() {
                            if measuring {
                                lat_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                                claims += 1;
                                ok += m.contains(r#""ok":true"#) as usize;
                                let r = m
                                    .split_once(r#""reason":""#)
                                    .and_then(|(_, r)| r.split('"').next())
                                    .unwrap_or("ok");
                                *reasons.entry(r.to_string()).or_default() += 1;
                            }
                        }
                    }
                }
                let len = s.dist(w.from, w.to).max(0.1);
                w.walked += SPEED_MS * dt;
                if w.walked >= len {
                    if w.asked.is_none() {
                        w.asked = Some(Instant::now());
                        let _ = tx.send(FromWeb::Claim {
                            conn: w.conn,
                            node: s.at[w.to].0,
                            via: None,
                        });
                    }
                    let came = w.from;
                    w.from = w.to;
                    w.to = next_hop(s, w.from, came);
                    w.walked = 0.0;
                }
                if Instant::now() >= w.next_fix {
                    w.next_fix += Duration::from_secs(1);
                    let k = (w.walked / s.dist(w.from, w.to).max(0.1)).min(1.0) as f64;
                    let (_, la, oa) = s.at[w.from];
                    let (_, lb, ob) = s.at[w.to];
                    // GPS-like noise, ±3 m.
                    let n = || (rand::random::<f64>() - 0.5) * 6.0 / 111_000.0;
                    let (lat, lon) = (la + (lb - la) * k + n(), oa + (ob - oa) * k + n());
                    let _ = tx.send(FromWeb::Pos {
                        conn: w.conn,
                        lat,
                        lon,
                        speed: Some(SPEED_MS),
                        acc: Some(8.0),
                    });
                    let t = tile_of(lat, lon);
                    if t != w.tile {
                        w.tile = t;
                        w.view = around(t);
                        // The app forgets the sources of tiles it lets go.
                        if let (Some(seen), Some(set)) = (
                            w.seen.as_mut(),
                            regions[w.region].tiles.read().unwrap().clone(),
                        ) {
                            let v: HashSet<TileKey> = w.view.iter().copied().collect();
                            seen.retain(|id, _| set.tiles_of(*id).any(|k| v.contains(&k)));
                        }
                        let _ = tx.send(FromWeb::View {
                            conn: w.conn,
                            tiles: w.view.clone(),
                        });
                    }
                }
            }
        };
        let t1 = Instant::now();
        while t1.elapsed() < Duration::from_secs(a.warmup) {
            step(false);
            std::thread::sleep(POLL);
        }
        let busy = measure(a.secs, &mut step);
        println!(
            "play: {:.1} % of a core in all, RSS {} MB, {claims} claims ({ok} ok), \
             claim answer p50 {:.1} / p95 {:.1} / p99 {:.1} / max {:.1} ms",
            busy * 100.0,
            crate::rss_mb(),
            pct(&mut lat_ms, 0.5),
            pct(&mut lat_ms, 0.95),
            pct(&mut lat_ms, 0.99),
            pct(&mut lat_ms, 1.0),
        );
        let per = |b: usize| b as f64 / walkers.len() as f64 / a.secs as f64 / 1000.0;
        let mut by: Vec<_> = kinds.iter().collect();
        by.sort_by(|x, y| y.1.cmp(x.1));
        let by: Vec<String> = by
            .iter()
            .take(4)
            .map(|(k, v)| format!("{k} {:.1}", per(**v)))
            .collect();
        println!("down: {:.1} kB/s per player ({})", per(down), by.join(", "));
        println!("claims by answer: {reasons:?}");

        // Check: the first walker's sources, built from diffs only, against
        // a fresh connection with the same tiles.
        let w = &mut walkers[0];
        let (out, mut rx) = mpsc::unbounded_channel();
        let tx = &regions[w.region].tx;
        tx.send(FromWeb::Hello {
            conn: 999_999_999,
            token: None,
            global: None,
            bot: false,
            tiles: true,
            pow_ok: true,
            watch: false,
            invite: Default::default(),
            out,
        })?;
        tx.send(FromWeb::View {
            conn: 999_999_999,
            tiles: w.view.clone(),
        })?;
        let mut truth = HashMap::new();
        let t2 = Instant::now();
        while t2.elapsed() < Duration::from_millis(500) {
            while let Ok(m) = rx.try_recv() {
                track(&mut truth, &m);
            }
            if let Some(seen) = w.seen.as_mut() {
                while let Ok(m) = w.rx.try_recv() {
                    track(seen, &m);
                }
            }
            std::thread::sleep(POLL);
        }
        let seen = w.seen.take().unwrap_or_default();
        let missing = truth.keys().filter(|k| !seen.contains_key(k)).count();
        let extra = seen.keys().filter(|k| !truth.contains_key(k)).count();
        let side = truth
            .iter()
            .filter(|(k, f)| seen.get(k).is_some_and(|g| g != *f))
            .count();
        println!(
            "check: {} sources in view, from diffs: {missing} missing, {extra} extra, {side} other side",
            truth.len()
        );
    }

    for r in &regions {
        r.request_stop();
    }
    for r in &regions {
        r.done.wait();
    }
    Ok(())
}

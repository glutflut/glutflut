//! Demo dataset: one simulated day (default a past Weihnachtsmarkt Saturday)
//! driven through the live `Aggregator` and `Hotspots` with a simulated clock,
//! so the dashboard shows exactly what the server would have written. All rows
//! are `synthetic = true`; a rerun replaces the day. The game field (claims) is
//! not simulated: analytics doesn't depend on it.
//!
//! Simulated walkers on the street graph; arrivals follow a scenario.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::prelude::{Vec2, Vec3};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Deserialize;

use elements_shared::prelude::*;

use crate::analytics::{spawn_writer, Aggregator, Profile, Snapper, Tracked};
use crate::hotspot::Hotspots;
use crate::web::{CATEGORIES, RATE_COOLDOWN_S, RATE_RADIUS_M};

#[derive(clap::Args, Debug, Clone)]
pub struct DemoArgs {
    /// UTC day `YYYY-MM-DD` (default: `ELEMENTS_DEMO_DAY`).
    #[arg(long)]
    pub day: Option<String>,
    /// Simulated players over the day.
    #[arg(long, default_value_t = 1000)]
    pub players: u32,
    /// Chance to walk to an active hotspot within 300 m.
    #[arg(long, default_value_t = 0.5)]
    pub follow: f64,
    #[arg(long, default_value_t = 7)]
    pub seed: u64,
    /// Scenario TOML (hourly arrivals, attractors); default: Weihnachtsmarkt.
    #[arg(long)]
    pub scenario: Option<PathBuf>,
    /// Mean seconds between two ratings of one player.
    #[arg(long, default_value_t = 300.0)]
    pub rate_every: f64,
    /// Replace the day even if it holds real (non-synthetic) rows.
    #[arg(long)]
    pub force: bool,
    /// Simulation series for the hotspot experiment: write nothing to the
    /// database, append the counts per site and slot to this JSON-lines file.
    #[arg(long)]
    pub visits: Option<PathBuf>,
    /// With `--visits`: days to simulate per setting (seeds `seed..seed+runs`).
    #[arg(long, default_value_t = 1)]
    pub runs: u64,
    /// With `--visits`: comma-separated follow chances (default: `--follow`).
    #[arg(long)]
    pub follows: Option<String>,
    /// With `--visits`: comma-separated player counts (default: `--players`).
    #[arg(long, name = "players-list")]
    pub players_list: Option<String>,
}

const SCENARIO: &str = include_str!("../demo/weihnachtsmarkt.toml");
/// Bots see hotspots within this distance.
const SEE_HOTSPOT_M: f32 = 300.0;
const NEAR_ATTRACTOR_M: f32 = 150.0;

#[derive(Deserialize)]
struct Scenario {
    /// Relative arrivals per local hour (the region's time zone).
    hourly: Vec<f64>,
    session_min_mean: f64,
    walk_speed: f32,
    pause_per_s: f64,
    pause_s: (f64, f64),
    attractors: Vec<Attractor>,
}

#[derive(Deserialize)]
struct Attractor {
    lat: f64,
    lon: f64,
    weight: f64,
}

/// Walkable street graph in ENU meters (x east, y = z south).
struct Graph {
    pos: Vec<Vec2>,
    /// node -> (to, polyline from -> to, length)
    adj: Vec<Vec<(usize, Vec<Vec2>, f32)>>,
}

impl Graph {
    fn path(&self, from: usize, to: usize) -> VecDeque<usize> {
        let n = self.pos.len();
        let mut d = vec![f32::INFINITY; n];
        let mut prev = vec![usize::MAX; n];
        let mut heap = BinaryHeap::new();
        d[from] = 0.0;
        heap.push(Reverse(((0.0f32).to_bits(), from)));
        while let Some(Reverse((db, u))) = heap.pop() {
            if u == to {
                break;
            }
            if f32::from_bits(db) > d[u] {
                continue;
            }
            for (v, _, len) in &self.adj[u] {
                let nd = d[u] + len;
                if nd < d[*v] {
                    d[*v] = nd;
                    prev[*v] = u;
                    // Non-negative f32 bits sort like the values.
                    heap.push(Reverse((nd.to_bits(), *v)));
                }
            }
        }
        let mut out = VecDeque::new();
        if from == to || prev[to] == usize::MAX {
            return out;
        }
        let mut x = to;
        while x != from {
            out.push_front(x);
            x = prev[x];
        }
        out
    }

    fn near(&self, c: Vec2, r: f32) -> Vec<usize> {
        (0..self.pos.len())
            .filter(|&i| self.pos[i].distance(c) < r)
            .collect()
    }
}

struct Walker {
    t: Tracked,
    home: Vec2,
    node: usize,
    route: VecDeque<usize>,
    /// (adjacency index at `node`, meters done)
    seg: Option<(usize, f32)>,
    pos: Vec2,
    pause: f64,
    following: bool,
    ends: f64,
    next_rate: f64,
}

/// Assumed player mix, from token bits so the rng stream stays as before:
/// young and local-heavy, a fifth without an answer.
fn demo_profile(bits: u128) -> Profile {
    let (a, l) = ((bits % 100) as u32, ((bits >> 8) % 100) as u32);
    let age = match a {
        0..20 => None,
        20..70 => Some("u30"),
        70..95 => Some("30_59"),
        _ => Some("60p"),
    };
    let local = match l {
        0..20 => None,
        20..70 => Some(true),
        _ => Some(false),
    };
    Profile::parse(age, local)
}

fn exp(rng: &mut StdRng, mean: f64) -> f64 {
    -mean * (1.0 - rng.random::<f64>()).ln()
}

fn pick_weighted(rng: &mut StdRng, w: &[f64]) -> usize {
    let mut r = rng.random::<f64>() * w.iter().sum::<f64>();
    for (i, x) in w.iter().enumerate() {
        r -= x;
        if r <= 0.0 {
            return i;
        }
    }
    w.len() - 1
}

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn parse_day(s: &str) -> Result<i64, Box<dyn Error>> {
    crate::db::Period::day(s).ok_or("day must be YYYY-MM-DD")?;
    let p = |r: std::ops::Range<usize>| s[r].parse::<i64>();
    Ok(days_from_civil(p(0..4)?, p(5..7)?, p(8..10)?) * 86_400)
}

/// Rating category from place and local hour.
fn category(rng: &mut StdRng, near_attractor: bool, hour: i64) -> &'static str {
    let evening = hour >= 16;
    let dark = !(8..17).contains(&hour);
    // voll, ruhig, laut, dunkel, kalt, einladend
    let w: [f64; 6] = if near_attractor {
        [
            if evening { 5.0 } else { 2.0 },
            0.3,
            if evening { 2.5 } else { 1.2 },
            0.3,
            if dark { 1.5 } else { 0.8 },
            2.0,
        ]
    } else {
        [
            0.3,
            3.0,
            0.5,
            if dark { 3.0 } else { 0.3 },
            if dark { 1.5 } else { 0.8 },
            1.0,
        ]
    };
    CATEGORIES[pick_weighted(rng, &w)]
}

pub fn run(db: &str, slug: &str, day: &str, a: &DemoArgs) -> Result<(), Box<dyn Error>> {
    use bevy::log::tracing_subscriber;
    let _ = tracing_subscriber::fmt()
        .with_max_level(bevy::log::Level::WARN)
        .try_init();
    let t_wall = Instant::now();
    let day_start = parse_day(day)?;
    let day_end = day_start + 86_400;
    let sc: Scenario = match &a.scenario {
        Some(p) => toml::from_str(&std::fs::read_to_string(p)?)?,
        None => toml::from_str(SCENARIO)?,
    };
    if sc.hourly.len() != 24 {
        return Err("scenario: hourly needs 24 values".into());
    }

    // World, as the server loads it.
    let graph = crate::db::load_region(db, slug)?;
    let region = graph
        .region
        .clone()
        .ok_or("no active region with that slug")?;
    let enu = |lat: f64, lon: f64| {
        let p = project_enu(lat, lon, region.centroid_lat, region.centroid_lon);
        Vec2::new(p.x, p.z)
    };
    let lines: HashMap<i64, Vec<Vec2>> = crate::db::load_edge_lines(db, slug)?
        .into_iter()
        .map(|(id, pts)| (id, pts.into_iter().map(|(la, lo)| enu(la, lo)).collect()))
        .collect();
    // Zones as the region loads them: flows, and visitors at focus places.
    let zone_rows = crate::places::load(&mut crate::db::connect(db)?, slug)?.zones;
    let zones: Vec<_> = zone_rows
        .iter()
        .map(|z| (z.id.clone(), enu(z.lat, z.lon), z.radius_m))
        .collect();
    let focus = crate::places::for_focus(&zone_rows, &region);
    // Sorted: ties at shared nodes then snap the same way every run.
    let mut snap_lines: Vec<_> = lines.iter().map(|(&id, p)| (id, p.clone())).collect();
    snap_lines.sort_by_key(|l| l.0);

    let idx: HashMap<i64, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect();
    let mut g = Graph {
        pos: graph.nodes.iter().map(|n| enu(n.lat, n.lon)).collect(),
        adj: vec![Vec::new(); graph.nodes.len()],
    };
    for e in &graph.edges {
        let (Some(&s), Some(&t)) = (idx.get(&e.source), idx.get(&e.target)) else {
            continue;
        };
        let mut pts = lines
            .get(&e.id)
            .filter(|p| p.len() >= 2)
            .cloned()
            .unwrap_or_else(|| vec![g.pos[s], g.pos[t]]);
        if pts[0].distance(g.pos[s]) > pts[pts.len() - 1].distance(g.pos[s]) {
            pts.reverse();
        }
        let len = pts
            .windows(2)
            .map(|w| w[0].distance(w[1]))
            .sum::<f32>()
            .max(1.0);
        let back: Vec<Vec2> = pts.iter().rev().copied().collect();
        g.adj[s].push((t, pts, len));
        g.adj[t].push((s, back, len));
    }
    let attractors: Vec<Vec2> = sc.attractors.iter().map(|a| enu(a.lat, a.lon)).collect();
    let att_w: Vec<f64> = sc.attractors.iter().map(|a| a.weight).collect();
    let near_home: Vec<Vec<usize>> = attractors
        .iter()
        .map(|&c| {
            g.near(c, NEAR_ATTRACTOR_M)
                .into_iter()
                .filter(|&i| !g.adj[i].is_empty())
                .collect()
        })
        .collect();
    let near_dest: Vec<Vec<usize>> = attractors.iter().map(|&c| g.near(c, 120.0)).collect();
    if near_home.iter().any(|v| v.is_empty()) {
        return Err("an attractor has no street nodes within 150 m".into());
    }

    // Clear the day, then write through the live code path.
    let mut c = crate::db::connect(db)?;
    // Local time of the region at noon of the demo day (no DST change within a day).
    let tz_offset: i64 = c
        .query_one(
            "SELECT extract(epoch FROM (to_timestamp($2) AT TIME ZONE coalesce( \
               (SELECT timezone FROM geo.regions WHERE slug = $1), 'Europe/Berlin')) \
               - (to_timestamp($2) AT TIME ZONE 'UTC'))::bigint",
            &[&slug, &((day_start + 43_200) as f64)],
        )?
        .get(0);
    let (from, to) = (day_start as f64, day_end as f64);
    // One simulated day through the live code path (writer or collector).
    let simulate = |a: &DemoArgs,
                    tx: crate::analytics::AnalyticsTx|
     -> Result<(u32, usize, u32), Box<dyn Error>> {
        let mut agg = Aggregator::new(Snapper::new(&snap_lines), zones.clone(), tx.clone())
            .with_hexes(region.centroid_lat, region.centroid_lon);
        agg.set_focus(focus.clone());
        let mut hs = Hotspots::new(&graph.nodes, &region, &[], tx.clone());
        let hot_m = elements_shared::rules::Rules::default().hotspot_radius_m;
        hs.seed(a.seed);
        let mut rng = StdRng::seed_from_u64(a.seed);
        let hourly_sum: f64 = sc.hourly.iter().sum::<f64>().max(1e-9);
        let mut walkers: Vec<Walker> = Vec::new();
        let (mut arrived, mut peak, mut ratings) = (0u32, 0usize, 0u32);

        for s in day_start..day_end {
            let now = s as f64;
            let hour = (s + tz_offset).div_euclid(3600).rem_euclid(24);

            // Arrivals (Poisson per second).
            let rate = a.players as f64 * sc.hourly[hour as usize] / hourly_sum / 3600.0;
            if rng.random::<f64>() < rate {
                let h = pick_weighted(&mut rng, &att_w);
                let node = near_home[h][rng.random_range(0..near_home[h].len())];
                let bits = rng.random::<u128>();
                let token = format!("{bits:032x}");
                walkers.push(Walker {
                    t: Tracked {
                        counted: true,
                        pseudo: agg.pseudonym(&token, now),
                        synthetic: true,
                        last_rate: 0.0,
                        profile: demo_profile(bits),
                    },
                    home: attractors[h],
                    node,
                    route: VecDeque::new(),
                    seg: None,
                    pos: g.pos[node],
                    pause: 0.0,
                    following: false,
                    ends: now + exp(&mut rng, sc.session_min_mean * 60.0).max(120.0),
                    next_rate: now + exp(&mut rng, a.rate_every).max(RATE_COOLDOWN_S),
                });
                arrived += 1;
            }
            walkers.retain(|w| {
                let stay = w.ends > now;
                if !stay {
                    agg.forget(w.t.pseudo);
                }
                stay
            });
            peak = peak.max(walkers.len());

            let active: Vec<usize> = hs
                .list(now)
                .iter()
                .filter_map(|h| idx.get(&h.node).copied())
                .collect();
            for w in walkers.iter_mut() {
                step(w, &g, &sc, a, &active, &near_dest, &att_w, &mut rng);
                let p3 = Vec3::new(w.pos.x, 0.0, w.pos.y);
                agg.observe(&w.t, p3, now, hs.bonus(p3, hot_m));
                hs.observe(&w.t, p3);
                if now >= w.next_rate {
                    w.next_rate = now + exp(&mut rng, a.rate_every).max(RATE_COOLDOWN_S);
                    if g.pos[w.node].distance(w.pos) <= RATE_RADIUS_M {
                        let near = attractors
                            .iter()
                            .any(|&c| c.distance(w.pos) < NEAR_ATTRACTOR_M);
                        let cat = category(&mut rng, near, hour);
                        agg.rate(graph.nodes[w.node].id, cat, w.t.pseudo, true, now);
                        w.t.last_rate = now;
                        ratings += 1;
                    }
                }
            }
            hs.step(
                now,
                walkers
                    .iter()
                    .map(|w| (Vec3::new(w.pos.x, 0.0, w.pos.y), w.t)),
                &|_| true,
            );
            if s % 900 == 0 {
                agg.flush(now, false);
            }
        }
        agg.flush(day_end as f64, true);
        hs.finish(&|_| true);
        if !tx.barrier(Duration::from_secs(900)) {
            return Err("writer did not finish in time".into());
        }
        Ok((arrived, peak, ratings))
    };

    if let Some(path) = &a.visits {
        return series(slug, a, path, &simulate);
    }

    let real: i64 = c
        .query_one(
            "SELECT (SELECT count(*) FROM analytics.dwell WHERE region = $1 AND NOT synthetic \
               AND window_start >= to_timestamp($2) AND window_start < to_timestamp($3)) \
             + (SELECT count(*) FROM analytics.daily WHERE region = $1 AND NOT synthetic \
               AND day = (to_timestamp($2) AT TIME ZONE 'UTC')::date)",
            &[&slug, &from, &to],
        )?
        .get(0);
    if real > 0 && !a.force {
        return Err(format!("{day} holds {real} real rows; pick another day or --force").into());
    }
    let deleted = clear_day(&mut c, slug, from, to)?;
    println!("demo {day} ({slug}): cleared {deleted} old rows");

    let (arrived, peak, ratings) =
        simulate(a, spawn_writer(Some(db.to_string()), slug.to_string()))?;
    let sim_s = t_wall.elapsed().as_secs_f64();

    // Summary from what actually landed in the database.
    let q = |c: &mut postgres::Client, sql: &str| -> Result<i64, postgres::Error> {
        Ok(c.query_one(sql, &[&slug, &from, &to])?.get(0))
    };
    let ts = "region = $1 AND synthetic AND {c} >= to_timestamp($2) AND {c} < to_timestamp($3)";
    let dt = "region = $1 AND synthetic AND day = (to_timestamp($2) AT TIME ZONE 'UTC')::date AND $3::float8 > 0";
    let sup = c.query_one(
        &format!(
            "SELECT coalesce(sum(kept), 0)::bigint, coalesce(sum(dropped), 0)::bigint \
             FROM analytics.suppressed WHERE {}",
            ts.replace("{c}", "window_start")
        ),
        &[&slug, &from, &to],
    )?;
    let (kept, dropped): (i64, i64) = (sup.get(0), sup.get(1));
    let players = q(
        &mut c,
        &format!("SELECT coalesce(max(players), 0)::bigint FROM analytics.daily WHERE {dt}"),
    )?;
    println!(
        "players: {arrived} arrived (daily row: {players}), peak {peak} at once; {ratings} ratings"
    );
    println!(
        "windows (edge x 15 min): kept {kept}, dropped {dropped} below k = {} ({:.0} % dropped)",
        crate::analytics::k_min(),
        100.0 * dropped as f64 / (kept + dropped).max(1) as f64
    );
    for (table, col) in [
        ("dwell", "window_start"),
        ("flow", "hour_start"),
        ("site_visits", "slot_start"),
        ("suppressed", "window_start"),
        ("hex_dwell", "hour_start"),
        ("hex_suppressed", "hour_start"),
        ("place_visits", "start"),
    ] {
        let n = q(
            &mut c,
            &format!(
                "SELECT count(*) FROM analytics.{table} WHERE {}",
                ts.replace("{c}", col)
            ),
        )?;
        println!("  analytics.{table}: {n} rows");
    }
    for table in ["perception", "daily", "daily_profile", "flow_day"] {
        let n = q(
            &mut c,
            &format!("SELECT count(*) FROM analytics.{table} WHERE {dt}"),
        )?;
        println!("  analytics.{table}: {n} rows");
    }
    let n = q(
        &mut c,
        "SELECT count(*) FROM analytics.interventions WHERE region = $1 \
         AND starts_at >= to_timestamp($2) AND starts_at < to_timestamp($3)",
    )?;
    println!("  analytics.interventions: {n} rows");
    println!("done in {sim_s:.0} s");
    Ok(())
}

/// The hotspot experiment over many simulated days: every combination of
/// player count and follow chance, `runs` days each (one seed per day). One
/// JSON line per site and slot, with the setting; nothing goes to the database.
#[allow(clippy::type_complexity)]
fn series(
    slug: &str,
    a: &DemoArgs,
    path: &std::path::Path,
    simulate: &dyn Fn(
        &DemoArgs,
        crate::analytics::AnalyticsTx,
    ) -> Result<(u32, usize, u32), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    use std::io::Write;
    let list = |v: &Option<String>, d: f64| -> Result<Vec<f64>, Box<dyn Error>> {
        match v {
            None => Ok(vec![d]),
            Some(s) => Ok(s
                .split(',')
                .map(|x| x.trim().parse())
                .collect::<Result<_, _>>()?),
        }
    };
    let follows = list(&a.follows, a.follow)?;
    let players = list(&a.players_list, a.players as f64)?;
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for &p in &players {
        for &f in &follows {
            for run in 0..a.runs {
                let mut b = a.clone();
                b.players = p as u32;
                b.follow = f;
                b.seed = a.seed + run;
                let (tx, rows) = crate::analytics::spawn_visit_collector();
                let t = Instant::now();
                let (arrived, peak, _) = simulate(&b, tx)?;
                let rows = rows.lock().unwrap();
                for r in rows.iter() {
                    let line = serde_json::json!({
                        "region": slug, "players": b.players, "follow": f, "seed": b.seed,
                        "site": r.site, "slot": r.slot, "active": r.active,
                        "nearby": r.nearby, "reached": r.reached, "reached_nearby": r.reached_nearby,
                    });
                    writeln!(out, "{line}")?;
                }
                let sum = |act: bool, k: fn(&crate::analytics::VisitCount) -> i32| -> i64 {
                    rows.iter()
                        .filter(|r| r.active == act)
                        .map(|r| k(r) as i64)
                        .sum()
                };
                let rate = |act: bool| {
                    sum(act, |r| r.reached_nearby) as f64 / sum(act, |r| r.nearby).max(1) as f64
                };
                println!(
                    "{slug} players={} follow={f} seed={}: arrived {arrived}, peak {peak}; follow rate active {:.1} % (n={}), control {:.1} % (n={}); {:.1} s",
                    b.players,
                    b.seed,
                    100.0 * rate(true),
                    sum(true, |r| r.nearby),
                    100.0 * rate(false),
                    sum(false, |r| r.nearby),
                    t.elapsed().as_secs_f64()
                );
            }
        }
    }
    Ok(())
}

/// Delete the day's synthetic rows (and its interventions).
fn clear_day(
    c: &mut postgres::Client,
    slug: &str,
    from: f64,
    to: f64,
) -> Result<u64, Box<dyn Error>> {
    let mut tx = c.transaction()?;
    let mut n = 0;
    for (table, col) in [
        ("dwell", "window_start"),
        ("flow", "hour_start"),
        ("site_visits", "slot_start"),
        ("suppressed", "window_start"),
        ("hex_dwell", "hour_start"),
        ("hex_suppressed", "hour_start"),
        ("place_visits", "start"),
    ] {
        n += tx.execute(
            &format!(
                "DELETE FROM analytics.{table} WHERE region = $1 AND synthetic \
                 AND {col} >= to_timestamp($2) AND {col} < to_timestamp($3)"
            ),
            &[&slug, &from, &to],
        )?;
    }
    for table in ["perception", "daily", "daily_profile", "flow_day"] {
        n += tx.execute(
            &format!(
                "DELETE FROM analytics.{table} WHERE region = $1 AND synthetic \
                 AND day = (to_timestamp($2) AT TIME ZONE 'UTC')::date AND $3::float8 > 0"
            ),
            &[&slug, &from, &to],
        )?;
    }
    n += tx.execute(
        "DELETE FROM analytics.interventions WHERE region = $1 \
         AND starts_at >= to_timestamp($2) AND starts_at < to_timestamp($3)",
        &[&slug, &from, &to],
    )?;
    tx.commit()?;
    Ok(n)
}

/// One simulated second of walking.
fn step(
    w: &mut Walker,
    g: &Graph,
    sc: &Scenario,
    a: &DemoArgs,
    hotspots: &[usize],
    near_dest: &[Vec<usize>],
    att_w: &[f64],
    rng: &mut StdRng,
) {
    if w.seg.is_none() {
        next_edge(w, g, a, hotspots, near_dest, att_w, rng);
    }
    let Some((e, done)) = w.seg.as_mut() else {
        return;
    };
    // Pause now and then (stall, chat).
    if w.pause > 0.0 {
        w.pause -= 1.0;
    } else {
        *done += sc.walk_speed;
        if !w.following && rng.random::<f64>() < sc.pause_per_s {
            w.pause = rng.random_range(sc.pause_s.0..=sc.pause_s.1.max(sc.pause_s.0));
        }
    }
    let (to, pts, len) = &g.adj[w.node][*e];
    if *done >= *len {
        w.node = *to;
        w.pos = g.pos[*to];
        w.seg = None;
        return;
    }
    let mut left = *done;
    for s in pts.windows(2) {
        let l = s[0].distance(s[1]);
        if left <= l {
            w.pos = s[0].lerp(s[1], left / l.max(1e-3));
            return;
        }
        left -= l;
    }
}

fn next_edge(
    w: &mut Walker,
    g: &Graph,
    a: &DemoArgs,
    hotspots: &[usize],
    near_dest: &[Vec<usize>],
    att_w: &[f64],
    rng: &mut StdRng,
) {
    if w.route.is_empty() {
        let here = g.pos[w.node];
        let hs: Vec<usize> = hotspots
            .iter()
            .copied()
            .filter(|&h| g.pos[h].distance(here) < SEE_HOTSPOT_M)
            .collect();
        w.following = false;
        if !hs.is_empty() && rng.random::<f64>() < a.follow {
            w.route = g.path(w.node, hs[rng.random_range(0..hs.len())]);
            w.following = !w.route.is_empty();
        } else if rng.random::<f64>() < 0.15 {
            let near = &near_dest[pick_weighted(rng, att_w)];
            if !near.is_empty() {
                w.route = g.path(w.node, near[rng.random_range(0..near.len())]);
            }
        }
    }
    let opts = &g.adj[w.node];
    if opts.is_empty() {
        w.seg = None;
        return;
    }
    let e = w
        .route
        .front()
        .and_then(|&n| opts.iter().position(|(to, ..)| *to == n));
    let e = match e {
        Some(e) => {
            w.route.pop_front();
            e
        }
        None => {
            w.route.clear();
            // Random step, biased towards home.
            let wts: Vec<f64> = opts
                .iter()
                .map(|(to, ..)| 1.0 / (1.0 + g.pos[*to].distance(w.home) as f64 / 150.0))
                .collect();
            pick_weighted(rng, &wts)
        }
    };
    w.seg = Some((e, 0.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(parse_day("2025-12-06").unwrap(), 1_764_979_200);
    }
}

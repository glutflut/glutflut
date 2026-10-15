//! Privacy-preserving aggregation. Positions are snapped to street segments and
//! counted in 15-minute windows in memory; only windows with >= k distinct
//! persons are written. Pseudonyms (hash of session token + daily salt) exist in
//! memory only. A writer thread owns the DB connection.
//!
//! Only established players count for real (non-synthetic) data: those who
//! played before (on an earlier day) or for [`establish_s`] today. Windows are
//! written that much later, so a newcomer's first minutes still count once they
//! qualify; drive-by sessions and throwaway tokens drop out.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

/// Aggregation window in seconds (`ELEMENTS_WINDOW_S`, default 900; lower only for tests).
pub fn window_s() -> i64 {
    static W: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *W.get_or_init(|| {
        std::env::var("ELEMENTS_WINDOW_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(900)
    })
}
/// Playing time after which a new player counts (`ELEMENTS_ESTABLISH_S`,
/// default 600; 0 = everyone at once).
pub fn establish_s() -> f64 {
    static E: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *E.get_or_init(|| {
        std::env::var("ELEMENTS_ESTABLISH_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(600.0)
    })
}

/// Max snapping distance to a street segment.
const SNAP_M: f32 = 30.0;
/// Gaps longer than this don't count as dwell.
const MAX_GAP_S: f64 = 10.0;
/// A position belongs to the nearest flow zone within this distance: a
/// measurement for the city, kept when `[progression] zone_r_m` is tuned.
pub const ZONE_R_M: f32 = 400.0;

pub fn now_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Minimum distinct persons per written window (`ELEMENTS_K`, default 5).
pub fn k_min() -> usize {
    std::env::var("ELEMENTS_K")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
}

/// One aggregate to persist.
pub enum Row {
    Dwell {
        edge: i64,
        window: i64,
        /// Length: one window, or an hour or day of pooled leftovers.
        span: i32,
        synthetic: bool,
        persons: i32,
        dwell_s: i32,
        hotspot_dwell_s: i32,
    },
    Perception {
        node: i64,
        day: i64,
        /// UTC hour of day.
        hour: i16,
        category: &'static str,
        synthetic: bool,
        add: i32,
    },
    Intervention {
        node: i64,
        starts: f64,
        ends: f64,
        source: &'static str,
    },
    Visit {
        site: i64,
        slot: f64,
        synthetic: bool,
        active: bool,
        nearby: i32,
        reached: i32,
        reached_nearby: i32,
    },
    Flow {
        from: String,
        to: String,
        hour: i64,
        synthetic: bool,
        persons: i32,
    },
    Daily {
        day: i64,
        synthetic: bool,
        players: i32,
        /// Left without counting (played too briefly).
        newcomers: i32,
        /// Flagged by the plausibility check (shadow ban).
        flagged: i32,
    },
    /// Players per day by self-reported profile.
    DailyProfile {
        day: i64,
        synthetic: bool,
        age: &'static str,
        local: &'static str,
        players: i32,
    },
    Suppressed {
        window: i64,
        synthetic: bool,
        kept: i32,
        dropped: i32,
        /// Distinct persons on the streets of the region in the window, if >= k.
        persons: Option<i32>,
    },
    /// Dwell per H3 cell and hour (coarser than edges, so more passes k).
    HexDwell {
        res: u8,
        cell: u64,
        hour: i64,
        /// 3600, or 86400 for a day of pooled leftovers.
        span: i32,
        synthetic: bool,
        persons: i32,
        dwell_s: i32,
        hotspot_dwell_s: i32,
    },
    HexSuppressed {
        res: u8,
        hour: i64,
        synthetic: bool,
        kept: i32,
        dropped: i32,
    },
    /// Persons between two zones on a local day (day number), pooled from hours below k.
    FlowDay {
        from: String,
        to: String,
        day: i64,
        synthetic: bool,
        persons: i32,
    },
    /// Distinct persons at a focus place in an hour (or a day of leftovers).
    PlaceVisit {
        zone: String,
        start: i64,
        span: i32,
        synthetic: bool,
        persons: i32,
        dwell_s: i32,
    },
    /// Held area per side over an hour (`area::AreaLog`): mean, mean with a
    /// player on the border, peak.
    AreaHour {
        hour: i64,
        minutes: i16,
        ha: [f32; 2],
        player_ha: [f32; 2],
        max: [f32; 2],
    },
    /// Minutes each block was held by (fire, water) on a UTC day, to add.
    BlockMinutes {
        day: i64,
        blocks: Vec<(i32, i32, i32)>,
    },
}

/// H3 resolutions aggregated per hour; edge ~460 m, ~170 m, ~65 m, ~25 m.
/// The finest level is for the dashboard only, never exported.
pub const HEX_RES: [u8; 4] = [8, 9, 10, 11];

enum Msg {
    Rows(Vec<Row>),
    /// Reply once everything before it is written.
    Barrier(Sender<()>),
}

/// To the region's analytics writer (a lane of `writers`), or a plain
/// channel (demo collector, tests).
#[derive(Resource, Clone)]
pub struct AnalyticsTx(Out);

#[derive(Clone)]
enum Out {
    Lane(crate::writers::LaneTx<Msg>),
    Chan(Sender<Msg>),
}

impl Out {
    fn send(&self, m: Msg) -> bool {
        match self {
            Out::Lane(t) => t.send(m).is_ok(),
            Out::Chan(t) => t.send(m).is_ok(),
        }
    }
}

impl AnalyticsTx {
    pub fn send(&self, rows: Vec<Row>) {
        if !rows.is_empty() {
            self.0.send(Msg::Rows(rows));
        }
    }

    /// Block until pending rows are written (or the timeout passes).
    pub fn barrier(&self, timeout: std::time::Duration) -> bool {
        let (ack, done) = crossbeam_channel::bounded(1);
        self.0.send(Msg::Barrier(ack)) && done.recv_timeout(timeout).is_ok()
    }
}

/// One site and slot of the hotspot experiment (see `hotspot.rs`).
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct VisitCount {
    pub site: i64,
    pub slot: f64,
    pub active: bool,
    pub nearby: i32,
    pub reached: i32,
    pub reached_nearby: i32,
}

/// Instead of the writer: keep only the experiment rows in memory, write
/// nothing (simulation series, `elements-server demo --visits`).
/// Tests: rows go nowhere.
#[cfg(test)]
pub fn test_tx() -> AnalyticsTx {
    AnalyticsTx(Out::Chan(crossbeam_channel::unbounded().0))
}

pub fn spawn_visit_collector() -> (
    AnalyticsTx,
    std::sync::Arc<std::sync::Mutex<Vec<VisitCount>>>,
) {
    let (tx, rx) = crossbeam_channel::unbounded::<Msg>();
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let keep = out.clone();
    std::thread::Builder::new()
        .name("visit-collector".into())
        .spawn(move || {
            for msg in rx {
                match msg {
                    Msg::Rows(rows) => {
                        let mut v = keep.lock().unwrap();
                        for r in rows {
                            if let Row::Visit {
                                site,
                                slot,
                                active,
                                nearby,
                                reached,
                                reached_nearby,
                                ..
                            } = r
                            {
                                v.push(VisitCount {
                                    site,
                                    slot,
                                    active,
                                    nearby,
                                    reached,
                                    reached_nearby,
                                });
                            }
                        }
                    }
                    Msg::Barrier(ack) => {
                        let _ = ack.send(());
                    }
                }
            }
        })
        .expect("collector thread");
    (AnalyticsTx(Out::Chan(tx)), out)
}

/// The region's writer (a lane on the shared writer threads). Without a DB,
/// rows are dropped.
pub fn spawn_writer(db: Option<String>, region: String) -> AnalyticsTx {
    let key = format!("analytics {region}");
    AnalyticsTx(Out::Lane(crate::writers::lane(&key, |rx| Writer {
        db,
        region,
        rx,
    })))
}

struct Writer {
    db: Option<String>,
    region: String,
    rx: Receiver<Msg>,
}

impl crate::writers::Lane for Writer {
    fn run(&mut self, pg: &mut crate::writers::Db) -> bool {
        loop {
            let rows = match self.rx.try_recv() {
                Ok(Msg::Rows(rows)) => rows,
                Ok(Msg::Barrier(ack)) => {
                    let _ = ack.send(());
                    continue;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => return true,
                Err(crossbeam_channel::TryRecvError::Disconnected) => return false,
            };
            let db = self.db.as_deref();
            let Some(c) = pg.get(db) else { continue };
            let n = rows.len();
            let t = std::time::Instant::now();
            if let Err(e) = write_rows(c, &self.region, rows) {
                pg.failed(db, "analytics write", &e);
            } else {
                debug!(
                    rows = n,
                    ms = t.elapsed().as_millis() as u64,
                    region = %self.region,
                    "analytics written"
                );
            }
        }
    }
}

/// One statement per row type (arrays + `unnest`): a few round trips to the
/// database instead of one per row.
fn write_rows(
    c: &mut postgres::Client,
    region: &str,
    rows: Vec<Row>,
) -> Result<(), postgres::Error> {
    let (mut d_edge, mut d_win, mut d_syn, mut d_per, mut d_dw, mut d_hot, mut d_span) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    let (mut p_node, mut p_day, mut p_hour, mut p_cat, mut p_syn, mut p_add) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    let (mut v_site, mut v_slot, mut v_syn, mut v_act, mut v_near, mut v_reach, mut v_rn) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    let mut interventions = vec![];
    let (mut f_from, mut f_to, mut f_hour, mut f_syn, mut f_per) =
        (vec![], vec![], vec![], vec![], vec![]);
    let mut small = vec![];
    let (mut h_res, mut h_cell, mut h_hour, mut h_syn, mut h_per, mut h_dw, mut h_hot, mut h_span) = (
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    let mut hex_sup = vec![];
    for r in rows {
        match r {
            Row::Dwell {
                edge,
                window,
                span,
                synthetic,
                persons,
                dwell_s,
                hotspot_dwell_s,
            } => {
                d_span.push(span);
                d_edge.push(edge);
                d_win.push(window as f64);
                d_syn.push(synthetic);
                d_per.push(persons);
                d_dw.push(dwell_s);
                d_hot.push(hotspot_dwell_s);
            }
            Row::Perception {
                node,
                day,
                hour,
                category,
                synthetic,
                add,
            } => {
                p_node.push(node);
                p_day.push(day as f64);
                p_hour.push(hour);
                p_cat.push(category.to_string());
                p_syn.push(synthetic);
                p_add.push(add);
            }
            Row::Visit {
                site,
                slot,
                synthetic,
                active,
                nearby,
                reached,
                reached_nearby,
            } => {
                v_site.push(site);
                v_slot.push(slot);
                v_syn.push(synthetic);
                v_act.push(active);
                v_near.push(nearby);
                v_reach.push(reached);
                v_rn.push(reached_nearby);
            }
            Row::Intervention {
                node,
                starts,
                ends,
                source,
            } => interventions.push((node, starts, ends, source)),
            Row::Flow {
                from,
                to,
                hour,
                synthetic,
                persons,
            } => {
                f_from.push(from);
                f_to.push(to);
                f_hour.push(hour as f64);
                f_syn.push(synthetic);
                f_per.push(persons);
            }
            r @ (Row::Daily { .. }
            | Row::DailyProfile { .. }
            | Row::Suppressed { .. }
            | Row::AreaHour { .. }
            | Row::BlockMinutes { .. }
            | Row::PlaceVisit { .. }
            | Row::FlowDay { .. }) => small.push(r),
            Row::HexDwell {
                res,
                cell,
                hour,
                span,
                synthetic,
                persons,
                dwell_s,
                hotspot_dwell_s,
            } => {
                h_span.push(span);
                h_res.push(res as i16);
                h_cell.push(cell as i64);
                h_hour.push(hour as f64);
                h_syn.push(synthetic);
                h_per.push(persons);
                h_dw.push(dwell_s);
                h_hot.push(hotspot_dwell_s);
            }
            Row::HexSuppressed {
                res,
                hour,
                synthetic,
                kept,
                dropped,
            } => hex_sup.push((res as i16, hour as f64, synthetic, kept, dropped)),
        }
    }
    let mut tx = c.transaction()?;
    if !d_edge.is_empty() {
        tx.execute(
            "INSERT INTO analytics.dwell (region, edge_id, window_start, synthetic, persons, dwell_s, hotspot_dwell_s, span_s) \
             SELECT $1, e, to_timestamp(w), s, p, d, h, sp \
             FROM unnest($2::bigint[], $3::float8[], $4::bool[], $5::int[], $6::int[], $7::int[], $8::int[]) AS t(e, w, s, p, d, h, sp) \
             ON CONFLICT (region, edge_id, window_start, synthetic, span_s) \
             DO UPDATE SET persons = EXCLUDED.persons, dwell_s = EXCLUDED.dwell_s, \
                           hotspot_dwell_s = EXCLUDED.hotspot_dwell_s",
            &[&region, &d_edge, &d_win, &d_syn, &d_per, &d_dw, &d_hot, &d_span],
        )?;
    }
    if !p_node.is_empty() {
        tx.execute(
            "INSERT INTO analytics.perception (region, node_id, day, hour, category, synthetic, count) \
             SELECT $1, n, (to_timestamp(d) AT TIME ZONE 'UTC')::date, h, c, s, a \
             FROM unnest($2::bigint[], $3::float8[], $4::int2[], $5::text[], $6::bool[], $7::int[]) \
                  AS t(n, d, h, c, s, a) \
             ON CONFLICT (region, node_id, day, hour, category, synthetic) \
             DO UPDATE SET count = analytics.perception.count + EXCLUDED.count",
            &[&region, &p_node, &p_day, &p_hour, &p_cat, &p_syn, &p_add],
        )?;
    }
    if !v_site.is_empty() {
        tx.execute(
            "INSERT INTO analytics.site_visits \
               (region, site_node, slot_start, synthetic, active, nearby, reached, reached_nearby) \
             SELECT $1, n, to_timestamp(t), s, a, ne, r, rn \
             FROM unnest($2::bigint[], $3::float8[], $4::bool[], $5::bool[], $6::int[], $7::int[], $8::int[]) \
               AS x(n, t, s, a, ne, r, rn) \
             ON CONFLICT (region, site_node, slot_start, synthetic) \
             DO UPDATE SET active = EXCLUDED.active, nearby = EXCLUDED.nearby, reached = EXCLUDED.reached, \
                           reached_nearby = EXCLUDED.reached_nearby",
            &[&region, &v_site, &v_slot, &v_syn, &v_act, &v_near, &v_reach, &v_rn],
        )?;
    }
    if !f_from.is_empty() {
        tx.execute(
            "INSERT INTO analytics.flow (region, from_zone, to_zone, hour_start, synthetic, persons) \
             SELECT $1, f, t, to_timestamp(h), s, p \
             FROM unnest($2::text[], $3::text[], $4::float8[], $5::bool[], $6::int[]) AS x(f, t, h, s, p) \
             ON CONFLICT (region, from_zone, to_zone, hour_start, synthetic) DO UPDATE SET persons = EXCLUDED.persons",
            &[&region, &f_from, &f_to, &f_hour, &f_syn, &f_per],
        )?;
    }
    for r in small {
        match r {
            Row::Daily { day, synthetic, players, newcomers, flagged } => tx.execute(
                "INSERT INTO analytics.daily (region, day, synthetic, players, newcomers, flagged) \
                 VALUES ($1, DATE '1970-01-01' + $2::int, $3, $4, $5, $6) \
                 ON CONFLICT (region, day, synthetic) \
                 DO UPDATE SET players = GREATEST(analytics.daily.players, EXCLUDED.players), \
                 newcomers = GREATEST(analytics.daily.newcomers, EXCLUDED.newcomers), \
                 flagged = GREATEST(analytics.daily.flagged, EXCLUDED.flagged)",
                &[&region, &(day as i32), &synthetic, &players, &newcomers, &flagged],
            )?,
            Row::DailyProfile { day, synthetic, age, local, players } => tx.execute(
                "INSERT INTO analytics.daily_profile (region, day, synthetic, age, local, players) \
                 VALUES ($1, DATE '1970-01-01' + $2::int, $3, $4, $5, $6) \
                 ON CONFLICT (region, day, synthetic, age, local) \
                 DO UPDATE SET players = GREATEST(analytics.daily_profile.players, EXCLUDED.players)",
                &[&region, &(day as i32), &synthetic, &age, &local, &players],
            )?,
            Row::Suppressed { window, synthetic, kept, dropped, persons } => tx.execute(
                "INSERT INTO analytics.suppressed (region, window_start, synthetic, kept, dropped, persons) \
                 VALUES ($1, to_timestamp($2), $3, $4, $5, $6) \
                 ON CONFLICT (region, window_start, synthetic) \
                 DO UPDATE SET kept = EXCLUDED.kept, dropped = EXCLUDED.dropped, persons = EXCLUDED.persons",
                &[&region, &(window as f64), &synthetic, &kept, &dropped, &persons],
            )?,
            // A second part of the same hour (restart): weighted means, peaks.
            Row::AreaHour { hour, minutes, ha, player_ha, max } => tx.execute(
                "INSERT INTO analytics.area_hour AS a (region, hour_start, minutes, fire_ha, water_ha, \
                   fire_player_ha, water_player_ha, fire_max_ha, water_max_ha) \
                 VALUES ($1, to_timestamp($2), $3, $4, $5, $6, $7, $8, $9) \
                 ON CONFLICT (region, hour_start) DO UPDATE SET \
                   minutes = least(60, a.minutes + EXCLUDED.minutes), \
                   fire_ha = (a.fire_ha * a.minutes + EXCLUDED.fire_ha * EXCLUDED.minutes) / (a.minutes + EXCLUDED.minutes), \
                   water_ha = (a.water_ha * a.minutes + EXCLUDED.water_ha * EXCLUDED.minutes) / (a.minutes + EXCLUDED.minutes), \
                   fire_player_ha = (a.fire_player_ha * a.minutes + EXCLUDED.fire_player_ha * EXCLUDED.minutes) / (a.minutes + EXCLUDED.minutes), \
                   water_player_ha = (a.water_player_ha * a.minutes + EXCLUDED.water_player_ha * EXCLUDED.minutes) / (a.minutes + EXCLUDED.minutes), \
                   fire_max_ha = greatest(a.fire_max_ha, EXCLUDED.fire_max_ha), \
                   water_max_ha = greatest(a.water_max_ha, EXCLUDED.water_max_ha)",
                &[&region, &(hour as f64), &minutes, &ha[0], &ha[1], &player_ha[0], &player_ha[1], &max[0], &max[1]],
            )?,
            Row::FlowDay { from, to, day, synthetic, persons } => tx.execute(
                "INSERT INTO analytics.flow_day (region, from_zone, to_zone, day, synthetic, persons) \
                 VALUES ($1, $2, $3, DATE '1970-01-01' + $4::int, $5, $6) \
                 ON CONFLICT (region, from_zone, to_zone, day, synthetic) DO UPDATE SET persons = EXCLUDED.persons",
                &[&region, &from, &to, &(day as i32), &synthetic, &persons],
            )?,
            Row::PlaceVisit { zone, start, span, synthetic, persons, dwell_s } => tx.execute(
                "INSERT INTO analytics.place_visits (region, zone_id, start, span_s, synthetic, persons, dwell_s) \
                 VALUES ($1, $2, to_timestamp($3), $4, $5, $6, $7) \
                 ON CONFLICT (region, zone_id, start, span_s, synthetic) \
                 DO UPDATE SET persons = EXCLUDED.persons, dwell_s = EXCLUDED.dwell_s",
                &[&region, &zone, &(start as f64), &span, &synthetic, &persons, &dwell_s],
            )?,
            Row::BlockMinutes { day, blocks } => {
                let ids: Vec<i32> = blocks.iter().map(|b| b.0).collect();
                let f: Vec<i32> = blocks.iter().map(|b| b.1).collect();
                let w: Vec<i32> = blocks.iter().map(|b| b.2).collect();
                tx.execute(
                    "INSERT INTO analytics.block_day AS b (region, day, block_id, fire_min, water_min) \
                     SELECT $1, DATE '1970-01-01' + $2::int, i, f, w \
                     FROM unnest($3::int[], $4::int[], $5::int[]) AS x(i, f, w) \
                     ON CONFLICT (region, day, block_id) DO UPDATE SET \
                       fire_min = b.fire_min + EXCLUDED.fire_min, water_min = b.water_min + EXCLUDED.water_min",
                    &[&region, &(day as i32), &ids, &f, &w],
                )?
            }
            _ => 0,
        };
    }
    if !h_cell.is_empty() {
        tx.execute(
            "INSERT INTO analytics.hex_dwell (region, res, cell, hour_start, synthetic, persons, dwell_s, hotspot_dwell_s, span_s) \
             SELECT $1, r, c, to_timestamp(h), s, p, d, x, sp \
             FROM unnest($2::int2[], $3::int8[], $4::float8[], $5::bool[], $6::int[], $7::int[], $8::int[], $9::int[]) \
                  AS t(r, c, h, s, p, d, x, sp) \
             ON CONFLICT (region, res, cell, hour_start, synthetic, span_s) \
             DO UPDATE SET persons = EXCLUDED.persons, dwell_s = EXCLUDED.dwell_s, \
                           hotspot_dwell_s = EXCLUDED.hotspot_dwell_s",
            &[&region, &h_res, &h_cell, &h_hour, &h_syn, &h_per, &h_dw, &h_hot, &h_span],
        )?;
    }
    for (res, hour, synthetic, kept, dropped) in hex_sup {
        tx.execute(
            "INSERT INTO analytics.hex_suppressed (region, res, hour_start, synthetic, kept, dropped) \
             VALUES ($1, $2, to_timestamp($3), $4, $5, $6) \
             ON CONFLICT (region, res, hour_start, synthetic) \
             DO UPDATE SET kept = EXCLUDED.kept, dropped = EXCLUDED.dropped",
            &[&region, &res, &hour, &synthetic, &kept, &dropped],
        )?;
    }
    for (node, starts, ends, source) in interventions {
        tx.execute(
            "INSERT INTO analytics.interventions (region, node_id, starts_at, ends_at, source) \
             VALUES ($1, $2, to_timestamp($3), to_timestamp($4), $5)",
            &[&region, &node, &starts, &ends, &source],
        )?;
    }
    tx.commit()
}

/// Grid index over street segments in local ENU meters (x east, z south).
/// Compact: the points of all lines in one
/// array, a segment is a point and the next; cells sorted with their segments
/// in one flat list (`u32`), searched in the same order as a map of lists.
pub struct Snapper {
    cell: f32,
    pts: Vec<Vec2>,
    /// Per point: the edge (index into `edges`) of the segment starting there,
    /// `NO_SEG` at a line's last point.
    seg: Vec<u32>,
    edges: Vec<i64>,
    cells: Vec<(i32, i32)>,
    /// Segments of `cells[c]` at `idx[start[c]..start[c + 1]]`.
    start: Vec<u32>,
    idx: Vec<u32>,
}

const NO_SEG: u32 = u32::MAX;

impl Snapper {
    pub fn new(lines: &[(i64, Vec<Vec2>)]) -> Self {
        let mut s = Snapper {
            cell: SNAP_M,
            pts: Vec::new(),
            seg: Vec::new(),
            edges: Vec::new(),
            cells: Vec::new(),
            start: Vec::new(),
            idx: Vec::new(),
        };
        let mut hits: Vec<((i32, i32), u32)> = Vec::new();
        for (edge, pts) in lines {
            if pts.len() < 2 {
                continue;
            }
            let e = s.edges.len() as u32;
            s.edges.push(*edge);
            for w in pts.windows(2) {
                let k = s.pts.len() as u32;
                s.pts.push(w[0]);
                s.seg.push(e);
                let (lo, hi) = (w[0].min(w[1]), w[0].max(w[1]));
                for gx in s.key(lo).0..=s.key(hi).0 {
                    for gz in s.key(lo).1..=s.key(hi).1 {
                        hits.push(((gx, gz), k));
                    }
                }
            }
            s.pts.push(pts[pts.len() - 1]);
            s.seg.push(NO_SEG);
        }
        hits.sort_unstable();
        for (c, k) in hits {
            if s.cells.last() != Some(&c) {
                s.cells.push(c);
                s.start.push(s.idx.len() as u32);
            }
            s.idx.push(k);
        }
        s.start.push(s.idx.len() as u32);
        s.pts.shrink_to_fit();
        s.seg.shrink_to_fit();
        s
    }

    fn key(&self, p: Vec2) -> (i32, i32) {
        (
            (p.x / self.cell).floor() as i32,
            (p.y / self.cell).floor() as i32,
        )
    }

    fn in_cell(&self, c: (i32, i32)) -> &[u32] {
        match self.cells.binary_search(&c) {
            Ok(i) => &self.idx[self.start[i] as usize..self.start[i + 1] as usize],
            Err(_) => &[],
        }
    }

    /// Nearest edge within `SNAP_M`.
    pub fn snap(&self, p: Vec2) -> Option<i64> {
        self.nearest(p).map(|(e, _)| e)
    }

    /// Distance to the nearest street axis, if one is within `SNAP_M`.
    pub fn distance(&self, p: Vec2) -> Option<f32> {
        self.nearest(p).map(|(_, d)| d)
    }

    fn nearest(&self, p: Vec2) -> Option<(i64, f32)> {
        let (kx, kz) = self.key(p);
        let mut best: Option<(i64, f32)> = None;
        for dx in -1..=1 {
            for dz in -1..=1 {
                for &k in self.in_cell((kx + dx, kz + dz)) {
                    let k = k as usize;
                    let (a, b) = (self.pts[k], self.pts[k + 1]);
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let d = p.distance(a + ab * t);
                    if d <= SNAP_M && best.is_none_or(|(_, bd)| d < bd) {
                        best = Some((self.edges[self.seg[k] as usize], d));
                    }
                }
            }
        }
        best
    }
}

/// Age bands a player may pick (optional).
pub const AGE_BANDS: [&str; 3] = ["u30", "30_59", "60p"];

/// Optional self-reported profile. Lives in the browser; the server keeps it in
/// memory and only counts players per day by it.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Profile {
    pub age: Option<&'static str>,
    pub local: Option<bool>,
}

impl Profile {
    /// Unknown age bands count as no answer.
    pub fn parse(age: Option<&str>, local: Option<bool>) -> Self {
        Profile {
            age: age.and_then(|a| AGE_BANDS.iter().find(|b| **b == a).copied()),
            local,
        }
    }

    /// Keys as stored in `analytics.daily_profile`.
    fn keys(&self) -> (&'static str, &'static str) {
        let local = match self.local {
            Some(true) => "yes",
            Some(false) => "no",
            None => "na",
        };
        (self.age.unwrap_or("na"), local)
    }
}

/// Per-player analytics identity (memory only).
#[derive(Component, Clone, Copy)]
pub struct Tracked {
    pub pseudo: u64,
    pub synthetic: bool,
    /// False: the player objected; nothing of them reaches the aggregates.
    pub counted: bool,
    pub last_rate: f64,
    pub profile: Profile,
}

/// Person -> (dwell, part of it near an active hotspot), kept per person so
/// those who never count can be left out when written.
#[derive(Default)]
struct Bucket {
    persons: HashMap<u64, (f64, f64)>,
}

impl Bucket {
    fn add(&mut self, pseudo: u64, dt: f64, hot: bool) {
        let d = self.persons.entry(pseudo).or_default();
        d.0 += dt;
        if hot {
            d.1 += dt;
        }
    }

    /// Add the persons who count to `into` (pooling leftovers).
    fn pool(&self, into: &mut Bucket, counts: impl Fn(u64) -> bool) {
        for (&p, &(d, h)) in self.persons.iter().filter(|(p, _)| counts(**p)) {
            let e = into.persons.entry(p).or_default();
            e.0 += d;
            e.1 += h;
        }
    }

    /// Persons, dwell and hotspot dwell of those who count.
    fn sum(&self, counts: impl Fn(u64) -> bool) -> (usize, f64, f64) {
        self.persons
            .iter()
            .filter(|(p, _)| counts(**p))
            .fold((0, 0.0, 0.0), |(n, d, h), (_, (pd, ph))| {
                (n + 1, d + pd, h + ph)
            })
    }
}

/// Who counts: playing time today per pseudonym (memory only, like the
/// pseudonyms themselves).
pub struct Standing {
    /// Pseudonym -> (last fix, seconds played, counts).
    players: HashMap<u64, (f64, f64, bool)>,
    /// Seconds to play before counting ([`establish_s`]).
    need: f64,
}

impl Default for Standing {
    fn default() -> Self {
        Self {
            players: HashMap::new(),
            need: establish_s().max(0.0),
        }
    }
}

impl Standing {
    /// A fix at `now`: adds the time since the last one (gaps excluded).
    fn fix(&mut self, pseudo: u64, now: f64) {
        let e = self.players.entry(pseudo).or_insert((now, 0.0, false));
        let dt = now - e.0;
        if dt > 0.0 && dt <= MAX_GAP_S * 3.0 {
            e.1 += dt;
        }
        e.0 = now;
        e.2 |= e.1 >= self.need;
    }

    /// Counts at once (played on an earlier day).
    pub fn establish(&mut self, pseudo: u64, now: f64) {
        self.players.entry(pseudo).or_insert((now, 0.0, true)).2 = true;
    }

    pub fn counts(&self, pseudo: u64) -> bool {
        self.need <= 0.0 || self.players.get(&pseudo).is_some_and(|e| e.2)
    }

    /// Not counting and gone for longer than the grace time.
    fn left_early(&self, pseudo: u64, now: f64) -> bool {
        !self.counts(pseudo)
            && self
                .players
                .get(&pseudo)
                .is_none_or(|e| now - e.0 > self.need)
    }
}

#[derive(Resource)]
pub struct Aggregator {
    k: usize,
    snap: Snapper,
    salt: (i64, u64),
    last_seen: HashMap<u64, (f64, Option<i64>)>,
    dwell: HashMap<(i64, i64, bool), Bucket>,
    /// (node, day, UTC hour, category, synthetic) -> ratings.
    perception: HashMap<(i64, i64, i16, &'static str, bool), i32>,
    /// Zones (id, ENU centre, own radius or `ZONE_R_M`).
    zones: Vec<(String, Vec2, Option<f32>)>,
    /// Last zone per pseudonym.
    zone_of: HashMap<u64, usize>,
    /// (from, to, hour, synthetic) -> persons.
    flows: HashMap<(usize, usize, i64, bool), HashSet<u64>>,
    /// Leftovers of hours below k: (from, to, day, synthetic) -> persons.
    flow_rest: HashMap<(usize, usize, i64, bool), HashSet<u64>>,
    /// (day, synthetic) -> person -> latest profile keys.
    daily: HashMap<(i64, bool), HashMap<u64, (&'static str, &'static str)>>,
    /// Region origin for H3 (ENU -> lat/lon); `None` disables hexes.
    hex_origin: Option<(f64, f64)>,
    /// (res, cell, hour, synthetic) -> persons, dwell.
    hexes: HashMap<(u8, u64, i64, bool), Bucket>,
    /// Last fix time and cells per pseudonym, for hex dwell.
    last_hex: HashMap<u64, (f64, [u64; HEX_RES.len()])>,
    /// Focus places (id, centre, radius) for visitors per hour.
    focus: Vec<(String, Vec2, f32)>,
    /// (zone id, hour, synthetic) -> persons, dwell inside.
    places: HashMap<(String, i64, bool), Bucket>,
    /// Last fix time and focus places (indices) per pseudonym.
    last_place: HashMap<u64, (f64, Vec<usize>)>,
    /// Leftovers below k, pooled coarser in time (memory only, like all
    /// buckets): (edge, start, span, synthetic), (res, cell, day, synthetic),
    /// (zone id, day, synthetic).
    dwell_rest: HashMap<(i64, i64, i64, bool), Bucket>,
    hex_rest: HashMap<(u8, u64, i64, bool), Bucket>,
    place_rest: HashMap<(String, i64, bool), Bucket>,
    pub standing: Standing,
    /// Day -> pseudonyms flagged by the plausibility check.
    flagged: HashMap<i64, HashSet<u64>>,
    tx: AnalyticsTx,
    /// Local days of the region (`clock.rs`): pooling, people per day, the daily salt.
    pub clock: crate::clock::Clock,
}

impl Aggregator {
    pub fn new(snap: Snapper, zones: Vec<(String, Vec2, Option<f32>)>, tx: AnalyticsTx) -> Self {
        Self {
            clock: Default::default(),
            k: k_min(),
            snap,
            salt: (0, 0),
            last_seen: HashMap::new(),
            dwell: HashMap::new(),
            perception: HashMap::new(),
            zones,
            zone_of: HashMap::new(),
            flows: HashMap::new(),
            flow_rest: HashMap::new(),
            daily: HashMap::new(),
            hex_origin: None,
            hexes: HashMap::new(),
            last_hex: HashMap::new(),
            focus: Vec::new(),
            places: HashMap::new(),
            last_place: HashMap::new(),
            dwell_rest: HashMap::new(),
            hex_rest: HashMap::new(),
            place_rest: HashMap::new(),
            standing: Standing::default(),
            flagged: HashMap::new(),
            tx,
        }
    }

    /// Also aggregate into H3 cells ([`HEX_RES`]) per hour.
    pub fn with_hexes(mut self, origin_lat: f64, origin_lon: f64) -> Self {
        self.hex_origin = Some((origin_lat, origin_lon));
        self
    }

    fn observe_hexes(&mut self, t: &Tracked, pos: Vec3, now: f64, hot: bool) {
        let Some((lat0, lon0)) = self.hex_origin else {
            return;
        };
        let (lat, lon) = elements_shared::geo::unproject_enu(pos, lat0, lon0);
        let Ok(ll) = h3o::LatLng::new(lat, lon) else {
            return;
        };
        let cells = HEX_RES
            .map(|r| h3o::Resolution::try_from(r).map_or(0, |res| u64::from(ll.to_cell(res))));
        let hour = (now as i64).div_euclid(3600) * 3600;
        let prev = self.last_hex.insert(t.pseudo, (now, cells));
        for (i, &cell) in cells.iter().enumerate() {
            let b = self
                .hexes
                .entry((HEX_RES[i], cell, hour, t.synthetic))
                .or_default();
            let mut dt = 0.0;
            if let Some((last, pc)) = prev {
                if pc[i] == cell && now - last <= MAX_GAP_S {
                    dt = now - last;
                }
            }
            b.add(t.pseudo, dt, hot);
        }
    }

    fn zone(&self, p: Vec2) -> Option<usize> {
        self.zones
            .iter()
            .enumerate()
            .map(|(i, (_, c, r))| (i, c.distance(p), r.unwrap_or(ZONE_R_M)))
            .filter(|&(_, d, r)| d <= r)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _, _)| i)
    }

    /// New zones (editor): pending flows and last zones follow their ids;
    /// those of removed zones are dropped.
    pub fn set_zones(&mut self, zones: Vec<(String, Vec2, Option<f32>)>) {
        let to_new: Vec<Option<usize>> = self
            .zones
            .iter()
            .map(|(id, _, _)| zones.iter().position(|z| &z.0 == id))
            .collect();
        let map = |i: usize| to_new.get(i).copied().flatten();
        self.zone_of = std::mem::take(&mut self.zone_of)
            .into_iter()
            .filter_map(|(p, z)| Some((p, map(z)?)))
            .collect();
        self.flows = std::mem::take(&mut self.flows)
            .into_iter()
            .filter_map(|((a, b, h, s), v)| Some(((map(a)?, map(b)?, h, s), v)))
            .collect();
        self.flow_rest = std::mem::take(&mut self.flow_rest)
            .into_iter()
            .filter_map(|((a, b, d, s), v)| Some(((map(a)?, map(b)?, d, s), v)))
            .collect();
        self.zones = zones;
    }

    /// Focus places (editor): visitors per hour from now on.
    pub fn set_focus(&mut self, focus: Vec<(String, Vec2, f32)>) {
        self.focus = focus;
        self.last_place.clear();
    }

    fn observe_places(&mut self, t: &Tracked, p: Vec2, now: f64) {
        if self.focus.is_empty() {
            return;
        }
        let inside: Vec<usize> = (0..self.focus.len())
            .filter(|&i| self.focus[i].1.distance(p) <= self.focus[i].2)
            .collect();
        let hour = (now as i64).div_euclid(3600) * 3600;
        let prev = self.last_place.insert(t.pseudo, (now, inside.clone()));
        for i in inside {
            let dt = prev
                .as_ref()
                .filter(|(last, ids)| now - last <= MAX_GAP_S && ids.contains(&i))
                .map_or(0.0, |(last, _)| now - last);
            self.places
                .entry((self.focus[i].0.clone(), hour, t.synthetic))
                .or_default()
                .add(t.pseudo, dt, false);
        }
    }

    /// Hash of token and a salt that changes every local day.
    pub fn pseudonym(&mut self, token: &str, now: f64) -> u64 {
        let day = self.clock.day(now);
        if self.salt.0 != day {
            self.salt = (
                day,
                std::collections::hash_map::RandomState::new()
                    .build_hasher()
                    .finish(),
            );
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        h.write_u64(self.salt.1);
        h.write(token.as_bytes());
        h.finish()
    }

    /// Record a position fix (ENU meters); `hot` = near an active hotspot.
    /// Distance to the nearest street axis (plausibility check).
    pub fn street_distance(&self, pos: Vec3) -> Option<f32> {
        self.snap.distance(Vec2::new(pos.x, pos.z))
    }

    pub fn observe(&mut self, t: &Tracked, pos: Vec3, now: f64, hot: bool) {
        let p2 = Vec2::new(pos.x, pos.z);
        if !t.synthetic {
            self.standing.fix(t.pseudo, now);
        }
        let day = self.day0(now);
        self.daily
            .entry((day, t.synthetic))
            .or_default()
            .insert(t.pseudo, t.profile.keys());
        if let Some(z) = self.zone(p2) {
            if let Some(prev) = self.zone_of.insert(t.pseudo, z).filter(|&p| p != z) {
                let hour = (now as i64).div_euclid(3600) * 3600;
                self.flows
                    .entry((prev, z, hour, t.synthetic))
                    .or_default()
                    .insert(t.pseudo);
            }
        }
        self.observe_hexes(t, pos, now, hot);
        self.observe_places(t, p2, now);
        let edge = self.snap.snap(p2);
        let prev = self.last_seen.insert(t.pseudo, (now, edge));
        let Some(edge) = edge else { return };
        let window = (now as i64).div_euclid(window_s()) * window_s();
        let b = self.dwell.entry((edge, window, t.synthetic)).or_default();
        let mut dt = 0.0;
        if let Some((last, Some(prev_edge))) = prev {
            if prev_edge == edge && now - last <= MAX_GAP_S {
                dt = now - last;
            }
        }
        b.add(t.pseudo, dt, hot);
    }

    pub fn rate(&mut self, node: i64, category: &'static str, synthetic: bool, now: f64) {
        let day = (now as i64).div_euclid(86_400) * 86_400;
        let hour = (now as i64).div_euclid(3600).rem_euclid(24) as i16;
        *self
            .perception
            .entry((node, day, hour, category, synthetic))
            .or_default() += 1;
    }

    /// Start (unix s) of the local day of `now`.
    fn day0(&self, now: f64) -> i64 {
        self.clock.day_start(self.clock.day(now)) as i64
    }

    /// A real player flagged by the plausibility check today (counted per day).
    pub fn flagged(&mut self, pseudo: u64, now: f64) {
        let day = self.day0(now);
        self.flagged.entry(day).or_default().insert(pseudo);
        self.daily.entry((day, false)).or_default();
    }

    /// Take a person out of everything not written yet (they objected).
    pub fn withdraw(&mut self, pseudo: u64) {
        self.forget(pseudo);
        for b in self
            .dwell
            .values_mut()
            .chain(self.hexes.values_mut())
            .chain(self.places.values_mut())
            .chain(self.dwell_rest.values_mut())
            .chain(self.hex_rest.values_mut())
            .chain(self.place_rest.values_mut())
        {
            b.persons.remove(&pseudo);
        }
        for p in self.flows.values_mut().chain(self.flow_rest.values_mut()) {
            p.remove(&pseudo);
        }
        for p in self.daily.values_mut() {
            p.remove(&pseudo);
        }
        self.standing.players.remove(&pseudo);
    }

    pub fn forget(&mut self, pseudo: u64) {
        self.last_seen.remove(&pseudo);
        self.zone_of.remove(&pseudo);
        self.last_hex.remove(&pseudo);
        self.last_place.remove(&pseudo);
    }

    /// Write closed windows with >= k counting persons (drop the rest) and
    /// pending ratings. A window is closed once it ended at least the grace
    /// time ago ([`establish_s`]); `all` writes everything (shutdown).
    pub fn flush(&mut self, now: f64, all: bool) {
        let grace = self.standing.need;
        let closed = |end: i64| all || end as f64 + grace <= now;
        // Local days (23 or 25 hours around a clock change).
        let clock = self.clock.clone();
        let day0 = |t: i64| clock.day_start(clock.day(t as f64)) as i64;
        let day_end = |d: i64| clock.day_start(clock.day(d as f64) + 1) as i64;
        let standing = &self.standing;
        let counts = |synthetic: bool, p: u64| synthetic || standing.counts(p);
        let mut rows = Vec::new();
        let (mut kept, mut dropped) = (0, 0);
        let k = self.k;
        let mut per_window: HashMap<(i64, bool), (i32, i32, HashSet<u64>)> = HashMap::new();
        let dwell_rest = &mut self.dwell_rest;
        self.dwell.retain(|&(edge, window, synthetic), b| {
            if !closed(window + window_s()) {
                return true;
            }
            let w = per_window.entry((window, synthetic)).or_default();
            w.2.extend(b.persons.keys().filter(|&&p| counts(synthetic, p)));
            let (n, dwell_s, hot_s) = b.sum(|p| counts(synthetic, p));
            if n >= k {
                w.0 += 1;
                kept += 1;
                rows.push(Row::Dwell {
                    edge,
                    window,
                    span: window_s() as i32,
                    synthetic,
                    persons: n as i32,
                    dwell_s: dwell_s.round() as i32,
                    hotspot_dwell_s: hot_s.round() as i32,
                });
            } else {
                w.1 += 1;
                dropped += 1;
                // Into the segment's leftovers of this hour.
                let hour = window.div_euclid(3600) * 3600;
                b.pool(
                    dwell_rest.entry((edge, hour, 3600, synthetic)).or_default(),
                    |p| counts(synthetic, p),
                );
            }
            false
        });
        // Leftovers: an hour that reaches k is written, else it joins the day.
        let mut to_day = Vec::new();
        self.dwell_rest
            .retain(|&(edge, start, span, synthetic), b| {
                if span != 3600 || !closed(start + 3600) {
                    return true;
                }
                let (n, dwell_s, hot_s) = b.sum(|p| counts(synthetic, p));
                if n >= k {
                    rows.push(Row::Dwell {
                        edge,
                        window: start,
                        span: 3600,
                        synthetic,
                        persons: n as i32,
                        dwell_s: dwell_s.round() as i32,
                        hotspot_dwell_s: hot_s.round() as i32,
                    });
                } else if n > 0 {
                    to_day.push(((edge, day0(start), 86_400, synthetic), std::mem::take(b)));
                }
                false
            });
        for (key, b) in to_day {
            b.pool(self.dwell_rest.entry(key).or_default(), |_| true);
        }
        self.dwell_rest
            .retain(|&(edge, start, span, synthetic), b| {
                if span != 86_400 || !closed(day_end(start)) {
                    return true;
                }
                let (n, dwell_s, hot_s) = b.sum(|p| counts(synthetic, p));
                if n >= k {
                    rows.push(Row::Dwell {
                        edge,
                        window: start,
                        span: 86_400,
                        synthetic,
                        persons: n as i32,
                        dwell_s: dwell_s.round() as i32,
                        hotspot_dwell_s: hot_s.round() as i32,
                    });
                }
                false
            });
        for ((window, synthetic), (kept, dropped, persons)) in per_window {
            rows.push(Row::Suppressed {
                window,
                synthetic,
                kept,
                dropped,
                persons: (persons.len() >= k).then_some(persons.len() as i32),
            });
        }
        // Flows: closed hours, >= k persons; the rest pooled per day.
        let zones = &self.zones;
        let flow_rest = &mut self.flow_rest;
        self.flows.retain(|&(from, to, hour, synthetic), persons| {
            if !closed(hour + 3600) {
                return true;
            }
            let n = persons.iter().filter(|&&p| counts(synthetic, p)).count();
            if n >= k {
                rows.push(Row::Flow {
                    from: zones[from].0.clone(),
                    to: zones[to].0.clone(),
                    hour,
                    synthetic,
                    persons: n as i32,
                });
            } else {
                let day = day0(hour);
                flow_rest
                    .entry((from, to, day, synthetic))
                    .or_default()
                    .extend(persons.iter().filter(|&&p| counts(synthetic, p)));
            }
            false
        });
        self.flow_rest
            .retain(|&(from, to, day, synthetic), persons| {
                if !closed(day_end(day)) {
                    return true;
                }
                if persons.len() >= k {
                    rows.push(Row::FlowDay {
                        from: zones[from].0.clone(),
                        to: zones[to].0.clone(),
                        day: clock.day(day as f64),
                        synthetic,
                        persons: persons.len() as i32,
                    });
                }
                false
            });
        // Hexes: closed hours, >= k persons; kept/dropped per resolution.
        let mut hex_sup: HashMap<(u8, i64, bool), (i32, i32)> = HashMap::new();
        let hex_rest = &mut self.hex_rest;
        self.hexes.retain(|&(res, cell, hour, synthetic), b| {
            if !closed(hour + 3600) {
                return true;
            }
            let s = hex_sup.entry((res, hour, synthetic)).or_default();
            let (n, dwell_s, hot_s) = b.sum(|p| counts(synthetic, p));
            if n >= k {
                s.0 += 1;
                rows.push(Row::HexDwell {
                    res,
                    cell,
                    hour,
                    span: 3600,
                    synthetic,
                    persons: n as i32,
                    dwell_s: dwell_s.round() as i32,
                    hotspot_dwell_s: hot_s.round() as i32,
                });
            } else {
                s.1 += 1;
                let day = day0(hour);
                b.pool(
                    hex_rest.entry((res, cell, day, synthetic)).or_default(),
                    |p| counts(synthetic, p),
                );
            }
            false
        });
        self.hex_rest.retain(|&(res, cell, day, synthetic), b| {
            if !closed(day_end(day)) {
                return true;
            }
            let (n, dwell_s, hot_s) = b.sum(|p| counts(synthetic, p));
            if n >= k {
                rows.push(Row::HexDwell {
                    res,
                    cell,
                    hour: day,
                    span: 86_400,
                    synthetic,
                    persons: n as i32,
                    dwell_s: dwell_s.round() as i32,
                    hotspot_dwell_s: hot_s.round() as i32,
                });
            }
            false
        });
        // Focus places: visitors per hour, leftovers per day.
        let place_rest = &mut self.place_rest;
        self.places.retain(|(zone, hour, synthetic), b| {
            if !closed(hour + 3600) {
                return true;
            }
            let (n, dwell_s, _) = b.sum(|p| counts(*synthetic, p));
            if n >= k {
                rows.push(Row::PlaceVisit {
                    zone: zone.clone(),
                    start: *hour,
                    span: 3600,
                    synthetic: *synthetic,
                    persons: n as i32,
                    dwell_s: dwell_s.round() as i32,
                });
            } else {
                let day = day0(*hour);
                b.pool(
                    place_rest
                        .entry((zone.clone(), day, *synthetic))
                        .or_default(),
                    |p| counts(*synthetic, p),
                );
            }
            false
        });
        self.place_rest.retain(|(zone, day, synthetic), b| {
            if !closed(day_end(*day)) {
                return true;
            }
            let (n, dwell_s, _) = b.sum(|p| counts(*synthetic, p));
            if n >= k {
                rows.push(Row::PlaceVisit {
                    zone: zone.clone(),
                    start: *day,
                    span: 86_400,
                    synthetic: *synthetic,
                    persons: n as i32,
                    dwell_s: dwell_s.round() as i32,
                });
            }
            false
        });
        self.last_place.retain(|_, (t, _)| now - *t < 3600.0);
        for ((res, hour, synthetic), (kept, dropped)) in hex_sup {
            rows.push(Row::HexSuppressed {
                res,
                hour,
                synthetic,
                kept,
                dropped,
            });
        }
        self.last_hex.retain(|_, (t, _)| now - *t < 3600.0);
        // Players per day, in total and by profile: counts only; old days dropped.
        // Newcomers: left without counting.
        let today = day0(now as i64);
        for (&(day, synthetic), persons) in &self.daily {
            let newcomers = if synthetic {
                0
            } else {
                persons
                    .keys()
                    .filter(|&&p| all && !standing.counts(p) || standing.left_early(p, now))
                    .count()
            };
            let mut by: HashMap<(&'static str, &'static str), i32> = HashMap::new();
            let mut players = 0;
            for (&p, keys) in persons {
                if counts(synthetic, p) {
                    players += 1;
                    *by.entry(*keys).or_default() += 1;
                }
            }
            let flagged = if synthetic {
                0
            } else {
                self.flagged.get(&day).map_or(0, HashSet::len) as i32
            };
            rows.push(Row::Daily {
                day: clock.day(day as f64),
                synthetic,
                players,
                newcomers: newcomers as i32,
                flagged,
            });
            for ((age, local), players) in by {
                rows.push(Row::DailyProfile {
                    day: clock.day(day as f64),
                    synthetic,
                    age,
                    local,
                    players,
                });
            }
        }
        self.daily.retain(|&(day, _), _| day >= today);
        self.flagged.retain(|&day, _| day >= today);
        for ((node, day, hour, category, synthetic), add) in self.perception.drain() {
            rows.push(Row::Perception {
                node,
                day,
                hour,
                category,
                synthetic,
                add,
            });
        }
        self.last_seen.retain(|_, (t, _)| now - *t < 3600.0);
        // Standing lasts the day (pseudonyms change daily anyway).
        self.standing.players.retain(|_, e| now - e.0 < 86_400.0);
        if kept + dropped > 0 {
            info!(kept, dropped_below_k = dropped, "analytics flush");
        }
        if !rows.is_empty() {
            self.tx.send(rows);
        }
    }
}

/// Every 30 s: flush closed windows.
pub fn flush_system(mut agg: ResMut<Aggregator>, mut next: Local<f64>) {
    let now = now_s();
    if now >= *next {
        *next = now + 30.0;
        agg.flush(now, false);
    }
}

/// On SIGTERM or unload: write open windows and the running hotspot slot, then exit.
pub fn shutdown_system(
    agg: Option<ResMut<Aggregator>>,
    hotspots: Option<ResMut<crate::hotspot::Hotspots>>,
    scores: Option<ResMut<crate::score::Scores>>,
    tx: Res<AnalyticsTx>,
    stop: Res<crate::Stop>,
    mut exit: MessageWriter<AppExit>,
    mut done: Local<bool>,
) {
    use std::sync::atomic::Ordering::Relaxed;
    if *done || !(crate::gateway::SHUTDOWN.load(Relaxed) || stop.0.load(Relaxed)) {
        return;
    }
    *done = true;
    if let Some(mut hs) = hotspots {
        match agg.as_deref() {
            Some(a) => hs.finish(&|p| a.standing.counts(p)),
            None => hs.finish(&|_| true),
        }
    }
    if let Some(mut agg) = agg {
        agg.flush(now_s(), true);
    }
    if let Some(mut s) = scores {
        let ok = s.flush_wait(std::time::Duration::from_secs(10));
        info!(written = ok, "scores flushed");
    }
    let ok = tx.barrier(std::time::Duration::from_secs(20));
    info!(written = ok, "analytics flushed, exiting");
    exit.write(AppExit::Success);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agg(k: usize) -> (Aggregator, Receiver<Msg>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let snap = Snapper::new(&[(7, vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)])]);
        let mut a = Aggregator::new(snap, Vec::new(), AnalyticsTx(Out::Chan(tx)));
        a.k = k;
        a.standing.need = 0.0; // everyone counts at once
        (a, rx)
    }

    /// Fixes every 5 s on edge 7 for `secs` seconds from `t0`.
    fn walk(a: &mut Aggregator, p: u64, t0: f64, secs: u32) {
        for s in (0..=secs).step_by(5) {
            a.observe(&tracked(p), Vec3::new(20.0, 0.0, 5.0), t0 + s as f64, false);
        }
    }

    fn daily_rows(rx: &Receiver<Msg>) -> Vec<(i32, i32)> {
        rx.try_iter()
            .flat_map(|m| match m {
                Msg::Rows(r) => r,
                Msg::Barrier(_) => vec![],
            })
            .filter_map(|r| match r {
                Row::Daily {
                    players, newcomers, ..
                } => Some((players, newcomers)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn withdrawn_person_leaves_open_windows() {
        let (mut a, rx) = agg(3);
        let t0 = 900_000.0;
        for p in 0..3 {
            walk(&mut a, p, t0, 60);
        }
        a.withdraw(2);
        a.flush(t0 + 2000.0, true);
        assert!(dwell_rows(&rx).is_empty(), "2 left < k = 3");
    }

    #[test]
    fn newcomers_count_once_they_played_long_enough() {
        let (mut a, rx) = agg(3);
        a.standing.need = 600.0;
        let t0 = 900_000.0; // start of a window
                            // 3 stay 12 minutes, 2 leave after 2; one returning player at once.
        for p in 0..3 {
            walk(&mut a, p, t0, 720);
        }
        for p in 10..12 {
            walk(&mut a, p, t0, 120);
        }
        a.standing.establish(20, t0);
        walk(&mut a, 20, t0, 60);
        // First window closed at t0 + 900, written only after the grace time.
        a.flush(t0 + 900.0 + 100.0, false);
        assert!(dwell_rows(&rx).is_empty(), "held during the grace time");
        a.flush(t0 + 900.0 + 600.0, false);
        let rows = dwell_rows(&rx);
        assert_eq!(rows.len(), 1);
        let (_, persons, dwell_s) = rows[0];
        assert_eq!(
            persons, 4,
            "3 who stayed (counted back to their start) + 1 returning"
        );
        assert_eq!(dwell_s, 3 * 720 + 60, "newcomers' dwell left out");
        a.flush(t0 + 2000.0, true);
        let daily = daily_rows(&rx);
        assert_eq!(daily.last(), Some(&(4, 2)), "players 4, newcomers 2");
    }

    fn tracked(pseudo: u64) -> Tracked {
        Tracked {
            pseudo,
            synthetic: false,
            counted: true,
            last_rate: 0.0,
            profile: Profile::default(),
        }
    }

    fn dwell_rows(rx: &Receiver<Msg>) -> Vec<(i64, i32, i32)> {
        rx.try_iter()
            .flat_map(|m| match m {
                Msg::Rows(r) => r,
                Msg::Barrier(_) => vec![],
            })
            .filter_map(|r| match r {
                Row::Dwell {
                    edge,
                    persons,
                    dwell_s,
                    ..
                } => Some((edge, persons, dwell_s)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn snaps_within_30_m_only() {
        let (a, _) = agg(1);
        assert_eq!(a.snap.snap(Vec2::new(50.0, 10.0)), Some(7));
        assert_eq!(a.snap.snap(Vec2::new(50.0, 40.0)), None);
    }

    #[test]
    fn drops_windows_below_k_and_counts_dwell() {
        let (mut a, rx) = agg(3);
        let t0 = 1_000_000.0; // inside one window
        for p in 0..3 {
            for s in 0..5 {
                a.observe(&tracked(p), Vec3::new(20.0, 0.0, 5.0), t0 + s as f64, false);
            }
        }
        a.flush(t0 + 10.0, true);
        assert_eq!(dwell_rows(&rx), vec![(7, 3, 12)]); // 3 persons x 4 s

        let (mut a, rx) = agg(3);
        for p in 0..2 {
            a.observe(&tracked(p), Vec3::new(20.0, 0.0, 5.0), t0, false);
        }
        a.flush(t0 + 10.0, true);
        assert!(dwell_rows(&rx).is_empty(), "2 < k must be dropped");
    }

    /// Dwell rows as (persons, span).
    fn spans(rx: &Receiver<Msg>) -> Vec<(i32, i32)> {
        all_rows(rx)
            .into_iter()
            .filter_map(|r| match r {
                Row::Dwell { persons, span, .. } => Some((persons, span)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn leftovers_pool_into_the_hour_then_the_day() {
        let (mut a, rx) = agg(3);
        let h = 3600.0 * 24.0 * 20_000.0; // a UTC midnight
                                          // Three people in three different windows of one hour: each window
                                          // is below k, the hour is not.
        for (p, w) in [(1, 0.0), (2, 900.0), (3, 1800.0)] {
            walk(&mut a, p, h + w + 10.0, 30);
        }
        a.flush(h + 3600.0 + 1.0, false);
        assert_eq!(spans(&rx), vec![(3, 3600)], "pooled hour");
        // Three people in three different hours: only the day reaches k.
        for (p, hr) in [(4, 2.0), (5, 5.0), (6, 9.0)] {
            walk(&mut a, p, h + hr * 3600.0 + 10.0, 30);
        }
        a.flush(h + 12.0 * 3600.0, false);
        assert!(spans(&rx).is_empty(), "day still open");
        a.flush(h + 86_400.0 + 1.0, false);
        assert_eq!(spans(&rx), vec![(3, 86_400)], "pooled day");
        // Two in a day: dropped for good.
        for (p, hr) in [(7, 2.0), (8, 5.0)] {
            walk(&mut a, p, h + 86_400.0 + hr * 3600.0 + 10.0, 30);
        }
        a.flush(h + 2.0 * 86_400.0 + 1.0, false);
        assert!(spans(&rx).is_empty());
        assert!(a.dwell_rest.is_empty(), "nothing left in memory");
    }

    #[test]
    fn written_window_is_not_pooled_again() {
        let (mut a, rx) = agg(3);
        let h = 3600.0 * 24.0 * 20_000.0;
        for p in 1..=3 {
            walk(&mut a, p, h + 10.0, 30);
        }
        walk(&mut a, 4, h + 910.0, 30);
        a.flush(h + 86_400.0 + 1.0, false);
        // The first window is written; person 4 alone stays below k everywhere.
        assert_eq!(spans(&rx), vec![(3, 900)]);
    }

    #[test]
    fn flows_below_k_pool_into_the_day() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let zones = vec![
            ("a".to_string(), Vec2::new(0.0, 0.0), Some(50.0)),
            ("b".to_string(), Vec2::new(500.0, 0.0), Some(50.0)),
        ];
        let mut agg = Aggregator::new(Snapper::new(&[]), zones, AnalyticsTx(Out::Chan(tx)));
        agg.k = 3;
        agg.standing.need = 0.0;
        let h = 3600.0 * 24.0 * 20_000.0;
        // One person a->b in each of three hours: no hour reaches k, the day does.
        for p in 1..=3u64 {
            let t = h + p as f64 * 3600.0;
            agg.observe(&tracked(p), Vec3::new(0.0, 0.0, 0.0), t, false);
            agg.observe(&tracked(p), Vec3::new(500.0, 0.0, 0.0), t + 60.0, false);
        }
        agg.flush(h + 86_400.0 + 1.0, false);
        let rows = all_rows(&rx);
        assert!(!rows.iter().any(|r| matches!(r, Row::Flow { .. })));
        let day: Vec<(String, String, i32)> = rows
            .into_iter()
            .filter_map(|r| match r {
                Row::FlowDay {
                    from, to, persons, ..
                } => Some((from, to, persons)),
                _ => None,
            })
            .collect();
        assert_eq!(day, vec![("a".into(), "b".into(), 3)]);
        assert!(agg.flow_rest.is_empty());
    }

    #[test]
    fn days_pool_in_local_time() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let zones = vec![
            ("a".to_string(), Vec2::new(0.0, 0.0), Some(50.0)),
            ("b".to_string(), Vec2::new(500.0, 0.0), Some(50.0)),
        ];
        let mut agg = Aggregator::new(Snapper::new(&[]), zones, AnalyticsTx(Out::Chan(tx)));
        agg.k = 3;
        agg.standing.need = 0.0;
        agg.clock = crate::clock::Clock::new(Some("America/Bogota")); // UTC-5, no DST
        let h = 3600.0 * 24.0 * 20_000.0; // a UTC midnight = 19:00 the day before in Bogotá

        // Trips at 21:00, 22:00 and 23:00 Bogotá time (02-04 UTC, the next UTC day).
        for p in 1..=3u64 {
            let t = h + (p as f64 + 1.0) * 3600.0;
            agg.observe(&tracked(p), Vec3::new(0.0, 0.0, 0.0), t, false);
            agg.observe(&tracked(p), Vec3::new(500.0, 0.0, 0.0), t + 60.0, false);
        }
        agg.flush(h + 4.5 * 3600.0, false);
        let early = all_rows(&rx);
        assert!(
            !early.iter().any(|r| matches!(r, Row::FlowDay { .. })),
            "23:30 local: day still open"
        );
        agg.flush(h + 6.0 * 3600.0, false);
        let day: Vec<(i64, i32)> = all_rows(&rx)
            .into_iter()
            .filter_map(|r| match r {
                Row::FlowDay { day, persons, .. } => Some((day, persons)),
                _ => None,
            })
            .collect();
        assert_eq!(day, vec![(19_999, 3)], "the local day before the UTC date");
    }

    #[test]
    fn focus_place_counts_visitors_per_hour() {
        let (mut a, rx) = agg(3);
        a.set_focus(vec![("markt".into(), Vec2::new(20.0, 5.0), 50.0)]);
        let h = 3600.0 * 24.0 * 20_000.0;
        for p in 1..=3 {
            walk(&mut a, p, h + 100.0 * p as f64, 20);
        }
        // Two more in a later hour: below k, pooled into the day with nobody else.
        for p in 4..=5 {
            walk(&mut a, p, h + 7200.0 + 10.0, 20);
        }
        a.flush(h + 86_400.0 + 1.0, false);
        let visits: Vec<(String, i32, i32, i32)> = all_rows(&rx)
            .into_iter()
            .filter_map(|r| match r {
                Row::PlaceVisit {
                    zone,
                    span,
                    persons,
                    dwell_s,
                    ..
                } => Some((zone, span, persons, dwell_s)),
                _ => None,
            })
            .collect();
        assert_eq!(visits, vec![("markt".into(), 3600, 3, 60)]);
    }

    #[test]
    fn hexes_keep_cells_at_k_per_resolution() {
        let (a, rx) = agg(3);
        let mut a = a.with_hexes(47.6603, 9.1764);
        let t0 = 1_000_000.0 * 3600.0; // start of an hour
                                       // 3 persons at the same spot (-> one cell per resolution), 1 far away.
        for p in 0..3 {
            for s in 0..5 {
                a.observe(&tracked(p), Vec3::new(20.0, 0.0, 5.0), t0 + s as f64, false);
            }
        }
        a.observe(&tracked(9), Vec3::new(3000.0, 0.0, 3000.0), t0, false);
        a.flush(t0 + 7200.0, false);
        let rows: Vec<Row> = rx
            .try_iter()
            .flat_map(|m| match m {
                Msg::Rows(r) => r,
                Msg::Barrier(_) => vec![],
            })
            .collect();
        let kept: Vec<(u8, i32, i32)> = rows
            .iter()
            .filter_map(|r| match r {
                Row::HexDwell {
                    res,
                    persons,
                    dwell_s,
                    ..
                } => Some((*res, *persons, *dwell_s)),
                _ => None,
            })
            .collect();
        assert_eq!(kept.len(), HEX_RES.len());
        assert!(kept.iter().all(|&(_, p, d)| p == 3 && d == 12));
        let dropped: i32 = rows
            .iter()
            .filter_map(|r| match r {
                Row::HexSuppressed { dropped, .. } => Some(*dropped),
                _ => None,
            })
            .sum();
        assert_eq!(dropped, HEX_RES.len() as i32, "the lone player's cells");
    }

    #[test]
    fn flows_count_zone_changes_above_k() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let snap = Snapper::new(&[]);
        let zones = vec![
            ("a".into(), Vec2::new(0.0, 0.0), None),
            ("b".into(), Vec2::new(1000.0, 0.0), None),
        ];
        let mut agg = Aggregator::new(snap, zones, AnalyticsTx(Out::Chan(tx)));
        agg.k = 2;
        agg.standing.need = 0.0;
        let t0 = 3600.0 * 1000.0;
        for p in 0..3 {
            agg.observe(&tracked(p), Vec3::new(10.0, 0.0, 0.0), t0, false);
            agg.observe(&tracked(p), Vec3::new(990.0, 0.0, 0.0), t0 + 60.0, false);
        }
        agg.observe(&tracked(9), Vec3::new(990.0, 0.0, 0.0), t0, false);
        agg.observe(&tracked(9), Vec3::new(10.0, 0.0, 0.0), t0 + 60.0, false); // b->a: 1 < k
        agg.flush(t0 + 7200.0, false);
        let flows: Vec<(String, String, i32)> = rx
            .try_iter()
            .flat_map(|m| match m {
                Msg::Rows(r) => r,
                Msg::Barrier(_) => vec![],
            })
            .filter_map(|r| match r {
                Row::Flow {
                    from, to, persons, ..
                } => Some((from, to, persons)),
                _ => None,
            })
            .collect();
        assert_eq!(flows, vec![("a".into(), "b".into(), 3)]);
    }

    #[test]
    fn flows_follow_zone_ids_when_zones_change() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let zones = vec![
            ("a".into(), Vec2::new(0.0, 0.0), None),
            ("b".into(), Vec2::new(1000.0, 0.0), None),
            ("c".into(), Vec2::new(0.0, 1000.0), None),
        ];
        let mut agg = Aggregator::new(Snapper::new(&[]), zones, AnalyticsTx(Out::Chan(tx)));
        agg.k = 2;
        agg.standing.need = 0.0;
        let t0 = 3600.0 * 1000.0;
        for p in 0..3 {
            agg.observe(&tracked(p), Vec3::new(10.0, 0.0, 0.0), t0, false);
            agg.observe(&tracked(p), Vec3::new(990.0, 0.0, 0.0), t0 + 60.0, false);
            agg.observe(&tracked(p), Vec3::new(0.0, 0.0, 990.0), t0 + 120.0, false);
        }
        // The editor: a new zone first, "a" gets a radius, "c" goes.
        agg.set_zones(vec![
            ("new".into(), Vec2::new(5000.0, 0.0), Some(100.0)),
            ("b".into(), Vec2::new(1000.0, 0.0), None),
            ("a".into(), Vec2::new(0.0, 0.0), Some(50.0)),
        ]);
        agg.flush(t0 + 7200.0, false);
        let flows: Vec<(String, String)> = all_rows(&rx)
            .into_iter()
            .filter_map(|r| match r {
                Row::Flow { from, to, .. } => Some((from, to)),
                _ => None,
            })
            .collect();
        assert_eq!(flows, vec![("a".into(), "b".into())], "b->c dropped with c");
        // Own radius: 60 m from "a" is outside its 50 m.
        assert_eq!(agg.zone(Vec2::new(60.0, 0.0)), None);
        assert_eq!(agg.zone(Vec2::new(40.0, 0.0)), Some(2));
    }

    fn all_rows(rx: &Receiver<Msg>) -> Vec<Row> {
        rx.try_iter()
            .flat_map(|m| match m {
                Msg::Rows(r) => r,
                Msg::Barrier(_) => vec![],
            })
            .collect()
    }

    #[test]
    fn hotspot_dwell_is_part_of_dwell() {
        let (mut a, rx) = agg(1);
        let t0 = 1_000_000.0;
        for s in 0..5 {
            a.observe(
                &tracked(1),
                Vec3::new(20.0, 0.0, 5.0),
                t0 + s as f64,
                s >= 3,
            );
        }
        a.flush(t0 + 10.0, true);
        let hot: Vec<(i32, i32)> = all_rows(&rx)
            .into_iter()
            .filter_map(|r| match r {
                Row::Dwell {
                    dwell_s,
                    hotspot_dwell_s,
                    ..
                } => Some((dwell_s, hotspot_dwell_s)),
                _ => None,
            })
            .collect();
        assert_eq!(hot, vec![(4, 2)]);
    }

    #[test]
    fn daily_profile_counts_latest_answer_once() {
        let (mut a, rx) = agg(1);
        let t0 = 1_000_000.0;
        let mut p = tracked(1);
        a.observe(&p, Vec3::ZERO, t0, false);
        p.profile = Profile::parse(Some("60p"), Some(true));
        a.observe(&p, Vec3::ZERO, t0 + 1.0, false);
        let mut q = tracked(2);
        q.profile = Profile::parse(Some("bogus"), Some(false));
        a.observe(&q, Vec3::ZERO, t0, false);
        a.flush(t0 + 10.0, true);
        let mut got: Vec<(&str, &str, i32)> = all_rows(&rx)
            .into_iter()
            .filter_map(|r| match r {
                Row::DailyProfile {
                    age,
                    local,
                    players,
                    ..
                } => Some((age, local, players)),
                _ => None,
            })
            .collect();
        got.sort();
        assert_eq!(got, vec![("60p", "yes", 1), ("na", "no", 1)]);
    }

    #[test]
    fn open_window_kept_until_final_flush() {
        let (mut a, rx) = agg(1);
        let now = now_s();
        a.observe(&tracked(1), Vec3::new(20.0, 0.0, 0.0), now, false);
        a.flush(now, false);
        assert!(dwell_rows(&rx).is_empty());
        a.flush(now, true);
        assert_eq!(dwell_rows(&rx).len(), 1);
    }

    /// The index before the compact one (2026-10-07), as the reference.
    struct MapSnapper {
        grid: HashMap<(i32, i32), Vec<usize>>,
        segs: Vec<(i64, Vec2, Vec2)>,
    }

    impl MapSnapper {
        fn new(lines: &[(i64, Vec<Vec2>)]) -> Self {
            let key = |p: Vec2| ((p.x / SNAP_M).floor() as i32, (p.y / SNAP_M).floor() as i32);
            let mut s = MapSnapper {
                grid: HashMap::new(),
                segs: Vec::new(),
            };
            for (edge, pts) in lines {
                for w in pts.windows(2) {
                    let idx = s.segs.len();
                    s.segs.push((*edge, w[0], w[1]));
                    let (lo, hi) = (w[0].min(w[1]), w[0].max(w[1]));
                    for gx in key(lo).0..=key(hi).0 {
                        for gz in key(lo).1..=key(hi).1 {
                            s.grid.entry((gx, gz)).or_default().push(idx);
                        }
                    }
                }
            }
            s
        }

        fn nearest(&self, p: Vec2) -> Option<(i64, f32)> {
            let (kx, kz) = ((p.x / SNAP_M).floor() as i32, (p.y / SNAP_M).floor() as i32);
            let mut best: Option<(i64, f32)> = None;
            for dx in -1..=1 {
                for dz in -1..=1 {
                    for &i in self.grid.get(&(kx + dx, kz + dz)).into_iter().flatten() {
                        let (edge, a, b) = self.segs[i];
                        let ab = b - a;
                        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                        let d = p.distance(a + ab * t);
                        if d <= SNAP_M && best.is_none_or(|(_, bd)| d < bd) {
                            best = Some((edge, d));
                        }
                    }
                }
            }
            best
        }
    }

    /// Same edge and distance as the map of lists, bit for bit, on random
    /// street networks (lines crossing cells, shared points, ties).
    #[test]
    fn compact_snapper_equals_map() {
        use rand::{Rng, SeedableRng};
        let mut r = rand::rngs::StdRng::seed_from_u64(3);
        for _ in 0..10 {
            let lines: Vec<(i64, Vec<Vec2>)> = (0..300)
                .map(|e| {
                    let mut p =
                        Vec2::new(r.random_range(-800.0..800.0), r.random_range(-800.0..800.0));
                    // Some on a coarse lattice: equal distances, ties.
                    if e % 4 == 0 {
                        p = (p / 50.0).round() * 50.0;
                    }
                    let pts = (0..r.random_range(1..8))
                        .map(|_| {
                            p +=
                                Vec2::new(r.random_range(-90.0..90.0), r.random_range(-90.0..90.0));
                            p
                        })
                        .collect();
                    (1000 + e, pts)
                })
                .collect();
            let (old, new) = (MapSnapper::new(&lines), Snapper::new(&lines));
            for _ in 0..5000 {
                let p = Vec2::new(r.random_range(-900.0..900.0), r.random_range(-900.0..900.0));
                let (a, b) = (old.nearest(p), new.nearest(p));
                assert_eq!(
                    a.map(|(e, d)| (e, d.to_bits())),
                    b.map(|(e, d)| (e, d.to_bits())),
                    "{p:?}"
                );
            }
        }
    }
}

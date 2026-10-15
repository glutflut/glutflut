//! Tide as a rule (`[tide]`): a region with a
//! gauge (`geo.region_tide`) and junction distances to tidal open water
//! (`geo.node_tide`) leans to water at the waterfront at high tide and to fire
//! at low tide, through the natural sources and ageing like live rain.
//! A host thread reads each gauge every 5 minutes, always in the terrain's datum:
//! `pegelonline` (WSV, m NHN = W/100 + gauge zero, DL-DE Zero 2.0), `noaa`
//! (CO-OPS, NAVD88 like 3DEP, public domain), `ea` (Environment Agency, mAOD
//! like its lidar, OGL v3), `imi` (Marine Institute, OD Malin like the OPW
//! lidar, CC BY 4.0). Stale levels, implausible jumps or a storm surge switch
//! the rule off.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bevy::prelude::*;
use elements_shared::geo::GeoNode;

use crate::sim::ElemIdx;
use crate::weather::BaseRules;

const EVERY: Duration = Duration::from_secs(300);
/// Older levels don't count (gauge or fetch down).
const MAX_AGE_S: f64 = 1800.0;
/// The EA publishes 15-minute values 15-30 minutes late.
const MAX_AGE_EA_S: f64 = 3600.0;
/// A jump this big between two readings 5 minutes apart is a fault.
const MAX_JUMP_M: f32 = 1.0;
/// Phase changes below this don't rebuild the field.
const STEP: f32 = 0.02;
/// Safety: nothing sends players this close to tidal water (sparks, automatic
/// hotspots), whether the rule is on or not.
pub const SHORE_M: f32 = 30.0;

/// A region's gauge and its junctions near tidal water (loaded with the region).
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub source: String,
    pub station: String,
    pub mlw: f32,
    pub mhw: f32,
    /// Node id -> distance to tidal open water (m).
    pub dist: HashMap<i64, f32>,
}

impl Setup {
    /// Junctions too close to the water to send anyone there.
    pub fn shore(&self) -> HashSet<i64> {
        self.dist
            .iter()
            .filter(|(_, d)| **d < SHORE_M)
            .map(|(id, _)| *id)
            .collect()
    }
}

/// A gauge reading: unix seconds, level (m, the terrain's datum).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    t: f64,
    level: f32,
}

#[derive(Default)]
struct Gauges {
    wanted: HashSet<(String, String)>,
    zero: HashMap<String, f32>,
    last: HashMap<String, Reading>,
    /// Forecast highs and lows where the source has one (NOAA): fetched at,
    /// then (unix s, high?) in time order.
    hilo: HashMap<String, (f64, Vec<(f64, bool)>)>,
    /// The last [`HIST_S`] of readings, to estimate highs and lows where the
    /// source has no forecast (PEGELONLINE).
    hist: HashMap<String, Vec<Reading>>,
}

/// Readings kept per gauge: two tides, so the last high and low are always in.
const HIST_S: f64 = 26.0 * 3600.0;
/// Mean tidal period (M2, 12 h 25 min).
const TIDE_PERIOD_S: f64 = 44_712.0;
/// A high (low) is the highest (lowest) reading within ± this.
const EXTREME_WINDOW_S: f64 = 2.0 * 3600.0;

/// Add a reading to the gauge's history (time order, trimmed).
fn remember(g: &mut Gauges, station: &str, r: Reading) {
    let h = g.hist.entry(station.into()).or_default();
    if h.last().is_none_or(|l| r.t > l.t) {
        h.push(r);
    }
    let cut = r.t - HIST_S;
    h.retain(|x| x.t >= cut);
}

/// The last high and low in `h` (unix s): extremes within ± [`EXTREME_WINDOW_S`],
/// so the newest are found two hours after the turn.
fn last_extremes(h: &[Reading]) -> (Option<f64>, Option<f64>) {
    let (Some(first), Some(end)) = (h.first(), h.last()) else {
        return (None, None);
    };
    let w = EXTREME_WINDOW_S;
    let (mut hi, mut lo) = (None::<f64>, None::<f64>);
    let (mut a, mut b) = (0, 0);
    for r in h {
        while h[a].t < r.t - w {
            a += 1;
        }
        while b < h.len() && h[b].t <= r.t + w {
            b += 1;
        }
        if r.t - w < first.t || r.t + w > end.t {
            continue;
        }
        let win = &h[a..b];
        if win.iter().all(|x| x.level <= r.level) && hi.is_none_or(|t| r.t - t > w) {
            hi = Some(r.t);
        } else if win.iter().all(|x| x.level >= r.level) && lo.is_none_or(|t| r.t - t > w) {
            lo = Some(r.t);
        }
    }
    (hi, lo)
}

/// The first `t + k × period` after `now`.
fn next_after(t: f64, now: f64) -> f64 {
    t + ((now - t) / TIDE_PERIOD_S).floor().max(0.0) * TIDE_PERIOD_S + TIDE_PERIOD_S
}

fn gauges() -> &'static Mutex<Gauges> {
    static G: OnceLock<Mutex<Gauges>> = OnceLock::new();
    G.get_or_init(Mutex::default)
}

/// Ask the host thread for this gauge (started on first use).
fn want(source: &str, station: &str) {
    gauges()
        .lock()
        .unwrap()
        .wanted
        .insert((source.into(), station.into()));
    static THREAD: OnceLock<()> = OnceLock::new();
    THREAD.get_or_init(|| {
        let _ = std::thread::Builder::new().name("tide".into()).spawn(run);
    });
}

/// The newest plausible reading of a gauge, if fresh.
fn reading(source: &str, station: &str) -> Option<Reading> {
    let g = gauges().lock().unwrap();
    let r = *g.last.get(station)?;
    let max = if source == "ea" {
        MAX_AGE_EA_S
    } else {
        MAX_AGE_S
    };
    (crate::analytics::now_s() - r.t <= max).then_some(r)
}

/// Refresh the forecast highs and lows this often.
const HILO_EVERY_S: f64 = 6.0 * 3600.0;

/// The next high and low water after `now` (unix s): the forecast where
/// there is one, else estimated from the last ones plus the tidal period
/// (true: estimated).
fn next_hilo(station: &str, now: f64) -> (Option<i64>, Option<i64>, bool) {
    let g = gauges().lock().unwrap();
    if let Some((_, v)) = g.hilo.get(station) {
        let next = |high: bool| {
            v.iter()
                .find(|(t, h)| *h == high && *t > now)
                .map(|(t, _)| *t as i64)
        };
        return (next(true), next(false), false);
    }
    let Some(h) = g.hist.get(station) else {
        return (None, None, false);
    };
    let (hi, lo) = last_extremes(h);
    let est = |t: Option<f64>| t.map(|t| next_after(t, now) as i64);
    (est(hi), est(lo), true)
}

/// What the app shows (`ServerMsg::Tide`): off, or the phase and its direction.
#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, ts_rs::TS)]
#[ts(rename = "TideShown")]
pub struct Shown {
    pub on: bool,
    /// -1 mean low water .. +1 mean high water.
    pub phase: f32,
    pub rising: bool,
    /// Gauge level (m, the terrain's datum).
    pub level_m: f32,
    /// Next forecast high / low water (unix s), where the source has a forecast.
    #[ts(type = "number | null")]
    pub next_high: Option<i64>,
    #[ts(type = "number | null")]
    pub next_low: Option<i64>,
    /// The two above are estimated from the last tides (no forecast).
    pub hilo_estimated: bool,
}

fn agent() -> Option<ureq::Agent> {
    let tls = native_tls::TlsConnector::new()
        .map_err(|e| warn!(error = %e, "tide: no TLS"))
        .ok()?;
    Some(
        ureq::AgentBuilder::new()
            .tls_connector(Arc::new(tls))
            .timeout(Duration::from_secs(30))
            .user_agent("glutflut.com")
            .build(),
    )
}

const PEGEL: &str = "https://www.pegelonline.wsv.de/webservices/rest-api/v2/stations";

fn run() {
    let Some(agent) = agent() else { return };
    loop {
        let wanted: Vec<(String, String)> =
            gauges().lock().unwrap().wanted.iter().cloned().collect();
        for (source, station) in wanted {
            let r = match source.as_str() {
                "pegelonline" => read_pegel(&agent, &station),
                "noaa" => read_noaa(&agent, &station),
                "ea" => read_ea(&agent, &station),
                "imi" => read_imi(&agent, &station),
                _ => Err(format!("unknown source {source}").into()),
            };
            if source == "pegelonline" && !gauges().lock().unwrap().hist.contains_key(&station) {
                // Seed the history once, so highs and lows show right after a start.
                match pegel_history(&agent, &station) {
                    Ok(v) => {
                        let mut g = gauges().lock().unwrap();
                        for r in v {
                            remember(&mut g, &station, r);
                        }
                    }
                    Err(e) => warn!(station, error = %e, "tide: gauge history read failed"),
                }
            }
            match r {
                Ok(r) => {
                    let mut g = gauges().lock().unwrap();
                    let jump = g.last.get(&station).is_some_and(|o| {
                        (r.level - o.level).abs() > MAX_JUMP_M && r.t - o.t < 900.0
                    });
                    if jump {
                        warn!(station, level = r.level, "tide: implausible jump; ignored");
                    } else {
                        g.last.insert(station.clone(), r);
                        remember(&mut g, &station, r);
                    }
                }
                Err(e) => warn!(station, error = %e, "tide: gauge read failed"),
            }
            if source == "noaa" {
                let now = crate::analytics::now_s();
                let due = gauges()
                    .lock()
                    .unwrap()
                    .hilo
                    .get(&station)
                    .is_none_or(|(at, _)| now - at > HILO_EVERY_S);
                if due {
                    match noaa_hilo(&agent, &station) {
                        Ok(v) => {
                            gauges().lock().unwrap().hilo.insert(station, (now, v));
                        }
                        Err(e) => warn!(station, error = %e, "tide: forecast read failed"),
                    }
                }
            }
        }
        std::thread::sleep(EVERY);
    }
}

/// The last 26 hours of a PEGELONLINE gauge (after `read_pegel`: the gauge zero is known).
fn pegel_history(
    agent: &ureq::Agent,
    station: &str,
) -> Result<Vec<Reading>, Box<dyn std::error::Error>> {
    let zero = gauges()
        .lock()
        .unwrap()
        .zero
        .get(station)
        .copied()
        .ok_or("no gauge zero yet")?;
    let m: serde_json::Value = agent
        .get(&format!(
            "{PEGEL}/{station}/W/measurements.json?start=PT26H"
        ))
        .call()?
        .into_json()?;
    let mut v = Vec::new();
    for x in m.as_array().ok_or("no measurements")? {
        let (Some(t), Some(cm)) = (x["timestamp"].as_str(), x["value"].as_f64()) else {
            continue;
        };
        let t: jiff::Timestamp = t.parse()?;
        v.push(Reading {
            t: t.as_second() as f64,
            level: cm as f32 / 100.0 + zero,
        });
    }
    v.sort_by(|a, b| a.t.total_cmp(&b.t));
    Ok(v)
}

fn read_pegel(agent: &ureq::Agent, station: &str) -> Result<Reading, Box<dyn std::error::Error>> {
    if !station.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("station: a PEGELONLINE uuid".into());
    }
    let zero = gauges().lock().unwrap().zero.get(station).copied();
    let zero = match zero {
        Some(z) => z,
        None => {
            let s: serde_json::Value = agent
                .get(&format!("{PEGEL}/{station}.json?includeTimeseries=true"))
                .call()?
                .into_json()?;
            let z = s["timeseries"]
                .as_array()
                .and_then(|t| t.iter().find(|t| t["shortname"] == "W"))
                .and_then(|t| t["gaugeZero"]["value"].as_f64())
                .ok_or("no gauge zero")? as f32;
            gauges().lock().unwrap().zero.insert(station.into(), z);
            z
        }
    };
    let m: serde_json::Value = agent
        .get(&format!("{PEGEL}/{station}/W/currentmeasurement.json"))
        .call()?
        .into_json()?;
    let cm = m["value"].as_f64().ok_or("no value")? as f32;
    let t: jiff::Timestamp = m["timestamp"].as_str().ok_or("no time")?.parse()?;
    Ok(Reading {
        t: t.as_second() as f64,
        level: cm / 100.0 + zero,
    })
}

fn read_noaa(agent: &ureq::Agent, station: &str) -> Result<Reading, Box<dyn std::error::Error>> {
    if !station.chars().all(|c| c.is_ascii_digit()) {
        return Err("station: a NOAA station number".into());
    }
    let m: serde_json::Value = agent
        .get("https://api.tidesandcurrents.noaa.gov/api/prod/datagetter")
        .query_pairs([
            ("date", "latest"),
            ("station", station),
            ("product", "water_level"),
            ("datum", "NAVD"),
            ("units", "metric"),
            ("time_zone", "gmt"),
            ("format", "json"),
            ("application", "glutflut"),
        ])
        .call()?
        .into_json()?;
    let d = &m["data"][0];
    let level: f32 = d["v"].as_str().ok_or("no value")?.parse()?;
    Ok(Reading {
        t: noaa_time(d["t"].as_str().ok_or("no time")?)?,
        level,
    })
}

/// NOAA "2026-10-12 16:24" (GMT) as unix seconds.
fn noaa_time(s: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let t: jiff::Timestamp = format!("{}:00Z", s.replace(' ', "T")).parse()?;
    Ok(t.as_second() as f64)
}

/// Forecast highs and lows for the next two days.
fn noaa_hilo(
    agent: &ureq::Agent,
    station: &str,
) -> Result<Vec<(f64, bool)>, Box<dyn std::error::Error>> {
    if !station.chars().all(|c| c.is_ascii_digit()) {
        return Err("station: a NOAA station number".into());
    }
    let day = jiff::Timestamp::now().strftime("%Y%m%d").to_string();
    let m: serde_json::Value = agent
        .get("https://api.tidesandcurrents.noaa.gov/api/prod/datagetter")
        .query_pairs([
            ("begin_date", day.as_str()),
            ("range", "48"),
            ("station", station),
            ("product", "predictions"),
            ("interval", "hilo"),
            ("datum", "NAVD"),
            ("units", "metric"),
            ("time_zone", "gmt"),
            ("format", "json"),
            ("application", "glutflut"),
        ])
        .call()?
        .into_json()?;
    let mut v = Vec::new();
    for p in m["predictions"].as_array().ok_or("no predictions")? {
        let t = noaa_time(p["t"].as_str().ok_or("no time")?)?;
        v.push((t, p["type"].as_str() == Some("H")));
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(v)
}

fn read_ea(agent: &ureq::Agent, station: &str) -> Result<Reading, Box<dyn std::error::Error>> {
    if !station.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("station: an EA station reference".into());
    }
    let m: serde_json::Value = agent
        .get(&format!(
            "https://environment.data.gov.uk/flood-monitoring/id/stations/{station}/readings?latest"
        ))
        .call()?
        .into_json()?;
    // The newest of the station's mAOD measures (Tower Pier also has 1-minute values).
    let r = m["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["measure"].as_str().is_some_and(|s| s.ends_with("-mAOD")))
        .filter_map(|i| {
            let t: jiff::Timestamp = i["dateTime"].as_str()?.parse().ok()?;
            Some(Reading {
                t: t.as_second() as f64,
                level: i["value"].as_f64()? as f32,
            })
        })
        .max_by(|a, b| a.t.total_cmp(&b.t))
        .ok_or("no mAOD reading")?;
    Ok(r)
}

fn read_imi(agent: &ureq::Agent, station: &str) -> Result<Reading, Box<dyn std::error::Error>> {
    // "Dublin_Port" for the station id "Dublin Port".
    if !station
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("station: letters, digits, _".into());
    }
    let id = station.replace('_', " ");
    let m: serde_json::Value = agent
        .get(&format!(
            "https://erddap.marine.ie/erddap/tabledap/IrishNationalTideGaugeNetwork.json\
             ?time,Water_Level_OD_Malin&station_id=%22{}%22&time%3E=now-1hour",
            id.replace(' ', "%20")
        ))
        .call()?
        .into_json()?;
    let row = m["table"]["rows"]
        .as_array()
        .and_then(|r| r.iter().rev().find(|r| r[1].is_number()))
        .ok_or("no level in the last hour")?;
    let t: jiff::Timestamp = row[0].as_str().ok_or("no time")?.parse()?;
    Ok(Reading {
        t: t.as_second() as f64,
        level: row[1].as_f64().ok_or("no value")? as f32,
    })
}

/// Tide intensity per junction: phase (-1 low .. +1 high) × weight.
#[derive(Resource, Default)]
pub struct LiveTide {
    setup: Option<Setup>,
    /// Per junction index: (distance m, elevation m) for those near the water.
    near: Vec<(usize, f32, f32)>,
    /// Junction indices within [`SHORE_M`].
    shore: HashSet<usize>,
    k: Vec<f32>,
    pub on: bool,
    /// Changes whenever `k` or `on` do (`weather::apply` follows it).
    pub version: u32,
    phase: Option<f32>,
    /// The last two distinct readings: is the water rising?
    last: Option<Reading>,
    rising: bool,
}

impl LiveTide {
    pub fn new(setup: Option<Setup>) -> Self {
        if let Some(s) = &setup {
            info!(station = %s.station, junctions = s.dist.len(), mlw = s.mlw, mhw = s.mhw, "tide: gauge");
            want(&s.source, &s.station);
        }
        Self {
            setup,
            ..Default::default()
        }
    }

    /// Intensity at junction `i` (0 while off).
    pub fn at(&self, i: usize) -> f32 {
        if self.on {
            self.k.get(i).copied().unwrap_or(0.0)
        } else {
            0.0
        }
    }

    fn off(&mut self, why: &str) {
        if self.on {
            self.on = false;
            self.phase = None;
            self.version += 1;
            info!("tide: off ({why})");
        }
    }

    /// Junction `i` lies within [`SHORE_M`] of tidal water.
    pub fn shore(&self, i: usize) -> bool {
        self.shore.contains(&i)
    }

    /// For the app: `None` in regions without a gauge.
    pub fn shown(&self) -> Option<Shown> {
        let s = self.setup.as_ref()?;
        let (next_high, next_low, hilo_estimated) =
            next_hilo(&s.station, crate::analytics::now_s());
        Some(Shown {
            on: self.on,
            phase: self.phase.unwrap_or(0.0),
            rising: self.rising,
            level_m: self.last.map_or(0.0, |r| r.level),
            next_high,
            next_low,
            hilo_estimated,
        })
    }

    /// Recompute from a reading (None: no fresh one).
    fn step(&mut self, rules: &elements_shared::tide::Tide, r: Option<Reading>, n: usize) {
        let Some(s) = &self.setup else { return };
        let Some(r) = r else {
            return self.off("no current level");
        };
        if self.last.is_none_or(|l| l.t != r.t) {
            if let Some(l) = self.last {
                self.rising = r.level > l.level;
            }
            self.last = Some(r);
        }
        let level = r.level;
        let Some(phase) = rules.phase(level, s.mlw, s.mhw) else {
            return self.off("storm surge");
        };
        if self.on && self.phase.is_some_and(|p| (p - phase).abs() < STEP) {
            return;
        }
        let mut k = vec![0.0; n];
        for &(i, d, e) in &self.near {
            if let Some(x) = k.get_mut(i) {
                *x = phase * rules.weight(d, e, s.mhw);
            }
        }
        if !self.on {
            info!(level, phase, "tide: on");
        }
        self.k = k;
        self.phase = Some(phase);
        self.on = true;
        self.version += 1;
    }
}

/// Follow the gauge (every 30 s; the reading changes every 5 minutes).
pub fn update(
    base: Res<BaseRules>,
    mut tide: ResMut<LiveTide>,
    nodes: Query<(&GeoNode, &ElemIdx)>,
    mut next: Local<f64>,
) {
    let rules = &base.0.tide;
    if tide.setup.is_none() {
        return;
    }
    let now = crate::analytics::now_s();
    if now < *next {
        return;
    }
    *next = now + 30.0;
    if tide.near.is_empty() {
        let dist = &tide.setup.as_ref().expect("setup").dist;
        let near: Vec<(usize, f32, f32)> = nodes
            .iter()
            .filter_map(|(g, i)| dist.get(&g.id).map(|&d| (i.0, d, g.elevation_m as f32)))
            .collect();
        if near.is_empty() {
            return;
        }
        tide.shore = near.iter().filter(|n| n.1 < SHORE_M).map(|n| n.0).collect();
        tide.near = near;
    }
    if !rules.enabled() {
        return;
    }
    let n = nodes.iter().map(|(_, i)| i.0 + 1).max().unwrap_or(0);
    let s = tide.setup.as_ref().expect("setup");
    let r = reading(&s.source, &s.station);
    tide.step(rules, r, n);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tide() -> LiveTide {
        LiveTide {
            setup: Some(Setup {
                source: "pegelonline".into(),
                station: "x".into(),
                mlw: -1.5,
                mhw: 2.0,
                dist: HashMap::new(),
            }),
            near: vec![(0, 0.0, 3.0), (2, 200.0, 3.0)],
            ..Default::default()
        }
    }

    #[test]
    fn phase_times_weight_off_without_level_or_in_a_surge() {
        let r = elements_shared::tide::Tide {
            strength: 0.5,
            decay: 0.3,
            ..Default::default()
        };
        let mut t = tide();
        let rd = |level, t| Some(Reading { t, level });
        t.step(&r, rd(2.0, 1.0), 4);
        assert!(t.on);
        assert_eq!(t.at(0), 1.0);
        assert!((t.at(2) - 0.5).abs() < 1e-6);
        assert_eq!((t.at(1), t.at(3)), (0.0, 0.0), "inland: nothing");
        let v = t.version;
        t.step(&r, rd(2.01, 2.0), 4);
        assert_eq!(t.version, v, "small change: same field");
        assert!(t.shown().unwrap().rising);
        t.step(&r, rd(-1.5, 3.0), 4);
        assert_eq!(t.at(0), -1.0, "low water: towards fire");
        assert!(!t.shown().unwrap().rising);
        t.step(&r, rd(4.0, 4.0), 4);
        assert!(!t.on && t.at(0) == 0.0, "storm surge: off");
        t.step(&r, rd(2.0, 5.0), 4);
        t.step(&r, None, 4);
        assert!(!t.on, "no level: off");
    }

    /// `cargo test -p elements-server live_gauges -- --ignored --nocapture`
    #[test]
    #[ignore = "network"]
    fn live_gauges() {
        let agent = agent().unwrap();
        let now = crate::analytics::now_s();
        for (src, r) in [
            (
                "pegelonline",
                read_pegel(&agent, "b26bfc88-eb8a-45b9-9dd0-0e41949a2648"),
            ),
            ("noaa", read_noaa(&agent, "8518750")),
            ("ea", read_ea(&agent, "0007")),
            ("imi", read_imi(&agent, "Dublin_Port")),
        ] {
            let r = r.unwrap_or_else(|e| panic!("{src}: {e}"));
            println!("{src}: {:.2} m, {:.0} min old", r.level, (now - r.t) / 60.0);
            assert!(r.level.abs() < 10.0 && now - r.t < 7200.0, "{src}");
        }
        let st = "d488c5cc-4de9-4631-8ce1-0db0e700b546"; // Hamburg St. Pauli
        read_pegel(&agent, st).unwrap();
        let h = pegel_history(&agent, st).unwrap();
        let (hi, lo) = last_extremes(&h);
        let fmt = |t: f64| jiff::Timestamp::from_second(t as i64).unwrap().to_string();
        println!(
            "pegelonline history: {} readings; last high {}, low {}; next high ~{}, low ~{}",
            h.len(),
            fmt(hi.unwrap()),
            fmt(lo.unwrap()),
            fmt(next_after(hi.unwrap(), now)),
            fmt(next_after(lo.unwrap(), now))
        );
        let hl = noaa_hilo(&agent, "8518750").unwrap();
        println!(
            "noaa forecast: {} highs/lows, first {:.0} min from now",
            hl.len(),
            (hl[0].0 - now) / 60.0
        );
        assert!(hl.len() >= 6 && hl.iter().any(|x| x.1) && hl.iter().any(|x| !x.1));
    }

    #[test]
    fn estimated_highs_and_lows() {
        // A sine tide with the M2 period, high at t = 3 h, readings every 5 min.
        let h: Vec<Reading> = (0..=(26 * 12))
            .map(|i| {
                let t = i as f64 * 300.0;
                let ph = 2.0 * std::f64::consts::PI * (t - 3.0 * 3600.0) / TIDE_PERIOD_S;
                Reading {
                    t,
                    level: (2.0 * ph.cos()) as f32,
                }
            })
            .collect();
        let (hi, lo) = last_extremes(&h);
        let hi = hi.unwrap();
        let lo = lo.unwrap();
        assert!(
            (hi - (3.0 * 3600.0 + TIDE_PERIOD_S)).abs() <= 300.0,
            "last high {hi}"
        );
        assert!(
            (lo - (3.0 * 3600.0 + 1.5 * TIDE_PERIOD_S)).abs() <= 300.0,
            "last low {lo}"
        );
        let now = 26.0 * 3600.0;
        let next = next_after(hi, now);
        assert!(next > now && next - now <= TIDE_PERIOD_S);
        assert!((next - (3.0 * 3600.0 + 2.0 * TIDE_PERIOD_S)).abs() <= 300.0);
    }

    #[test]
    fn pegelonline_time() {
        let t: jiff::Timestamp = "2026-10-11T18:30:00+02:00".parse().unwrap();
        assert_eq!(t.as_second(), 1_791_736_200);
    }
}

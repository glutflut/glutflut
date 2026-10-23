//! Campaigns: chapters on
//! weekends (`game.chapters`), each a race between the sides towards a goal.
//! Only totals per side are stored (`game.chapter_progress`); taking part is
//! a badge (`chapter:<key>`), winning one more (`chapter-win:<key>`).
//! A worker thread reads the chapters every few minutes and writes progress,
//! so the tick never waits for the database.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use serde::Serialize;

use elements_shared::prelude::*;
use elements_shared::weather::WeatherDay;

const RELOAD: Duration = Duration::from_secs(300);
/// Weather: a failed forecast is tried again after this long.
const WEATHER_RETRY: Duration = Duration::from_secs(1800);
const SAVE_EVERY_S: f64 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "ChapterGoal")]
pub enum Goal {
    Claims,
    Fronts,
    Finds,
    Points,
    /// Hold area (ha) with players' sources on the border; progress is the
    /// most a side held at once.
    Area,
    /// Hold the chapter's junctions (`nodes`): progress is how many a side
    /// holds now; who holds more at the end wins.
    Hold,
}

impl Goal {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "claims" => Self::Claims,
            "fronts" => Self::Fronts,
            "finds" => Self::Finds,
            "points" => Self::Points,
            "area" => Self::Area,
            "hold" => Self::Hold,
            _ => return None,
        })
    }
}

#[derive(Clone, Serialize, ts_rs::TS)]
pub struct Chapter {
    pub key: String,
    pub campaign: String,
    /// `{lang: {title, text}}`
    #[ts(type = "Record<string, { title: string; text: string }>")]
    pub names: serde_json::Value,
    #[serde(skip)]
    pub starts: f64,
    #[serde(skip)]
    pub ends: f64,
    pub goal: Goal,
    /// Targets of fire and water (the same in a campaign).
    pub target: i64,
    pub target_water: i64,
    /// `weather:<kind>` for a weather day (the app names it), else a campaign.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<String>,
    /// Hold: the junctions (geo.nodes ids) and where they are ([lon, lat]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, type = "number[]")]
    pub nodes: Vec<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[ts(optional, type = "Array<[number, number]>")]
    pub at: Vec<[f64; 2]>,
    /// The story: `{lang: {intro, fire, water, tie}}` (all optional); the
    /// intro when it starts, the ending for the outcome when it ends.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    #[ts(
        optional,
        type = "Record<string, { intro?: string; fire?: string; water?: string; tie?: string }>"
    )]
    pub story: serde_json::Value,
}

/// A news flash of the region (`game.notices`), shown while it runs.
#[derive(Clone, PartialEq, Serialize, Debug, ts_rs::TS)]
pub struct Notice {
    pub id: i64,
    /// `{lang: {title, text}}`
    #[ts(type = "Record<string, { title: string; text: string }>")]
    pub names: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
}

/// First side to its target wins; if neither got there, after the end the
/// one closer to its target (cross-multiplied); a tie has no winner.
pub(crate) fn decide(
    progress: [i64; 2],
    reached: [Option<f64>; 2],
    targets: [i64; 2],
    ended: bool,
) -> Option<Faction> {
    let side = |i: usize| {
        if i == 0 {
            Faction::Fire
        } else {
            Faction::Water
        }
    };
    match reached {
        [Some(a), Some(b)] => Some(side(usize::from(b < a))),
        [Some(_), None] => Some(Faction::Fire),
        [None, Some(_)] => Some(Faction::Water),
        [None, None] if ended => {
            let f = progress[0] * targets[1].max(1);
            let w = progress[1] * targets[0].max(1);
            (f != w).then(|| side(usize::from(w > f)))
        }
        _ => None,
    }
}

impl Chapter {
    fn target_of(&self, i: usize) -> i64 {
        if i == 0 {
            self.target
        } else {
            self.target_water
        }
    }

    /// Campaign chapters give badges; weather days (daily) don't.
    pub fn badges(&self) -> bool {
        self.kind.is_none()
    }
}

/// An approved phrase of a running campaign (`game.campaign_mottos`).
#[derive(Clone, Debug)]
pub struct CityMotto {
    pub num: u16,
    /// None: both sides.
    pub side: Option<Faction>,
    /// `{lang: text}`
    pub names: serde_json::Value,
}

/// What the worker read.
struct Loaded {
    chapter: Option<Chapter>,
    progress: [i64; 2],
    reached: [Option<f64>; 2],
    /// Finished chapters of this region and their winners.
    past: Vec<(String, Faction)>,
    /// Today's weather (stored once a day).
    weather: Option<WeatherDay>,
    /// The latest campaign start or end that has passed (unix s).
    boundary: Option<f64>,
    mottos: Vec<CityMotto>,
    periods: Vec<(f64, f64)>,
    /// Campaigns of the region: slug, first start, last end.
    campaigns: Vec<(String, f64, f64)>,
    /// Notices running now.
    notices: Vec<Notice>,
}

struct Save {
    key: String,
    progress: [i64; 2],
    reached: [Option<f64>; 2],
    /// Hold: the value is replaced, not raised.
    hold: bool,
}

#[derive(Resource)]
pub struct Campaign {
    /// The current or next chapter.
    pub chapter: Option<Chapter>,
    pub progress: [i64; 2],
    /// Unix seconds when a side reached the target.
    pub reached: [Option<f64>; 2],
    pub past: Vec<(String, Faction)>,
    /// Today's weather, if known (the weather rule and the app's display).
    pub weather: Option<WeatherDay>,
    /// The latest campaign start or end that has passed (unix s): levels
    /// start again there (`Scores::campaign_reset`), as permanent sources do.
    pub boundary: Option<f64>,
    /// Approved phrases of the running campaign(s).
    pub mottos: Vec<CityMotto>,
    /// Campaigns of the region, first start to last end (unix s): streak
    /// weeks outside them pause.
    pub periods: Vec<(f64, f64)>,
    /// The same with the campaign's slug (finds bound to a campaign).
    pub campaigns: Vec<(String, f64, f64)>,
    /// News flashes running now.
    pub notices: Vec<Notice>,
    /// The notices changed: send them to the apps.
    pub notices_changed: bool,
    /// The chapter whose end was handled (winner badges).
    finished: Option<String>,
    dirty: bool,
    /// Something to send to the apps.
    pub changed: bool,
    save_at: f64,
    tx: Option<crate::writers::LaneTx<Save>>,
    rx: Option<Receiver<Loaded>>,
}

impl Campaign {
    pub fn spawn(db: Option<String>, region: String) -> Self {
        let mut c = Self {
            chapter: None,
            progress: [0; 2],
            reached: [None; 2],
            past: Vec::new(),
            weather: None,
            boundary: None,
            mottos: Vec::new(),
            periods: Vec::new(),
            campaigns: Vec::new(),
            notices: Vec::new(),
            notices_changed: false,
            finished: None,
            dirty: false,
            changed: false,
            save_at: 0.0,
            tx: None,
            rx: None,
        };
        let Some(db) = db else { return c };
        let (load_tx, load_rx) = crossbeam_channel::unbounded();
        let key = format!("campaign {region}");
        c.tx = Some(crate::writers::lane(&key, |saves| Worker {
            db,
            region,
            saves,
            out: load_tx,
            fetched_at: None,
            reload_at: Instant::now(),
        }));
        c.rx = Some(load_rx);
        c
    }

    /// The chapter running at `now`.
    pub fn active(&self, now: f64) -> Option<&Chapter> {
        self.chapter
            .as_ref()
            .filter(|c| c.starts <= now && now < c.ends)
    }

    /// Slugs of the campaigns running at `now` (first start to last end).
    pub fn running(&self, now: f64) -> Vec<String> {
        self.campaigns
            .iter()
            .filter(|(_, s, e)| *s <= now && now < *e)
            .map(|(c, _, _)| c.clone())
            .collect()
    }

    /// Count `n` for `faction` if the running chapter's goal is `goal`.
    /// Returns the chapter key when it counted and gives badges.
    pub fn add(&mut self, faction: Faction, goal: Goal, n: i64, now: f64) -> Option<String> {
        let ch = self.active(now).filter(|c| c.goal == goal && n > 0)?;
        let i = faction.index();
        let (key, target) = (ch.badges().then(|| ch.key.clone()), ch.target_of(i));
        self.progress[i] += n;
        if self.progress[i] >= target && self.reached[i].is_none() {
            self.reached[i] = Some(now);
        }
        self.dirty = true;
        self.changed = true;
        key
    }

    /// Area held now (ha, players' blocks only): the side's best counts.
    /// Returns the chapter key when it counted (participation badges).
    pub fn hold(&mut self, faction: Faction, ha: f64, now: f64) -> Option<String> {
        let ch = self
            .active(now)
            .filter(|c| c.goal == Goal::Area && ha > 0.0)?;
        let i = faction.index();
        let (key, target) = (ch.badges().then(|| ch.key.clone()), ch.target_of(i));
        let ha = ha.floor() as i64;
        if ha > self.progress[i] {
            self.progress[i] = ha;
            self.dirty = true;
            self.changed = true;
        }
        if self.progress[i] >= target && self.reached[i].is_none() {
            self.reached[i] = Some(now);
            self.dirty = true;
            self.changed = true;
        }
        key
    }

    /// Hold: how many of the chapter's junctions each side holds now.
    pub fn held(&mut self, n: [i64; 2], now: f64) {
        let on = self.active(now).is_some_and(|c| c.goal == Goal::Hold);
        if on && n != self.progress {
            self.progress = n;
            self.dirty = true;
            self.changed = true;
        }
    }

    /// The running hold chapter's junctions.
    pub fn hold_nodes(&self, now: f64) -> Option<&[i64]> {
        self.active(now)
            .filter(|c| c.goal == Goal::Hold)
            .map(|c| c.nodes.as_slice())
    }

    /// A claim on one of the hold chapter's junctions: its key (badge).
    pub fn hold_claim(&self, node: i64, now: f64) -> Option<String> {
        self.active(now)
            .filter(|c| c.goal == Goal::Hold && c.badges() && c.nodes.contains(&node))
            .map(|c| c.key.clone())
    }

    /// First side to the target; after the end, else the one closer to its target.
    pub fn winner(&self, now: f64) -> Option<Faction> {
        let ch = self.chapter.as_ref()?;
        decide(
            self.progress,
            self.reached,
            [ch.target, ch.target_water],
            now >= ch.ends,
        )
    }

    /// Take what the worker read (our own progress is never older).
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok(l) = rx.try_recv() {
            let same = matches!((&self.chapter, &l.chapter), (Some(a), Some(b)) if a.key == b.key);
            // Hold counts what is held now: ours is the live one.
            let hold = l.chapter.as_ref().is_some_and(|c| c.goal == Goal::Hold);
            if same {
                for i in 0..2 {
                    if !hold {
                        self.progress[i] = self.progress[i].max(l.progress[i]);
                    }
                    self.reached[i] = self.reached[i].or(l.reached[i]);
                }
            } else {
                self.progress = l.progress;
                self.reached = l.reached;
            }
            self.chapter = l.chapter;
            self.past = l.past;
            self.weather = l.weather.or(self.weather);
            self.boundary = l.boundary.or(self.boundary);
            self.mottos = l.mottos;
            self.periods = l.periods;
            self.campaigns = l.campaigns;
            if self.notices != l.notices {
                self.notices = l.notices;
                self.notices_changed = true;
            }
            self.changed = true;
        }
    }

    pub fn save(&mut self, now: f64, force: bool) {
        if !self.dirty || (!force && now < self.save_at) {
            return;
        }
        self.save_at = now + SAVE_EVERY_S;
        self.dirty = false;
        if let (Some(tx), Some(ch)) = (&self.tx, &self.chapter) {
            let _ = tx.send(Save {
                key: ch.key.clone(),
                progress: self.progress,
                reached: self.reached,
                hold: ch.goal == Goal::Hold,
            });
        }
    }

    /// The chapter just ended (once): its key and winner (for badges: only
    /// campaign chapters).
    pub fn take_end(&mut self, now: f64) -> Option<(String, Option<Faction>)> {
        let ch = self.chapter.as_ref()?;
        if now < ch.ends || self.finished.as_deref() == Some(ch.key.as_str()) {
            return None;
        }
        let (key, badges) = (ch.key.clone(), ch.badges());
        self.finished = Some(key.clone());
        let w = self.winner(now).filter(|_| badges);
        if let Some(w) = w {
            self.past.push((key.clone(), w));
        }
        self.save(now, true);
        self.changed = true;
        Some((key, w))
    }
}

/// The region's campaign worker (a lane on the shared writer threads): loads
/// chapters and the weather day every `RELOAD`, saves progress.
struct Worker {
    db: String,
    region: String,
    saves: Receiver<Save>,
    out: Sender<Loaded>,
    fetched_at: Option<Instant>,
    reload_at: Instant,
}

impl crate::writers::Lane for Worker {
    fn run(&mut self, pg: &mut crate::writers::Db) -> bool {
        let db = Some(self.db.as_str());
        if Instant::now() >= self.reload_at {
            self.reload_at = Instant::now() + RELOAD;
            let mut weather = None;
            if let Some(c) = pg.get(db) {
                match weather_day(c, &self.region, &mut self.fetched_at) {
                    Ok(w) => weather = w,
                    Err(e) => warn!(error = %e, region = %self.region, "weather day"),
                }
            }
            if let Some(c) = pg.get(db) {
                match load(c, &self.region) {
                    Ok(mut l) => {
                        l.weather = weather;
                        if self.out.send(l).is_err() {
                            return false;
                        }
                    }
                    Err(e) => pg.failed(db, "campaign load", &e),
                }
            }
        }
        loop {
            match self.saves.try_recv() {
                Ok(s) => {
                    if let Some(c) = pg.get(db) {
                        if let Err(e) = save(c, &self.region, &s) {
                            pg.failed(db, "campaign save", &e);
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => return true,
                Err(crossbeam_channel::TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn due(&self) -> Option<Instant> {
        Some(self.reload_at)
    }
}

/// Today's weather: stored once a day from the first forecast
/// (`game.weather_days`), so a restart doesn't change the day. Then today's
/// weather day (weekdays, 9 to 21 h local), unless a chapter runs then.
fn weather_day(
    c: &mut postgres::Client,
    region: &str,
    fetched_at: &mut Option<Instant>,
) -> Result<Option<WeatherDay>, Box<dyn std::error::Error>> {
    let stored = |c: &mut postgres::Client| {
        c.query_opt(
            "SELECT w.tmax::float8, w.tmin::float8, w.precip_mm::float8, w.sun_h::float8, w.usual_tmax::float8 \
             FROM game.weather_days w JOIN geo.regions r ON r.slug = w.region \
             WHERE w.region = $1 AND w.day = (now() AT TIME ZONE r.timezone)::date",
            &[&region],
        )
        .map(|r| {
            r.map(|r| crate::weather::Day {
                tmax: r.get(0),
                tmin: r.get(1),
                precip_mm: r.get(2),
                sun_h: r.get(3),
                usual_tmax: r.get(4),
            })
        })
    };
    let day = match stored(c)? {
        Some(d) => d,
        None => {
            if fetched_at.is_some_and(|t| t.elapsed() < WEATHER_RETRY) {
                return Ok(None);
            }
            *fetched_at = Some(Instant::now());
            let Some(r) = c.query_opt(
                "SELECT ST_Y(centroid_geom), ST_X(centroid_geom) FROM geo.regions WHERE slug = $1",
                &[&region],
            )?
            else {
                return Ok(None);
            };
            let d = crate::weather::fetch(r.get(0), r.get(1))?;
            c.execute(
                "INSERT INTO game.weather_days (region, day, tmax, tmin, precip_mm, sun_h, usual_tmax) \
                 SELECT $1, (now() AT TIME ZONE r.timezone)::date, \
                        $2::float8, $3::float8, $4::float8, $5::float8, $6::float8 \
                 FROM geo.regions r WHERE r.slug = $1 ON CONFLICT DO NOTHING",
                &[&region, &d.tmax, &d.tmin, &d.precip_mm, &d.sun_h, &d.usual_tmax],
            )?;
            info!(?d, "weather: today");
            // Another server may have stored it first: that one counts.
            stored(c)?.unwrap_or(d)
        }
    };
    let plan = crate::weather::plan(&day);
    // Fire's share of the last two weeks (claims or points), if enough.
    let (col, min) = match plan.goal {
        "claims" => ("claims", 100i64),
        "points" => ("points", 2000),
        _ => ("", 0),
    };
    let share = if col.is_empty() {
        None
    } else {
        let r = c.query_one(
            &format!(
                "SELECT coalesce(sum({col}) FILTER (WHERE faction = 'fire'), 0)::int8, coalesce(sum({col}), 0)::int8 \
                 FROM game.faction_day WHERE region = $1 AND day >= current_date - 14 AND day < current_date"
            ),
            &[&region],
        )?;
        let (fire, all): (i64, i64) = (r.get(0), r.get(1));
        (all >= min).then(|| fire as f64 / all as f64)
    };
    let (fire, water) = crate::weather::targets(plan.total, share);
    c.execute(
        "INSERT INTO game.chapters (region, key, campaign, names, starts, ends, goal, target, target_water, kind) \
         SELECT $1, 'wx-' || d, 'weather', '{}', (d + time '09:00') AT TIME ZONE r.timezone, \
                (d + time '21:00') AT TIME ZONE r.timezone, $2, $3, $4, $5 \
         FROM geo.regions r, LATERAL (SELECT (now() AT TIME ZONE r.timezone)::date AS d) x \
         WHERE r.slug = $1 AND extract(isodow FROM d) <= 5 \
           AND NOT EXISTS (SELECT 1 FROM game.chapters ch WHERE ch.region = $1 \
                AND ch.status = 'published' AND ch.starts < (d + time '21:00') AT TIME ZONE r.timezone \
                AND ch.ends > (d + time '09:00') AT TIME ZONE r.timezone) \
         ON CONFLICT (region, key) DO NOTHING",
        &[&region, &plan.goal, &fire, &water, &plan.kind],
    )?;
    Ok(Some(day.into()))
}

fn load(c: &mut postgres::Client, region: &str) -> Result<Loaded, postgres::Error> {
    let chapter = c
        .query_opt(
            "SELECT key, campaign, names::text, extract(epoch FROM starts)::float8, \
             extract(epoch FROM ends)::float8, goal, target, coalesce(target_water, target), kind, \
             coalesce(nodes, '{}'), story::text \
             FROM game.chapters WHERE region = $1 AND status = 'published' AND ends > now() ORDER BY starts LIMIT 1",
            &[&region],
        )?
        .and_then(|r| {
            Some(Chapter {
                key: r.get(0),
                campaign: r.get(1),
                names: serde_json::from_str(r.get(2)).unwrap_or_default(),
                starts: r.get(3),
                ends: r.get(4),
                goal: Goal::parse(r.get(5))?,
                target: i64::from(r.get::<_, i32>(6)),
                target_water: i64::from(r.get::<_, i32>(7)),
                kind: r.get(8),
                nodes: r.get(9),
                at: Vec::new(),
                story: r
                    .get::<_, Option<&str>>(10)
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(serde_json::Value::Null),
            })
        });
    // Hold: where the junctions are (unknown ones are dropped); the target is
    // all of them.
    let mut chapter = chapter;
    if let Some(ch) = chapter.as_mut().filter(|c| c.goal == Goal::Hold) {
        let found: std::collections::HashMap<i64, [f64; 2]> = c
            .query(
                "SELECT id, lon, lat FROM geo.nodes WHERE id = ANY($1)",
                &[&ch.nodes],
            )?
            .into_iter()
            .map(|r| (r.get(0), [r.get(1), r.get(2)]))
            .collect();
        ch.nodes.retain(|n| found.contains_key(n));
        ch.at = ch.nodes.iter().map(|n| found[n]).collect();
        ch.target = (ch.nodes.len() as i64).max(1);
        ch.target_water = ch.target;
    }
    let (mut progress, mut reached) = ([0i64; 2], [None; 2]);
    if let Some(ch) = &chapter {
        for r in c.query(
            "SELECT faction, value, extract(epoch FROM reached_at)::float8 \
             FROM game.chapter_progress WHERE region = $1 AND key = $2",
            &[&region, &ch.key],
        )? {
            let i = Faction::parse(r.get(0)).unwrap_or(Faction::Fire).index();
            progress[i] = r.get(1);
            reached[i] = r.get(2);
        }
    }
    // Winners of finished chapters, as `Campaign::winner` decides.
    let mut ended: std::collections::BTreeMap<String, ([i64; 2], [Option<f64>; 2], [i64; 2])> =
        Default::default();
    for r in c.query(
        "SELECT ch.key, p.faction, p.value, extract(epoch FROM p.reached_at)::float8, \
           CASE ch.goal WHEN 'hold' THEN greatest(1, cardinality(ch.nodes)) ELSE ch.target END, \
           CASE ch.goal WHEN 'hold' THEN greatest(1, cardinality(ch.nodes)) ELSE coalesce(ch.target_water, ch.target) END \
         FROM game.chapters ch JOIN game.chapter_progress p ON p.region = ch.region AND p.key = ch.key \
         WHERE ch.region = $1 AND ch.status = 'published' AND ch.ends <= now() AND ch.kind IS NULL",
        &[&region],
    )? {
        let e = ended.entry(r.get(0)).or_default();
        let i = Faction::parse(r.get(1)).unwrap_or(Faction::Fire).index();
        e.0[i] = r.get(2);
        e.1[i] = r.get(3);
        e.2 = [i64::from(r.get::<_, i32>(4)), i64::from(r.get::<_, i32>(5))];
    }
    let past = ended
        .into_iter()
        .filter_map(|(k, (p, r, t))| Some((k, decide(p, r, t, true)?)))
        .collect();
    // Weather days don't count; the same boundary as keep.rs.
    let boundary = c
        .query_one(
            "SELECT extract(epoch FROM max(b))::float8 FROM ( \
               SELECT min(starts) AS b FROM game.chapters WHERE region = $1 AND kind IS NULL AND status = 'published' GROUP BY campaign \
               UNION ALL \
               SELECT max(ends) FROM game.chapters WHERE region = $1 AND kind IS NULL AND status = 'published' GROUP BY campaign \
             ) x WHERE b <= now()",
            &[&region],
        )?
        .get(0);
    // Phrases of campaigns running now (first start to last end).
    let mottos = c
        .query(
            "SELECT m.num, m.side, m.names::text FROM game.campaign_mottos m \
             WHERE m.region = $1 AND m.status = 'approved' AND m.campaign IN ( \
               SELECT campaign FROM game.chapters WHERE region = $1 AND kind IS NULL AND status = 'published' \
               GROUP BY campaign HAVING min(starts) <= now() AND max(ends) > now()) \
             ORDER BY m.num",
            &[&region],
        )?
        .into_iter()
        .map(|r| CityMotto {
            num: r.get::<_, i16>(0) as u16,
            side: Faction::parse(r.get(1)),
            names: serde_json::from_str(r.get(2)).unwrap_or_default(),
        })
        .collect();
    let campaigns: Vec<(String, f64, f64)> = c
        .query(
            "SELECT campaign, extract(epoch FROM min(starts))::float8, extract(epoch FROM max(ends))::float8 \
             FROM game.chapters WHERE region = $1 AND kind IS NULL AND status = 'published' \
             GROUP BY campaign ORDER BY campaign",
            &[&region],
        )?
        .into_iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();
    let periods = campaigns.iter().map(|(_, s, e)| (*s, *e)).collect();
    let notices = c
        .query(
            "SELECT id, names::text, url FROM game.notices \
             WHERE region = $1 AND starts <= now() AND ends > now() ORDER BY starts DESC, id DESC",
            &[&region],
        )?
        .into_iter()
        .map(|r| Notice {
            id: r.get(0),
            names: serde_json::from_str(r.get(1)).unwrap_or_default(),
            url: r.get(2),
        })
        .collect();
    Ok(Loaded {
        chapter,
        progress,
        reached,
        past,
        weather: None,
        boundary,
        mottos,
        periods,
        campaigns,
        notices,
    })
}

fn save(c: &mut postgres::Client, region: &str, s: &Save) -> Result<(), postgres::Error> {
    for i in 0..2 {
        c.execute(
            "INSERT INTO game.chapter_progress (region, key, faction, value, reached_at) \
             VALUES ($1, $2, $3, $4, to_timestamp($5)) \
             ON CONFLICT (region, key, faction) DO UPDATE SET \
             value = CASE WHEN $6 THEN EXCLUDED.value ELSE GREATEST(game.chapter_progress.value, EXCLUDED.value) END, \
             reached_at = COALESCE(game.chapter_progress.reached_at, EXCLUDED.reached_at)",
            &[
                &region,
                &s.key,
                &Faction::ALL[i].as_str(),
                &s.progress[i],
                &s.reached[i],
                &s.hold,
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_chapter(goal: Goal, target: i64) -> Campaign {
        let mut c = Campaign::spawn(None, "test".into());
        c.chapter = Some(Chapter {
            key: "k".into(),
            campaign: "c".into(),
            names: serde_json::Value::Null,
            starts: 100.0,
            ends: 200.0,
            goal,
            target,
            target_water: target,
            kind: None,
            nodes: Vec::new(),
            at: Vec::new(),
            story: serde_json::Value::Null,
        });
        c
    }

    #[test]
    fn race_first_to_the_target_wins() {
        let mut c = with_chapter(Goal::Claims, 3);
        assert!(
            c.add(Faction::Fire, Goal::Claims, 1, 50.0).is_none(),
            "not started"
        );
        assert!(
            c.add(Faction::Fire, Goal::Finds, 1, 120.0).is_none(),
            "other goal"
        );
        c.add(Faction::Water, Goal::Claims, 3, 130.0);
        c.add(Faction::Fire, Goal::Claims, 5, 140.0);
        assert_eq!(
            c.winner(150.0),
            Some(Faction::Water),
            "water reached it first"
        );
        assert_eq!(c.take_end(210.0), Some(("k".into(), Some(Faction::Water))));
        assert_eq!(c.take_end(220.0), None, "once");
    }

    #[test]
    fn weather_day_own_targets_and_no_badges() {
        let mut c = with_chapter(Goal::Claims, 3);
        if let Some(ch) = c.chapter.as_mut() {
            ch.target_water = 5;
            ch.kind = Some("weather:rain".into());
        }
        assert_eq!(
            c.add(Faction::Water, Goal::Claims, 4, 120.0),
            None,
            "counts, no badge"
        );
        c.add(Faction::Fire, Goal::Claims, 3, 130.0);
        assert_eq!(
            c.winner(140.0),
            Some(Faction::Fire),
            "fire's 3 beat water's 4 of 5"
        );
        assert_eq!(
            c.take_end(210.0),
            Some(("k".into(), None)),
            "no winner badges"
        );
    }

    #[test]
    fn nobody_at_the_target_more_wins_at_the_end() {
        let mut c = with_chapter(Goal::Points, 1000);
        c.add(Faction::Fire, Goal::Points, 40, 120.0);
        c.add(Faction::Water, Goal::Points, 30, 120.0);
        assert_eq!(c.winner(150.0), None, "still running");
        assert_eq!(c.winner(200.0), Some(Faction::Fire));
    }

    #[test]
    fn hold_counts_now_and_more_wins_at_the_end() {
        let mut c = with_chapter(Goal::Hold, 3);
        if let Some(ch) = c.chapter.as_mut() {
            ch.nodes = vec![11, 12, 13];
        }
        assert_eq!(c.hold_nodes(50.0), None, "not started");
        assert_eq!(c.hold_nodes(120.0), Some(&[11, 12, 13][..]));
        assert_eq!(c.hold_claim(12, 120.0), Some("k".into()));
        assert_eq!(c.hold_claim(14, 120.0), None, "not one of them");
        c.held([3, 0], 120.0);
        assert_eq!(c.winner(150.0), None, "all three is no early win");
        c.held([1, 2], 190.0);
        c.held([3, 0], 210.0);
        assert_eq!(c.progress, [1, 2], "after the end nothing changes");
        assert_eq!(c.take_end(210.0), Some(("k".into(), Some(Faction::Water))));
    }

    #[test]
    fn hold_tie_has_no_winner() {
        let mut c = with_chapter(Goal::Hold, 3);
        c.held([1, 1], 120.0);
        assert_eq!(c.take_end(210.0), Some(("k".into(), None)));
        assert_eq!(
            decide([0, 0], [None, None], [3, 3], true),
            None,
            "nobody held any"
        );
    }

    #[test]
    fn at_the_end_relative_to_the_target() {
        let mut c = with_chapter(Goal::Claims, 30);
        if let Some(ch) = c.chapter.as_mut() {
            ch.target_water = 50;
        }
        c.add(Faction::Fire, Goal::Claims, 20, 120.0);
        c.add(Faction::Water, Goal::Claims, 30, 120.0);
        assert_eq!(
            c.winner(200.0),
            Some(Faction::Fire),
            "20 of 30 beats 30 of 50"
        );
    }
}

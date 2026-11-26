//! Finds: named places,
//! hidden until a successful claim near them uncovers them, or being there a
//! while (also where all is one's own already; distances: `[finds]`). Stored
//! per player only for those who opted in to collecting (`Rec::collect`), and
//! then without times; everyone else sees them for the session only.
//! A find may carry a text (a letter, a diary page: shown when found, kept in
//! the album), belong to a campaign (then it only plays while that campaign
//! runs, `Campaign::running`) and have a period (plays only from/until; a
//! campaign find needs both). Once found it stays known either way.
//!
//! Shared finds (`quorum`): hidden from everyone, no hint, not counted, until
//! `quorum` trusted players have stayed near it (`[finds] shared_*`). Nobody
//! is told they count, so fresh accounts can't probe it; on the reveal those
//! who helped get it (and a badge), everyone else hears of it and can find it
//! as usual. Seekers are kept in `game.find_seekers` only until then.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::Serialize;

use elements_shared::geo::{project_enu, RegionInfo};
use elements_shared::prelude::Faction;

#[derive(Serialize, Clone, ts_rs::TS)]
#[ts(rename = "FindItem")]
pub struct Find {
    pub id: i64,
    #[serde(rename = "c")]
    pub collection: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// `{lang: text}`: shown when found, kept in the album.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "Record<string, string>")]
    pub text: Option<serde_json::Value>,
    /// Plays only while this campaign runs.
    #[serde(skip)]
    pub campaign: Option<String>,
    /// Plays only from / until (unix seconds).
    #[serde(skip)]
    pub starts: Option<f64>,
    #[serde(skip)]
    pub ends: Option<f64>,
    /// Shared: hidden until this many trusted players were near.
    #[serde(skip)]
    pub quorum: Option<u32>,
    /// Shared: how near counts (m; `None`: `[finds] shared_m`).
    #[serde(skip)]
    pub reach_m: Option<f32>,
    #[serde(skip)]
    pub revealed: bool,
    #[serde(skip)]
    pub p: Vec2,
}

impl Find {
    /// Does it play at `now`: its campaign (if any) among `running`, and
    /// `now` within its period (if any)?
    fn on(&self, running: &[String], now: f64) -> bool {
        self.open() && self.plays(running, now)
    }

    /// Not a shared find still waiting for its quorum.
    fn open(&self) -> bool {
        self.quorum.is_none() || self.revealed
    }

    /// Campaign and period allow it (shared or not).
    fn plays(&self, running: &[String], now: f64) -> bool {
        self.campaign.as_ref().is_none_or(|c| running.contains(c))
            && self.starts.is_none_or(|s| s <= now)
            && self.ends.is_none_or(|e| now < e)
    }
}

/// One who was near a shared find (`game.find_seekers`).
#[derive(Clone, Debug, PartialEq)]
pub struct Seeker {
    pub id: [u8; 32],
    pub faction: Faction,
    pub day: i64,
}

/// A shared find just revealed: who helped.
pub struct Revealed {
    pub find: i64,
    pub seekers: Vec<Seeker>,
}

enum Out {
    Seek { find: i64, s: Seeker },
    Reveal(i64),
}

#[derive(Resource, Default)]
pub struct Finds {
    items: Vec<Find>,
    /// Shared finds not revealed yet: who was near.
    seekers: HashMap<i64, Vec<Seeker>>,
    tx: Option<crate::writers::LaneTx<Out>>,
}

impl Finds {
    pub fn load(db: &str, region: &RegionInfo) -> Self {
        let mut f = match crate::db::load_finds(db, &region.slug) {
            Ok(rows) => Self::from_rows(rows, region),
            Err(e) => {
                warn!(error = %e, "no finds");
                return Self::default();
            }
        };
        match load_seekers(db, &region.slug) {
            Ok(s) => f.seekers = s,
            Err(e) => warn!(error = %e, "no seekers of shared finds"),
        }
        let db = db.to_string();
        f.tx = Some(crate::writers::lane(
            &format!("finds {}", region.slug),
            |rx| Writer { db, rx },
        ));
        f
    }

    /// After an edit: new rows, the same seekers and writer; reveals not
    /// written yet stay.
    pub fn refresh(&mut self, rows: Vec<crate::db::FindRow>, region: &RegionInfo) {
        let mut new = Self::from_rows(rows, region);
        for f in &mut new.items {
            f.revealed |= self.items.iter().any(|o| o.id == f.id && o.revealed);
        }
        new.seekers = std::mem::take(&mut self.seekers);
        new.tx = self.tx.take();
        *self = new;
    }

    /// From stored rows (also after an edit, `FromWeb::Finds`).
    pub fn from_rows(rows: Vec<crate::db::FindRow>, region: &RegionInfo) -> Self {
        let items: Vec<Find> = rows
            .into_iter()
            .map(|r| {
                let p = project_enu(r.lat, r.lon, region.centroid_lat, region.centroid_lon);
                Find {
                    id: r.id,
                    collection: r.collection,
                    name: r.name,
                    lat: r.lat,
                    lon: r.lon,
                    text: r.text,
                    campaign: r.campaign,
                    starts: r.starts,
                    ends: r.ends,
                    quorum: r.quorum.map(|q| q.max(1) as u32),
                    reach_m: r.reach_m.map(|m| m as f32),
                    revealed: r.revealed,
                    p: Vec2::new(p.x, p.z),
                }
            })
            .collect();
        info!(finds = items.len(), "finds");
        Self {
            items,
            ..Default::default()
        }
    }

    /// Shared finds waiting for their quorum within reach of `p` (`shared_m`
    /// unless the find has its own) that `id` hasn't counted for yet.
    pub fn seekable(
        &self,
        p: Vec2,
        shared_m: f32,
        id: &[u8; 32],
        running: &[String],
        now: f64,
    ) -> Option<i64> {
        self.items
            .iter()
            .filter(|f| !f.open() && f.plays(running, now))
            .filter(|f| f.p.distance(p) <= f.reach_m.unwrap_or(shared_m))
            .find(|f| {
                self.seekers
                    .get(&f.id)
                    .is_none_or(|v| v.iter().all(|s| s.id != *id))
            })
            .map(|f| f.id)
    }

    /// `s` was near shared find `find` long enough: count them (once), and
    /// reveal it if that makes the quorum (`days` distinct days, both sides
    /// with `both`).
    pub fn seek(&mut self, find: i64, s: Seeker, days: u32, both: bool) -> Option<Revealed> {
        let f = self.items.iter_mut().find(|f| f.id == find)?;
        let quorum = f.quorum.filter(|_| !f.revealed)?;
        let v = self.seekers.entry(find).or_default();
        if v.iter().any(|x| x.id == s.id) {
            return None;
        }
        v.push(s.clone());
        if let Some(tx) = &self.tx {
            let _ = tx.send(Out::Seek { find, s });
        }
        let mut day: Vec<i64> = v.iter().map(|x| x.day).collect();
        day.sort_unstable();
        day.dedup();
        let sides = |fa: Faction| v.iter().any(|x| x.faction == fa);
        let met = v.len() >= quorum as usize
            && day.len() >= days.max(1) as usize
            && (!both || (sides(Faction::Fire) && sides(Faction::Water)));
        if !met {
            return None;
        }
        f.revealed = true;
        let seekers = self.seekers.remove(&find).unwrap_or_default();
        if let Some(tx) = &self.tx {
            let _ = tx.send(Out::Reveal(find));
        }
        info!(find, seekers = seekers.len(), "shared find revealed");
        Some(Revealed { find, seekers })
    }

    /// The find with this id.
    pub fn get(&self, id: i64) -> Option<&Find> {
        self.items.iter().find(|f| f.id == id)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Finds per collection that play at `now` (`running`: campaigns running).
    pub fn totals(&self, running: &[String], now: f64) -> HashMap<String, u32> {
        let mut t = HashMap::new();
        for f in self.items.iter().filter(|f| f.on(running, now)) {
            *t.entry(f.collection.clone()).or_default() += 1;
        }
        t
    }

    /// Collection sizes for a player's album: what plays at `now`, plus the
    /// player's `found` ones that no longer (or not yet) play, so a campaign's
    /// letters stay listed, and counted fairly, after it ended.
    pub fn album_totals(
        &self,
        running: &[String],
        now: f64,
        found: &std::collections::HashSet<i64>,
    ) -> HashMap<String, u32> {
        let mut t = HashMap::new();
        for f in self
            .items
            .iter()
            .filter(|f| f.on(running, now) || found.contains(&f.id))
        {
            *t.entry(f.collection.clone()).or_default() += 1;
        }
        t
    }

    /// Finds playing at `now` within `r` m of `p` (ENU x/z), nearest first.
    pub fn near(&self, p: Vec2, r: f32, running: &[String], now: f64) -> Vec<&Find> {
        let mut v: Vec<(f32, &Find)> = self
            .items
            .iter()
            .filter(|f| f.on(running, now))
            .map(|f| (f.p.distance(p), f))
            .filter(|(d, _)| *d <= r)
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().map(|(_, f)| f).collect()
    }

    /// How many of `collection` playing at `now` are in `ids` (ids of any region).
    pub fn count(
        &self,
        collection: &str,
        ids: impl Iterator<Item = i64> + Clone,
        running: &[String],
        now: f64,
    ) -> u32 {
        self.items
            .iter()
            .filter(|f| f.collection == collection && f.on(running, now))
            .filter(|f| ids.clone().any(|i| i == f.id))
            .count() as u32
    }

    /// The finds of this region among `ids` (found ones stay, also after
    /// their campaign).
    pub fn of<'a>(
        &'a self,
        ids: &'a std::collections::HashSet<i64>,
    ) -> impl Iterator<Item = &'a Find> {
        self.items.iter().filter(|f| ids.contains(&f.id))
    }
}

fn load_seekers(
    db: &str,
    slug: &str,
) -> Result<HashMap<i64, Vec<Seeker>>, Box<dyn std::error::Error>> {
    let mut c = crate::db::connect(db)?;
    let mut out: HashMap<i64, Vec<Seeker>> = HashMap::new();
    for r in c.query(
        "SELECT s.find, s.id, s.faction, s.day FROM game.find_seekers s \
         JOIN geo.finds f ON f.id = s.find WHERE f.region = $1 AND f.revealed IS NULL",
        &[&slug],
    )? {
        let Ok(id) = <[u8; 32]>::try_from(r.get::<_, &[u8]>(1)) else {
            continue;
        };
        out.entry(r.get(0)).or_default().push(Seeker {
            id,
            faction: Faction::parse(r.get(2)).unwrap_or(Faction::Fire),
            day: i64::from(r.get::<_, i32>(3)),
        });
    }
    Ok(out)
}

/// Writes seekers and reveals (a lane on the shared writer threads).
struct Writer {
    db: String,
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
                Out::Seek { find, s } => c
                    .execute(
                        "INSERT INTO game.find_seekers (find, id, faction, day) \
                         VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
                        &[find, &&s.id[..], &s.faction.as_str(), &(s.day as i32)],
                    )
                    .map(|_| ()),
                Out::Reveal(find) => c
                    .execute(
                        "UPDATE geo.finds SET revealed = now() WHERE id = $1 AND revealed IS NULL",
                        &[find],
                    )
                    .and_then(|_| {
                        c.execute("DELETE FROM game.find_seekers WHERE find = $1", &[find])
                    })
                    .map(|_| ()),
            };
            if let Err(e) = res {
                pg.failed(db, "shared finds", &crate::db::detail(&e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, campaign: Option<&str>) -> crate::db::FindRow {
        crate::db::FindRow {
            id,
            collection: "fountains".into(),
            name: format!("f{id}"),
            lat: 47.66,
            lon: 9.17,
            text: None,
            campaign: campaign.map(String::from),
            starts: None,
            ends: None,
            quorum: None,
            reach_m: None,
            revealed: false,
        }
    }

    #[test]
    fn campaign_finds_play_only_while_it_runs() {
        let region = crate::testkit::region();
        let f = Finds::from_rows(vec![row(1, None), row(2, Some("advent"))], &region);
        let p = f.items[0].p;
        assert_eq!(f.near(p, 10.0, &[], 0.0).len(), 1, "the letter waits");
        let adv = ["advent".to_string()];
        assert_eq!(f.near(p, 10.0, &adv, 0.0).len(), 2);
        assert_eq!(f.totals(&[], 0.0)["fountains"], 1);
        assert_eq!(f.totals(&adv, 0.0)["fountains"], 2);
        let ids: std::collections::HashSet<i64> = [1, 2].into();
        assert_eq!(f.count("fountains", ids.iter().copied(), &[], 0.0), 1);
        assert_eq!(f.of(&ids).count(), 2, "found ones stay known");
    }

    #[test]
    fn album_keeps_found_letters_after_the_campaign() {
        let region = crate::testkit::region();
        let f = Finds::from_rows(
            vec![row(1, None), row(2, Some("advent")), row(3, Some("advent"))],
            &region,
        );
        let found: std::collections::HashSet<i64> = [2].into();
        let t = f.album_totals(&[], 0.0, &found);
        assert_eq!(t["fountains"], 2, "playing one plus the found letter");
        assert_eq!(
            f.album_totals(&["advent".into()], 0.0, &found)["fountains"],
            3
        );
    }

    #[test]
    fn period_bounds_play_also_within_a_campaign() {
        let region = crate::testkit::region();
        let mut rows = vec![row(1, None), row(2, Some("advent")), row(3, None)];
        rows[0].starts = Some(100.0);
        rows[1].starts = Some(100.0);
        rows[1].ends = Some(200.0);
        rows[2].ends = Some(50.0);
        let f = Finds::from_rows(rows, &region);
        let p = f.items[0].p;
        let adv = ["advent".to_string()];
        let ids = |v: Vec<&Find>| v.iter().map(|f| f.id).collect::<Vec<_>>();
        assert_eq!(
            ids(f.near(p, 10.0, &adv, 0.0)),
            [3],
            "before: only the seasonal one"
        );
        assert_eq!(
            ids(f.near(p, 10.0, &adv, 150.0)),
            [1, 2],
            "within the period"
        );
        assert_eq!(
            ids(f.near(p, 10.0, &[], 150.0)),
            [1],
            "campaign over: the letter is gone"
        );
        assert_eq!(ids(f.near(p, 10.0, &adv, 200.0)), [1], "period over");
        assert_eq!(f.totals(&adv, 150.0)["fountains"], 2);
    }

    fn seeker(n: u8, faction: Faction, day: i64) -> Seeker {
        Seeker {
            id: [n; 32],
            faction,
            day,
        }
    }

    #[test]
    fn shared_find_hidden_until_the_quorum() {
        let region = crate::testkit::region();
        let mut rows = vec![row(1, None), row(2, None)];
        rows[1].quorum = Some(3);
        let mut f = Finds::from_rows(rows, &region);
        let p = f.items[0].p;
        let ids = |f: &Finds| {
            f.near(p, 10.0, &[], 0.0)
                .iter()
                .map(|x| x.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&f), [1], "no hint, no uncovering");
        assert_eq!(f.totals(&[], 0.0)["fountains"], 1, "not counted");
        let far = p + Vec2::new(140.0, 0.0);
        assert_eq!(
            f.seekable(far, 150.0, &[1; 32], &[], 0.0),
            Some(2),
            "generous reach"
        );
        assert_eq!(f.seekable(far, 100.0, &[1; 32], &[], 0.0), None);
        // Three on one day: not yet (two days needed).
        for n in 1..=3 {
            assert!(f.seek(2, seeker(n, Faction::Fire, 10), 2, false).is_none());
        }
        assert!(
            f.seek(2, seeker(1, Faction::Fire, 11), 2, false).is_none(),
            "once each"
        );
        assert_eq!(
            f.seekable(p, 150.0, &[1; 32], &[], 0.0),
            None,
            "counted already"
        );
        let r = f
            .seek(2, seeker(4, Faction::Fire, 11), 2, false)
            .expect("revealed");
        assert_eq!(r.seekers.len(), 4);
        assert_eq!(ids(&f), [1, 2], "an ordinary find now");
        assert_eq!(f.totals(&[], 0.0)["fountains"], 2);
        assert_eq!(f.seekable(p, 150.0, &[9; 32], &[], 0.0), None);
        assert!(f.seek(2, seeker(5, Faction::Fire, 11), 2, false).is_none());
    }

    #[test]
    fn shared_find_both_sides_and_own_reach() {
        let region = crate::testkit::region();
        let mut rows = vec![row(1, None)];
        rows[0].quorum = Some(2);
        rows[0].reach_m = Some(60);
        let mut f = Finds::from_rows(rows, &region);
        let p = f.items[0].p;
        assert_eq!(
            f.seekable(p + Vec2::new(80.0, 0.0), 150.0, &[1; 32], &[], 0.0),
            None,
            "own reach wins"
        );
        assert!(f.seek(1, seeker(1, Faction::Fire, 1), 1, true).is_none());
        assert!(
            f.seek(1, seeker(2, Faction::Fire, 1), 1, true).is_none(),
            "fire only"
        );
        assert!(f.seek(1, seeker(3, Faction::Water, 1), 1, true).is_some());
    }

    #[test]
    fn refresh_keeps_seekers_and_reveals() {
        let region = crate::testkit::region();
        let mut rows = vec![row(1, None), row(2, None)];
        rows[0].quorum = Some(2);
        rows[1].quorum = Some(5);
        let mut f = Finds::from_rows(rows.clone(), &region);
        f.seek(1, seeker(1, Faction::Fire, 1), 1, false);
        f.seek(1, seeker(2, Faction::Water, 1), 1, false);
        f.seek(2, seeker(1, Faction::Fire, 1), 1, false);
        f.refresh(rows, &region);
        assert!(f.get(1).unwrap().revealed, "reveal not written yet stays");
        assert_eq!(
            f.seekable(f.items[1].p, 150.0, &[1; 32], &[], 0.0),
            None,
            "seeker kept"
        );
    }
}

//! Finds: named places,
//! hidden until a successful claim near them uncovers them, or being there a
//! while (also where all is one's own already; distances: `[finds]`). Stored
//! per player only for those who opted in to collecting (`Rec::collect`), and
//! then without times; everyone else sees them for the session only.
//! A find may carry a text (a letter, a diary page: shown when found, kept in
//! the album), belong to a campaign (then it only plays while that campaign
//! runs, `Campaign::running`) and have a period (plays only from/until; a
//! campaign find needs both). Once found it stays known either way.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::Serialize;

use elements_shared::geo::{project_enu, RegionInfo};

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
    #[serde(skip)]
    pub p: Vec2,
}

impl Find {
    /// Does it play at `now`: its campaign (if any) among `running`, and
    /// `now` within its period (if any)?
    fn on(&self, running: &[String], now: f64) -> bool {
        self.campaign.as_ref().is_none_or(|c| running.contains(c))
            && self.starts.is_none_or(|s| s <= now)
            && self.ends.is_none_or(|e| now < e)
    }
}

#[derive(Resource, Default)]
pub struct Finds {
    items: Vec<Find>,
}

impl Finds {
    pub fn load(db: &str, region: &RegionInfo) -> Self {
        match crate::db::load_finds(db, &region.slug) {
            Ok(rows) => Self::from_rows(rows, region),
            Err(e) => {
                warn!(error = %e, "no finds");
                Self::default()
            }
        }
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
                    p: Vec2::new(p.x, p.z),
                }
            })
            .collect();
        info!(finds = items.len(), "finds");
        Self { items }
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
}

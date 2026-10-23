//! Finds: named places,
//! hidden until a successful claim near them uncovers them, or being there a
//! while (also where all is one's own already; distances: `[finds]`). Stored
//! per player only for those who opted in to collecting (`Rec::collect`), and
//! then without times; everyone else sees them for the session only.
//! A find may carry a text (a letter, a diary page: shown when found, kept in
//! the album) and belong to a campaign: then it only plays while that
//! campaign runs (`Campaign::running`), but stays known once found.

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
    #[serde(skip)]
    pub p: Vec2,
}

impl Find {
    /// Does it play now (no campaign, or one of `running`)?
    fn on(&self, running: &[String]) -> bool {
        self.campaign.as_ref().is_none_or(|c| running.contains(c))
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

    /// Finds per collection that play now (`running`: campaigns running).
    pub fn totals(&self, running: &[String]) -> HashMap<String, u32> {
        let mut t = HashMap::new();
        for f in self.items.iter().filter(|f| f.on(running)) {
            *t.entry(f.collection.clone()).or_default() += 1;
        }
        t
    }

    /// Finds playing now within `r` m of `p` (ENU x/z), nearest first.
    pub fn near(&self, p: Vec2, r: f32, running: &[String]) -> Vec<&Find> {
        let mut v: Vec<(f32, &Find)> = self
            .items
            .iter()
            .filter(|f| f.on(running))
            .map(|f| (f.p.distance(p), f))
            .filter(|(d, _)| *d <= r)
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().map(|(_, f)| f).collect()
    }

    /// How many of `collection` playing now are in `ids` (ids of any region).
    pub fn count(
        &self,
        collection: &str,
        ids: impl Iterator<Item = i64> + Clone,
        running: &[String],
    ) -> u32 {
        self.items
            .iter()
            .filter(|f| f.collection == collection && f.on(running))
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
        }
    }

    #[test]
    fn campaign_finds_play_only_while_it_runs() {
        let region = crate::testkit::region();
        let f = Finds::from_rows(vec![row(1, None), row(2, Some("advent"))], &region);
        let p = f.items[0].p;
        assert_eq!(f.near(p, 10.0, &[]).len(), 1, "the letter waits");
        let adv = ["advent".to_string()];
        assert_eq!(f.near(p, 10.0, &adv).len(), 2);
        assert_eq!(f.totals(&[])["fountains"], 1);
        assert_eq!(f.totals(&adv)["fountains"], 2);
        let ids: std::collections::HashSet<i64> = [1, 2].into();
        assert_eq!(f.count("fountains", ids.iter().copied(), &[]), 1);
        assert_eq!(f.of(&ids).count(), 2, "found ones stay known");
    }
}

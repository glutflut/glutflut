//! Finds: named places,
//! hidden until a successful claim near them uncovers them, or being there a
//! while (also where all is one's own already; distances: `[finds]`). Stored
//! per player only for those who opted in to collecting (`Rec::collect`), and
//! then without times; everyone else sees them for the session only.

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
    #[serde(skip)]
    pub p: Vec2,
}

#[derive(Resource, Default)]
pub struct Finds {
    items: Vec<Find>,
    /// Finds per collection in this region.
    pub totals: HashMap<String, u32>,
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
        let mut totals = HashMap::new();
        let items: Vec<Find> = rows
            .into_iter()
            .map(|(id, collection, name, lat, lon)| {
                *totals.entry(collection.clone()).or_default() += 1;
                let p = project_enu(lat, lon, region.centroid_lat, region.centroid_lon);
                Find {
                    id,
                    collection,
                    name,
                    lat,
                    lon,
                    p: Vec2::new(p.x, p.z),
                }
            })
            .collect();
        info!(finds = items.len(), "finds");
        Self { items, totals }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Finds within `r` m of `p` (ENU x/z), nearest first.
    pub fn near(&self, p: Vec2, r: f32) -> Vec<&Find> {
        let mut v: Vec<(f32, &Find)> = self
            .items
            .iter()
            .map(|f| (f.p.distance(p), f))
            .filter(|(d, _)| *d <= r)
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().map(|(_, f)| f).collect()
    }

    /// How many of `collection` in `ids` (ids of any region).
    pub fn count(&self, collection: &str, ids: impl Iterator<Item = i64> + Clone) -> u32 {
        self.items
            .iter()
            .filter(|f| f.collection == collection && ids.clone().any(|i| i == f.id))
            .count() as u32
    }

    /// The finds of this region among `ids`.
    pub fn of<'a>(
        &'a self,
        ids: &'a std::collections::HashSet<i64>,
    ) -> impl Iterator<Item = &'a Find> {
        self.items.iter().filter(|f| ids.contains(&f.id))
    }
}

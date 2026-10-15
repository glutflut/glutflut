//! Hotspots. In each time slot a few candidate sites are switched on at random
//! (the experiment); the rest are the control. Claims near an active hotspot
//! last longer. Per site and slot we count players nearby at the start and
//! players who reached the site, by pseudonym, in memory only.

use std::collections::HashSet;

use bevy::prelude::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;

use elements_shared::prelude::*;

use crate::analytics::{now_s, AnalyticsTx, Row, Tracked};

/// "Reached" = came this close during the slot; the follow rate counts only
/// players who were nearby at the start.
const REACH_M: f32 = 30.0;
/// "Nearby" at slot start.
const NEARBY_M: f32 = 250.0;
/// Candidate sites lie within the region's hotspot area (`geo.regions`:
/// `hotspot_lat`/`_lon`, `hotspot_area_m`; default: centroid, 700 m). For a
/// whole city, the centre keeps the experiment where the crowd is.
/// `ELEMENTS_HOTSPOT_CENTER` ("lat,lon") and `ELEMENTS_HOTSPOT_AREA_M` override.
const SITE_AREA_M: f32 = 700.0;

fn to_sites(rows: &[crate::places::SiteRow], region: &RegionInfo) -> Vec<Site> {
    rows.iter()
        .map(|r| Site {
            node: r.node,
            pos: project_enu(r.lat, r.lon, region.centroid_lat, region.centroid_lon),
            ll: [r.lon, r.lat],
        })
        .collect()
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone)]
pub struct Site {
    pub node: i64,
    pub pos: Vec3,
    /// lon, lat (the app places the marker without holding the node's tile).
    pub ll: [f64; 2],
}

#[derive(Serialize, Clone, ts_rs::TS)]
pub struct HotspotInfo {
    pub node: i64,
    pub ends_in: u32,
    pub manual: bool,
    pub ll: [f64; 2],
}

#[derive(Resource)]
pub struct Hotspots {
    sites: Vec<Site>,
    /// Automatic sites, for when the city removes all of its own.
    auto: Vec<Site>,
    /// Sites fixed by `ELEMENTS_HOTSPOT_SITES` in this region (the editor's
    /// don't replace them).
    pinned: bool,
    /// New sites from the editor, taken at the next slot.
    pending: Option<Vec<Site>>,
    /// Planned hotspots (editor): id, site, start, end.
    plans: Vec<(i64, Site, f64, f64)>,
    /// Plans switched on already (not again after they end).
    planned_on: std::collections::HashSet<i64>,
    slot_s: f64,
    n_active: usize,
    slot_start: f64,
    active: Vec<usize>,
    manual: Vec<(Site, f64)>,
    // per site, index by synthetic flag
    nearby: Vec<[HashSet<u64>; 2]>,
    reached: Vec<[HashSet<u64>; 2]>,
    pub changed: bool,
    rng: StdRng,
    tx: AnalyticsTx,
}

/// The nodes of `ELEMENTS_HOTSPOT_SITES` (comma-separated ids) in this region.
fn fixed_sites(nodes: &[GeoNode], pos: impl Fn(&GeoNode) -> Vec3) -> Vec<Site> {
    let wanted: Vec<i64> = std::env::var("ELEMENTS_HOTSPOT_SITES")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_default();
    nodes
        .iter()
        .filter(|n| wanted.contains(&n.id))
        .map(|n| Site {
            node: n.id,
            pos: pos(n),
            ll: [n.lon, n.lat],
        })
        .collect()
}

impl Hotspots {
    /// Sites from `ELEMENTS_HOTSPOT_SITES` (node ids; only those in this
    /// region count, so other regions of the container ignore them), else the
    /// city's (`geo.hotspot_sites`), else spread by farthest-point sampling
    /// around the region's hotspot centre (env fallback) or centroid.
    pub fn new(
        nodes: &[GeoNode],
        region: &RegionInfo,
        chosen: &[crate::places::SiteRow],
        tx: AnalyticsTx,
    ) -> Self {
        let pos = |n: &GeoNode| project_enu(n.lat, n.lon, region.centroid_lat, region.centroid_lon);
        let fixed = fixed_sites(nodes, pos);
        let auto: Vec<Site> = if !fixed.is_empty() {
            fixed.clone()
        } else {
            let center = region
                .hotspot_center
                .or_else(|| {
                    let v = std::env::var("ELEMENTS_HOTSPOT_CENTER").ok()?;
                    let (a, b) = v.split_once(',')?;
                    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                })
                .map_or(Vec3::ZERO, |(lat, lon)| {
                    project_enu(lat, lon, region.centroid_lat, region.centroid_lon)
                });
            let area = region
                .hotspot_area_m
                .unwrap_or_else(|| env_or("ELEMENTS_HOTSPOT_AREA_M", SITE_AREA_M));
            let pool: Vec<Site> = nodes
                .iter()
                .map(|n| Site {
                    node: n.id,
                    pos: pos(n) - center,
                    ll: [n.lon, n.lat],
                })
                .filter(|s| s.pos.length() <= area)
                .collect();
            farthest_points(pool, env_or("ELEMENTS_HOTSPOT_COUNT", 12))
                .into_iter()
                .map(|s| Site {
                    pos: s.pos + center,
                    ..s
                })
                .collect()
        };
        let pinned = !fixed.is_empty();
        let sites = if chosen.is_empty() || pinned {
            auto.clone()
        } else {
            to_sites(chosen, region)
        };
        let n = sites.len();
        info!(sites = n, chosen = chosen.len(), "hotspot sites");
        Self {
            sites,
            auto,
            pinned,
            pending: None,
            plans: Vec::new(),
            planned_on: Default::default(),
            slot_s: env_or("ELEMENTS_HOTSPOT_SLOT", 600.0),
            n_active: env_or("ELEMENTS_HOTSPOT_ACTIVE", 2),
            slot_start: 0.0,
            active: Vec::new(),
            manual: Vec::new(),
            nearby: vec![Default::default(); n],
            reached: vec![Default::default(); n],
            changed: true,
            rng: StdRng::from_os_rng(),
            tx,
        }
    }

    /// The city's sites changed (none: automatic ones again); from the next slot.
    pub fn set_sites(&mut self, chosen: &[crate::places::SiteRow], region: &RegionInfo) {
        if self.pinned {
            return;
        }
        self.pending = Some(if chosen.is_empty() {
            self.auto.clone()
        } else {
            to_sites(chosen, region)
        });
    }

    /// The city's planned hotspots (on load and after each edit). A removed
    /// plan that is on stays on until its end.
    pub fn set_plans(&mut self, plans: &[crate::places::PlanRow], region: &RegionInfo) {
        self.plans = plans
            .iter()
            .map(|p| {
                let site = to_sites(std::slice::from_ref(&p.site), region).remove(0);
                (p.id, site, p.starts, p.ends)
            })
            .collect();
    }

    /// Switch on the plans whose window has begun (as manual hotspots).
    fn start_plans(&mut self, now: f64) {
        for (id, site, starts, ends) in &self.plans {
            if *starts <= now && now < *ends && self.planned_on.insert(*id) {
                self.manual.push((site.clone(), *ends));
                self.tx.send(vec![Row::Intervention {
                    node: site.node,
                    starts: now,
                    ends: *ends,
                    source: "planned",
                }]);
                info!(plan = id, node = site.node, "planned hotspot on");
                self.changed = true;
            }
        }
    }

    /// Reproducible site picks (demo day).
    pub fn seed(&mut self, seed: u64) {
        self.rng = StdRng::seed_from_u64(seed);
    }

    fn is_active(&self, i: usize) -> bool {
        self.active.contains(&i)
    }

    /// Within `radius_m` (`hotspot_radius_m`) of an active (random or manual) hotspot?
    pub fn bonus(&self, p: Vec3, radius_m: f32) -> bool {
        let near = |s: &Site| s.pos.distance(p) <= radius_m;
        self.active.iter().any(|&i| near(&self.sites[i]))
            || self.manual.iter().any(|(s, _)| near(s))
    }

    pub fn observe(&mut self, t: &Tracked, p: Vec3) {
        for (i, s) in self.sites.iter().enumerate() {
            if s.pos.distance(p) <= REACH_M {
                self.reached[i][t.synthetic as usize].insert(t.pseudo);
            }
        }
    }

    /// Take a person out of the running slot (they objected).
    pub fn withdraw(&mut self, pseudo: u64) {
        for sets in self.nearby.iter_mut().chain(self.reached.iter_mut()) {
            for s in sets.iter_mut() {
                s.remove(&pseudo);
            }
        }
    }

    pub fn add_manual(&mut self, node: &GeoNode, region: &RegionInfo, minutes: f64, now: f64) {
        let pos = project_enu(node.lat, node.lon, region.centroid_lat, region.centroid_lon);
        let ends = now + minutes * 60.0;
        let ll = [node.lon, node.lat];
        self.manual.push((
            Site {
                node: node.id,
                pos,
                ll,
            },
            ends,
        ));
        self.tx.send(vec![Row::Intervention {
            node: node.id,
            starts: now,
            ends,
            source: "manual",
        }]);
        self.changed = true;
    }

    pub fn list(&self, now: f64) -> Vec<HotspotInfo> {
        let random_end = self.slot_start + self.slot_s;
        self.active
            .iter()
            .map(|&i| HotspotInfo {
                node: self.sites[i].node,
                ends_in: (random_end - now).max(0.0) as u32,
                manual: false,
                ll: self.sites[i].ll,
            })
            .chain(self.manual.iter().map(|(s, end)| HotspotInfo {
                node: s.node,
                ends_in: (end - now).max(0.0) as u32,
                manual: true,
                ll: s.ll,
            }))
            .collect()
    }

    /// Log the running slot (on shutdown); `counts` as in [`Hotspots::step`].
    pub fn finish(&mut self, counts: &dyn Fn(u64) -> bool) {
        let rows = self.close_slot(counts);
        self.tx.send(rows);
    }

    fn close_slot(&mut self, counts: &dyn Fn(u64) -> bool) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.slot_start > 0.0 {
            for i in 0..self.sites.len() {
                for synthetic in [false, true] {
                    let k = synthetic as usize;
                    // Real players only once they count (see analytics.rs).
                    let ok = |p: &&u64| synthetic || counts(**p);
                    let nearby = self.nearby[i][k].iter().filter(ok).count();
                    let reached = self.reached[i][k].iter().filter(ok).count();
                    let reached_nearby = self.reached[i][k]
                        .intersection(&self.nearby[i][k])
                        .filter(ok)
                        .count();
                    if nearby + reached == 0 {
                        continue;
                    }
                    rows.push(Row::Visit {
                        site: self.sites[i].node,
                        slot: self.slot_start,
                        synthetic,
                        active: self.is_active(i),
                        nearby: nearby as i32,
                        reached: reached as i32,
                        reached_nearby: reached_nearby as i32,
                    });
                }
            }
        }
        rows
    }
}

fn farthest_points(pool: Vec<Site>, k: usize) -> Vec<Site> {
    if pool.is_empty() {
        return pool;
    }
    let mut pool = pool;
    let start = (0..pool.len())
        .min_by(|&a, &b| pool[a].pos.length().total_cmp(&pool[b].pos.length()))
        .unwrap();
    let mut out = vec![pool.swap_remove(start)];
    while out.len() < k && !pool.is_empty() {
        let far = (0..pool.len())
            .max_by(|&a, &b| {
                let d = |i: usize| {
                    out.iter()
                        .map(|s| s.pos.distance(pool[i].pos))
                        .fold(f32::MAX, f32::min)
                };
                d(a).total_cmp(&d(b))
            })
            .unwrap();
        out.push(pool.swap_remove(far));
    }
    out
}

/// Rotate slots: log the finished slot, pick new random sites, count who is nearby.
pub fn tick(
    mut hs: ResMut<Hotspots>,
    agg: Res<crate::analytics::Aggregator>,
    players: Query<(&Position, &Tracked), With<Player>>,
) {
    let counted = players.iter().filter(|(_, t)| t.counted);
    hs.step(now_s(), counted.map(|(p, t)| (p.0, *t)), &|p| {
        agg.standing.counts(p)
    });
}

impl Hotspots {
    /// One tick at `now`; `players` is only read when a new slot starts.
    /// `counts`: whether a real player's pseudonym counts yet.
    pub fn step(
        &mut self,
        now: f64,
        players: impl Iterator<Item = (Vec3, Tracked)>,
        counts: &dyn Fn(u64) -> bool,
    ) {
        self.start_plans(now);
        let before = self.manual.len();
        self.manual.retain(|(_, end)| *end > now);
        if self.manual.len() != before {
            self.changed = true;
        }
        if (self.sites.is_empty() && self.pending.is_none()) || now < self.slot_start + self.slot_s
        {
            return;
        }
        let mut rows = self.close_slot(counts);
        if let Some(sites) = self.pending.take() {
            let n = sites.len();
            self.sites = sites;
            self.nearby = vec![Default::default(); n];
            self.reached = vec![Default::default(); n];
            info!(sites = n, "hotspot sites changed");
        }
        let slot_s = self.slot_s;
        self.slot_start = (now / slot_s).floor() * slot_s;
        let n = self.sites.len();
        let mut idx: Vec<usize> = (0..n).collect();
        // Fisher-Yates for the first n_active picks.
        for i in 0..self.n_active.min(n) {
            let j = self.rng.random_range(i..n);
            idx.swap(i, j);
        }
        self.active = idx[..self.n_active.min(n)].to_vec();
        for i in 0..n {
            self.nearby[i] = Default::default();
            self.reached[i] = Default::default();
        }
        for (pos, t) in players {
            for i in 0..n {
                if self.sites[i].pos.distance(pos) <= NEARBY_M {
                    self.nearby[i][t.synthetic as usize].insert(t.pseudo);
                }
            }
        }
        let (start, end) = (self.slot_start, self.slot_start + slot_s);
        for &i in &self.active {
            rows.push(Row::Intervention {
                node: self.sites[i].node,
                starts: start,
                ends: end,
                source: "random",
            });
        }
        self.tx.send(rows);
        self.changed = true;
        info!(active = ?self.active.iter().map(|&i| self.sites[i].node).collect::<Vec<_>>(), "hotspot slot");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::places::SiteRow;
    use crate::testkit::{node, region};

    #[test]
    fn city_sites_replace_automatic_ones_from_the_next_slot() {
        let nodes: Vec<GeoNode> = (0..20)
            .map(|i| node(i, i as f64 * 50.0, 0.0, "crossing"))
            .collect();
        let r = region();
        let row = |n: &GeoNode| SiteRow {
            node: n.id,
            name: "x".into(),
            lat: n.lat,
            lon: n.lon,
        };
        let mut h = Hotspots::new(&nodes, &r, &[], crate::analytics::test_tx());
        let auto = h.sites.len();
        assert!(auto > 2, "automatic sites");
        let h2 = Hotspots::new(&nodes, &r, &[row(&nodes[3])], crate::analytics::test_tx());
        assert_eq!(h2.sites.iter().map(|s| s.node).collect::<Vec<_>>(), vec![3]);

        h.step(1000.0, std::iter::empty(), &|_| true);
        h.set_sites(&[row(&nodes[5]), row(&nodes[7])], &r);
        assert_eq!(h.sites.len(), auto, "not before the slot ends");
        h.step(1000.0 + h.slot_s, std::iter::empty(), &|_| true);
        assert_eq!(
            h.sites.iter().map(|s| s.node).collect::<Vec<_>>(),
            vec![5, 7]
        );
        assert_eq!(h.nearby.len(), 2);
        // A plan: on at its start, once, until its end.
        let plan = crate::places::PlanRow {
            id: 7,
            site: row(&nodes[9]),
            starts: 5000.0,
            ends: 6000.0,
        };
        h.set_plans(&[plan], &r);
        h.step(4999.0, std::iter::empty(), &|_| true);
        assert!(
            h.list(4999.0).iter().all(|x| !x.manual),
            "not before its start"
        );
        h.step(5000.0, std::iter::empty(), &|_| true);
        let on: Vec<_> = h.list(5000.0).into_iter().filter(|x| x.manual).collect();
        assert_eq!((on.len(), on[0].node, on[0].ends_in), (1, 9, 1000));
        assert!(h.bonus(h.manual[0].0.pos, 10.0));
        h.step(5500.0, std::iter::empty(), &|_| true);
        assert_eq!(h.manual.len(), 1, "switched on once");
        h.step(6000.0, std::iter::empty(), &|_| true);
        assert!(h.manual.is_empty(), "off at its end");
        // All removed: automatic again.
        h.set_sites(&[], &r);
        h.step(7000.0, std::iter::empty(), &|_| true);
        assert_eq!(h.sites.len(), auto);
    }
}

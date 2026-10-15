//! Live weather as a rule, per junction and quarter hour:
//! where the values (`radar.rs`) cover the region, they replace
//! that part of the daily rule; without data the daily rule stays as it was.
//! - Rain (`[weather] live_decay`, `live_natural`; rounds 19, 19b): fire
//!   sources age faster, water ones slower, natural fire weakens (0 at
//!   0.2 mm/h, full at 4 mm/h).
//! - Sun (`sun_decay`, `sun_natural`; round 21): clear sky by day, the other
//!   way round (full at 20 % cloud cover or less with the sun 10° up).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use crossbeam_channel::Receiver;
use elements_shared::geo::GeoNode;
use elements_shared::radar::{epoch, mm_h, Blocks, Frame};
use elements_shared::weather::sun_elevation;

use crate::sim::ElemIdx;
use crate::weather::BaseRules;

/// Share of junctions with data needed to drop the daily rule's part.
const COVER: f32 = 0.5;
/// Older frames don't count (the host stopped delivering).
const MAX_AGE_S: i64 = 3600;

/// Junctions grouped by value cell, and the number of junctions.
type Cells = (u8, Vec<(u64, Vec<u32>)>, usize);

/// Intensity 0..1 per junction from one kind of values.
#[derive(Default)]
pub struct LiveField {
    k: Vec<f32>,
    /// The values cover the region: live instead of the daily part.
    pub on: bool,
    /// Changes whenever `k` or `on` do (`weather::apply` follows it).
    pub version: u32,
    cells: Option<Arc<Cells>>,
    rx: Option<Receiver<Cells>>,
    shown: String,
}

impl LiveField {
    /// Intensity at junction `i` (0 while off).
    pub fn at(&self, i: usize) -> f32 {
        if self.on {
            self.k.get(i).copied().unwrap_or(0.0)
        } else {
            0.0
        }
    }

    /// Tests: these intensities, on.
    #[cfg(test)]
    pub fn with(k: Vec<f32>) -> Self {
        Self {
            k,
            on: true,
            version: 1,
            ..Default::default()
        }
    }

    fn off(&mut self, name: &str) {
        if self.on {
            self.on = false;
            self.version += 1;
            info!("live {name}: off (no current values)");
        }
    }

    /// Group the junctions by cell once (a thread: big regions have many),
    /// then turn the frame into intensities when `key` changes.
    fn step(
        &mut self,
        name: &'static str,
        frame: Option<Arc<Frame>>,
        key: impl FnOnce(&Frame) -> String,
        nodes: &Query<(&GeoNode, &ElemIdx)>,
        decode: fn(u8) -> Option<f32>,
        intensity: impl Fn(f32) -> f32,
    ) {
        let Some(frame) = frame else {
            return self.off(name);
        };
        let now = crate::analytics::now_s() as i64;
        if epoch(&frame.time).is_none_or(|t| now - t > MAX_AGE_S) {
            return self.off(name);
        }
        let res = frame.blocks.res();
        if self.cells.as_ref().is_none_or(|c| c.0 != res) {
            if let Some(rx) = &self.rx {
                match rx.try_recv() {
                    Ok(c) if c.0 == res => {
                        self.cells = Some(Arc::new(c));
                        self.rx = None;
                    }
                    Ok(_) => self.rx = None,
                    Err(_) => return,
                }
            } else {
                let pts: Vec<(f64, f64, u32)> = nodes
                    .iter()
                    .map(|(g, i)| (g.lat, g.lon, i.0 as u32))
                    .collect();
                if pts.is_empty() {
                    return;
                }
                let (tx, rx) = crossbeam_channel::bounded(1);
                self.rx = Some(rx);
                let _ = std::thread::Builder::new()
                    .name(format!("{name}-cells"))
                    .spawn(move || {
                        let n = pts.iter().map(|p| p.2 as usize + 1).max().unwrap_or(0);
                        let mut by: HashMap<u64, Vec<u32>> = HashMap::new();
                        for (lat, lon, i) in pts {
                            if let Some(c) = Blocks::cell(lat, lon, res) {
                                by.entry(c).or_default().push(i);
                            }
                        }
                        let _ = tx.send((res, by.into_iter().collect(), n));
                    });
                return;
            }
        }
        let key = key(&frame);
        if key == self.shown {
            return;
        }
        let cells = self.cells.clone().expect("cells");
        let (_, groups, n) = &*cells;
        let mut k = vec![0.0; *n];
        let (mut with, mut hit) = (0usize, 0usize);
        for (cell, idx) in groups {
            let Some(v) = frame
                .blocks
                .offset_of(*cell)
                .and_then(|o| frame.values.get(o))
                .and_then(|&v| decode(v))
            else {
                continue;
            };
            with += idx.len();
            let x = intensity(v);
            if x > 0.0 {
                hit += idx.len();
                for &i in idx {
                    k[i as usize] = x;
                }
            }
        }
        let on = with as f32 >= COVER * *n as f32;
        if on != self.on {
            info!(on, covered = with, junctions = n, "live {name}: coverage");
        }
        if on {
            debug!(%key, hit, "live {name}: frame");
        }
        self.shown = key;
        if !on && !self.on {
            return;
        }
        self.k = k;
        self.on = on;
        self.version += 1;
    }
}

/// Live rain (`radar.rs` frames).
#[derive(Resource, Default, Deref, DerefMut)]
pub struct LiveRain(pub LiveField);

/// Live sun (cloud frames and the sun's elevation over the region).
#[derive(Resource, Default, Deref, DerefMut)]
pub struct LiveSun(pub LiveField);

fn percent(v: u8) -> Option<f32> {
    (v <= 100).then_some(f32::from(v))
}

/// Read the newest frames (rain on each new frame; sun also as the sun moves).
pub fn update(
    base: Res<BaseRules>,
    region: Option<Res<elements_shared::geo::RegionInfo>>,
    mut rain: ResMut<LiveRain>,
    mut sun: ResMut<LiveSun>,
    nodes: Query<(&GeoNode, &ElemIdx)>,
) {
    let w = &base.0.weather;
    if w.enabled && w.live() {
        rain.step(
            "rain",
            crate::radar::frame(),
            |f| f.time.clone(),
            &nodes,
            mm_h,
            |mm| w.live_intensity(mm),
        );
    }
    if let Some(region) = region.filter(|_| w.enabled && w.live_sun()) {
        let now = crate::analytics::now_s();
        let elev = sun_elevation(region.centroid_lat, region.centroid_lon, now);
        // Half a degree of sun is enough of a change to recompute.
        let key = |f: &Frame| format!("{} {}", f.time, (elev * 2.0).round().clamp(-1.0, 99.0));
        sun.step("sun", crate::radar::clouds(), key, &nodes, percent, |c| {
            w.sun_intensity(c, elev)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{expire_sources, ElementSource, GameRules, SourceKind};
    use crate::testkit::{advance, ent, line, world};
    use bevy::ecs::system::RunSystemOnce;
    use elements_shared::prelude::Faction;
    use elements_shared::rules::Rules;
    use elements_shared::weather::Weather;

    fn rules() -> Rules {
        Rules {
            claim_ttl_s: 1000.0,
            weather: Weather {
                enabled: true,
                live_decay: 0.3,
                live_natural: 0.5,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn idx(w: &World, id: i64) -> usize {
        w.get::<ElemIdx>(ent(w, id)).unwrap().0
    }

    #[test]
    fn water_ages_faster_in_the_sun_and_natural_water_weakens() {
        let mut r = rules();
        r.weather.sun_decay = 0.3;
        r.weather.sun_natural = 0.5;
        let (n, e) = line(5, 200.0, &[]);
        let mut w = world(r.clone(), n, e);
        let claim = ElementSource::claim(
            &r,
            Faction::Water,
            SourceKind::Claim,
            1000.0,
            Some(1),
            1.0,
            false,
            0.0,
        );
        let (sunny, shade) = (ent(&w, 1), ent(&w, 3));
        w.entity_mut(sunny).insert(claim);
        w.entity_mut(shade).insert(claim);
        let mut k = vec![0.0; 5];
        k[idx(&w, 1)] = 1.0;
        w.insert_resource(LiveSun(LiveField::with(k)));
        let id = w.register_system(expire_sources);
        advance(&mut w, 100.0);
        w.run_system(id).unwrap();
        let age = |e| w.get::<ElementSource>(e).unwrap().age;
        assert!(
            (age(sunny) / age(shade) - 1.3).abs() < 1e-3,
            "full sun: 30 % faster"
        );

        w.insert_resource(LiveSun(LiveField::with(vec![1.0; 5])));
        w.insert_resource(BaseRules(r.clone()));
        w.insert_resource(crate::campaign::Campaign::spawn(None, "test".into()));
        w.run_system_once(crate::weather::apply).unwrap();
        let mut q = w.query::<&crate::sim::NaturalSource>();
        for n in q.iter(&w) {
            let want = r.natural_strength(n.0.faction)
                * if n.0.faction == Faction::Water {
                    0.5
                } else {
                    1.0
                };
            assert!((n.0.strength - want).abs() < 1e-5, "{:?}", n.0.faction);
        }
    }

    #[test]
    fn fire_ages_faster_in_the_rain_and_natural_fire_weakens() {
        let r = rules();
        let (n, e) = line(5, 200.0, &[]);
        let mut w = world(r.clone(), n, e);
        let claim = ElementSource::claim(
            &r,
            Faction::Fire,
            SourceKind::Claim,
            1000.0,
            Some(1),
            1.0,
            false,
            0.0,
        );
        let (wet, dry) = (ent(&w, 1), ent(&w, 3));
        w.entity_mut(wet).insert(claim);
        w.entity_mut(dry).insert(claim);
        let mut k = vec![0.0; 5];
        k[idx(&w, 1)] = 1.0;
        w.insert_resource(LiveRain(LiveField::with(k)));
        let id = w.register_system(expire_sources);
        advance(&mut w, 100.0);
        w.run_system(id).unwrap();
        let age = |e| w.get::<ElementSource>(e).unwrap().age;
        assert!(
            (age(wet) / age(dry) - 1.3).abs() < 1e-3,
            "full rain: 30 % faster"
        );

        w.insert_resource(BaseRules(r.clone()));
        w.insert_resource(crate::campaign::Campaign::spawn(None, "test".into()));
        w.run_system_once(crate::weather::apply).unwrap();
        // Every junction wet: natural fire at half strength, water unchanged.
        w.insert_resource(LiveRain(LiveField::with(vec![1.0; 5])));
        w.run_system_once(crate::weather::apply).unwrap();
        let mut q = w.query::<&crate::sim::NaturalSource>();
        let nat: Vec<(Faction, f32)> = q.iter(&w).map(|n| (n.0.faction, n.0.strength)).collect();
        assert!(nat.iter().any(|n| n.0 == Faction::Fire));
        for (f, s) in nat {
            let want = r.natural_strength(f) * if f == Faction::Fire { 0.5 } else { 1.0 };
            assert!((s - want).abs() < 1e-5, "{f:?}: {s} vs {want}");
        }
        assert_eq!(
            w.resource::<GameRules>().0.natural_fire_strength,
            r.natural_fire_strength
        );
    }
}

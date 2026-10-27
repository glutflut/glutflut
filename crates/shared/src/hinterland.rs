//! The hinterland:
//! sparks of the other element near players with nothing to do, and supply
//! from anchors deep in a side's territory to its sources at the front, and
//! permanent player sources.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::field::EdgeCosts;

/// Glutnest / Quelle: when a player has nothing to claim within `idle_m`, an
/// ownerless source of the other element sometimes appears `min_m..max_m`
/// away in the player's territory. It grows until put out with a claim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sparks {
    pub enabled: bool,
    /// "Nothing to do": no node that isn't the player's side within this.
    pub idle_m: f32,
    /// Chance per idle minute of one player.
    pub per_min: f32,
    pub min_m: f32,
    pub max_m: f32,
    /// Only while the player's side owns at least this share of the owned nodes (0 = always).
    pub share: f32,
    /// At most this many at once against one side.
    pub max: u32,
    /// Strength at the start and after `grow_s`.
    pub start: f32,
    pub full: f32,
    pub grow_s: f32,
    /// Gone by itself after this.
    pub life_s: f32,
    /// Extra points for putting one out.
    pub points: i64,
}

/// A source rises where a player waits (`[rise]`, lab round 28): water at a
/// sink, fire at a peak (at least `dh_m` below / above the neighbours' mean).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rise {
    pub enabled: bool,
    /// Standing this long within `radius_m` of a fitting junction.
    pub dwell_s: f32,
    pub radius_m: f32,
    pub dh_m: f32,
    /// Strength and life of the risen source (an anchor on own ground).
    pub strength: f32,
    pub life_s: f32,
    /// Once per player, and once per junction, within these.
    pub player_gap_s: f32,
    pub place_gap_s: f32,
    /// The hollow fills, the hilltop catches (round 28b): also up to
    /// `fill_max` junctions within `fill_m` that lie no higher (water) / no
    /// lower (fire) than the spot plus `fill_margin_m` (0 m = off).
    pub fill_m: f32,
    pub fill_margin_m: f32,
    pub fill_max: u32,
    /// The risen sources don't age for this long, then go (0 = they age as claims).
    pub hold_s: f32,
}

impl Default for Rise {
    fn default() -> Self {
        Self {
            enabled: false,
            dwell_s: 60.0,
            radius_m: 25.0,
            dh_m: 1.0,
            strength: 1.0,
            life_s: 900.0,
            player_gap_s: 1800.0,
            place_gap_s: 7200.0,
            fill_m: 0.0,
            fill_margin_m: 0.5,
            fill_max: 8,
            hold_s: 0.0,
        }
    }
}

impl Rise {
    /// Does a junction at `elev` belong to the hollow (water) / hilltop
    /// (fire) of a spot at `spot` m?
    pub fn fills(&self, f: crate::elements::Faction, spot: f64, elev: f64) -> bool {
        let m = f64::from(self.fill_margin_m);
        match f {
            crate::elements::Faction::Fire => elev >= spot - m,
            crate::elements::Faction::Water => elev <= spot + m,
        }
    }

    /// Does a junction `dh` m above its neighbours' mean fit `f`?
    pub fn fits(&self, f: crate::elements::Faction, dh: f32) -> bool {
        match f {
            crate::elements::Faction::Fire => dh >= self.dh_m,
            crate::elements::Faction::Water => dh <= -self.dh_m,
        }
    }
}

impl Default for Sparks {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_m: 120.0,
            per_min: 1.0,
            min_m: 120.0,
            max_m: 300.0,
            share: 0.0,
            max: 3,
            start: 0.3,
            full: 0.9,
            grow_s: 300.0,
            life_s: 900.0,
            points: 20,
        }
    }
}

impl Sparks {
    /// Strength `age` s after it appeared; `None` once it is over.
    pub fn strength(&self, age: f32) -> Option<f32> {
        (age < self.life_s).then(|| {
            let k = (age / self.grow_s.max(1.0)).min(1.0);
            self.start + (self.full - self.start) * k
        })
    }
}

/// Nachschub: a claim on ground your side already owns places an anchor
/// (Brunnen / Herd). Claimed sources within `range_m` of street through own
/// territory from an anchor age at `supplied` speed, the others at `cut`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Supply {
    pub enabled: bool,
    pub range_m: f32,
    pub anchor_ttl_s: f32,
    /// Natural sources supply too.
    pub natural: bool,
    /// Ageing speed of supplied and cut-off sources (1 = normal).
    pub supplied: f32,
    pub cut: f32,
    /// Points for an anchor (a claim on own ground brings none otherwise).
    pub anchor_points: i64,
    /// An anchor only where the nearest one is farther than this (0 = anywhere).
    pub anchor_gap_m: f32,
}

impl Default for Supply {
    fn default() -> Self {
        Self {
            enabled: false,
            range_m: 500.0,
            anchor_ttl_s: 900.0,
            natural: false,
            supplied: 0.75,
            cut: 1.5,
            anchor_points: 10,
            anchor_gap_m: 200.0,
        }
    }
}

impl Keep {
    /// Permanent sources a player at `level` may have.
    pub fn allowed(&self, level: u8) -> u32 {
        self.per_player + self.per_level * u32::from(level.saturating_sub(1))
    }
}

impl Supply {
    /// Ageing factor of a claimed source at supply distance `d` (m).
    pub fn speed(&self, d: f32) -> f32 {
        if !self.enabled {
            1.0
        } else if d <= self.range_m {
            self.supplied
        } else {
            self.cut
        }
    }
}

/// Dauerquelle (Glut / Eis): one source per player that does not run out. It
/// fades from `strength` to `floor` over `fade_s`; each enemy claim that can't
/// take it over wears it down by `hit`. Its owner tops it up with a claim on it.
/// Takeovers follow `Rules::takeover_below` as for any claimed source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keep {
    pub enabled: bool,
    pub per_player: u32,
    /// One more per level above 1 (with `[progression]`).
    pub per_level: u32,
    /// At least this far from natural sources and other permanent sources.
    pub gap_m: f32,
    pub strength: f32,
    pub floor: f32,
    pub fade_s: f32,
    pub hit: f32,
    /// Persistence points per hour standing (paid out via a bearer ticket).
    pub points_per_h: f32,
    /// Gone after this long without a top-up (lost tickets, players who
    /// don't come back).
    pub expire_s: f32,
    /// Linked permanent sources: a player connects two of their own; at one
    /// of them, claims reach the junctions around the other (both ways, any
    /// distance), at `link_factor` of their strength. The other itself can't
    /// be topped up that way. Others see only that a source is linked.
    pub link: bool,
    pub link_factor: f32,
    /// A campaign start or end clears the map of permanent sources, except
    /// those at least this strong (0 = none survive).
    pub survive: f32,
}

impl Default for Keep {
    fn default() -> Self {
        Self {
            enabled: false,
            per_player: 1,
            per_level: 0,
            gap_m: 60.0,
            strength: 0.8,
            floor: 0.2,
            fade_s: 3600.0,
            hit: 0.1,
            points_per_h: 10.0,
            expire_s: 3.0 * 86_400.0,
            link: false,
            link_factor: 0.7,
            survive: 0.0,
        }
    }
}

impl Keep {
    /// Strength `age` s after placing or the last top-up, minus wear.
    pub fn strength(&self, age: f32, wear: f32) -> f32 {
        let k = (age / self.fade_s.max(1.0)).min(1.0);
        (self.strength - (self.strength - self.floor) * k - wear).max(0.0)
    }
}

/// Street lengths between nodes (dense indices), for distances along streets.
pub struct Streets {
    start: Vec<u32>,
    adj: Vec<(u32, f32)>,
}

#[derive(PartialEq)]
struct Item(f32, u32);

impl Eq for Item {}

impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}

impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Streets {
    pub fn new(n: usize, edges: &[EdgeCosts]) -> Self {
        let mut start = vec![0u32; n + 1];
        for e in edges {
            start[e.a + 1] += 1;
            start[e.b + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut fill: Vec<u32> = start[..n].to_vec();
        let mut adj = vec![(0u32, 0.0f32); start[n] as usize];
        for e in edges {
            adj[fill[e.a] as usize] = (e.b as u32, e.len);
            fill[e.a] += 1;
            adj[fill[e.b] as usize] = (e.a as u32, e.len);
            fill[e.b] += 1;
        }
        Self { start, adj }
    }

    /// Street distance from the nearest of `seeds` to every node, going on only
    /// from nodes where `pass` holds (a step out of it still arrives); `INFINITY`
    /// beyond `max_m`.
    pub fn reach(
        &self,
        seeds: impl IntoIterator<Item = usize>,
        pass: impl Fn(usize) -> bool,
        max_m: f32,
    ) -> Vec<f32> {
        let n = self.start.len() - 1;
        let mut d = vec![f32::INFINITY; n];
        let mut heap = BinaryHeap::new();
        for s in seeds {
            if s < n && d[s] > 0.0 {
                d[s] = 0.0;
                heap.push(Item(0.0, s as u32));
            }
        }
        while let Some(Item(di, i)) = heap.pop() {
            let i = i as usize;
            if di > d[i] || !pass(i) {
                continue;
            }
            for &(j, len) in &self.adj[self.start[i] as usize..self.start[i + 1] as usize] {
                let dj = di + len;
                if dj <= max_m && dj < d[j as usize] {
                    d[j as usize] = dj;
                    heap.push(Item(dj, j));
                }
            }
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: usize, len: f32) -> Vec<EdgeCosts> {
        (0..n - 1)
            .map(|i| EdgeCosts {
                a: i,
                b: i + 1,
                fire_fwd: len,
                fire_rev: len,
                water_fwd: len,
                water_rev: len,
                len,
                dh: 0.0,
            })
            .collect()
    }

    #[test]
    fn reach_stops_outside_own_ground_and_range() {
        let s = Streets::new(6, &line(6, 100.0));
        // Own ground 0..=2: node 3 is reached (the step out), 4 not.
        let d = s.reach([0], |i| i <= 2, 1000.0);
        assert_eq!(&d[..4], &[0.0, 100.0, 200.0, 300.0]);
        assert!(d[4].is_infinite());
        // Range cuts too.
        let d = s.reach([0], |_| true, 250.0);
        assert_eq!(d[2], 200.0);
        assert!(d[3].is_infinite());
        // Nearest of several seeds.
        let d = s.reach([0, 5], |_| true, 1000.0);
        assert_eq!(d[4], 100.0);
    }

    #[test]
    fn sparks_grow_then_end() {
        let s = Sparks::default();
        assert_eq!(s.strength(0.0), Some(0.3));
        assert!((s.strength(150.0).unwrap() - 0.6).abs() < 1e-6);
        assert_eq!(s.strength(600.0), Some(0.9));
        assert_eq!(s.strength(900.0), None);
    }

    #[test]
    fn keep_fades_to_floor_and_wears() {
        let k = Keep::default();
        assert_eq!(k.strength(0.0, 0.0), 0.8);
        assert!((k.strength(1800.0, 0.0) - 0.5).abs() < 1e-6);
        assert!((k.strength(1e6, 0.0) - 0.2).abs() < 1e-6);
        assert!((k.strength(0.0, 0.3) - 0.5).abs() < 1e-6);
        assert_eq!(k.strength(1e6, 1.0), 0.0);
    }

    #[test]
    fn supply_speeds() {
        let s = Supply {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(s.speed(100.0), 0.75);
        assert_eq!(s.speed(f32::INFINITY), 1.5);
        assert_eq!(Supply::default().speed(f32::INFINITY), 1.0);
    }
}

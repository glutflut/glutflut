//! Blocks: areas enclosed by streets. A side
//! holds one when it owns `[area] share` of the border junctions and the other
//! side none. Every minute held blocks pay points to the players whose sources
//! hold the border (by junctions), and the border's sources age by
//! `[area] age_factor`. Breaking a ring that players held for a while pays the
//! other side's players on its border (`[area] break_*`). The app fills held
//! blocks from the node states it gets.

use std::collections::HashMap;

use bevy::prelude::*;
use elements_shared::elements::ElementState;
use elements_shared::prelude::Faction;
use elements_shared::rules::Area;

/// A block as loaded: border junctions and outline rings (flat lon, lat).
pub struct Block {
    pub id: i32,
    pub area: f32,
    pub nodes: Vec<i64>,
    pub rings: Vec<Vec<f64>>,
}

#[derive(Resource, Default)]
pub struct Blocks {
    /// Id, area and border node ids, until the sim's dense indices exist.
    raw: Vec<(i32, f32, Vec<i64>)>,
    dense: Option<Vec<(i32, f32, Vec<u32>)>>,
    /// Ageing factor per dense node index (empty: 1 everywhere).
    pub slow: Vec<f32>,
    /// Per block: owner (0 none, 1 fire, 2 water), since when, held with a
    /// player's source on the border, last paid break.
    own: Vec<u8>,
    since: Vec<f64>,
    player: Vec<bool>,
    broken: Vec<f64>,
}

/// One minute of blocks: points per slot for held area and for broken rings.
#[derive(Default)]
pub struct Minute {
    pub area: HashMap<u32, f32>,
    pub broken: HashMap<u32, f32>,
    /// Area (ha) held per side (fire, water) in blocks with a player's
    /// source on the border (chapter goal "hold area").
    pub player_ha: [f64; 2],
    /// Area (ha) held per side, any holder.
    pub ha: [f64; 2],
    /// Held blocks: id and owner (1 fire, 2 water).
    pub held: Vec<(i32, u8)>,
    /// Held blocks with players' sources on the border: id and those slots.
    pub players: Vec<(i32, Vec<u32>)>,
}

impl Blocks {
    pub fn new(blocks: &[Block]) -> Self {
        Self {
            raw: blocks
                .iter()
                .map(|b| (b.id, b.area, b.nodes.clone()))
                .collect(),
            ..Default::default()
        }
    }

    pub fn slow_at(&self, i: usize) -> f32 {
        self.slow.get(i).copied().unwrap_or(1.0)
    }

    /// One minute at `now`: points per slot for held blocks and broken
    /// rings; sets `slow`. `holder`: per dense node, the slot whose source holds it.
    pub fn minute(
        &mut self,
        rules: &Area,
        state: &[(f32, f32)],
        holder: &[Option<u32>],
        now: f64,
        index: impl Fn(i64) -> Option<usize>,
    ) -> Minute {
        let mut out = Minute::default();
        if !rules.enabled || self.raw.is_empty() {
            self.slow.clear();
            return out;
        }
        let dense = self.dense.get_or_insert_with(|| {
            self.raw
                .iter()
                .filter_map(|(b, a, ids)| {
                    let n: Option<Vec<u32>> =
                        ids.iter().map(|&id| index(id).map(|i| i as u32)).collect();
                    Some((*b, *a, n?))
                })
                .collect()
        });
        let n = dense.len();
        if self.own.len() != n {
            self.own = vec![0; n];
            self.since = vec![now; n];
            self.player = vec![false; n];
            self.broken = vec![f64::NEG_INFINITY; n];
        }
        self.slow.clear();
        self.slow.resize(state.len(), 1.0);
        let side = |i: u32| {
            let (fire, water) = state[i as usize];
            ElementState { fire, water }.owner()
        };
        for (k, (id, area, border)) in dense.iter().enumerate() {
            let count = |f: Faction| border.iter().filter(|&&i| side(i) == Some(f)).count();
            let o = rules.owner(count(Faction::Fire), count(Faction::Water), border.len());
            let prev = self.own[k];
            if o != prev {
                let held = (now - self.since[k]) as f32;
                let since_break = (now - self.broken[k]) as f32;
                if prev != 0 && rules.breaks(held, self.player[k], since_break) {
                    // The other side's players on the border share the break.
                    let other = if prev == 1 {
                        Faction::Water
                    } else {
                        Faction::Fire
                    };
                    let by: Vec<u32> = border
                        .iter()
                        .filter(|&&i| side(i) == Some(other))
                        .filter_map(|&i| holder[i as usize])
                        .collect();
                    if !by.is_empty() {
                        let each = rules.break_points(*area) / by.len() as f32;
                        for slot in by {
                            *out.broken.entry(slot).or_default() += each;
                        }
                        self.broken[k] = now;
                    }
                }
                self.own[k] = o;
                self.since[k] = now;
                self.player[k] = false;
            }
            if o == 0 {
                continue;
            }
            out.ha[usize::from(o == 2)] += f64::from(*area) / 1e4;
            out.held.push((*id, o));
            let by: Vec<u32> = border.iter().filter_map(|&i| holder[i as usize]).collect();
            self.player[k] |= !by.is_empty();
            if !by.is_empty() {
                out.player_ha[usize::from(o == 2)] += f64::from(*area) / 1e4;
                let mut slots = by.clone();
                slots.sort_unstable();
                slots.dedup();
                out.players.push((*id, slots));
            }
            let each = rules.points(*area) / by.len().max(1) as f32;
            for slot in by {
                *out.area.entry(slot).or_default() += each;
            }
            for &i in border {
                self.slow[i as usize] = rules.age_factor;
            }
        }
        out
    }
}

/// Held area for the dashboard: per hour the mean and peak per side, per
/// block the minutes held by each side (local day). Rows go out when the hour
/// turns; a restart loses at most the running hour.
#[derive(Default)]
pub struct AreaLog {
    hour: i64,
    minutes: u32,
    ha: [f64; 2],
    player_ha: [f64; 2],
    max: [f64; 2],
    blocks: HashMap<i32, [i32; 2]>,
    /// Local day number of the running hour (`clock.rs`).
    day: i64,
}

impl AreaLog {
    /// Count one minute at `now` (local day `day`); the finished hour's rows, if it turned.
    pub fn add(&mut self, now: f64, day: i64, m: &Minute) -> Vec<crate::analytics::Row> {
        let hour = (now as i64).div_euclid(3600) * 3600;
        let rows = if hour != self.hour {
            self.take()
        } else {
            Vec::new()
        };
        self.hour = hour;
        self.day = day;
        self.minutes += 1;
        for i in 0..2 {
            self.ha[i] += m.ha[i];
            self.player_ha[i] += m.player_ha[i];
            self.max[i] = self.max[i].max(m.ha[i]);
        }
        for &(id, o) in &m.held {
            self.blocks.entry(id).or_default()[usize::from(o == 2)] += 1;
        }
        rows
    }

    /// The running hour's rows (empty if nothing was counted).
    pub fn take(&mut self) -> Vec<crate::analytics::Row> {
        let n = self.minutes;
        if n == 0 {
            return Vec::new();
        }
        let mean = |v: [f64; 2]| v.map(|x| (x / f64::from(n)) as f32);
        let mut rows = vec![crate::analytics::Row::AreaHour {
            hour: self.hour,
            minutes: n.min(60) as i16,
            ha: mean(self.ha),
            player_ha: mean(self.player_ha),
            max: self.max.map(|x| x as f32),
        }];
        if !self.blocks.is_empty() {
            rows.push(crate::analytics::Row::BlockMinutes {
                day: self.day,
                blocks: self.blocks.drain().map(|(id, [f, w])| (id, f, w)).collect(),
            });
        }
        *self = Self::default();
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_block_pays_its_holders_slows_its_border_and_pays_a_break() {
        let blocks = [Block {
            id: 1,
            area: 20_000.0,
            nodes: vec![10, 11, 12],
            rings: Vec::new(),
        }];
        let mut b = Blocks::new(&blocks);
        let rules = Area {
            enabled: true,
            share: 1.0,
            points_per_ha_min: 10.0,
            age_factor: 0.5,
            break_minutes: 5.0,
            ..Default::default()
        };
        let index = |id: i64| (10..=13).contains(&id).then(|| (id - 10) as usize);
        let (fire, water) = ((0.9, 0.0), (0.0, 0.9));
        // Node 3 is outside the block.
        let state = [fire, fire, fire, water];
        let holder = [Some(7), Some(7), Some(8), None];
        let t0 = 1000.0;
        let m = b.minute(&rules, &state, &holder, t0, index);
        assert!((m.area[&7] - 2.0 * 20.0 / 3.0).abs() < 1e-4);
        assert!((m.area[&8] - 20.0 / 3.0).abs() < 1e-4);
        assert_eq!(b.slow_at(0), 0.5);
        assert_eq!(b.slow_at(3), 1.0);
        // Held 5 minutes, then water (slot 9) takes a border junction: a break.
        b.minute(&rules, &state, &holder, t0 + 300.0, index);
        let broken = [fire, fire, water, water];
        let attackers = [Some(7), Some(7), Some(9), None];
        let m = b.minute(&rules, &broken, &attackers, t0 + 360.0, index);
        assert!(m.area.is_empty());
        // 5 minutes of 20 points, capped at 50.
        assert_eq!(m.broken[&9], 50.0);
        assert_eq!(b.slow_at(0), 1.0);
        // Closed and broken again within the hour: no second break.
        b.minute(&rules, &state, &holder, t0 + 420.0, index);
        b.minute(&rules, &state, &holder, t0 + 800.0, index);
        let m = b.minute(&rules, &broken, &attackers, t0 + 860.0, index);
        assert!(m.broken.is_empty());
    }

    #[test]
    fn area_log_writes_each_hour() {
        use crate::analytics::Row;
        let mut log = AreaLog::default();
        let m = |f: f64, w: f64, held: Vec<(i32, u8)>| Minute {
            ha: [f, w],
            player_ha: [f / 2.0, 0.0],
            held,
            ..Default::default()
        };
        let h = 3600.0 * 1000.0;
        let d = 11_574; // the local day number the caller passes
        assert!(log.add(h + 60.0, d, &m(2.0, 0.0, vec![(1, 1)])).is_empty());
        assert!(log
            .add(h + 120.0, d, &m(4.0, 1.0, vec![(1, 1), (2, 2)]))
            .is_empty());
        let rows = log.add(h + 3660.0, d, &m(1.0, 1.0, vec![(2, 2)]));
        let Row::AreaHour {
            hour,
            minutes,
            ha,
            player_ha,
            max,
        } = &rows[0]
        else {
            panic!()
        };
        assert_eq!((*hour, *minutes), (h as i64, 2));
        assert_eq!(
            (*ha, *player_ha, *max),
            ([3.0, 0.5], [1.5, 0.0], [4.0, 1.0])
        );
        let Row::BlockMinutes { day, blocks } = &rows[1] else {
            panic!()
        };
        assert_eq!(*day, d);
        let mut b = blocks.clone();
        b.sort();
        assert_eq!(b, vec![(1, 2, 0), (2, 0, 1)]);
        // The next hour started with one minute.
        let rows = log.take();
        assert!(matches!(rows[0], Row::AreaHour { minutes: 1, .. }));
        assert!(log.take().is_empty());
    }

    #[test]
    fn natural_rings_and_short_holds_pay_no_break() {
        let blocks = [Block {
            id: 1,
            area: 5_000.0,
            nodes: vec![10, 11],
            rings: Vec::new(),
        }];
        let rules = Area {
            enabled: true,
            break_minutes: 5.0,
            ..Default::default()
        };
        let index = |id: i64| (10..=11).contains(&id).then(|| (id - 10) as usize);
        let (fire, water) = ((0.9, 0.0), (0.0, 0.9));
        // Held by natural sources only (no holder), long: no break.
        let mut b = Blocks::new(&blocks);
        b.minute(&rules, &[fire, fire], &[None, None], 0.0, index);
        b.minute(&rules, &[fire, fire], &[None, None], 600.0, index);
        let m = b.minute(&rules, &[fire, water], &[None, Some(2)], 660.0, index);
        assert!(m.broken.is_empty());
        // Held by a player, but only 2 minutes: no break.
        let mut b = Blocks::new(&blocks);
        b.minute(&rules, &[fire, fire], &[Some(1), None], 0.0, index);
        let m = b.minute(&rules, &[fire, water], &[Some(1), Some(2)], 120.0, index);
        assert!(m.broken.is_empty());
    }
}

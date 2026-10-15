//! Element spread on the road graph.
//!
//! Max-reach (bottleneck) field: each step a node takes the strongest signal
//! from a neighbour, `v[i] = max_j v[j] * t(j -> i)`, with transmission
//! `t = reach_scale / (reach_scale + cost)`. Directional costs make fire climb
//! and water descend. Node affinity (surroundings) scales transmission into a
//! node: fire-leaning places let fire in more easily, water-leaning ones water.
//! Sources are pinned at their strength. Monotone and bounded, so it converges.

use serde::{Deserialize, Serialize};

use crate::elements::Faction;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldParams {
    /// Higher = farther reach per edge.
    pub reach_scale: f32,
    /// Cost floor so tiny edges don't transmit at ~1.0.
    pub min_cost: f32,
    /// How strongly node affinity (-1 water .. +1 fire) tilts transmission.
    pub affinity_weight: f32,
    /// Slope from node heights instead of the imported costs (0 = imported):
    /// cost = length * clamp(1 -/+ slope_k * grade, slope_min, slope_max),
    /// cheaper for fire uphill and for water downhill.
    pub slope_k: f32,
    pub slope_min: f32,
    pub slope_max: f32,
    /// Against the element's nature, per metre of height (0 = off): water
    /// climbing `h` m keeps `exp(-h / water_climb_m)` of its transmission,
    /// fire going down `h` m keeps `exp(-h / fire_drop_m)`. Flat streets are
    /// unaffected, long gentle ones and short steep ones alike.
    pub water_climb_m: f32,
    pub fire_drop_m: f32,
}

impl Default for FieldParams {
    fn default() -> Self {
        Self {
            reach_scale: 200.0,
            min_cost: 1.0,
            affinity_weight: 0.0,
            slope_k: 0.0,
            slope_min: 0.4,
            slope_max: 3.0,
            water_climb_m: 0.0,
            fire_drop_m: 0.0,
        }
    }
}

/// Directed element costs of one edge between dense node indices `a` -> `b`.
#[derive(Clone, Copy, Debug)]
pub struct EdgeCosts {
    pub a: usize,
    pub b: usize,
    pub fire_fwd: f32,
    pub fire_rev: f32,
    pub water_fwd: f32,
    pub water_rev: f32,
    /// Length (m) and height gain `a` -> `b` (m), for `FieldParams::slope_k`.
    pub len: f32,
    pub dh: f32,
}

/// Grade over at least this run: a DEM can't resolve slope on shorter edges.
const MIN_RUN_M: f32 = 5.0;
const MAX_GRADE: f32 = 0.4;

impl EdgeCosts {
    /// Costs `(fire_fwd, fire_rev, water_fwd, water_rev)`: the imported ones,
    /// or from the grade when `p.slope_k` is set.
    pub fn directed(&self, p: &FieldParams) -> (f32, f32, f32, f32) {
        if p.slope_k <= 0.0 {
            return (self.fire_fwd, self.fire_rev, self.water_fwd, self.water_rev);
        }
        let g = (self.dh / self.len.max(MIN_RUN_M)).clamp(-MAX_GRADE, MAX_GRADE);
        let m = |x: f32| self.len * x.clamp(p.slope_min, p.slope_max);
        let (up, down) = (m(1.0 - p.slope_k * g), m(1.0 + p.slope_k * g));
        // Forward climbs by g: fire pays `up`, water `down`; reverse the other way.
        (up, down, down, up)
    }
}

/// Transmission never reaches 1, so the field stays bounded.
const MAX_T: f32 = 0.97;

pub struct Field {
    /// Inbound `(neighbour, fire_t, water_t)` of node `i` at
    /// `inbound[start[i]..start[i + 1]]` (one flat array: a city has ~1M nodes).
    start: Vec<u32>,
    inbound: Vec<(u32, f32, f32)>,
}

impl Field {
    pub fn new(n: usize, edges: &[EdgeCosts], affinity: &[f32], p: &FieldParams) -> Self {
        let t = |cost: f32| p.reach_scale / (p.reach_scale + cost.max(p.min_cost));
        let tilt = |i: usize, sign: f32| {
            let a = affinity.get(i).copied().unwrap_or(0.0);
            (1.0 + sign * p.affinity_weight * a).max(0.0)
        };
        let mut start = vec![0u32; n + 1];
        for e in edges {
            start[e.a + 1] += 1;
            start[e.b + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut fill: Vec<u32> = start[..n].to_vec();
        let mut inbound = vec![(0u32, 0.0f32, 0.0f32); start[n] as usize];
        // Height against the element: `rise` m up for water, down for fire.
        let against = |rise: f32, scale: f32| {
            if scale > 0.0 && rise > 0.0 {
                (-rise / scale).exp()
            } else {
                1.0
            }
        };
        for e in edges {
            let (ff, fr, wf, wr) = e.directed(p);
            let (up_w, down_f) = (
                against(e.dh, p.water_climb_m),
                against(-e.dh, p.fire_drop_m),
            );
            let (up_w_rev, down_f_rev) = (
                against(-e.dh, p.water_climb_m),
                against(e.dh, p.fire_drop_m),
            );
            let fb = (t(ff) * tilt(e.b, 1.0) * down_f).min(MAX_T);
            let wb = (t(wf) * tilt(e.b, -1.0) * up_w).min(MAX_T);
            let fa = (t(fr) * tilt(e.a, 1.0) * down_f_rev).min(MAX_T);
            let wa = (t(wr) * tilt(e.a, -1.0) * up_w_rev).min(MAX_T);
            inbound[fill[e.b] as usize] = (e.a as u32, fb, wb);
            fill[e.b] += 1;
            inbound[fill[e.a] as usize] = (e.b as u32, fa, wa);
            fill[e.a] += 1;
        }
        Self { start, inbound }
    }

    pub fn len(&self) -> usize {
        self.start.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn inbound(&self, i: usize) -> &[(u32, f32, f32)] {
        &self.inbound[self.start[i] as usize..self.start[i + 1] as usize]
    }

    /// Neighbour indices of node `i`.
    pub fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.inbound(i).iter().map(|&(j, _, _)| j as usize)
    }

    /// Hilltops and sinks: nodes higher (lower) than every node within `hops`
    /// by at least `margin` m (`elev` per node).
    pub fn extrema(&self, elev: &[f32], hops: u32, margin: f32) -> (Vec<u32>, Vec<u32>) {
        let n = self.len();
        let (mut peaks, mut sinks) = (Vec::new(), Vec::new());
        // Visited stamps and BFS frontiers reused across nodes.
        let mut seen = vec![u32::MAX; n];
        let (mut cur, mut next) = (Vec::new(), Vec::new());
        for i in 0..n {
            let (mut hi, mut lo) = (f32::MIN, f32::MAX);
            seen[i] = i as u32;
            cur.clear();
            cur.push(i);
            for _ in 0..hops {
                next.clear();
                for &a in &cur {
                    for b in self.neighbours(a) {
                        if seen[b] != i as u32 {
                            seen[b] = i as u32;
                            hi = hi.max(elev[b]);
                            lo = lo.min(elev[b]);
                            next.push(b);
                        }
                    }
                }
                std::mem::swap(&mut cur, &mut next);
            }
            if hi == f32::MIN {
                continue; // no neighbours
            }
            if elev[i] >= hi + margin {
                peaks.push(i as u32);
            } else if elev[i] <= lo - margin {
                sinks.push(i as u32);
            }
        }
        (peaks, sinks)
    }

    /// One step: `next` from `cur`, then pin sources `(node, faction, strength)`.
    pub fn step(
        &self,
        cur: &[(f32, f32)],
        sources: impl IntoIterator<Item = (usize, Faction, f32)>,
        next: &mut [(f32, f32)],
    ) {
        for (i, slot) in next.iter_mut().enumerate().take(self.len()) {
            let (mut fire, mut water) = (0.0f32, 0.0f32);
            for &(j, tf, tw) in self.inbound(i) {
                let j = j as usize;
                fire = fire.max(cur[j].0 * tf);
                water = water.max(cur[j].1 * tw);
            }
            *slot = (fire, water);
        }
        for (i, f, s) in sources {
            match f {
                Faction::Fire => next[i].0 = next[i].0.max(s),
                Faction::Water => next[i].1 = next[i].1.max(s),
            }
        }
    }

    /// Like [`Field::step`], and also tracks which source node each value comes
    /// from (`NO_ORIGIN` if none): the strongest neighbour passes its origin on.
    pub fn step_traced(
        &self,
        cur: &[(f32, f32)],
        cur_origin: &[(u32, u32)],
        sources: impl IntoIterator<Item = (usize, Faction, f32)>,
        next: &mut [(f32, f32)],
        next_origin: &mut [(u32, u32)],
    ) {
        for i in 0..self.len() {
            let (mut fire, mut water) = (0.0f32, 0.0f32);
            let (mut of, mut ow) = (NO_ORIGIN, NO_ORIGIN);
            for &(j, tf, tw) in self.inbound(i) {
                let j = j as usize;
                if cur[j].0 * tf > fire {
                    fire = cur[j].0 * tf;
                    of = cur_origin[j].0;
                }
                if cur[j].1 * tw > water {
                    water = cur[j].1 * tw;
                    ow = cur_origin[j].1;
                }
            }
            next[i] = (fire, water);
            next_origin[i] = (of, ow);
        }
        for (i, f, s) in sources {
            match f {
                Faction::Fire if s >= next[i].0 => (next[i].0, next_origin[i].0) = (s, i as u32),
                Faction::Water if s >= next[i].1 => (next[i].1, next_origin[i].1) = (s, i as u32),
                _ => {}
            }
        }
    }
}

/// No source reaches this node.
pub const NO_ORIGIN: u32 = u32::MAX;

/// [`Field::step_traced`] that remembers its last step and then recomputes
/// only the nodes whose inputs changed: a neighbour's value or origin, or the
/// node's own pin. Same result as the full step, bit for bit; a field at rest
/// costs a compare per node instead of a pass over all edges.
pub struct Quiet {
    /// Values after the last step and the source node each comes from; the
    /// origins are the next step's input.
    pub next: Vec<(f32, f32)>,
    pub origin: Vec<(u32, u32)>,
    /// Inputs of the last step: values and pins (sorted by node, stable).
    cur: Vec<(f32, f32)>,
    pins: Vec<(usize, Faction, f32)>,
    /// Nodes whose origin changed in the last step.
    moved: Vec<u32>,
    /// Next step recomputes everything (start, new field parameters).
    all: bool,
    stamp: Vec<u32>,
    round: u32,
    todo: Vec<u32>,
    /// Nodes recomputed by the last step.
    pub recomputed: usize,
}

impl Default for Quiet {
    fn default() -> Self {
        Self::new(Vec::new(), Vec::new())
    }
}

impl Quiet {
    /// Start from these values and origins (the first step is a full one).
    pub fn new(next: Vec<(f32, f32)>, origin: Vec<(u32, u32)>) -> Self {
        let n = next.len();
        Self {
            next,
            origin,
            cur: vec![(0.0, 0.0); n],
            pins: Vec::new(),
            moved: Vec::new(),
            all: true,
            stamp: vec![0; n],
            round: 0,
            todo: Vec::new(),
            recomputed: 0,
        }
    }

    /// The field changed (weather): the next step recomputes everything.
    pub fn reset(&mut self) {
        self.all = true;
    }
}

impl Field {
    /// One node of [`Field::step_traced`] before pinning.
    fn reach(
        &self,
        i: usize,
        cur: &[(f32, f32)],
        origin: &[(u32, u32)],
    ) -> ((f32, f32), (u32, u32)) {
        let (mut fire, mut water) = (0.0f32, 0.0f32);
        let (mut of, mut ow) = (NO_ORIGIN, NO_ORIGIN);
        for &(j, tf, tw) in self.inbound(i) {
            let j = j as usize;
            if cur[j].0 * tf > fire {
                fire = cur[j].0 * tf;
                of = origin[j].0;
            }
            if cur[j].1 * tw > water {
                water = cur[j].1 * tw;
                ow = origin[j].1;
            }
        }
        ((fire, water), (of, ow))
    }

    /// [`Field::step_traced`] from `cur` and the last step's origins, into
    /// `q.next`/`q.origin`, recomputing only what can have changed.
    pub fn step_quiet(
        &self,
        q: &mut Quiet,
        cur: &[(f32, f32)],
        sources: impl IntoIterator<Item = (usize, Faction, f32)>,
    ) {
        let n = self.len();
        let mut pins: Vec<_> = sources.into_iter().collect();
        pins.sort_by_key(|p| p.0);
        q.round = q.round.wrapping_add(1);
        if q.round == 0 {
            q.stamp.fill(0);
            q.round = 1;
        }
        let round = q.round;
        q.todo.clear();
        {
            let (stamp, todo) = (&mut q.stamp, &mut q.todo);
            let mut mark = |i: usize| {
                if stamp[i] != round {
                    stamp[i] = round;
                    todo.push(i as u32);
                }
            };
            if q.all {
                (0..n).for_each(&mut mark);
            } else {
                // A neighbour's value or origin changed: its neighbours.
                for (j, (a, b)) in cur.iter().zip(&q.cur).enumerate() {
                    if a.0.to_bits() != b.0.to_bits() || a.1.to_bits() != b.1.to_bits() {
                        self.neighbours(j).for_each(&mut mark);
                    }
                }
                for &j in &q.moved {
                    self.neighbours(j as usize).for_each(&mut mark);
                }
                // A pin came, went or changed: that node.
                let (old, new) = (&q.pins, &pins);
                let (mut a, mut b) = (0, 0);
                while a < old.len() || b < new.len() {
                    let ia = old.get(a).map_or(usize::MAX, |p| p.0);
                    let ib = new.get(b).map_or(usize::MAX, |p| p.0);
                    let i = ia.min(ib);
                    let (ea, eb) = (
                        old[a..].iter().take_while(|p| p.0 == i).count(),
                        new[b..].iter().take_while(|p| p.0 == i).count(),
                    );
                    let same = ea == eb
                        && old[a..a + ea]
                            .iter()
                            .zip(&new[b..b + eb])
                            .all(|(x, y)| x.1 == y.1 && x.2.to_bits() == y.2.to_bits());
                    if !same {
                        mark(i);
                    }
                    (a, b) = (a + ea, b + eb);
                }
            }
        }
        q.all = false;
        // Compute from the last origins, then write (origins are inputs too).
        let results: Vec<(u32, (f32, f32), (u32, u32))> = q
            .todo
            .iter()
            .map(|&i| {
                let i = i as usize;
                let (mut v, mut o) = self.reach(i, cur, &q.origin);
                let from = pins.partition_point(|p| p.0 < i);
                for &(_, f, s) in pins[from..].iter().take_while(|p| p.0 == i) {
                    match f {
                        Faction::Fire if s >= v.0 => (v.0, o.0) = (s, i as u32),
                        Faction::Water if s >= v.1 => (v.1, o.1) = (s, i as u32),
                        _ => {}
                    }
                }
                (i as u32, v, o)
            })
            .collect();
        q.moved.clear();
        for &(i, v, o) in &results {
            let i = i as usize;
            if q.origin[i] != o {
                q.origin[i] = o;
                q.moved.push(i as u32);
            }
            q.next[i] = v;
        }
        q.recomputed = results.len();
        q.cur.clear();
        q.cur.extend_from_slice(cur);
        q.pins = pins;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: usize, cost: f32) -> Vec<EdgeCosts> {
        (0..n - 1)
            .map(|i| EdgeCosts {
                a: i,
                b: i + 1,
                fire_fwd: cost,
                fire_rev: cost,
                water_fwd: cost,
                water_rev: cost,
                len: cost,
                dh: 0.0,
            })
            .collect()
    }

    fn settle(f: &Field, src: &[(usize, Faction, f32)]) -> Vec<(f32, f32)> {
        let mut cur = vec![(0.0, 0.0); f.len()];
        let mut next = cur.clone();
        for _ in 0..100 {
            f.step(&cur, src.iter().copied(), &mut next);
            std::mem::swap(&mut cur, &mut next);
        }
        cur
    }

    #[test]
    fn decays_along_a_line() {
        let f = Field::new(5, &line(5, 200.0), &[], &FieldParams::default());
        let v = settle(&f, &[(0, Faction::Fire, 1.0)]);
        assert!((v[1].0 - 0.5).abs() < 1e-5 && (v[2].0 - 0.25).abs() < 1e-5);
        assert_eq!(v[1].1, 0.0);
    }

    #[test]
    fn traces_the_strongest_source() {
        let f = Field::new(5, &line(5, 200.0), &[], &FieldParams::default());
        let src = [(0, Faction::Fire, 1.0), (4, Faction::Fire, 0.6)];
        let mut cur = vec![(0.0, 0.0); 5];
        let mut org = vec![(NO_ORIGIN, NO_ORIGIN); 5];
        let (mut next, mut next_org) = (cur.clone(), org.clone());
        for _ in 0..20 {
            f.step_traced(&cur, &org, src, &mut next, &mut next_org);
            std::mem::swap(&mut cur, &mut next);
            std::mem::swap(&mut org, &mut next_org);
        }
        // Node 2: 0.25 from 0 beats 0.15 from 4; node 3: 0.3 from 4 beats 0.125.
        let fire: Vec<u32> = org.iter().map(|o| o.0).collect();
        assert_eq!(fire, vec![0, 0, 0, 4, 4]);
        assert!(org.iter().all(|o| o.1 == NO_ORIGIN));
    }

    #[test]
    fn affinity_tilts_transmission() {
        let p = FieldParams {
            affinity_weight: 0.5,
            ..Default::default()
        };
        // node 1 fire-leaning (+1): fire enters at 0.5 * 1.5, water at 0.5 * 0.5
        let f = Field::new(2, &line(2, 200.0), &[0.0, 1.0], &p);
        let v = settle(&f, &[(0, Faction::Fire, 1.0), (0, Faction::Water, 1.0)]);
        assert!((v[1].0 - 0.75).abs() < 1e-5 && (v[1].1 - 0.25).abs() < 1e-5);
    }

    #[test]
    fn height_against_the_element() {
        // a -> b climbs 3 m: water into b loses 1/e, fire into a (downhill) too.
        let e = EdgeCosts {
            a: 0,
            b: 1,
            fire_fwd: 100.0,
            fire_rev: 100.0,
            water_fwd: 100.0,
            water_rev: 100.0,
            len: 100.0,
            dh: 3.0,
        };
        let p = FieldParams {
            water_climb_m: 3.0,
            fire_drop_m: 3.0,
            ..Default::default()
        };
        let base = Field::new(2, &[e], &[], &FieldParams::default());
        let f = Field::new(2, &[e], &[], &p);
        let k = (-1.0f32).exp();
        // inbound of b = (a, fire, water): water climbs, fire climbs (no penalty).
        assert!((f.inbound(1)[0].2 - base.inbound(1)[0].2 * k).abs() < 1e-6);
        assert_eq!(f.inbound(1)[0].1, base.inbound(1)[0].1);
        // inbound of a: fire descends, water descends (no penalty).
        assert!((f.inbound(0)[0].1 - base.inbound(0)[0].1 * k).abs() < 1e-6);
        assert_eq!(f.inbound(0)[0].2, base.inbound(0)[0].2);
    }

    #[test]
    fn hilltops_and_sinks() {
        // Heights 5 9 5 3 1 3 5 in a line, 2 hops: hilltops 1 (by 4 m) and the
        // end 6 (2 m), a sink at 4 (2 m).
        let f = Field::new(7, &line(7, 100.0), &[], &FieldParams::default());
        let elev = [5.0, 9.0, 5.0, 3.0, 1.0, 3.0, 5.0];
        assert_eq!(f.extrema(&elev, 2, 1.0), (vec![1, 6], vec![4]));
        // A 3 m margin keeps only the 4 m hilltop.
        assert_eq!(f.extrema(&elev, 2, 3.0), (vec![1], vec![]));
    }

    #[test]
    fn slope_favours_fire_up_and_water_down() {
        // 100 m climbing 10 m (10 %): with slope_k 5, fire pays 0.5x uphill.
        let e = EdgeCosts {
            a: 0,
            b: 1,
            fire_fwd: 100.0,
            fire_rev: 100.0,
            water_fwd: 100.0,
            water_rev: 100.0,
            len: 100.0,
            dh: 10.0,
        };
        let p = FieldParams {
            slope_k: 5.0,
            ..Default::default()
        };
        let (ff, fr, wf, wr) = e.directed(&p);
        assert_eq!((ff, fr, wf, wr), (50.0, 150.0, 150.0, 50.0));
        // slope_k 0: the imported costs.
        assert_eq!(
            e.directed(&FieldParams::default()),
            (100.0, 100.0, 100.0, 100.0)
        );
    }

    /// Small deterministic generator (no rand in shared).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
        fn below(&mut self, n: usize) -> usize {
            self.next() as usize % n.max(1)
        }
        fn unit(&mut self) -> f32 {
            self.next() as f32 / (1u64 << 31) as f32
        }
    }

    /// Random graph: a ring (connected) plus chords, random costs.
    fn random_graph(r: &mut Lcg, n: usize) -> Field {
        let mut edges = Vec::new();
        let mut e = |a: usize, b: usize, r: &mut Lcg| {
            edges.push(EdgeCosts {
                a,
                b,
                fire_fwd: 5.0 + 400.0 * r.unit(),
                fire_rev: 5.0 + 400.0 * r.unit(),
                water_fwd: 5.0 + 400.0 * r.unit(),
                water_rev: 5.0 + 400.0 * r.unit(),
                len: 50.0,
                dh: 0.0,
            })
        };
        for i in 0..n {
            e(i, (i + 1) % n, r);
        }
        for _ in 0..n / 2 {
            let (a, b) = (r.below(n), r.below(n));
            if a != b {
                e(a, b, r);
            }
        }
        let aff: Vec<f32> = (0..n).map(|_| r.unit() * 2.0 - 1.0).collect();
        let p = FieldParams {
            affinity_weight: 0.3,
            ..Default::default()
        };
        Field::new(n, &edges, &aff, &p)
    }

    /// The quiet step equals the full step bit for bit, with values fed back
    /// (and now and then disturbed from outside), pins that come, go, age and
    /// change sides, and a reset now and then.
    #[test]
    fn quiet_equals_full() {
        for seed in 1..=20u64 {
            let mut r = Lcg(seed);
            let n = 20 + r.below(200);
            let f = random_graph(&mut r, n);
            let mut cur = vec![(0.0f32, 0.0f32); n];
            let mut origin = vec![(NO_ORIGIN, NO_ORIGIN); n];
            let (mut next, mut next_origin) = (cur.clone(), origin.clone());
            let mut q = Quiet::new(cur.clone(), origin.clone());
            let mut pins: Vec<(usize, Faction, f32)> = Vec::new();
            for k in 0..8 {
                let f = if k < 4 { Faction::Fire } else { Faction::Water };
                pins.push((r.below(n), f, 1.0));
            }
            let mut least = usize::MAX;
            for step in 0..300 {
                match r.below(10) {
                    0 => pins.push((
                        r.below(n),
                        if r.below(2) == 0 {
                            Faction::Fire
                        } else {
                            Faction::Water
                        },
                        0.3 + 0.7 * r.unit(),
                    )),
                    1 if !pins.is_empty() => {
                        let k = r.below(pins.len());
                        pins.remove(k);
                    }
                    2 if !pins.is_empty() => {
                        let k = r.below(pins.len());
                        pins[k].1 = if pins[k].1 == Faction::Fire {
                            Faction::Water
                        } else {
                            Faction::Fire
                        };
                    }
                    _ => {}
                }
                // Claims age.
                for p in pins.iter_mut().skip(6) {
                    p.2 = (p.2 - 0.01).max(0.0);
                }
                f.step_traced(
                    &cur,
                    &origin,
                    pins.iter().copied(),
                    &mut next,
                    &mut next_origin,
                );
                f.step_quiet(&mut q, &cur, pins.iter().copied());
                for i in 0..n {
                    assert_eq!(
                        (next[i].0.to_bits(), next[i].1.to_bits(), next_origin[i]),
                        (q.next[i].0.to_bits(), q.next[i].1.to_bits(), q.origin[i]),
                        "seed {seed} step {step} node {i}"
                    );
                }
                least = least.min(q.recomputed);
                // Fed back as the server does: written only when it moved
                // more than 1e-3, sometimes disturbed from outside.
                for i in 0..n {
                    let (a, b) = (cur[i], next[i]);
                    if (a.0 - b.0).abs() > 1e-3 || (a.1 - b.1).abs() > 1e-3 {
                        cur[i] = b;
                    }
                }
                if r.below(20) == 0 {
                    let k = r.below(n);
                    cur[k] = (r.unit(), r.unit());
                }
                if r.below(100) == 0 {
                    q.reset();
                }
                std::mem::swap(&mut origin, &mut next_origin);
            }
            assert!(least < n, "seed {seed}: never fewer than all nodes");
        }
    }

    /// Settled with fixed pins: nothing to recompute.
    #[test]
    fn quiet_rests() {
        let f = Field::new(6, &line(6, 50.0), &[], &FieldParams::default());
        let mut q = Quiet::new(vec![(0.0, 0.0); 6], vec![(NO_ORIGIN, NO_ORIGIN); 6]);
        let pins = [(0, Faction::Fire, 1.0), (5, Faction::Water, 1.0)];
        let mut cur = vec![(0.0, 0.0); 6];
        for _ in 0..20 {
            f.step_quiet(&mut q, &cur, pins);
            cur.clone_from(&q.next);
        }
        f.step_quiet(&mut q, &cur, pins);
        assert_eq!(q.recomputed, 0);
    }
}

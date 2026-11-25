//! Walked shapes (vault `wilde-ideen.md`), stage 1: the ring (`[ring]`).
//! A player's route (accepted fixes, local metres) closes a loop when its
//! newest step crosses an earlier one within `max_s`; a loop counts when it
//! encloses `min_area_m2` and is `min_len_m` long (back and forth on one
//! street encloses nothing). The server then holds the loop for `hold_s`:
//! claims of the other side on and inside it have `weaken` of their strength
//! (a ring of fire, a moat).

use std::collections::VecDeque;

use bevy::math::Vec2;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ring {
    pub enabled: bool,
    pub min_area_m2: f32,
    pub min_len_m: f32,
    pub max_s: f32,
    pub hold_s: f32,
    pub weaken: f32,
    /// Junctions this close to the loop are on it (m).
    pub near_m: f32,
    /// Route points closer than this to the last one are skipped (m).
    pub step_m: f32,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            enabled: false,
            min_area_m2: 2500.0,
            min_len_m: 150.0,
            max_s: 1800.0,
            hold_s: 1800.0,
            weaken: 0.5,
            near_m: 20.0,
            step_m: 8.0,
        }
    }
}

/// A closed loop: its outline (local metres), length and area.
#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub outline: Vec<Vec2>,
    pub len_m: f32,
    pub area_m2: f32,
}

impl Loop {
    /// Inside the outline (even-odd).
    pub fn contains(&self, p: Vec2) -> bool {
        let o = &self.outline;
        let mut inside = false;
        let mut j = o.len() - 1;
        for i in 0..o.len() {
            let (a, b) = (o[i], o[j]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    /// Distance to the outline (m).
    pub fn distance(&self, p: Vec2) -> f32 {
        let o = &self.outline;
        (0..o.len())
            .map(|i| seg_dist(p, o[i], o[(i + 1) % o.len()]))
            .fold(f32::INFINITY, f32::min)
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Where segments p1-p2 and q1-q2 cross (not at shared ends).
fn cross(p1: Vec2, p2: Vec2, q1: Vec2, q2: Vec2) -> Option<Vec2> {
    let r = p2 - p1;
    let s = q2 - q1;
    let d = r.perp_dot(s);
    if d.abs() < 1e-6 {
        return None;
    }
    let t = (q1 - p1).perp_dot(s) / d;
    let u = (q1 - p1).perp_dot(r) / d;
    ((1e-4..=1.0).contains(&t) && (0.0..1.0 - 1e-4).contains(&u)).then(|| p1 + r * t)
}

/// One player's recent route.
#[derive(Clone, Debug, Default)]
pub struct Walk {
    pts: VecDeque<(Vec2, f64)>,
}

impl Walk {
    /// Nothing walked for `max_s` (the route can be dropped).
    pub fn idle(&self, t: f64, max_s: f32) -> bool {
        self.pts
            .back()
            .is_none_or(|&(_, t0)| t - t0 > f64::from(max_s))
    }

    /// A new fix; a loop if this step closed one (the route then starts anew from here).
    pub fn push(&mut self, rules: &Ring, p: Vec2, t: f64) -> Option<Loop> {
        while self
            .pts
            .front()
            .is_some_and(|&(_, t0)| t - t0 > f64::from(rules.max_s))
        {
            self.pts.pop_front();
        }
        let &(last, _) = match self.pts.back() {
            None => {
                self.pts.push_back((p, t));
                return None;
            }
            Some(x) => x,
        };
        if last.distance(p) < rules.step_m {
            return None;
        }
        let n = self.pts.len();
        // The newest step against every older one but its neighbour.
        let mut found = None;
        for i in (0..n.saturating_sub(2)).rev() {
            let (a, b) = (self.pts[i].0, self.pts[i + 1].0);
            if let Some(x) = cross(last, p, a, b) {
                found = Some((i, x));
                break;
            }
        }
        self.pts.push_back((p, t));
        let (i, x) = found?;
        let mut outline = vec![x];
        outline.extend(self.pts.iter().skip(i + 1).take(n - i - 1).map(|q| q.0));
        let len_m: f32 = (0..outline.len())
            .map(|k| outline[k].distance(outline[(k + 1) % outline.len()]))
            .sum();
        let area_m2 = (0..outline.len())
            .map(|k| outline[k].perp_dot(outline[(k + 1) % outline.len()]))
            .sum::<f32>()
            .abs()
            / 2.0;
        if len_m < rules.min_len_m || area_m2 < rules.min_area_m2 {
            return None;
        }
        self.pts.clear();
        self.pts.push_back((p, t));
        Some(Loop {
            outline,
            len_m,
            area_m2,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(r: &Ring, pts: &[(f32, f32)]) -> Option<Loop> {
        let mut w = Walk::default();
        let mut out = None;
        for (k, &(x, y)) in pts.iter().enumerate() {
            out = w.push(r, Vec2::new(x, y), k as f64 * 10.0).or(out);
        }
        out
    }

    #[test]
    fn a_block_round_closes() {
        let r = Ring::default();
        // 80 m square, then a step past the start.
        let mut pts = vec![];
        for k in 0..8 {
            pts.push((k as f32 * 10.0, 0.0));
        }
        for k in 0..8 {
            pts.push((80.0, k as f32 * 10.0));
        }
        for k in 0..8 {
            pts.push((80.0 - k as f32 * 10.0, 80.0));
        }
        for k in 0..9 {
            pts.push((0.0, 80.0 - k as f32 * 10.0 - 5.0 * (k == 8) as u8 as f32));
        }
        pts.push((5.0, -10.0));
        let l = walk(&r, &pts).expect("ring");
        assert!((l.area_m2 - 6400.0).abs() < 600.0, "{}", l.area_m2);
        assert!(l.contains(Vec2::new(40.0, 40.0)));
        assert!(!l.contains(Vec2::new(120.0, 40.0)));
        assert!(l.distance(Vec2::new(80.0, 40.0)) < 1.0);
    }

    #[test]
    fn back_and_forth_is_no_ring() {
        let r = Ring::default();
        let mut pts: Vec<(f32, f32)> = (0..20).map(|k| (k as f32 * 10.0, 0.0)).collect();
        pts.extend((0..20).map(|k| (190.0 - k as f32 * 10.0, 3.0 * (k % 2) as f32)));
        assert!(walk(&r, &pts).is_none());
    }

    #[test]
    fn too_small_is_no_ring() {
        let r = Ring::default();
        let pts = [
            (0.0, 0.0),
            (20.0, 0.0),
            (20.0, 20.0),
            (0.0, 20.0),
            (10.0, -10.0),
        ];
        assert!(walk(&r, &pts).is_none());
    }
}

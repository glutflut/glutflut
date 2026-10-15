//! Track plausibility (DSFA M17/R10, against GPS spoofing at walking speed,
//! which `motion.rs` lets through). Over the last fixes while moving, three
//! signals of a made-up track:
//! - the device reports the very same accuracy every time (mock-location apps),
//! - no noise: each fix lies almost exactly on the line through its neighbours (joystick
//!   and route players interpolate; real GPS jitters by decimetres or more),
//! - on the street axis: real phones are on the pavement, metres off it.
//!
//! Flagged (see web.rs: shadow ban) when on the axis plus one more signal, or
//! when the track is computed outright (exact interpolation, millimetres) and
//! the accuracy constant. Phones that smooth a lot and report a steady accuracy
//! (iPhones: often exactly 5 m) are neither. Memory only.

use std::collections::VecDeque;

use bevy::prelude::*;

/// A fix this close to the last one is a repeat (cached position), not a fix.
const REPEAT_M: f32 = 0.01;

#[derive(Clone, Copy)]
struct Fix {
    p: Vec3,
    acc: Option<f32>,
    axis: Option<f32>,
}

#[derive(Component, Default)]
pub struct Track {
    fixes: VecDeque<Fix>,
    /// Survey (2026-10-07): does the device report a speed while moving?
    /// DevTools and some spoofers never do; real phones mostly. Counts only.
    pub speed: SpeedSeen,
}

/// Good fixes (accuracy ≤ `SPEED_ACC_M`, the app drops the speed of worse
/// ones) at least `SPEED_STEP_M` from the last, and how many had a speed.
#[derive(Default, Clone, Copy)]
pub struct SpeedSeen {
    pub moving: u32,
    pub with_speed: u32,
    last: Option<Vec3>,
}

/// The app sends the speed only from fixes this good (web/src/gps.ts).
const SPEED_ACC_M: f32 = 40.0;
const SPEED_STEP_M: f32 = 1.0;

impl SpeedSeen {
    pub fn fix(&mut self, p: Vec3, acc: Option<f32>, speed: Option<f32>) {
        if !acc.is_some_and(|a| a <= SPEED_ACC_M) {
            return;
        }
        if self.last.is_some_and(|l| l.distance(p) >= SPEED_STEP_M) {
            self.moving += 1;
            self.with_speed += speed.is_some() as u32;
        }
        self.last = Some(p);
    }
}

/// Which signals hold (for logs and tests).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Signals {
    pub same_accuracy: bool,
    pub no_noise: bool,
    pub exact: bool,
    pub on_axis: bool,
    /// Median deviation and axis distance (cm), for the log.
    pub noise_cm: u32,
    pub axis_cm: u32,
}

impl Signals {
    pub fn flag(&self) -> bool {
        self.on_axis && (self.no_noise || self.same_accuracy) || self.exact && self.same_accuracy
    }
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

impl Track {
    /// Record a fix: position (ENU m), reported accuracy (m) and the
    /// distance to the nearest street axis. Signals once the window is full
    /// and the player moved. Repeats are skipped: a cached position sent twice
    /// lies exactly on its neighbours' line and would look noise-free.
    pub fn fix(&mut self, p: Vec3, acc: Option<f32>, axis: Option<f32>) -> Option<Signals> {
        let c = crate::cheat::get();
        if self
            .fixes
            .back()
            .is_some_and(|l| l.p.distance(p) < REPEAT_M)
        {
            return None;
        }
        self.fixes.push_back(Fix { p, acc, axis });
        if self.fixes.len() > c.track_fixes {
            self.fixes.pop_front();
        }
        if self.fixes.len() < c.track_fixes {
            return None;
        }
        let f = &self.fixes;
        let path: f32 = f
            .iter()
            .zip(f.iter().skip(1))
            .map(|(a, b)| a.p.distance(b.p))
            .sum();
        if path < c.track_min_path_m {
            return None;
        }
        let accs: Vec<f32> = f.iter().filter_map(|x| x.acc).collect();
        let same_accuracy = accs.len() >= c.min_with
            && accs.iter().fold(f32::MIN, |a, &b| a.max(b))
                - accs.iter().fold(f32::MAX, |a, &b| a.min(b))
                < 0.01;
        // Distance of each fix from the line through its neighbours, across
        // the way only: arrival times jitter by milliseconds, the line doesn't.
        let dev: Vec<f32> = (1..f.len() - 1)
            .filter_map(|i| {
                let (a, b, c) = (f[i - 1].p, f[i].p, f[i + 1].p);
                let ac = Vec2::new(c.x - a.x, c.z - a.z);
                let ab = Vec2::new(b.x - a.x, b.z - a.z);
                let len = ac.length();
                (len > 0.5).then(|| ac.perp_dot(ab).abs() / len)
            })
            .collect();
        let dev = median(dev);
        let no_noise = dev.is_some_and(|m| m < c.smooth_m);
        let exact = dev.is_some_and(|m| m < c.exact_m);
        let axes: Vec<f32> = f.iter().filter_map(|x| x.axis).collect();
        let axis_m = if axes.len() >= c.min_with {
            median(axes)
        } else {
            None
        };
        let on_axis = axis_m.is_some_and(|m| m < c.axis_m);
        let cm = |m: Option<f32>| m.map_or(u32::MAX, |m| (m * 100.0).round() as u32);
        Some(Signals {
            same_accuracy,
            no_noise,
            exact,
            on_axis,
            noise_cm: cm(dev),
            axis_cm: cm(axis_m),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_seen_counts_good_moving_fixes() {
        let mut s = SpeedSeen::default();
        let at = |x: f32| Vec3::new(x, 0.0, 0.0);
        s.fix(at(0.0), Some(5.0), None); // first: nothing to compare
        s.fix(at(1.5), Some(5.0), Some(1.4)); // moving, with speed
        s.fix(at(3.0), Some(5.0), None); // moving, none
        s.fix(at(3.2), Some(5.0), None); // standing
        s.fix(at(9.0), Some(60.0), None); // poor fix: not counted
        s.fix(at(10.5), None, None); // no accuracy: not counted
        assert_eq!((s.moving, s.with_speed), (2, 1));
    }

    /// 1.4 m/s along x, 90 s, with `noise` (m) and an axis distance.
    fn run(noise: f32, acc: impl Fn(usize) -> f32, axis: impl Fn(usize) -> f32) -> Signals {
        let mut tr = Track::default();
        let mut last = None;
        for i in 0..90 {
            // Deterministic pseudo-random jitter in [-1, 1] (two axes).
            let r = |k: u64| {
                ((k.wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407)
                    >> 33)
                    % 2001) as f32
                    / 1000.0
                    - 1.0
            };
            let (jx, jz) = (r(i as u64 * 2), r(i as u64 * 2 + 1));
            let p = Vec3::new(i as f32 * 1.4 + jx * noise, 0.0, jz * noise);
            last = tr.fix(p, Some(acc(i)), Some(axis(i)));
        }
        last.expect("window full")
    }

    #[test]
    fn real_walk_is_not_flagged() {
        // GPS noise ~2 m, accuracy wanders, 4-7 m off the axis.
        let s = run(
            2.0,
            |i| 4.0 + (i % 5) as f32 * 0.7,
            |i| 4.0 + (i % 4) as f32,
        );
        assert!(!s.flag(), "{s:?}");
    }

    #[test]
    fn smoothed_real_walk_is_not_flagged() {
        // A phone that smooths hard (3 cm left: still noise) and always says
        // 5 m, off the axis.
        let s = run(0.03, |_| 5.0, |i| 3.0 + (i % 3) as f32);
        assert!(!s.no_noise && s.same_accuracy && !s.exact, "{s:?}");
        assert!(!s.flag());
    }

    #[test]
    fn joystick_spoof_is_flagged() {
        // Exact line, constant accuracy, anywhere (off the axis).
        let s = run(0.0, |_| 3.0, |_| 6.0);
        assert!(s.exact && s.same_accuracy && !s.on_axis);
        assert!(s.flag());
    }

    #[test]
    fn route_spoof_on_the_axis_is_flagged() {
        // A street route played back: on the axis, a little noise, accuracy varies.
        let s = run(0.01, |i| 3.0 + (i % 4) as f32, |_| 0.05);
        assert!(s.on_axis && s.no_noise && !s.same_accuracy);
        assert!(s.flag());
    }

    #[test]
    fn smoothing_phone_in_narrow_streets_is_not_flagged() {
        // The 2026-10-07 false positive: noise 4 cm, 49 cm off the axis,
        // accuracy varying (logged: no_noise, on_axis, noise_cm 4, axis_cm 49).
        let s = run(
            0.08,
            |i| 3.0 + (i % 6) as f32 * 0.5,
            |i| 0.4 + (i % 3) as f32 * 0.1,
        );
        assert!(s.noise_cm >= 2 && s.axis_cm >= 40, "{s:?}");
        assert!(!s.no_noise && !s.on_axis && !s.flag(), "{s:?}");
    }

    #[test]
    fn repeated_fixes_are_not_noise_free() {
        // A real walk where the browser hands each cached position over twice.
        let mut tr = Track::default();
        let mut last = None;
        for i in 0..90 {
            let j = ((i as u64).wrapping_mul(2654435761) % 2001) as f32 / 1000.0 - 1.0;
            let p = Vec3::new(i as f32 * 2.8, 0.0, j * 2.0);
            for _ in 0..2 {
                last = tr.fix(p, Some(4.0 + (i % 5) as f32), Some(0.3)).or(last);
            }
        }
        let s = last.expect("window full");
        assert!(!s.no_noise && !s.flag(), "{s:?}");
    }

    #[test]
    fn standing_still_is_not_judged() {
        let mut tr = Track::default();
        let res: Vec<_> = (0..90)
            .map(|_| tr.fix(Vec3::ZERO, Some(3.0), Some(0.1)))
            .collect();
        assert!(res.iter().all(Option::is_none));
    }
}

//! Position plausibility. A fix that implies an impossible speed locks claims,
//! ratings, points and analytics for a while: stops teleport and tap-to-move
//! cheats, and keeps people on a bus out of the dwell counts. Two checks: the
//! jump since the last fix (with slack for GPS noise) and the mean speed over
//! the last 20-60 s, where jitter averages out but sustained speed doesn't.
//! Plus the phone's own (Doppler) speed, which catches city driving with its
//! stops right from the first fix: one fix above ~40 km/h, or two above
//! ~25 km/h within 30 s (a fast cyclist may hit it now and then: 1 min wait).

use std::collections::VecDeque;

use bevy::prelude::*;

/// `ELEMENTS_ALLOW_SIM=1`: `?sim` tap-to-move works and nothing is checked
/// (dev and tests only).
#[derive(Resource, Clone, Copy)]
pub struct AllowSim(pub bool);

impl AllowSim {
    pub fn from_env() -> Self {
        Self(std::env::var("ELEMENTS_ALLOW_SIM").is_ok_and(|v| v == "1"))
    }
}

#[derive(Component, Default)]
pub struct Motion {
    recent: VecDeque<(Vec3, f64)>,
    /// Times of fixes whose reported speed was above `fast_ms` (cheat.rs).
    fast: VecDeque<f64>,
    locked_until: f64,
    /// Made-up tracks at walking speed (see plausible.rs).
    pub track: crate::plausible::Track,
    /// Find not known yet within reach, and since when (uncover by being there).
    pub at_find: Option<(i64, f64)>,
    /// `Rules::rise`: waiting at this junction since then; no new rise before `rise_next`.
    pub at_rise: Option<(i64, f64)>,
    pub rise_next: f64,
    /// The last accepted fix (past the speed check; every one on test servers).
    pub last_fix: Option<(Vec3, f64)>,
    /// Plausible playing time this connection (gaps over 30 s don't count).
    played_s: f64,
    last_ok: Option<f64>,
    /// Full strength from the start: played on an earlier day, confirmed on
    /// site, or a test server.
    pub trusted: bool,
}

/// New identities claim with this share of their strength until they have
/// played `trust_after_s`.
pub const NEW_STRENGTH: f32 = 0.5;

/// `ELEMENTS_TRUST_AFTER_S` (default 600; 0 = full strength at once).
pub fn trust_after_s() -> f64 {
    static T: std::sync::LazyLock<f64> = std::sync::LazyLock::new(|| {
        std::env::var("ELEMENTS_TRUST_AFTER_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(600.0)
    });
    *T
}

impl Motion {
    /// Record a fix with the device's speed in m/s, if it reported one; false
    /// if it was implausible (the player is then locked).
    pub fn fix(&mut self, p: Vec3, now: f64, speed: Option<f32>) -> bool {
        let c = crate::cheat::get();
        let too_far = |&(q, t): &(Vec3, f64), slack: f32| {
            p.distance(q) > c.max_speed_ms * (now - t).max(0.0) as f32 + slack
        };
        let jump = self
            .recent
            .back()
            .is_some_and(|f| too_far(f, c.jump_slack_m));
        // oldest fix still inside the window, if it is old enough
        let window = self
            .recent
            .front()
            .filter(|(_, t)| now - t >= c.window_min_s)
            .is_some_and(|f| too_far(f, c.window_slack_m));
        while self.fast.front().is_some_and(|t| now - t > c.fast_window_s) {
            self.fast.pop_front();
        }
        let speed = speed.filter(|v| v.is_finite()).unwrap_or(0.0);
        if speed > c.fast_ms {
            self.fast.push_back(now);
        }
        let driving = speed > c.drive_ms || self.fast.len() >= 2;
        let ok = !jump && !window && !driving;
        if ok {
            if let Some(dt) = self
                .last_ok
                .map(|l| now - l)
                .filter(|&dt| dt > 0.0 && dt <= 30.0)
            {
                self.played_s += dt;
            }
            self.last_ok = Some(now);
        } else {
            self.last_ok = None;
        }
        if !ok {
            self.locked_until = now + c.lock_s;
            self.recent.clear();
            self.fast.clear();
        }
        self.recent.push_back((p, now));
        while self
            .recent
            .front()
            .is_some_and(|(_, t)| now - t > c.window_max_s)
        {
            self.recent.pop_front();
        }
        ok
    }

    pub fn with_trust(trusted: bool) -> Self {
        Self {
            trusted,
            ..Default::default()
        }
    }

    /// Claims at full strength (see [`NEW_STRENGTH`]).
    pub fn full_strength(&self) -> bool {
        self.trusted || self.played_s >= trust_after_s()
    }

    pub fn locked(&self, now: f64) -> bool {
        now < self.locked_until
    }

    /// Seconds until claims are possible again (0 when not locked).
    pub fn lock_left(&self, now: f64) -> f32 {
        (self.locked_until - now).max(0.0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(m: &mut Motion, pts: impl Iterator<Item = (f32, f64)>) -> Vec<bool> {
        pts.map(|(x, t)| m.fix(Vec3::new(x, 0.0, 0.0), t, None))
            .collect()
    }

    #[test]
    fn walking_and_jitter_pass() {
        let mut m = Motion::default();
        // 1.4 m/s with +-15 m jitter every other second, 5 minutes
        let pts = (0..300).map(|i| {
            let j = if i % 2 == 0 { 15.0 } else { -15.0 };
            (i as f32 * 1.4 + j, i as f64)
        });
        assert!(walk(&mut m, pts).into_iter().all(|ok| ok));
        assert!(!m.locked(300.0));
    }

    #[test]
    fn strength_grows_with_plausible_play() {
        let mut m = Motion::default();
        assert!(!m.full_strength());
        // 9 minutes walking, then a 5-minute gap that doesn't count
        for i in 0..540 {
            m.fix(Vec3::new(i as f32 * 1.4, 0.0, 0.0), i as f64, None);
        }
        m.fix(Vec3::new(756.0, 0.0, 0.0), 840.0, None);
        assert!(!m.full_strength(), "gap not counted");
        for i in 841..905 {
            m.fix(
                Vec3::new(756.0 + (i - 840) as f32 * 1.4, 0.0, 0.0),
                i as f64,
                None,
            );
        }
        assert!(m.full_strength(), "10 minutes of play");
        let trusted = Motion {
            trusted: true,
            ..Default::default()
        };
        assert!(trusted.full_strength());
    }

    #[test]
    fn teleport_locks_for_a_minute() {
        let mut m = Motion::default();
        assert!(m.fix(Vec3::ZERO, 0.0, None));
        assert!(!m.fix(Vec3::new(300.0, 0.0, 0.0), 1.0, None));
        assert!(m.locked(30.0) && !m.locked(62.0));
    }

    #[test]
    fn sustained_speed_is_caught() {
        let mut m = Motion::default();
        // 15 m/s (54 km/h): 7 m/s over the limit eats the slack in seconds
        let res = walk(&mut m, (0..30).map(|i| (i as f32 * 15.0, i as f64)));
        assert!(res.iter().any(|ok| !ok));
    }

    #[test]
    fn reported_speed_catches_city_driving() {
        // One fix at 45 km/h, even without any history.
        let mut m = Motion::default();
        assert!(!m.fix(Vec3::ZERO, 0.0, Some(12.5)));
        assert!(m.locked(1.0));
        // Two at 28 km/h within 30 s; stops in between don't help.
        let mut m = Motion::default();
        assert!(m.fix(Vec3::ZERO, 0.0, Some(7.8)));
        assert!(m.fix(Vec3::new(5.0, 0.0, 0.0), 10.0, Some(0.0)));
        assert!(!m.fix(Vec3::new(10.0, 0.0, 0.0), 20.0, Some(7.8)));
    }

    #[test]
    fn cycling_now_and_then_passes() {
        // 25+ km/h once, then again only after the 30 s window.
        let mut m = Motion::default();
        assert!(m.fix(Vec3::ZERO, 0.0, Some(7.5)));
        assert!(m.fix(Vec3::new(150.0, 0.0, 0.0), 40.0, Some(7.5)));
        assert!(!m.locked(41.0));
    }
}

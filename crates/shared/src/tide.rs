//! Tide as a rule: the
//! water level at a gauge gives a phase, -1 at mean low water .. +1 at mean
//! high water. Junctions near open tidal water lean to water at high tide and
//! to fire at low tide, like live rain through the natural sources and ageing.
//! Weight per junction: distance to the water (main factor; elevation alone
//! misleads behind dikes), gently less on high ground. Off unless `strength`
//! or `decay` > 0; without a fresh level nothing changes.

use serde::{Deserialize, Serialize};

use crate::elements::Faction;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tide {
    /// At full phase and weight, the losing side's natural sources are this
    /// much weaker (water at low tide, fire at high tide). 0 = off.
    pub strength: f32,
    /// Ageing: the favoured side's sources age this much slower, the other faster.
    pub decay: f32,
    /// Weight 1 at the water, 0 from `fade_m` (cosine).
    pub fade_m: f32,
    /// Full weight up to `elev_full_m` above mean high water, half from `elev_half_m`.
    pub elev_full_m: f32,
    pub elev_half_m: f32,
    /// Storm surge: above mean high water + `surge_m` the rule is off (safety).
    pub surge_m: f32,
}

impl Default for Tide {
    fn default() -> Self {
        Self {
            strength: 0.0,
            decay: 0.0,
            fade_m: 400.0,
            elev_full_m: 5.0,
            elev_half_m: 15.0,
            surge_m: 1.5,
        }
    }
}

impl Tide {
    pub fn enabled(&self) -> bool {
        self.strength > 0.0 || self.decay > 0.0
    }

    /// Phase -1..1 of `level` between mean low and high water (same datum);
    /// `None` during a storm surge.
    pub fn phase(&self, level: f32, mlw: f32, mhw: f32) -> Option<f32> {
        if level > mhw + self.surge_m {
            return None;
        }
        let half = ((mhw - mlw) / 2.0).max(0.1);
        Some(((level - (mhw + mlw) / 2.0) / half).clamp(-1.0, 1.0))
    }

    /// Weight 0..1 of a junction `dist_m` from the water, `elev_m` high.
    pub fn weight(&self, dist_m: f32, elev_m: f32, mhw: f32) -> f32 {
        if dist_m >= self.fade_m {
            return 0.0;
        }
        let d = 0.5 * (1.0 + (std::f32::consts::PI * dist_m.max(0.0) / self.fade_m).cos());
        let span = (self.elev_half_m - self.elev_full_m).max(0.1);
        let up = ((elev_m - mhw - self.elev_full_m) / span).clamp(0.0, 1.0);
        d * (1.0 - 0.5 * up)
    }

    /// Strength factor of a natural source of `f` at `k` = phase × weight.
    pub fn natural(&self, f: Faction, k: f32) -> f32 {
        let against = match f {
            Faction::Fire => k,
            Faction::Water => -k,
        };
        1.0 - (against.max(0.0) * self.strength).min(0.9)
    }

    /// Ageing factor of a source of `f` at `k`.
    pub fn age(&self, f: Faction, k: f32) -> f32 {
        let with = match f {
            Faction::Water => k,
            Faction::Fire => -k,
        };
        (1.0 - with * self.decay).max(0.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tide() {
        let t = Tide {
            strength: 0.5,
            decay: 0.3,
            ..Default::default()
        };
        assert!(t.enabled() && !Tide::default().enabled());
        assert_eq!(t.phase(2.0, -1.5, 2.0), Some(1.0));
        assert_eq!(t.phase(0.25, -1.5, 2.0), Some(0.0));
        assert_eq!(t.phase(-3.0, -1.5, 2.0), Some(-1.0));
        assert_eq!(t.phase(3.6, -1.5, 2.0), None);
        assert_eq!(t.weight(0.0, 3.0, 2.0), 1.0);
        assert!((t.weight(200.0, 3.0, 2.0) - 0.5).abs() < 1e-6);
        assert_eq!(t.weight(400.0, 3.0, 2.0), 0.0);
        assert!((t.weight(0.0, 30.0, 2.0) - 0.5).abs() < 1e-6);
        assert_eq!(t.natural(Faction::Fire, 1.0), 0.5);
        assert_eq!(t.natural(Faction::Water, 1.0), 1.0);
        assert_eq!(t.natural(Faction::Water, -1.0), 0.5);
        assert!((t.age(Faction::Water, 1.0) - 0.7).abs() < 1e-6);
        assert!((t.age(Faction::Fire, 1.0) - 1.3).abs() < 1e-6);
    }
}

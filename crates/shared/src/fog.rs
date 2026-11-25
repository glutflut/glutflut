//! Fog at the front (`[fog]`): where fire and water have both been present
//! and nearly equal (`margin`) at a junction for `after_s` (steam), fog lies there; it lifts `clear_s`
//! after they stop clashing. The app covers fogged streets except within
//! `see_m` of the player: go there to see. Off unless `enabled`.

use serde::{Deserialize, Serialize};

use crate::elements::ELEMENT_ACTIVE;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fog {
    pub enabled: bool,
    pub after_s: f32,
    pub clear_s: f32,
    pub see_m: f32,
    /// They clash where both are present and differ by less than this
    /// (0.05 = the front's own margin; lab: fog on 5-7 % of Konstanz, 0.2: 20-26 %).
    pub margin: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            enabled: false,
            after_s: 1200.0,
            clear_s: 900.0,
            see_m: 50.0,
            margin: 0.05,
        }
    }
}

/// Fire and water both present and nearly equal: they clash here.
pub fn clash(fire: f32, water: f32, margin: f32) -> bool {
    fire.min(water) >= ELEMENT_ACTIVE && (fire - water).abs() < margin
}

/// Per junction: seconds of clash counted up to `after_s`, down at
/// `after_s / clear_s` per second without; fog from full until empty.
#[derive(Clone, Debug, Default)]
pub struct FogClock {
    acc: Vec<f32>,
    on: Vec<bool>,
}

impl FogClock {
    /// Advance by `dt` s over the field's `(fire, water)`; true if any junction changed.
    pub fn step(&mut self, rules: &Fog, state: &[(f32, f32)], dt: f32) -> bool {
        if self.acc.len() != state.len() {
            self.acc = vec![0.0; state.len()];
            self.on = vec![false; state.len()];
        }
        let down = rules.after_s / rules.clear_s.max(1.0);
        let mut changed = false;
        for (i, &(f, w)) in state.iter().enumerate() {
            let a = &mut self.acc[i];
            if clash(f, w, rules.margin) {
                *a = (*a + dt).min(rules.after_s);
            } else {
                *a = (*a - dt * down).max(0.0);
            }
            let on = if self.on[i] {
                *a > 0.0
            } else {
                *a >= rules.after_s
            };
            if on != self.on[i] {
                self.on[i] = on;
                changed = true;
            }
        }
        changed
    }

    /// Fogged junctions (dense indices).
    pub fn fogged(&self) -> impl Iterator<Item = usize> + '_ {
        self.on
            .iter()
            .enumerate()
            .filter(|(_, &o)| o)
            .map(|(i, _)| i)
    }

    pub fn count(&self) -> usize {
        self.on.iter().filter(|&&o| o).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comes_after_a_while_and_lifts_later() {
        let r = Fog {
            enabled: true,
            after_s: 100.0,
            clear_s: 50.0,
            see_m: 50.0,
            margin: 0.1,
        };
        let mut c = FogClock::default();
        let steam = [(0.4, 0.4), (0.6, 0.0)];
        let calm = [(0.6, 0.0), (0.6, 0.0)];
        for _ in 0..9 {
            c.step(&r, &steam, 10.0);
        }
        assert_eq!(c.count(), 0);
        assert!(c.step(&r, &steam, 10.0));
        assert_eq!(c.fogged().collect::<Vec<_>>(), vec![0]);
        for _ in 0..4 {
            c.step(&r, &calm, 10.0);
        }
        assert_eq!(c.count(), 1);
        assert!(c.step(&r, &calm, 10.0));
        assert_eq!(c.count(), 0);
    }
}

//! Levels, experience and abilities.
//! Rule values live in [`Progression`] (TOML section `[progression]`), off by
//! default, so rule sets without it play as before.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progression {
    pub enabled: bool,
    /// XP needed for levels 2, 3, 4, 5.
    pub thresholds: Vec<f32>,
    /// Passive source strength bonus per level above 1, and its cap.
    pub bonus_per_level: f32,
    pub bonus_max: f32,
    /// Abilities (reach, nourish, boost, double) and challenges on/off.
    pub abilities: bool,
    pub challenges: bool,
    /// "Rückenwind" on a player's first day: longer lifetime, and enemies may
    /// only take over below this fraction (instead of `takeover_below`).
    pub newcomer_ttl_factor: f32,
    pub newcomer_takeover_below: f32,
    /// Weitblick: one claim with this reach.
    pub reach_m: f32,
    pub reach_cooldown_s: f32,
    /// Nähren: one own source back to full strength.
    pub nourish_cooldown_s: f32,
    /// Verstärken: a teammate's source nearby back to full strength.
    pub boost_cooldown_s: f32,
    /// Funkenflug / Welle: a second, neighbouring junction with a claim.
    pub double_cooldown_s: f32,
    /// A challenge must be done within this time (one round).
    pub challenge_s: f32,
    /// Rounds for challenges that send you somewhere (Entdecken, Ruhige Ecke).
    pub challenge_long_s: f32,
    /// Next challenge right after finishing one (else when the round ends).
    pub challenge_back_to_back: bool,
    /// Pause after a round before the next challenge (0: the next comes when
    /// the round ends), and at most this many challenges per player and day
    /// (0: no limit).
    pub challenge_gap_s: f32,
    pub challenges_per_day: u32,
    /// A campaign start sets the levels back; whoever reached
    /// `carry_from_level` or more starts the next campaign at
    /// `carry_to_level` instead of 1 (0 = everyone at 1).
    pub carry_from_level: u8,
    pub carry_to_level: u8,
    /// Points for a finished challenge (0 = XP only).
    pub challenge_points: f32,
    pub xp_challenge: f32,
    pub xp_round: f32,
    pub xp_zone: f32,
    pub xp_claim: f32,
    /// Plain claims give at most this much XP per day.
    pub xp_claim_cap: f32,
    pub xp_rating: f32,
    /// Verstärken: XP for both players.
    pub xp_boost: f32,
    /// Each further kind of goal met on a day adds this to the day's XP, up to `variety_max`.
    pub variety_step: f32,
    pub variety_max: f32,
    /// A claim belongs to the nearest zone centre (attractor) within this
    /// distance (zone XP, Entdecken).
    pub zone_r_m: f32,
    /// "Halten": junctions held from own sources that count.
    pub hold_junctions: u32,
}

impl Default for Progression {
    fn default() -> Self {
        Self {
            enabled: false,
            thresholds: vec![100.0, 600.0, 1500.0, 3000.0],
            bonus_per_level: 0.05,
            bonus_max: 0.15,
            abilities: true,
            challenges: true,
            newcomer_ttl_factor: 1.5,
            newcomer_takeover_below: 1.0 / 3.0,
            reach_m: 110.0,
            reach_cooldown_s: 600.0,
            nourish_cooldown_s: 600.0,
            boost_cooldown_s: 300.0,
            double_cooldown_s: 600.0,
            challenge_s: 300.0,
            challenge_long_s: 480.0,
            challenge_back_to_back: false,
            challenge_gap_s: 0.0,
            challenges_per_day: 0,
            carry_from_level: 0,
            carry_to_level: 0,
            challenge_points: 0.0,
            xp_challenge: 30.0,
            xp_round: 10.0,
            xp_zone: 15.0,
            xp_claim: 2.0,
            xp_claim_cap: 20.0,
            xp_rating: 5.0,
            xp_boost: 5.0,
            variety_step: 0.25,
            variety_max: 1.0,
            zone_r_m: 400.0,
            hold_junctions: 20,
        }
    }
}

/// Abilities, unlocked at levels 2 to 5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ability {
    Reach,
    Nourish,
    Boost,
    Double,
}

impl Ability {
    pub const ALL: [Ability; 4] = [
        Ability::Reach,
        Ability::Nourish,
        Ability::Boost,
        Ability::Double,
    ];

    pub fn level(self) -> u8 {
        self as u8 + 2
    }
}

impl Progression {
    /// Level 1..=5 for `xp`.
    pub fn level(&self, xp: f32) -> u8 {
        1 + self.thresholds.iter().filter(|&&t| xp >= t).count() as u8
    }

    /// Source strength factor at `level` (1.0 when off).
    pub fn strength_factor(&self, level: u8) -> f32 {
        if !self.enabled {
            return 1.0;
        }
        1.0 + (self.bonus_per_level * level.saturating_sub(1) as f32).min(self.bonus_max)
    }

    pub fn has(&self, level: u8, a: Ability) -> bool {
        self.enabled && self.abilities && level >= a.level()
    }

    pub fn cooldown_s(&self, a: Ability) -> f32 {
        match a {
            Ability::Reach => self.reach_cooldown_s,
            Ability::Nourish => self.nourish_cooldown_s,
            Ability::Boost => self.boost_cooldown_s,
            Ability::Double => self.double_cooldown_s,
        }
    }
}

/// Challenges the game proposes (no choice), unlocked by level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ChallengeKind"))]
#[serde(rename_all = "lowercase")]
pub enum Challenge {
    /// Claim 3 junctions.
    Claim3,
    /// Claim in a zone you haven't claimed in today.
    Discover,
    /// Turn a contested junction.
    Front,
    /// Claim in a quiet corner (needs quiet cells; not on the server yet).
    Quiet,
    /// Hold `hold_junctions` junctions from your sources for 3 minutes.
    Hold,
    /// Take over an enemy source.
    Retake,
    /// Boost a teammate's source (needs abilities; not on the server yet).
    Team,
    /// "Stadtfühler": answer "Wie ist es hier?" at 3 different places.
    Feel,
}

impl Challenge {
    pub const ALL: [Challenge; 8] = [
        Challenge::Claim3,
        Challenge::Discover,
        Challenge::Front,
        Challenge::Quiet,
        Challenge::Hold,
        Challenge::Retake,
        Challenge::Team,
        Challenge::Feel,
    ];

    pub fn level(self) -> u8 {
        match self {
            Challenge::Claim3 | Challenge::Discover | Challenge::Feel => 1,
            Challenge::Front | Challenge::Quiet => 2,
            Challenge::Hold | Challenge::Retake => 3,
            Challenge::Team => 4,
        }
    }

    /// Round length: longer for challenges that send you somewhere.
    pub fn round_s(self, p: &Progression) -> f32 {
        match self {
            Challenge::Discover | Challenge::Quiet | Challenge::Feel => p.challenge_long_s,
            _ => p.challenge_s,
        }
    }

    /// Target: claims for Claim3, minutes in a row with `hold_junctions` for
    /// Hold, places for Feel, else one success.
    pub fn goal(self) -> u32 {
        match self {
            Challenge::Claim3 | Challenge::Hold | Challenge::Feel => 3,
            _ => 1,
        }
    }
}

/// Kinds of goals for the variety bonus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal {
    Challenge,
    Round,
    Zone,
    Rating,
    Front,
    /// Plain claim: XP (capped) but not a kind of its own.
    Claim,
    /// Verstärken: XP, counts as a team goal.
    Boost,
}

/// One player's XP for one day.
#[derive(Clone, Debug, Default)]
pub struct DayXp {
    base: f32,
    claims: f32,
    kinds: u8,
}

impl DayXp {
    /// Stored form: XP from goals, XP from plain claims, kinds bit set.
    pub fn parts(&self) -> (f32, f32, u8) {
        (self.base, self.claims, self.kinds)
    }

    pub fn from_parts(base: f32, claims: f32, kinds: u8) -> Self {
        Self {
            base,
            claims,
            kinds,
        }
    }

    pub fn add(&mut self, p: &Progression, g: Goal) {
        let (xp, kind) = match g {
            Goal::Challenge => (p.xp_challenge, Some(0)),
            Goal::Round => (p.xp_round, Some(1)),
            Goal::Zone => (p.xp_zone, Some(2)),
            Goal::Rating => (p.xp_rating, Some(3)),
            Goal::Front => (0.0, Some(4)),
            Goal::Boost => (p.xp_boost, Some(5)),
            Goal::Claim => (0.0, None),
        };
        if matches!(g, Goal::Claim | Goal::Front) {
            self.claims = (self.claims + p.xp_claim).min(p.xp_claim_cap);
        }
        self.base += xp;
        if let Some(k) = kind {
            self.kinds |= 1 << k;
        }
    }

    /// Kinds of goals met today.
    pub fn kinds(&self) -> u32 {
        self.kinds.count_ones()
    }

    /// The day's XP including the variety bonus.
    pub fn total(&self, p: &Progression) -> f32 {
        let bonus = (p.variety_step * self.kinds().saturating_sub(1) as f32).min(p.variety_max);
        (self.base + self.claims) * (1.0 + bonus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> Progression {
        Progression {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn levels_and_capped_bonus() {
        let p = on();
        assert_eq!(p.level(0.0), 1);
        assert_eq!(p.level(100.0), 2);
        assert_eq!(p.level(5000.0), 5);
        assert!((p.strength_factor(1) - 1.0).abs() < 1e-6);
        assert!((p.strength_factor(3) - 1.10).abs() < 1e-6);
        assert!((p.strength_factor(5) - 1.15).abs() < 1e-6, "capped");
        assert_eq!(Progression::default().strength_factor(5), 1.0, "off");
        assert!(p.has(2, Ability::Reach) && !p.has(2, Ability::Nourish));
    }

    #[test]
    fn variety_beats_spam() {
        let p = on();
        let mut spam = DayXp::default();
        for _ in 0..100 {
            spam.add(&p, Goal::Claim);
        }
        assert_eq!(spam.total(&p), 20.0, "claims capped");
        let mut mixed = DayXp::default();
        mixed.add(&p, Goal::Challenge);
        mixed.add(&p, Goal::Zone);
        mixed.add(&p, Goal::Zone);
        mixed.add(&p, Goal::Rating);
        // 30 + 15 + 15 + 5 = 65, three kinds = +50 %
        assert!((mixed.total(&p) - 97.5).abs() < 1e-4);
    }
}

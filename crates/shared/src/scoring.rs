//! Points.

use crate::elements::{ElementState, Faction};
use crate::field::EdgeCosts;
use serde::{Deserialize, Serialize};

/// Points (TOML `[score]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Score {
    /// A claim on neutral ground.
    pub claim: i64,
    /// Extra for taking an enemy or contested node.
    pub front_bonus: i64,
    /// Factor near an active hotspot.
    pub hotspot_factor: i64,
    /// Rating a place, at most `rate_max_per_day` times.
    pub rate: i64,
    pub rate_max_per_day: u8,
    /// Holding: one point per minute for this many metres of street your
    /// sources colour (a junction carries ~55 m on average, so ~9 junctions).
    pub hold_m_per_point: f32,
    /// Points per player and hour / day at most (against bots playing all day).
    pub points_hour_max: i64,
    pub points_day_max: i64,
    /// The day cap grows by this for each level above 1 (where levels are on).
    pub points_day_per_level: i64,
    /// For inviting someone who then really plays: at least `invite_claims`
    /// claims and back on a later day. At most `invite_max` invites per
    /// player in 30 days count.
    pub invite: i64,
    pub invite_claims: i32,
    pub invite_max: i64,
}

impl Default for Score {
    fn default() -> Self {
        Self {
            claim: 10,
            front_bonus: 10,
            hotspot_factor: 2,
            rate: 2,
            rate_max_per_day: 5,
            hold_m_per_point: 500.0,
            points_hour_max: 1500,
            points_day_max: 6000,
            points_day_per_level: 3000,
            invite: 200,
            invite_claims: 5,
            invite_max: 10,
        }
    }
}

impl Score {
    /// Points a player at `level` may get per day.
    pub fn day_cap(&self, level: u8) -> i64 {
        self.points_day_max + self.points_day_per_level * i64::from(level.max(1) - 1)
    }

    /// Points for a successful claim, by what the node was before.
    pub fn claim_points(&self, prior: ElementState, faction: Faction, hotspot: bool) -> i64 {
        if prior.owner() == Some(faction) {
            return 0;
        }
        let front = prior.contested() || prior.owner().is_some();
        let base = self.claim + if front { self.front_bonus } else { 0 };
        base * if hotspot { self.hotspot_factor } else { 1 }
    }
}

/// Street a node carries: half the length of each of its edges.
pub fn node_metres(n: usize, edges: &[EdgeCosts]) -> Vec<f32> {
    let mut m = vec![0.0f32; n];
    for e in edges {
        m[e.a] += e.len / 2.0;
        m[e.b] += e.len / 2.0;
    }
    m
}

//! Element-simulation domain (S3): the two factions and the per-node element
//! intensities the server diffuses across the road graph.
//!
//! The diffusion itself is **server-authoritative** (the design's "Elementfront-
//! Zustände niemals rein clientseitig") and lives in `crates/server/src/sim.rs`.
//! This module is only the *state* + thresholds: [`ElementState`] and how a
//! node's owner follows from its two numbers.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// The two warring elements. "Neutral" is the absence of a clear owner (see
/// [`ElementState::owner`]), not a third variant. Also a [`Component`]: each
/// player is assigned one faction.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Faction {
    Fire,
    Water,
}

impl Faction {
    /// By `index()`.
    pub const ALL: [Faction; 2] = [Faction::Fire, Faction::Water];

    pub fn other(self) -> Self {
        match self {
            Faction::Fire => Faction::Water,
            Faction::Water => Faction::Fire,
        }
    }

    /// 0 for fire, 1 for water: for per-side arrays.
    pub fn index(self) -> usize {
        match self {
            Faction::Fire => 0,
            Faction::Water => 1,
        }
    }

    /// `"fire"` / `"water"`, as on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Faction::Fire => "fire",
            Faction::Water => "water",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fire" => Some(Faction::Fire),
            "water" => Some(Faction::Water),
            _ => None,
        }
    }
}

/// Live element intensity at a graph node, each in `[0, 1]`. The server
/// derives ownership from the pair.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct ElementState {
    pub fire: f32,
    pub water: f32,
}

/// Minimum intensity for an element to count as "present" — below this on both,
/// a node reads neutral.
pub const ELEMENT_ACTIVE: f32 = 0.15;
/// Dominance margin needed to own a node; within it the node is contested.
pub const ELEMENT_MARGIN: f32 = 0.05;

impl ElementState {
    /// The dominant element, or `None` when neutral or contested.
    pub fn owner(&self) -> Option<Faction> {
        if self.fire.max(self.water) < ELEMENT_ACTIVE
            || (self.fire - self.water).abs() < ELEMENT_MARGIN
        {
            None
        } else if self.fire > self.water {
            Some(Faction::Fire)
        } else {
            Some(Faction::Water)
        }
    }

    /// Both elements present and near-equal: an active front line.
    pub fn contested(&self) -> bool {
        self.fire.max(self.water) >= ELEMENT_ACTIVE
            && (self.fire - self.water).abs() < ELEMENT_MARGIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_and_contest_thresholds() {
        assert_eq!(ElementState::default().owner(), None); // neutral
        assert_eq!(
            ElementState {
                fire: 0.9,
                water: 0.1
            }
            .owner(),
            Some(Faction::Fire)
        );
        assert_eq!(
            ElementState {
                fire: 0.1,
                water: 0.9
            }
            .owner(),
            Some(Faction::Water)
        );
        let front = ElementState {
            fire: 0.6,
            water: 0.62,
        };
        assert_eq!(front.owner(), None);
        assert!(front.contested());
    }
}

//! Player components of the server's ECS.

use bevy::prelude::*;

/// Stable identity of a player in a region app, assigned by the server.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerId(pub u64);

/// Position in local world-space meters (ENU plane at the region centroid,
/// [`crate::geo::project_enu`]).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Position(pub Vec3);

/// Marker for an entity that represents a player.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Player;

//! `elements-shared` — game rules and simulation building blocks without
//! database or network: used by the server (and by offline tools).
//!
//! Crate layout:
//! - [`components`]  — player components
//! - [`elements`]    — faction + per-node element state (S3)
//! - [`field`]       — element spread on the graph
//! - [`rules`]       — game rules as data, claim state
//! - [`source`]      — a source's life: strength over age, takeover
//! - [`hinterland`]  — sparks and supply
//! - [`scoring`]     — points
//! - [`progression`] — levels, XP, abilities, challenges
//! - [`weather`]     — the day's weather as a rule
//! - [`tide`]        — tide at the waterfront as a rule
//! - [`streak`]      — weekly streaks of play days (server)
//! - [`geo`]         — static world: the region road graph (nodes/edges)

pub mod components;
pub mod elements;
pub mod field;
pub mod geo;
pub mod hinterland;
pub mod mottos;
pub mod progression;
pub mod radar;
pub mod rules;
pub mod scoring;
pub mod source;
pub mod streak;
pub mod tide;
pub mod weather;

/// Re-exports commonly used by the server.
pub mod prelude {
    pub use crate::components::*;
    pub use crate::elements::*;
    pub use crate::geo::*;
}

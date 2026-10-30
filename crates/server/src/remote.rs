//! Links into other regions (`[keep] link`): region X, where the player
//! stands at an own permanent source, asks region Y to claim around (or look
//! at) the source at the link's far end; Y answers with a message to X.
//! X checks the player, Y checks the key and the map. Same server: through
//! the pool (Y is loaded if needed). Other cells and shards: not yet.

use std::sync::Arc;

use elements_shared::prelude::Faction;

use crate::gateway::FromWeb;
use crate::regions::Host;

/// X to Y: through the source that `key` opens, claim (or only `look`).
#[derive(Debug)]
pub struct Ask {
    /// The asking region (slug), for the answer.
    pub from: String,
    pub id: u64,
    /// Ticket of the source in Y (from the app; passed on, never stored).
    pub key: String,
    pub faction: Faction,
    /// The player's strength in X (level, new identity), before link factors.
    pub mult: f32,
    /// X's `link_factor`; Y takes the smaller of both.
    pub factor: f32,
    pub newcomer: bool,
    pub look: bool,
}

/// Y to X: what fell there, or why nothing did.
#[derive(Debug, Default)]
pub struct Answer {
    pub id: u64,
    pub ok: bool,
    pub reason: Option<&'static str>,
    /// The junction in Y and its (lon, lat).
    pub node: Option<i64>,
    pub at: Option<[f64; 2]>,
    /// Its (fire, water) before the claim (points in X).
    pub prior: Option<(f32, f32)>,
    pub build: f32,
    pub doused: bool,
}

/// Hand `msg` to region `to`; false if it can't be reached from here.
pub fn deliver(host: &Arc<Host>, to: &str, msg: FromWeb) -> bool {
    match host.get(to) {
        Ok(r) => r.tx.send(msg).is_ok(),
        Err(_) => false,
    }
}

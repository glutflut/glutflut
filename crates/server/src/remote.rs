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
    /// Frames this ask already waited for Y's permanent sources (just woken).
    pub tries: u8,
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

/// X to Y: make the found key `key` (made in X) to a random permanent
/// source of `faction` in Y; answered with [`Minted`].
#[derive(Debug)]
pub struct Mint {
    pub from: String,
    pub id: u64,
    pub key: String,
    pub faction: Faction,
    pub origin: &'static str,
}

/// Y (or the database writer) to X: the found key exists now, or not
/// (no permanent source of that side there).
#[derive(Debug)]
pub struct Minted {
    pub id: u64,
    pub ok: bool,
}

/// To Y, which woke while the key was written to the database: take it
/// into memory too (its next save would drop it).
#[derive(Debug)]
pub struct AddKey {
    pub hash: crate::keep::Hash,
    pub key: crate::keep::Key,
}

/// Region `to` runs here now (not asleep, not still loading).
pub fn awake(host: &Arc<Host>, to: &str) -> bool {
    host.peek(to).is_some_and(|r| r.tx.running())
}

/// Hand `msg` to region `to`; false if it can't be reached from here.
pub fn deliver(host: &Arc<Host>, to: &str, msg: FromWeb) -> bool {
    match host.get(to) {
        Ok(r) => r.tx.send(msg).is_ok(),
        Err(_) => false,
    }
}

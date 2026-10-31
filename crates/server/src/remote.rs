//! Links into other regions (`[keep] link`): region X, where the player
//! stands at an own permanent source, asks region Y to claim around (or look
//! at) the source at the link's far end; Y answers with a message to X.
//! X checks the player, Y checks the key and the map. Same server: through
//! the pool (Y is loaded if needed). Another cell of this shard: a signed
//! `POST /api/cell/far?region=Y` to the shard's own address, which the proxy
//! routes to Y's cell ([`receive`] there). Other shards: not yet.

use std::sync::{Arc, OnceLock};

use crossbeam_channel::Sender;
use elements_shared::prelude::Faction;
use serde::{Deserialize, Deserializer, Serialize};
use tracing::warn;

use crate::gateway::FromWeb;
use crate::regions::Host;

/// X to Y: through the source that `key` opens, claim (or only `look`).
#[derive(Debug, Serialize, Deserialize)]
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
    /// The player's reach in X (Weitblick armed or not); Y caps it.
    pub reach_m: f32,
    pub look: bool,
    /// Frames this ask already waited for Y's permanent sources (just woken).
    pub tries: u8,
    /// The player's id (`score::token_id`): the claim is theirs in Y too,
    /// once they play there.
    pub player: Option<[u8; 32]>,
}

/// Y to X: what fell there, or why nothing did.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Answer {
    pub id: u64,
    pub ok: bool,
    #[serde(deserialize_with = "de_reason")]
    pub reason: Option<Fixed>,
    /// The junction in Y and its (lon, lat).
    pub node: Option<i64>,
    pub at: Option<[f64; 2]>,
    /// Its (fire, water) before the claim (points in X).
    pub prior: Option<(f32, f32)>,
    pub build: f32,
    pub doused: bool,
    /// Junctions within `NEAR_M` of it after the claim: (lon, lat, fire,
    /// water), and the streets between them (index pairs), for X's app to
    /// show the place.
    pub near: Vec<[f64; 4]>,
    pub streets: Vec<[u16; 2]>,
    /// The permanent source B it reached from (lon, lat).
    pub from: Option<[f64; 2]>,
}

/// How far around the claimed junction the answer shows the junctions.
pub const NEAR_M: f32 = 200.0;

/// X to Y: make the found key `key` (made in X) to a random permanent
/// source of `faction` in Y; answered with [`Minted`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Mint {
    pub from: String,
    pub id: u64,
    pub key: String,
    pub faction: Faction,
    #[serde(deserialize_with = "de_origin")]
    pub origin: Fixed,
    /// X's `found_uses` and `found_days`, for Y asleep in another cell
    /// (written to the database there).
    pub uses: u32,
    pub days: f32,
}

/// Y (or the database writer) to X: the found key exists now, or not
/// (no permanent source of that side there).
#[derive(Debug, Serialize, Deserialize)]
pub struct Minted {
    pub id: u64,
    pub ok: bool,
}

/// To Y, which woke while the key was written to the database: take it
/// into memory too (its next save would drop it).
#[derive(Debug, Serialize, Deserialize)]
pub struct AddKey {
    pub hash: crate::keep::Hash,
    pub key: crate::keep::Key,
}

/// A string from the code (`'static` hidden from serde's derive, which
/// would otherwise only deserialize from `'static` input).
pub type Fixed = &'static str;

/// Reasons and origins come back as the fixed strings the code uses.
const REASONS: &[&str] = &[
    "nokey", "linked", "worn", "linkdown", "asleep", "cooldown", "unknown", "range", "own",
    "strong", "supplied",
];
pub(crate) const ORIGINS: &[&str] = &["gift", "bottle", "ember", "city"];

fn de_reason<'de, D: Deserializer<'de>>(d: D) -> Result<Option<&'static str>, D::Error> {
    let s = Option::<String>::deserialize(d)?;
    Ok(s.map(|s| {
        REASONS
            .iter()
            .find(|r| **r == s)
            .copied()
            .unwrap_or("linkdown")
    }))
}

pub(crate) fn de_origin<'de, D: Deserializer<'de>>(d: D) -> Result<&'static str, D::Error> {
    let s = String::deserialize(d)?;
    ORIGINS
        .iter()
        .find(|o| **o == s)
        .copied()
        .ok_or_else(|| serde::de::Error::custom("unknown origin"))
}

/// What travels between cells.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    Ask(Ask),
    Answer(Answer),
    Mint(Mint),
    Minted(Minted),
    Key(AddKey),
    KeysGone(String),
}

impl Wire {
    fn of(msg: FromWeb) -> Option<Self> {
        Some(match msg {
            FromWeb::FarAsk(a) => Wire::Ask(a),
            FromWeb::FarAnswer(a) => Wire::Answer(a),
            FromWeb::FarMint(m) => Wire::Mint(m),
            FromWeb::FarMinted(m) => Wire::Minted(m),
            FromWeb::FarKey(k) => Wire::Key(k),
            FromWeb::FarKeysGone(b) => Wire::KeysGone(b),
            _ => return None,
        })
    }

    fn into_msg(self) -> FromWeb {
        match self {
            Wire::Ask(a) => FromWeb::FarAsk(a),
            Wire::Answer(a) => FromWeb::FarAnswer(a),
            Wire::Mint(m) => FromWeb::FarMint(m),
            Wire::Minted(m) => FromWeb::FarMinted(m),
            Wire::Key(k) => FromWeb::FarKey(k),
            Wire::KeysGone(b) => FromWeb::FarKeysGone(b),
        }
    }
}

/// The body of `POST /api/cell/far`.
#[derive(Serialize, Deserialize)]
pub struct Envelope {
    pub to: String,
    pub msg: Wire,
}

/// Region `to` runs in another cell of this shard.
pub fn other_cell(host: &Host, to: &str) -> bool {
    host.listing(to)
        .is_some_and(|l| l.shard == host.shard() && l.cell != host.cell())
}

/// Region `to` runs here now (not asleep, not still loading). In another
/// cell: unknown from here, so true (that cell answers a look with
/// `asleep` itself).
pub fn awake(host: &Arc<Host>, to: &str) -> bool {
    other_cell(host, to) || host.peek(to).is_some_and(|r| r.tx.running())
}

/// Hand `msg` to region `to`; false if it can't be reached from here.
/// Another cell: queued for the sender (a lost call ends in the asker's
/// timeout, `linkdown`).
pub fn deliver(host: &Arc<Host>, to: &str, msg: FromWeb) -> bool {
    if other_cell(host, to) {
        return Wire::of(msg).is_some_and(|w| send_cell(to, w));
    }
    local(host, to, msg)
}

fn local(host: &Arc<Host>, to: &str, msg: FromWeb) -> bool {
    match host.get(to) {
        Ok(r) => r.tx.send(msg).is_ok(),
        Err(_) => false,
    }
}

/// Only if loaded here (never wakes it).
fn loaded(host: &Arc<Host>, to: &str, msg: FromWeb) -> bool {
    host.peek(to).is_some_and(|r| r.tx.send(msg).is_ok())
}

/// Base URL that reaches every cell of this shard by `?region=`
/// (`ELEMENTS_CELL_URL`, else `ELEMENTS_PUBLIC_URL`).
fn cell_url() -> String {
    std::env::var("ELEMENTS_CELL_URL")
        .or_else(|_| std::env::var("ELEMENTS_PUBLIC_URL"))
        .unwrap_or_else(|_| "https://glutflut.com".into())
        .trim_end_matches('/')
        .to_string()
}

fn send_cell(to: &str, w: Wire) -> bool {
    static TX: OnceLock<Option<Sender<(String, String)>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<(String, String)>();
        let base = cell_url();
        std::thread::Builder::new()
            .name("cell-far".into())
            .spawn(move || {
                for (to, body) in rx {
                    post(&base, &to, &body);
                }
            })
            .ok()?;
        Some(tx)
    });
    let Some(tx) = tx else {
        return false;
    };
    let body = match serde_json::to_string(&Envelope {
        to: to.to_string(),
        msg: w,
    }) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "cell far: encode");
            return false;
        }
    };
    tx.send((to.to_string(), body)).is_ok()
}

fn post(base: &str, to: &str, body: &str) {
    let s = crate::shards::get();
    let Some(sig) = s.sign(body) else {
        warn!("cell far: ELEMENTS_SHARD_KEY unset, no links into other cells");
        return;
    };
    let Some(agent) = crate::shards::agent() else {
        return;
    };
    let res = agent
        .post(&format!("{base}/api/cell/far"))
        .query("region", to)
        .set("content-type", "application/json")
        .set("x-elements-sig", &sig)
        .send_string(body);
    if let Err(e) = res {
        warn!(region = to, error = %e, "cell far: call failed");
    }
}

/// In Y's cell: a message from another cell. Looks and the rest never wake
/// a region; a claim does; a found key for a sleeping region goes to the
/// database. Never passed on again (a stale proxy can't loop it).
pub fn receive(host: &Arc<Host>, to: &str, w: Wire) -> bool {
    match w {
        Wire::Ask(a) if a.look && !host.peek(to).is_some_and(|r| r.tx.running()) => {
            let from = a.from.clone();
            deliver(
                host,
                &from,
                FromWeb::FarAnswer(Answer {
                    id: a.id,
                    reason: Some("asleep"),
                    ..Default::default()
                }),
            )
        }
        Wire::Ask(a) => local(host, to, FromWeb::FarAsk(a)),
        Wire::Mint(m) if host.peek(to).is_none() => {
            write_found(host.clone(), to.to_string(), m);
            true
        }
        w => loaded(host, to, w.into_msg()),
    }
}

/// A found key for region `y` while it sleeps: straight into the database
/// (without waking it), then [`Minted`] to the asker.
pub fn write_found(host: Arc<Host>, y: String, m: Mint) {
    let Some(db) = host.db().map(String::from) else {
        return;
    };
    let Some((kh, _)) = crate::keep::hashes(&m.key) else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    std::thread::spawn(move || {
        let days = f64::from(m.days);
        let res = crate::found::write_key(
            &db,
            &y,
            m.faction.as_str(),
            m.origin,
            &kh,
            m.uses as i32,
            days,
        );
        if let Err(e) = &res {
            warn!(error = %e, "found key: write failed");
        }
        let keep = res.ok().flatten();
        // Woke meanwhile: it read its keys before this one.
        if let (Some(k), Some(_)) = (&keep, host.peek(&y)) {
            if let Ok(keep) = crate::keep::Hash::try_from(k.as_slice()) {
                let key = crate::keep::Key {
                    keep,
                    origin: m.origin,
                    uses: m.uses,
                    made: now,
                    expires: now + days * 86_400.0,
                    ..Default::default()
                };
                deliver(&host, &y, FromWeb::FarKey(AddKey { hash: kh, key }));
            }
        }
        let ok = keep.is_some();
        deliver(&host, &m.from, FromWeb::FarMinted(Minted { id: m.id, ok }));
    });
}

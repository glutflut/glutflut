//! Players across shards. Each shard has its own database; a player belongs to the
//! shard they first played on (`game.players.home` NULL there). Switching to
//! another shard, the app carries a signed passport (`/api/passport`, in the
//! URL fragment); that shard keeps a visitor row (`home`, `home_id`).
//! Badges earned abroad and deletions travel as events (`game.shard_outbox`,
//! sent by [`spawn_outbox`] to `/api/shard/event`); a recovery code unknown
//! here is tried on the other shards (`/api/shard/recover`).
//!
//! Shards share `ELEMENTS_SHARD_KEY` (HMAC-SHA256 over passports and calls).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::score::token_id;

/// How long a passport is good for.
const PASSPORT_S: u64 = 600;
/// Clock skew accepted on signed calls.
const SKEW_S: u64 = 300;
/// Undeliverable events are dropped after this many tries (~ a week).
const MAX_TRIES: i32 = 60;

pub struct Shards {
    /// This shard.
    pub me: String,
    /// The other shards' base URLs.
    pub urls: HashMap<String, String>,
    key: Option<Vec<u8>>,
}

static SHARDS: OnceLock<Shards> = OnceLock::new();

pub fn init(me: String, urls: HashMap<String, String>, key: Option<String>) {
    let key = key.filter(|k| !k.is_empty()).map(String::into_bytes);
    if key.is_none() && !urls.is_empty() {
        warn!("ELEMENTS_SHARD_KEY unset: no passports, players don't cross shards");
    }
    let urls = urls.into_iter().filter(|(k, _)| *k != me).collect();
    let _ = SHARDS.set(Shards { me, urls, key });
}

/// The configuration (a single `eu` shard until [`init`]).
pub fn get() -> &'static Shards {
    SHARDS.get_or_init(|| Shards {
        me: "eu".into(),
        urls: HashMap::new(),
        key: None,
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0 && s.is_ascii())
        .then(|| {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
                .collect()
        })
        .flatten()
}

fn id32(s: &str) -> Option<[u8; 32]> {
    unhex(s).and_then(|v| v.try_into().ok())
}

pub(crate) fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.iter().map(|x| x ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

/// Compare without leaking where they differ.
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Who a player is, signed by the shard that issues it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Passport {
    pub home: String,
    /// Their id (hex) on the home shard.
    pub id: String,
    pub faction: String,
    pub name: String,
    pub hidden: bool,
    pub xp_prev: f32,
    pub xp: (f32, f32, u8),
    pub xp_day: i64,
    pub claims: i32,
    pub collect: bool,
    pub badges: Vec<String>,
    /// Starts a new visitor row's streak (older shards don't send it).
    #[serde(default)]
    pub streak: elements_shared::streak::Streak,
    pub exp: u64,
}

impl Shards {
    pub fn enabled(&self) -> bool {
        self.key.is_some()
    }

    pub fn issue(&self, p: &Passport) -> Option<String> {
        let key = self.key.as_ref()?;
        let body = hex(serde_json::to_string(p).ok()?.as_bytes());
        let mac = hmac(key, body.as_bytes());
        Some(format!("{body}.{}", hex(&mac)))
    }

    pub fn verify(&self, s: &str) -> Option<Passport> {
        let key = self.key.as_ref()?;
        let (body, mac) = s.split_once('.')?;
        if !same(&unhex(mac)?, &hmac(key, body.as_bytes())) {
            return None;
        }
        let p: Passport = serde_json::from_slice(&unhex(body)?).ok()?;
        (p.exp >= now() && id32(&p.id).is_some()).then_some(p)
    }

    /// A verified passport, used up: the same one again is refused (a URL
    /// replayed from the history must not take the player over).
    pub fn redeem_passport(&self, signed: &str) -> Option<Passport> {
        static USED: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
        let p = self.verify(signed)?;
        let mac = signed.split_once('.')?.1.to_string();
        let mut used = USED.lock().unwrap();
        let t = now();
        used.retain(|(_, exp)| *exp >= t);
        if used.iter().any(|(m, _)| *m == mac)
            || !crate::cells::first_use(&format!("pass:{mac}"), p.exp)
        {
            warn!("passport used twice");
            return None;
        }
        used.push((mac, p.exp));
        Some(p)
    }

    /// `x-elements-sig` for a call to another shard: time, nonce, MAC.
    pub fn sign(&self, body: &str) -> Option<String> {
        let key = self.key.as_ref()?;
        let head = format!("{}.{:016x}", now(), rand::random::<u64>());
        let mac = hex(&hmac(key, format!("{head}.{body}").as_bytes()));
        Some(format!("{head}.{mac}"))
    }

    /// A valid signature, fresh and not seen before (a recorded call
    /// replayed within the skew window is refused).
    pub fn check(&self, sig: &str, body: &str) -> bool {
        static SEEN: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
        let Some(key) = self.key.as_ref() else {
            return false;
        };
        let Some((head, mac)) = sig.rsplit_once('.') else {
            return false;
        };
        let Some(ts) = head
            .split_once('.')
            .and_then(|(t, _)| t.parse::<u64>().ok())
        else {
            return false;
        };
        let t = now();
        if ts.abs_diff(t) > SKEW_S
            || !unhex(mac)
                .is_some_and(|m| same(&m, &hmac(key, format!("{head}.{body}").as_bytes())))
        {
            return false;
        }
        let mut seen = SEEN.lock().unwrap();
        seen.retain(|(_, at)| at.abs_diff(t) <= SKEW_S);
        if seen.iter().any(|(m, _)| m == mac)
            || !crate::cells::first_use(&format!("sig:{mac}"), ts + SKEW_S)
        {
            warn!("shard call replayed");
            return false;
        }
        seen.push((mac.to_string(), ts));
        true
    }

    /// The other shards (a shard may list itself in ELEMENTS_SHARDS).
    pub fn others(&self) -> Vec<&str> {
        self.urls
            .keys()
            .map(String::as_str)
            .filter(|k| *k != self.me)
            .collect()
    }

    /// POST a signed JSON body to shard `name`; the answer's body on 2xx.
    pub fn call(&self, name: &str, path: &str, body: &str) -> Option<String> {
        let url = self.urls.get(name)?;
        let sig = self.sign(body)?;
        let res = agent()?
            .post(&format!("{url}{path}"))
            .set("content-type", "application/json")
            .set("x-elements-sig", &sig)
            .send_string(body);
        match res {
            Ok(r) => r.into_string().ok(),
            Err(e) => {
                warn!(shard = name, path, error = %e, "shard call failed");
                None
            }
        }
    }
}

/// One HTTP agent for calls to other shards (ureq is built without a
/// default TLS backend: the connector is set here, as in weather.rs).
pub fn agent() -> Option<&'static ureq::Agent> {
    static AGENT: OnceLock<Option<ureq::Agent>> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            let tls = native_tls::TlsConnector::new()
                .map_err(|e| warn!(error = %e, "shard calls: no TLS"))
                .ok()?;
            Some(
                ureq::AgentBuilder::new()
                    .tls_connector(std::sync::Arc::new(tls))
                    .timeout(Duration::from_secs(10))
                    .user_agent("glutflut.com")
                    .build(),
            )
        })
        .as_ref()
}

/// The player of `token` here as a passport (their home's id and shard).
pub fn passport_of(
    c: &mut postgres::Client,
    me: &str,
    id: &[u8; 32],
) -> Result<Option<Passport>, postgres::Error> {
    let row = c.query_opt(
        "SELECT faction, name, hidden, xp_prev, xp_day_base, xp_day_claims, xp_day_kinds, \
         (day - DATE '1970-01-01')::int, claims, collect, home, home_id, \
         ARRAY(SELECT badge FROM game.player_badges b WHERE b.id = p.id), \
         streak_weeks, (streak_week - DATE '1970-01-01')::int, streak_days, \
         (streak_day - DATE '1970-01-01')::int, streak_acts, streak_spared \
         FROM game.players p WHERE id = $1",
        &[&&id[..]],
    )?;
    Ok(row.map(|r| {
        let home: Option<String> = r.get(10);
        let home_id: Option<Vec<u8>> = r.get(11);
        Passport {
            home: home.unwrap_or_else(|| me.to_string()),
            id: hex(home_id.as_deref().unwrap_or(&id[..])),
            faction: r.get(0),
            name: r.get(1),
            hidden: r.get(2),
            xp_prev: r.get(3),
            xp: (r.get(4), r.get(5), r.get::<_, i16>(6) as u8),
            xp_day: r.get::<_, i32>(7) as i64,
            claims: r.get(8),
            collect: r.get(9),
            badges: r.get(12),
            streak: crate::score::streak_of(&r, 13),
            exp: now() + PASSPORT_S,
        }
    }))
}

/// A player arrives with a passport. Their row here (the home row if this is
/// their home, else the visitor row) is refreshed or created, keyed by
/// `token` if it already is theirs, else by `fresh`. Returns the token to
/// use and, if an existing row changed id, `(old, new)` for the regions.
pub fn arrive(
    c: &mut postgres::Client,
    me: &str,
    p: &Passport,
    token: Option<&str>,
    fresh: &str,
    region: &str,
) -> Result<(String, Option<([u8; 32], [u8; 32])>), postgres::Error> {
    let pid = id32(&p.id).unwrap_or_default();
    let mut t = c.transaction()?;
    let row = if p.home == me {
        t.query_opt(
            "SELECT id FROM game.players WHERE id = $1 AND home IS NULL FOR UPDATE",
            &[&&pid[..]],
        )?
    } else {
        t.query_opt(
            "SELECT id FROM game.players WHERE home = $1 AND home_id = $2 FOR UPDATE",
            &[&p.home, &&pid[..]],
        )?
    };
    let old: Option<[u8; 32]> = row.and_then(|r| r.get::<_, Vec<u8>>(0).try_into().ok());
    let mine = token.filter(|tk| old == Some(token_id(tk)));
    let use_token = mine.unwrap_or(fresh).to_string();
    let new = token_id(&use_token);
    let rekey = match old {
        Some(o) if o != new => {
            t.execute(
                "UPDATE game.players SET id = $2 WHERE id = $1",
                &[&&o[..], &&new[..]],
            )?;
            if p.home == me {
                queue_rekey(&mut t, &o, &new)?;
            }
            Some((o, new))
        }
        _ => None,
    };
    let (home, home_id) = if p.home == me {
        (None, None)
    } else {
        (Some(p.home.as_str()), Some(&pid[..]))
    };
    let (base, claims_xp, kinds) = p.xp;
    let st = p.streak;
    // Side, name and hidden follow the passport; XP and claims never go down.
    t.execute(
        "INSERT INTO game.players (region, id, faction, name, day, hidden, last_seen, \
          xp_prev, xp_day_base, xp_day_claims, xp_day_kinds, collect, claims, home, home_id, \
          streak_weeks, streak_week, streak_days, streak_day, streak_acts, streak_spared) \
         VALUES ($1, $2, $3, $4, DATE '1970-01-01' + $5::int, $6, current_date, $7, $8, $9, $10, $11, $12, $13, $14, \
          $15, NULLIF($16::int, 0) + DATE '1970-01-01', $17, NULLIF($18::int, 0) + DATE '1970-01-01', $19, $20) \
         ON CONFLICT (id) DO UPDATE SET faction = EXCLUDED.faction, name = EXCLUDED.name, \
          hidden = EXCLUDED.hidden, collect = EXCLUDED.collect, \
          xp_prev = GREATEST(game.players.xp_prev, EXCLUDED.xp_prev), \
          claims = GREATEST(game.players.claims, EXCLUDED.claims), \
          last_seen = current_date",
        &[
            &region,
            &&new[..],
            &p.faction,
            &p.name,
            &(p.xp_day as i32),
            &p.hidden,
            &p.xp_prev,
            &base,
            &claims_xp,
            &(kinds as i16),
            &p.collect,
            &p.claims,
            &home,
            &home_id,
            &(st.weeks as i16),
            &(st.week as i32),
            &(st.days as i16),
            &(st.day as i32),
            &(st.acts as i16),
            &(st.spared as i16),
        ],
    )?;
    t.execute(
        "INSERT INTO game.player_badges (id, badge) SELECT $1, b FROM unnest($2::text[]) b \
         WHERE b ~ '^[a-z0-9:-]+$' ON CONFLICT DO NOTHING",
        &[&&new[..], &p.badges],
    )?;
    t.commit()?;
    info!(home = %p.home, visitor = p.home != me, rekeyed = rekey.is_some(), "player arrived with passport");
    Ok((use_token, rekey))
}

/// An event from another shard.
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    /// The player `id` of shard `home` was deleted somewhere.
    Forget { home: String, id: String },
    /// A badge earned abroad by the player `id` of this shard.
    Badge { id: String, badge: String },
    /// The player `old` of shard `home` is `new` there now (recovery code).
    Rekey {
        home: String,
        old: String,
        new: String,
    },
}

/// Apply an event; returns the ids of rows deleted here (to evict them).
pub fn apply(
    c: &mut postgres::Client,
    me: &str,
    ev: &Event,
) -> Result<Vec<[u8; 32]>, postgres::Error> {
    match ev {
        Event::Forget { home, id } => {
            let Some(id) = id32(id) else {
                return Ok(Vec::new());
            };
            let rows = if home == me {
                // Told by a visitor's shard: the home tells all the others
                // (after any rekey it sent them, so no copy is left behind).
                let mut t = c.transaction()?;
                queue_forget(&mut t, &id)?;
                let rows = t.query(
                    "DELETE FROM game.players WHERE id = $1 AND home IS NULL RETURNING id",
                    &[&&id[..]],
                )?;
                t.commit()?;
                rows
            } else {
                c.query(
                    "DELETE FROM game.players WHERE home = $1 AND home_id = $2 RETURNING id",
                    &[home, &&id[..]],
                )?
            };
            Ok(rows
                .into_iter()
                .filter_map(|r| r.get::<_, Vec<u8>>(0).try_into().ok())
                .collect())
        }
        Event::Rekey { home, old, new } => {
            let (Some(old), Some(new)) = (id32(old), id32(new)) else {
                return Ok(Vec::new());
            };
            // Already here under the new id (arrived first): the old copy goes.
            let mut t = c.transaction()?;
            let gone = t.query(
                "DELETE FROM game.players WHERE home = $1 AND home_id = $2 \
                 AND EXISTS (SELECT 1 FROM game.players WHERE home = $1 AND home_id = $3) RETURNING id",
                &[home, &&old[..], &&new[..]],
            )?;
            t.execute(
                "UPDATE game.players SET home_id = $3 WHERE home = $1 AND home_id = $2",
                &[home, &&old[..], &&new[..]],
            )?;
            t.commit()?;
            Ok(gone
                .into_iter()
                .filter_map(|r| r.get::<_, Vec<u8>>(0).try_into().ok())
                .collect())
        }
        Event::Badge { id, badge } => {
            if let Some(id) = id32(id) {
                c.execute(
                    "INSERT INTO game.player_badges (id, badge) SELECT id, $2 FROM game.players \
                     WHERE id = $1 AND home IS NULL AND $2 ~ '^[a-z0-9:-]+$' ON CONFLICT DO NOTHING",
                    &[&&id[..], badge],
                )?;
            }
            Ok(Vec::new())
        }
    }
}

/// Queue the deletion of player `id` for every other shard (before the row
/// goes; a visitor's home learns it too).
pub fn queue_forget(t: &mut postgres::Transaction, id: &[u8; 32]) -> Result<(), postgres::Error> {
    let s = get();
    let others = s.others();
    t.execute(
        "INSERT INTO game.shard_outbox (shard, body) \
         SELECT o, json_build_object('kind', 'forget', 'home', coalesce(p.home, $2), \
                'id', encode(coalesce(p.home_id, p.id), 'hex')) \
         FROM game.players p, unnest($3::text[]) o WHERE p.id = $1",
        &[&&id[..], &s.me, &others],
    )?;
    poke();
    Ok(())
}

/// A player of this shard changed id: the other shards' visitor rows follow.
pub fn queue_rekey(
    c: &mut impl postgres::GenericClient,
    old: &[u8; 32],
    new: &[u8; 32],
) -> Result<(), postgres::Error> {
    let s = get();
    let others = s.others();
    let body =
        serde_json::json!({ "kind": "rekey", "home": s.me, "old": hex(old), "new": hex(new) })
            .to_string();
    c.execute(
        "INSERT INTO game.shard_outbox (shard, body) SELECT o, $2::text::jsonb FROM unnest($1::text[]) o",
        &[&others, &body],
    )?;
    poke();
    Ok(())
}

/// Queue a badge earned here by a visitor for their home.
pub fn queue_badge(
    c: &mut postgres::Client,
    id: &[u8; 32],
    badge: &str,
) -> Result<(), postgres::Error> {
    c.execute(
        "INSERT INTO game.shard_outbox (shard, body) \
         SELECT home, json_build_object('kind', 'badge', 'id', encode(home_id, 'hex'), 'badge', $2::text) \
         FROM game.players WHERE id = $1 AND home IS NOT NULL",
        &[&&id[..], &badge],
    )?;
    poke();
    Ok(())
}

/// Redeem a recovery code for another shard: the player moves to a token no
/// device has (the old one loses them, as with a local code); the passport
/// lets the asking shard take them in. `(old, new)` for the regions here.
pub fn redeem(
    c: &mut postgres::Client,
    me: &str,
    code: &str,
) -> Result<Option<(Passport, [u8; 32], [u8; 32])>, postgres::Error> {
    let token = format!("{:032x}", rand::random::<u128>());
    let Some(old) = crate::score::recover(c, code, &token)? else {
        return Ok(None);
    };
    let new = token_id(&token);
    queue_rekey(c, &old, &new)?;
    Ok(passport_of(c, me, &new)?.map(|p| (p, old, new)))
}

/// Something was queued (`queue_*`): the outbox looks soon.
fn poke() {
    if let Some(tx) = WAKE.get() {
        let _ = tx.try_send(());
    }
}

static WAKE: std::sync::OnceLock<crossbeam_channel::Sender<()>> = std::sync::OnceLock::new();

/// After a poke, look this often for a while (the queuing transaction may
/// not be committed yet); otherwise hourly for retries and other cells, so an idle
/// database can scale to zero.
const BUSY_EVERY: Duration = Duration::from_secs(10);
const BUSY_FOR: Duration = Duration::from_secs(120);
const IDLE_EVERY: Duration = Duration::from_secs(3600);

/// Deliver queued events to the other shards, in a thread.
pub fn spawn_outbox(db: String) {
    let s = get();
    if s.urls.is_empty() {
        return;
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    let _ = WAKE.set(tx);
    std::thread::Builder::new()
        .name("shard-outbox".into())
        .spawn(move || {
            let mut conn = crate::db::Reconnecting::new("shard outbox");
            // Look once at start (left over from before a restart).
            let mut busy_until = std::time::Instant::now() + BUSY_FOR;
            loop {
                let now = std::time::Instant::now();
                let wait = if now < busy_until {
                    BUSY_EVERY
                } else {
                    IDLE_EVERY
                };
                if rx.recv_timeout(wait).is_ok() {
                    busy_until = std::time::Instant::now() + BUSY_FOR;
                    std::thread::sleep(Duration::from_secs(1));
                }
                let Some(c) = conn.get(&db) else { continue };
                if let Err(e) = deliver(c, s) {
                    conn.failed("delivery", &e);
                }
            }
        })
        .expect("shard outbox thread");
}

fn deliver(c: &mut postgres::Client, s: &Shards) -> Result<(), postgres::Error> {
    let mut t = c.transaction()?;
    let rows = t.query(
        "SELECT id, shard, body::text, tries FROM game.shard_outbox \
         WHERE next_at <= now() AND shard <> $1 ORDER BY id LIMIT 50 FOR UPDATE SKIP LOCKED",
        &[&s.me],
    )?;
    let mut failed: Vec<String> = Vec::new();
    for r in rows {
        let (id, shard, body, tries): (i64, String, String, i32) =
            (r.get(0), r.get(1), r.get(2), r.get(3));
        if failed.contains(&shard) {
            continue;
        }
        if s.call(&shard, "/api/shard/event", &body).is_some() {
            t.execute("DELETE FROM game.shard_outbox WHERE id = $1", &[&id])?;
        } else if tries + 1 >= MAX_TRIES {
            warn!(shard, body, "shard outbox: giving up");
            t.execute("DELETE FROM game.shard_outbox WHERE id = $1", &[&id])?;
        } else {
            // 20 s, 40 s, ... up to 3 hours; the rest for that shard waits
            // too, so events arrive in order (a deletion after a rekey).
            let wait = (20i64 << tries.min(10)).min(3 * 3600);
            t.execute(
                "UPDATE game.shard_outbox SET tries = tries + 1 WHERE id = $1",
                &[&id],
            )?;
            t.execute(
                "UPDATE game.shard_outbox SET next_at = now() + make_interval(secs => $2::int) \
                 WHERE shard = $1",
                &[&shard, &(wait as i32)],
            )?;
            failed.push(shard);
        }
    }
    t.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shards() -> Shards {
        Shards {
            me: "main".into(),
            urls: HashMap::new(),
            key: Some(b"secret".to_vec()),
        }
    }

    fn passport(exp: u64) -> Passport {
        Passport {
            home: "main".into(),
            id: hex(&token_id("t")),
            faction: "fire".into(),
            name: "Glimmender Kormoran 74".into(),
            hidden: false,
            xp_prev: 12.0,
            xp: (1.0, 2.0, 1),
            xp_day: 20_000,
            claims: 3,
            collect: true,
            badges: vec!["claims-1".into()],
            streak: Default::default(),
            exp,
        }
    }

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        let mac = hmac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn passport_round_trip_and_tampering() {
        let s = shards();
        let p = passport(now() + 60);
        let signed = s.issue(&p).unwrap();
        assert_eq!(s.verify(&signed), Some(p.clone()));
        // Another key, an edited body, an expired one: refused.
        let other = Shards {
            key: Some(b"other".to_vec()),
            ..shards()
        };
        assert_eq!(other.verify(&signed), None);
        let (body, mac) = signed.split_once('.').unwrap();
        let mut b = unhex(body).unwrap();
        let i = b.iter().position(|&c| c == b'f').unwrap();
        b[i] = b'g';
        assert_eq!(s.verify(&format!("{}.{mac}", hex(&b))), None);
        assert_eq!(s.verify(&s.issue(&passport(now() - 1)).unwrap()), None);
        // Once only.
        assert!(s.redeem_passport(&signed).is_some());
        assert!(s.redeem_passport(&signed).is_none());
    }

    #[test]
    fn signed_calls() {
        let s = shards();
        let sig = s.sign("{}").unwrap();
        assert!(s.check(&sig, "{}"));
        assert!(!s.check(&sig, "{\"x\":1}"));
        assert!(!s.check(&sig, "{}"), "replay");
        assert!(s.check(&s.sign("{}").unwrap(), "{}"));
        let old = format!("{}.0", now() - 2 * SKEW_S);
        let stale = format!(
            "{old}.{}",
            hex(&hmac(b"secret", format!("{old}.{{}}").as_bytes()))
        );
        assert!(!s.check(&stale, "{}"));
    }
}

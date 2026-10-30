//! Permanent sources (Dauerquelle: Glut / Eis,
//! rules `[keep]`). A player turns an own fresh claim into one; it
//! fades to a floor instead of running out, its owner tops it up, and it
//! stays across restarts (`game.keeps`).
//!
//! Privacy: the app gets a random bearer ticket. The source stores only
//! SHA-256("keep:" ticket), the player's slot only SHA-256("slot:" ticket);
//! without the ticket the two can't be linked. Persistence points collect on
//! the source; when it falls they go to a pot (`game.keep_pot`) without place
//! or time, rounded down to tens, until the app redeems the ticket.
//! Campaign starts and ends reset the map: all permanent sources go to the pot.
//! A player has `[keep] per_player` slots (more by level, `per_level`), one
//! per source; the app holds a ticket for each.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use sha2::{Digest, Sha256};

use elements_shared::prelude::*;

use crate::analytics::now_s;
use crate::sim::{ElementSource, GameRules, NodeById, SourceKind};

const SAVE_EVERY_S: f64 = 60.0;
const POINTS_EVERY_S: f64 = 60.0;
/// Campaign boundaries are read this often.
const RESET_EVERY: Duration = Duration::from_secs(300);
/// Points are paid in steps of this (a value to the point could point back
/// to one source).
pub const POINT_STEP: f32 = 10.0;
/// A player redeems at most this often.
const REDEEM_EVERY_S: f64 = 20.0;
/// Unredeemed points are dropped after this many weeks.
const POT_WEEKS: i64 = 8;

pub type Hash = [u8; 32];

/// What makes a node's `ElementSource` (kind `Keep`) a player's: the ticket
/// hash, wear from enemy claims (`Keep::hit`) and points not paid out yet.
#[derive(Component, Clone, Copy)]
pub struct KeepInfo {
    pub ticket: Hash,
    pub wear: f32,
    pub points: f32,
    /// Linked partner's ticket hash (`[keep] link`).
    pub link: Option<Hash>,
}

fn hash(prefix: &str, ticket: &str) -> Hash {
    Sha256::digest(format!("{prefix}:{ticket}").as_bytes()).into()
}

/// (source hash, slot hash) of a ticket as the app sends it (32 hex digits).
pub fn hashes(ticket: &str) -> Option<(Hash, Hash)> {
    (ticket.len() == 32 && ticket.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| (hash("keep", ticket), hash("slot", ticket)))
}

/// A new ticket (128 random bits) and its hashes.
pub fn new_ticket() -> (String, Hash, Hash) {
    let t = format!("{:032x}", rand::random::<u128>());
    let (a, b) = hashes(&t).expect("valid ticket");
    (t, a, b)
}

/// Points as paid: rounded down to `POINT_STEP`.
pub fn payable(points: f32) -> i64 {
    ((points / POINT_STEP).floor() * POINT_STEP).max(0.0) as i64
}

/// One stored permanent source.
#[derive(Clone)]
struct Row {
    node: i64,
    faction: Faction,
    age: f32,
    built: f32,
    ramp: f32,
    wear: f32,
    mult: f32,
    points: f32,
    ticket: Hash,
    /// Tag line (`mottos`, 0 none).
    motto: u16,
    link: Option<Hash>,
}

/// A key to a permanent source for someone else (`game.keep_keys`).
#[derive(Clone, Debug)]
pub struct Key {
    /// The source's ticket hash.
    pub keep: Hash,
    /// `gift`, `bottle`, `ember`, `city`.
    pub origin: &'static str,
    pub uses: u32,
    /// Unix s.
    pub made: f64,
    pub expires: f64,
}

fn origin(s: &str) -> &'static str {
    match s {
        "bottle" => "bottle",
        "ember" => "ember",
        "city" => "city",
        _ => "gift",
    }
}

struct Snapshot {
    keeps: Vec<Row>,
    keys: Vec<(Hash, Key)>,
    pot: Vec<(Hash, i64, i64)>,
    slots: Vec<(Hash, Hash)>,
    /// Campaign boundary (unix s) the reset was for.
    reset: Option<f64>,
}

enum Msg {
    Save(Snapshot),
    Barrier(Sender<()>),
}

#[derive(Resource)]
pub struct Keeps {
    enabled: bool,
    /// Loaded rows, put on the map once the nodes exist.
    pending: Option<Vec<Row>>,
    /// Ticket hash -> points waiting, week they fell (Monday, days since 1970).
    pot: HashMap<Hash, (i64, i64)>,
    /// Player id -> slot hashes (one per permanent source).
    slots: HashMap<Hash, Vec<Hash>>,
    /// Key hash -> key to a source here, for others.
    keys: HashMap<Hash, Key>,
    /// Memory only: last redeem (unix s) and the day of the last release
    /// without a ticket, per player id.
    redeemed: HashMap<Hash, f64>,
    released: HashMap<Hash, i64>,
    last_reset: f64,
    reset_pending: Option<f64>,
    pub dirty: bool,
    save_at: f64,
    points_at: f64,
    tx: Option<crate::writers::LaneTx<Msg>>,
    rx: Option<Receiver<f64>>,
    /// Local days of the region (weeks of the pot, releases per day).
    pub clock: crate::clock::Clock,
}

impl Keeps {
    /// Load the region's permanent sources, pot and slots, and start the
    /// writer. Without `[keep]` the stored rows are left alone.
    pub fn new(db: Option<String>, region: String, enabled: bool) -> Self {
        let mut k = Self {
            enabled,
            pending: None,
            pot: HashMap::new(),
            slots: HashMap::new(),
            keys: HashMap::new(),
            redeemed: HashMap::new(),
            released: HashMap::new(),
            last_reset: 0.0,
            reset_pending: None,
            dirty: false,
            save_at: 0.0,
            points_at: 0.0,
            tx: None,
            rx: None,
            clock: Default::default(),
        };
        let Some(db) = db.filter(|_| enabled) else {
            return k;
        };
        match load(&db, &region) {
            Ok((rows, pot, slots, last, keys)) => {
                info!(
                    keeps = rows.len(),
                    pot = pot.len(),
                    slots = slots.len(),
                    keys = keys.len(),
                    "permanent sources loaded"
                );
                k.pending = Some(rows);
                k.pot = pot;
                k.slots = slots;
                k.keys = keys;
                k.last_reset = last;
            }
            Err(e) => error!(error = %e, "permanent sources: load failed"),
        }
        let (btx, brx) = crossbeam_channel::unbounded();
        let key = format!("keeps {region}");
        k.tx = Some(crate::writers::lane(&key, |rx| Writer {
            db,
            region,
            rx,
            boundaries: btx,
            check_at: Instant::now(),
        }));
        k.rx = Some(brx);
        k
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The player's slots in use.
    pub fn slots(&self, player: &Hash) -> &[Hash] {
        self.slots.get(player).map_or(&[], Vec::as_slice)
    }

    pub fn add_slot(&mut self, player: Hash, slot: Hash) {
        self.slots.entry(player).or_default().push(slot);
        self.dirty = true;
    }

    /// Free the slot if it is this ticket's.
    pub fn clear_slot_if(&mut self, player: &Hash, slot: &Hash) {
        if let Some(v) = self.slots.get_mut(player) {
            if let Some(i) = v.iter().position(|s| s == slot) {
                v.swap_remove(i);
                self.dirty = true;
            }
            if v.is_empty() {
                self.slots.remove(player);
            }
        }
    }

    /// Free the slots whose tickets the app doesn't have (lost device),
    /// keeping those in `known`: once a day. Their sources stay until they
    /// fall or expire.
    pub fn release(&mut self, player: Hash, day: i64, known: &[Hash]) -> bool {
        if self.released.get(&player) == Some(&day) {
            return false;
        }
        self.released.insert(player, day);
        if let Some(v) = self.slots.get_mut(&player) {
            let before = v.len();
            v.retain(|s| known.contains(s));
            self.dirty |= v.len() != before;
            if v.is_empty() {
                self.slots.remove(&player);
            }
        }
        true
    }

    /// May this player redeem now?
    pub fn redeem_ok(&mut self, player: Hash, now: f64) -> bool {
        if self
            .redeemed
            .get(&player)
            .is_some_and(|&t| now - t < REDEEM_EVERY_S)
        {
            return false;
        }
        self.redeemed.insert(player, now);
        true
    }

    /// Points of a fallen source into the pot.
    pub fn pot_add(&mut self, ticket: Hash, points: f32) {
        let week = crate::score::week_of(self.clock.day(now_s()));
        let e = self.pot.entry(ticket).or_insert((0, week));
        e.0 += payable(points);
        e.1 = week;
        self.dirty = true;
    }

    /// Take a fallen source's points.
    pub fn pot_take(&mut self, ticket: &Hash) -> Option<i64> {
        let p = self.pot.remove(ticket)?;
        self.dirty = true;
        Some(p.0)
    }

    /// The valid key with this hash (uses left, not expired).
    pub fn key(&self, h: &Hash, now: f64) -> Option<&Key> {
        self.keys.get(h).filter(|k| k.uses > 0 && k.expires > now)
    }

    /// A claim went through the key: one use less.
    pub fn use_key(&mut self, h: &Hash) {
        if let Some(k) = self.keys.get_mut(h) {
            k.uses = k.uses.saturating_sub(1);
            self.dirty = true;
        }
    }

    /// A key of `origin` to the source with ticket hash `keep` was made since `t`.
    pub fn made_since(&self, keep: &Hash, origin: &str, t: f64) -> bool {
        self.keys
            .values()
            .any(|k| k.keep == *keep && k.origin == origin && k.made > t)
    }

    pub fn add_key(&mut self, h: Hash, key: Key) {
        self.keys.insert(h, key);
        self.dirty = true;
    }

    /// A recovery code moved a player to a new id.
    pub fn rekey(&mut self, old: &Hash, new: Hash) {
        if let Some(s) = self.slots.remove(old) {
            self.slots.insert(new, s);
        }
    }

    pub fn forget(&mut self, id: &Hash) {
        if self.slots.remove(id).is_some() {
            self.dirty = true;
        }
    }

    fn save(&mut self, keeps: Vec<Row>, reset: Option<f64>) {
        self.dirty = false;
        let Some(tx) = &self.tx else { return };
        // Sorted by key, not in placing order: rows of `game.keeps` and
        // `game.keep_slots` written side by side mustn't line up.
        let mut keeps = keeps;
        keeps.sort_by_key(|r| r.node);
        // Keys go with their source; used up or expired ones go now (a gift
        // stays a week after expiry: one per source and week).
        let now = now_s();
        let tickets: std::collections::HashSet<Hash> = keeps.iter().map(|r| r.ticket).collect();
        self.keys.retain(|_, k| {
            tickets.contains(&k.keep) && k.expires.max(k.made + 7.0 * 86_400.0) > now
        });
        let mut keys: Vec<(Hash, Key)> = self.keys.iter().map(|(h, k)| (*h, k.clone())).collect();
        keys.sort_by_key(|k| k.0);
        let old = crate::score::week_of(self.clock.day(now_s())) - 7 * POT_WEEKS;
        self.pot.retain(|_, p| p.1 > old);
        let mut pot: Vec<(Hash, i64, i64)> = self.pot.iter().map(|(h, p)| (*h, p.0, p.1)).collect();
        pot.sort();
        let mut slots: Vec<(Hash, Hash)> = self
            .slots
            .iter()
            .flat_map(|(a, v)| v.iter().map(|b| (*a, *b)))
            .collect();
        slots.sort();
        let _ = tx.send(Msg::Save(Snapshot {
            keeps,
            keys,
            pot,
            slots,
            reset,
        }));
    }

    /// Write now and wait (shutdown).
    fn save_wait(&mut self, keeps: Vec<Row>) -> bool {
        let reset = self.reset_pending.take();
        self.save(keeps, reset);
        let Some(tx) = &self.tx else { return true };
        let (ack, done) = crossbeam_channel::bounded(1);
        tx.send(Msg::Barrier(ack)).is_ok() && done.recv_timeout(Duration::from_secs(10)).is_ok()
    }
}

/// Put loaded permanent sources on the map (once the nodes exist).
pub fn restore(
    mut commands: Commands,
    mut keeps: ResMut<Keeps>,
    node_by_id: Res<NodeById>,
    rules: Res<GameRules>,
) {
    let Some(rows) = keeps.pending.take() else {
        return;
    };
    let mut lost = 0;
    for r in rows {
        let Some(&e) = node_by_id.get(&r.node) else {
            // The graph changed under it: points to the pot.
            keeps.pot_add(r.ticket, r.points);
            lost += 1;
            continue;
        };
        commands.entity(e).insert((
            ElementSource {
                faction: r.faction,
                life: Some(f32::INFINITY),
                age: r.age,
                strength: rules.0.keep.strength(r.age, r.wear)
                    * r.mult
                    * elements_shared::rules::Rules::ramp_factor(r.ramp, r.built),
                ramp: r.ramp,
                built: r.built,
                by: None,
                mult: r.mult,
                newcomer: false,
                kind: SourceKind::Keep,
                supply: 1.0,
                motto: r.motto,
                sieged: false,
                linked: r.link.is_some(),
            },
            KeepInfo {
                ticket: r.ticket,
                wear: r.wear,
                points: r.points,
                link: r.link,
            },
        ));
    }
    if lost > 0 {
        warn!(lost, "permanent sources on unknown nodes");
    }
}

/// Fallen and expired sources to the pot, points while standing, campaign
/// resets, and saving.
pub fn tick(
    mut commands: Commands,
    mut keeps: ResMut<Keeps>,
    rules: Res<GameRules>,
    stop: Res<crate::Stop>,
    mut q: Query<(Entity, &GeoNode, &mut KeepInfo, Option<&mut ElementSource>)>,
    mut done: Local<bool>,
) {
    if !keeps.enabled || keeps.pending.is_some() || *done {
        return;
    }
    let now = now_s();
    // Taken over, given up or expired: the source is no longer a Keep.
    let mut fallen = Vec::new();
    for (e, _, k, s) in &q {
        if s.is_none_or(|s| s.kind != SourceKind::Keep) {
            keeps.pot_add(k.ticket, k.points);
            commands.entity(e).remove::<KeepInfo>();
            fallen.push(k.ticket);
        }
    }
    // Their partners lose the link.
    if !fallen.is_empty() {
        for (_, _, mut k, s) in &mut q {
            if k.link.is_some_and(|l| fallen.contains(&l)) {
                k.link = None;
                if let Some(mut s) = s {
                    s.linked = false;
                }
                keeps.dirty = true;
            }
        }
    }
    if now >= keeps.points_at {
        let first = keeps.points_at == 0.0;
        keeps.points_at = now + POINTS_EVERY_S;
        if !first {
            let per = rules.0.keep.points_per_h * (POINTS_EVERY_S / 3600.0) as f32;
            for (_, _, mut k, s) in &mut q {
                if s.is_some_and(|s| s.kind == SourceKind::Keep) {
                    k.points += per;
                }
            }
        }
    }
    // A campaign started or ended since the last reset: a clean field.
    let boundary = keeps.rx.as_ref().and_then(|rx| rx.try_iter().last());
    if let Some(b) = boundary.filter(|&b| b > keeps.last_reset) {
        let mut n = 0;
        let mut gone = Vec::new();
        for (e, _, k, s) in &q {
            let Some(s) = s.filter(|s| s.kind == SourceKind::Keep) else {
                continue;
            };
            // Strong ones stay (`[keep] survive`).
            let strong = rules.0.keep.survive > 0.0
                && rules.0.keep.strength(s.age, k.wear) >= rules.0.keep.survive;
            if strong {
                continue;
            }
            keeps.pot_add(k.ticket, k.points);
            commands
                .entity(e)
                .remove::<KeepInfo>()
                .remove::<ElementSource>();
            gone.push(k.ticket);
            n += 1;
        }
        for (_, _, mut k, s) in &mut q {
            if k.link.is_some_and(|l| gone.contains(&l)) {
                k.link = None;
                if let Some(mut s) = s {
                    s.linked = false;
                }
            }
        }
        keeps.last_reset = b;
        keeps.reset_pending = Some(b);
        keeps.dirty = true;
        info!(n, "campaign boundary: permanent sources reset");
    }
    let rows = |q: &Query<(Entity, &GeoNode, &mut KeepInfo, Option<&mut ElementSource>)>,
                reset: bool| {
        q.iter()
            .filter(|_| !reset)
            .filter_map(|(_, g, k, s)| {
                let s = s.filter(|s| s.kind == SourceKind::Keep)?;
                Some(Row {
                    node: g.id,
                    faction: s.faction,
                    age: s.age,
                    built: s.built,
                    ramp: s.ramp,
                    wear: k.wear,
                    mult: s.mult,
                    points: k.points,
                    ticket: k.ticket,
                    motto: s.motto,
                    link: k.link,
                })
            })
            .collect::<Vec<_>>()
    };
    use std::sync::atomic::Ordering::Relaxed;
    if crate::gateway::SHUTDOWN.load(Relaxed) || stop.0.load(Relaxed) {
        *done = true;
        let resetting = keeps.reset_pending.is_some();
        let ok = keeps.save_wait(rows(&q, resetting));
        info!(written = ok, "permanent sources saved");
        return;
    }
    // Every minute (ages and points move on), at once after a reset.
    if keeps.reset_pending.is_some() || now >= keeps.save_at {
        keeps.save_at = now + SAVE_EVERY_S;
        let reset = keeps.reset_pending.take();
        let r = rows(&q, reset.is_some());
        keeps.save(r, reset);
    }
}

type Loaded = (
    Vec<Row>,
    HashMap<Hash, (i64, i64)>,
    HashMap<Hash, Vec<Hash>>,
    f64,
    HashMap<Hash, Key>,
);

fn load(db: &str, region: &str) -> Result<Loaded, Box<dyn std::error::Error>> {
    let mut c = crate::db::connect(db)?;
    let h = |v: Vec<u8>| <Hash>::try_from(v).ok();
    let rows = c
        .query(
            "SELECT node_id, faction, age_s, built_s, ramp_s, wear, mult, points, ticket, motto, link \
             FROM game.keeps WHERE region = $1",
            &[&region],
        )?
        .into_iter()
        .filter_map(|r| {
            Some(Row {
                node: r.get(0),
                faction: Faction::parse(r.get(1)).unwrap_or(Faction::Fire),
                age: r.get(2),
                built: r.get(3),
                ramp: r.get(4),
                wear: r.get(5),
                mult: r.get(6),
                points: r.get(7),
                ticket: h(r.get(8))?,
                motto: r
                    .get::<_, Option<&str>>(9)
                    .and_then(elements_shared::mottos::parse)
                    .unwrap_or(0),
                link: r.get::<_, Option<Vec<u8>>>(10).and_then(h),
            })
        })
        .collect();
    let pot = c
        .query(
            "SELECT ticket, points, (week - DATE '1970-01-01')::int FROM game.keep_pot WHERE region = $1",
            &[&region],
        )?
        .into_iter()
        .filter_map(|r| {
            Some((
                h(r.get(0))?,
                (i64::from(r.get::<_, i32>(1)), i64::from(r.get::<_, i32>(2))),
            ))
        })
        .collect();
    let mut slots: HashMap<Hash, Vec<Hash>> = HashMap::new();
    for r in c.query(
        "SELECT id, slot FROM game.keep_slots WHERE region = $1",
        &[&region],
    )? {
        if let (Some(id), Some(s)) = (h(r.get(0)), h(r.get(1))) {
            slots.entry(id).or_default().push(s);
        }
    }
    let last = c
        .query_opt(
            "SELECT extract(epoch FROM at)::float8 FROM game.keep_resets WHERE region = $1",
            &[&region],
        )?
        .map_or(0.0, |r| r.get(0));
    let keys = c
        .query(
            "SELECT key, keep, origin, uses, extract(epoch FROM made)::float8, extract(epoch FROM expires)::float8 \
             FROM game.keep_keys WHERE region = $1",
            &[&region],
        )?
        .into_iter()
        .filter_map(|r| {
            Some((
                h(r.get(0))?,
                Key {
                    keep: h(r.get(1))?,
                    origin: origin(r.get(2)),
                    uses: r.get::<_, i32>(3).max(0) as u32,
                    made: r.get(4),
                    expires: r.get(5),
                },
            ))
        })
        .collect();
    Ok((rows, pot, slots, last, keys))
}

/// The latest campaign start or end that has passed (weather days don't count).
fn boundary(c: &mut postgres::Client, region: &str) -> Result<Option<f64>, postgres::Error> {
    Ok(c.query_one(
        "SELECT extract(epoch FROM max(b))::float8 FROM ( \
           SELECT min(starts) AS b FROM game.chapters WHERE region = $1 AND kind IS NULL AND status = 'published' GROUP BY campaign \
           UNION ALL \
           SELECT max(ends) FROM game.chapters WHERE region = $1 AND kind IS NULL AND status = 'published' GROUP BY campaign \
         ) x WHERE b <= now()",
        &[&region],
    )?
    .get(0))
}

fn write(c: &mut postgres::Client, region: &str, s: &Snapshot) -> Result<(), postgres::Error> {
    let mut t = c.transaction()?;
    t.execute("DELETE FROM game.keeps WHERE region = $1", &[&region])?;
    let k = &s.keeps;
    let nodes: Vec<i64> = k.iter().map(|r| r.node).collect();
    let factions: Vec<&str> = k.iter().map(|r| r.faction.as_str()).collect();
    let col = |f: fn(&Row) -> f32| k.iter().map(f).collect::<Vec<f32>>();
    let tickets: Vec<&[u8]> = k.iter().map(|r| &r.ticket[..]).collect();
    let mottos: Vec<Option<String>> = k
        .iter()
        .map(|r| elements_shared::mottos::key_of(r.motto).map(String::from))
        .collect();
    let links: Vec<Option<&[u8]>> = k.iter().map(|r| r.link.as_ref().map(|l| &l[..])).collect();
    t.execute(
        "INSERT INTO game.keeps (region, node_id, faction, age_s, built_s, ramp_s, wear, mult, points, ticket, motto, link) \
         SELECT $1, n, f, a, b, r, w, m, p, tk, mo, li FROM unnest($2::int8[], $3::text[], $4::real[], $5::real[], \
           $6::real[], $7::real[], $8::real[], $9::real[], $10::bytea[], $11::text[], $12::bytea[]) AS x(n, f, a, b, r, w, m, p, tk, mo, li)",
        &[
            &region,
            &nodes,
            &factions,
            &col(|r| r.age),
            &col(|r| r.built),
            &col(|r| r.ramp),
            &col(|r| r.wear),
            &col(|r| r.mult),
            &col(|r| r.points),
            &tickets,
            &mottos,
            &links,
        ],
    )?;
    t.execute("DELETE FROM game.keep_pot WHERE region = $1", &[&region])?;
    let pt: Vec<&[u8]> = s.pot.iter().map(|p| &p.0[..]).collect();
    let pp: Vec<i32> = s.pot.iter().map(|p| p.1 as i32).collect();
    let pw: Vec<i32> = s.pot.iter().map(|p| p.2 as i32).collect();
    t.execute(
        "INSERT INTO game.keep_pot (region, ticket, points, week) \
         SELECT $1, tk, p, DATE '1970-01-01' + w FROM unnest($2::bytea[], $3::int4[], $4::int4[]) AS x(tk, p, w)",
        &[&region, &pt, &pp, &pw],
    )?;
    t.execute("DELETE FROM game.keep_slots WHERE region = $1", &[&region])?;
    let si: Vec<&[u8]> = s.slots.iter().map(|p| &p.0[..]).collect();
    let ss: Vec<&[u8]> = s.slots.iter().map(|p| &p.1[..]).collect();
    // A new player's row may not be written yet: that slot follows next time.
    t.execute(
        "INSERT INTO game.keep_slots (region, id, slot) \
         SELECT $1, i, s FROM unnest($2::bytea[], $3::bytea[]) AS x(i, s) \
         WHERE EXISTS (SELECT 1 FROM game.players p WHERE p.id = x.i)",
        &[&region, &si, &ss],
    )?;
    t.execute("DELETE FROM game.keep_keys WHERE region = $1", &[&region])?;
    let kk: Vec<&[u8]> = s.keys.iter().map(|k| &k.0[..]).collect();
    let kp: Vec<&[u8]> = s.keys.iter().map(|k| &k.1.keep[..]).collect();
    let ko: Vec<&str> = s.keys.iter().map(|k| k.1.origin).collect();
    let ku: Vec<i32> = s.keys.iter().map(|k| k.1.uses as i32).collect();
    let km: Vec<f64> = s.keys.iter().map(|k| k.1.made).collect();
    let ke: Vec<f64> = s.keys.iter().map(|k| k.1.expires).collect();
    t.execute(
        "INSERT INTO game.keep_keys (key, region, keep, origin, uses, made, expires) \
         SELECT k, $1, p, o, u, to_timestamp(m), to_timestamp(e) \
         FROM unnest($2::bytea[], $3::bytea[], $4::text[], $5::int4[], $6::float8[], $7::float8[]) AS x(k, p, o, u, m, e)",
        &[&region, &kk, &kp, &ko, &ku, &km, &ke],
    )?;
    if let Some(at) = s.reset {
        t.execute(
            "INSERT INTO game.keep_resets (region, at) VALUES ($1, to_timestamp($2)) \
             ON CONFLICT (region) DO UPDATE SET at = EXCLUDED.at",
            &[&region, &at],
        )?;
    }
    t.commit()
}

/// The region's keep writer (a lane on the shared writer threads): saves the
/// newest snapshot, and checks the campaign boundary every `RESET_EVERY`.
struct Writer {
    db: String,
    region: String,
    rx: Receiver<Msg>,
    boundaries: Sender<f64>,
    check_at: Instant,
}

impl crate::writers::Lane for Writer {
    fn run(&mut self, pg: &mut crate::writers::Db) -> bool {
        let db = Some(self.db.as_str());
        if Instant::now() >= self.check_at {
            self.check_at = Instant::now() + RESET_EVERY;
            if let Some(c) = pg.get(db) {
                match boundary(c, &self.region) {
                    Ok(Some(b)) => {
                        if self.boundaries.send(b).is_err() {
                            return false;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => pg.failed(db, "keeps: campaign boundary", &e),
                }
            }
        }
        // Only the newest snapshot matters; barriers after it is written.
        let mut save: Option<Snapshot> = None;
        let mut acks = Vec::new();
        let open = loop {
            match self.rx.try_recv() {
                Ok(Msg::Save(n)) => {
                    let reset = save.as_ref().and_then(|s| s.reset).or(n.reset);
                    save = Some(Snapshot { reset, ..n });
                }
                Ok(Msg::Barrier(a)) => acks.push(a),
                Err(crossbeam_channel::TryRecvError::Empty) => break true,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break false,
            }
        };
        if let Some(s) = save {
            if let Some(c) = pg.get(db) {
                if let Err(e) = write(c, &self.region, &s) {
                    pg.failed(db, "keeps write", &e);
                }
            }
        }
        for a in acks {
            let _ = a.send(());
        }
        open
    }

    fn due(&self) -> Option<Instant> {
        Some(self.check_at)
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::sim::expire_sources;
    use crate::testkit::*;
    use elements_shared::rules::Rules;

    #[test]
    fn gift_keys_count_down_and_expire() {
        let mut k = Keeps::new(None, "test".into(), true);
        let (_, kh, _) = new_ticket();
        let (_, src, _) = new_ticket();
        let now = 1_000_000.0;
        k.add_key(
            kh,
            Key {
                keep: src,
                origin: "gift",
                uses: 2,
                made: now,
                expires: now + 100.0,
            },
        );
        assert!(k.made_since(&src, "gift", now - 1.0));
        assert!(!k.made_since(&src, "bottle", now - 1.0));
        assert_eq!(k.key(&kh, now).map(|x| x.keep), Some(src));
        k.use_key(&kh);
        assert!(k.key(&kh, now).is_some());
        k.use_key(&kh);
        assert!(k.key(&kh, now).is_none(), "used up");
        k.use_key(&kh);
        assert_eq!(k.keys[&kh].uses, 0);
        k.keys.get_mut(&kh).unwrap().uses = 1;
        assert!(k.key(&kh, now + 101.0).is_none(), "expired");
    }

    fn rules() -> Rules {
        let mut r = Rules::default();
        r.keep.enabled = true;
        r.keep.expire_s = 10_000.0;
        r
    }

    /// A line of 3 nodes, natural fire at 1 and water at 3, with a water
    /// permanent source at node 2 as `restore` puts it.
    fn with_keep(r: &Rules) -> (World, Entity) {
        let (n, e) = line(3, 100.0, &[(1, "fire_source"), (3, "water_source")]);
        let mut w = world(r.clone(), n, e);
        let mut k = Keeps::new(None, "test".into(), true);
        k.pending = Some(vec![row(2, 0.0)]);
        w.insert_resource(k);
        w.insert_resource(crate::Stop(Default::default()));
        w.run_system_once(restore).unwrap();
        let e = ent(&w, 2);
        (w, e)
    }

    fn row(node: i64, points: f32) -> Row {
        Row {
            node,
            faction: Faction::Water,
            age: 0.0,
            built: 0.0,
            ramp: 0.0,
            wear: 0.0,
            mult: 1.0,
            points,
            ticket: [node as u8; 32],
            motto: 0,
            link: None,
        }
    }

    fn strength(w: &World, e: Entity) -> Option<f32> {
        w.get::<ElementSource>(e).map(|s| s.strength)
    }

    #[test]
    fn keep_fades_to_its_floor_and_wears() {
        let r = rules(); // 0.8 -> 0.2 over 3600 s
        let (mut w, e) = with_keep(&r);
        assert_eq!(strength(&w, e), Some(0.8));
        let s = *w.get::<ElementSource>(e).unwrap();
        assert_eq!((s.kind, s.faction), (SourceKind::Keep, Faction::Water));
        let id = w.register_system(expire_sources);
        let near = |a: Option<f32>, b: f32| (a.unwrap() - b).abs() < 1e-3;
        advance(&mut w, 1800.0);
        w.run_system(id).unwrap();
        assert!(near(strength(&w, e), 0.5));
        for _ in 0..3 {
            advance(&mut w, 1800.0);
            w.run_system(id).unwrap();
            assert!(near(strength(&w, e), 0.2));
        }
        w.get_mut::<KeepInfo>(e).unwrap().wear = 0.15;
        advance(&mut w, 1.0);
        w.run_system(id).unwrap();
        assert!(near(strength(&w, e), 0.05));
        // worn below the floor: 0, but still standing
        w.get_mut::<KeepInfo>(e).unwrap().wear = 0.5;
        advance(&mut w, 1.0);
        w.run_system(id).unwrap();
        assert_eq!(strength(&w, e), Some(0.0));
        // a top-up restores it
        w.get_mut::<KeepInfo>(e).unwrap().wear = 0.0;
        w.get_mut::<ElementSource>(e).unwrap().top_up(1.0);
        advance(&mut w, 1.0);
        w.run_system(id).unwrap();
        assert!(near(strength(&w, e), 0.8));
    }

    #[test]
    fn keep_expires_without_a_top_up_and_pays_into_the_pot() {
        let r = rules();
        let (mut w, e) = with_keep(&r);
        let exp = w.register_system(expire_sources);
        let kt = w.register_system(tick);
        w.run_system(kt).unwrap();
        // points while standing: 10 per hour, a minute's worth per round
        w.resource_mut::<Keeps>().points_at = 1.0;
        w.run_system(kt).unwrap();
        let p = w.get::<KeepInfo>(e).unwrap().points;
        assert!((p - 10.0 / 60.0).abs() < 1e-5);
        w.get_mut::<KeepInfo>(e).unwrap().points = 25.0;
        advance(&mut w, 9_999.0);
        w.run_system(exp).unwrap();
        assert!(strength(&w, e).is_some());
        advance(&mut w, 1.0);
        w.run_system(exp).unwrap();
        assert!(w.get::<ElementSource>(e).is_none());
        w.run_system(kt).unwrap();
        assert!(w.get::<KeepInfo>(e).is_none());
        let mut k = w.resource_mut::<Keeps>();
        assert_eq!(k.pot_take(&[2; 32]), Some(20));
        assert_eq!(k.pot_take(&[2; 32]), None);
    }

    #[test]
    fn taken_over_keep_goes_to_the_pot() {
        let r = rules();
        let (mut w, e) = with_keep(&r);
        let claim = ElementSource::claim(
            &r,
            Faction::Fire,
            SourceKind::Claim,
            100.0,
            None,
            1.0,
            false,
            0.0,
        );
        w.get_mut::<KeepInfo>(e).unwrap().points = 14.0;
        w.entity_mut(e).insert(claim);
        w.run_system_once(tick).unwrap();
        assert!(w.get::<KeepInfo>(e).is_none());
        assert_eq!(w.get::<ElementSource>(e).unwrap().faction, Faction::Fire);
        assert_eq!(w.resource_mut::<Keeps>().pot_take(&[2; 32]), Some(10));
    }

    #[test]
    fn restore_on_a_lost_node_pays_into_the_pot() {
        let r = rules();
        let (mut w, _) = with_keep(&r);
        let mut worn = row(2, 0.0);
        worn.wear = 0.3;
        worn.age = 3600.0;
        w.resource_mut::<Keeps>().pending = Some(vec![worn, row(77, 33.0)]);
        w.run_system_once(restore).unwrap();
        // faded and worn: 0.2 - 0.3 -> 0
        assert_eq!(strength(&w, ent(&w, 2)), Some(0.0));
        assert_eq!(w.get::<KeepInfo>(ent(&w, 2)).unwrap().wear, 0.3);
        assert_eq!(w.resource_mut::<Keeps>().pot_take(&[77; 32]), Some(30));
    }

    #[test]
    fn tickets_hash_apart_and_pay_in_steps() {
        let (t, a, b) = new_ticket();
        assert_eq!(hashes(&t), Some((a, b)));
        assert_ne!(a, b);
        assert!(hashes("xyz").is_none());
        assert_eq!(payable(39.9), 30);
        assert_eq!(payable(-1.0), 0);
    }

    #[test]
    fn release_once_a_day() {
        let mut k = Keeps::new(None, "test".into(), true);
        let p = [1u8; 32];
        k.add_slot(p, [2u8; 32]);
        k.add_slot(p, [3u8; 32]);
        // The app still has the ticket of slot 3: only slot 2 goes.
        assert!(k.release(p, 10, &[[3u8; 32]]));
        assert_eq!(k.slots(&p), &[[3u8; 32]]);
        assert!(!k.release(p, 10, &[]));
        assert!(k.release(p, 11, &[]));
        assert!(k.slots(&p).is_empty());
    }
}

//! Bus between the cells of a shard: a deletion, a new
//! player id or "write your players now" must reach the regions of the other
//! cells too, or their next flush writes an old player back. Rows in
//! `ops.cell_events`, polled twice a second; only while the shard has
//! other cells (a single server only checks `ops.cells` every 5 s). Also the
//! region leases: a region runs in one cell at a time, also while it moves.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use tracing::{info, warn};

use crate::gateway::FromWeb;
use crate::regions::Host;

const POLL: Duration = Duration::from_millis(500);
/// Regions loaded but nobody connected: poll this seldom (scale to zero).
const IDLE_POLL: Duration = Duration::from_secs(600);
/// How long a flush waits for the other cells.
const FLUSH_WAIT: Duration = Duration::from_secs(3);
/// A lease lasts this long; the holder renews it every [`RENEW`] while
/// players are connected, else at each idle poll (well within the lease).
const LEASE_S: f64 = 1800.0;
const RENEW: Duration = Duration::from_secs(15);

enum Out {
    Evict([u8; 32]),
    Rekey([u8; 32], [u8; 32]),
    Release(String),
}

struct Bus {
    db: String,
    cell: String,
    tx: Sender<Out>,
    /// The shard has other cells (set by the bus thread).
    active: AtomicBool,
    /// Their names: in the region list or registered in `ops.cells`.
    others: std::sync::Mutex<Vec<String>>,
}

static BUS: OnceLock<Bus> = OnceLock::new();

/// Start the bus thread (once).
pub fn spawn(host: Arc<Host>, db: String) {
    let (tx, rx) = crossbeam_channel::unbounded();
    if BUS
        .set(Bus {
            db: db.clone(),
            cell: host.cell().to_string(),
            tx,
            active: AtomicBool::new(false),
            others: Default::default(),
        })
        .is_err()
    {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("cells".into())
        .spawn(move || run(&host, &db, &rx));
}

fn active() -> Option<&'static Bus> {
    BUS.get().filter(|b| b.active.load(Ordering::Relaxed))
}

/// Tell the other cells: this player was deleted.
pub fn evict(id: [u8; 32]) {
    if let Some(b) = active() {
        let _ = b.tx.send(Out::Evict(id));
    }
}

/// Tell the other cells: this player's id changed.
pub fn rekey(old: [u8; 32], new: [u8; 32]) {
    if let Some(b) = active() {
        let _ = b.tx.send(Out::Rekey(old, new));
    }
}

/// Lease `slug` for this cell before loading it: false while another cell
/// holds it (moving: it still writes). Without other cells: true.
pub fn claim(slug: &str) -> bool {
    let Some(b) = active() else { return true };
    let run = || -> Result<bool, Box<dyn std::error::Error>> {
        let mut c = crate::db::connect(&b.db)?;
        Ok(lease(&mut c, slug, &b.cell)?)
    };
    let res = if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(run)
    } else {
        run()
    };
    // Database down: not loading anyway.
    res.unwrap_or_else(|e| {
        warn!(slug, error = %e, "cells: lease failed");
        false
    })
}

/// Let go of `slug` (unloaded, players written).
pub fn release(slug: &str) {
    if let Some(b) = active() {
        let _ = b.tx.send(Out::Release(slug.to_string()));
    }
}

fn lease(c: &mut postgres::Client, slug: &str, cell: &str) -> Result<bool, postgres::Error> {
    let n = c.execute(
        "INSERT INTO ops.region_lease (slug, cell, until) VALUES ($1, $2, now() + make_interval(secs => $3)) \
         ON CONFLICT (slug) DO UPDATE SET cell = EXCLUDED.cell, until = EXCLUDED.until \
         WHERE ops.region_lease.cell = EXCLUDED.cell OR ops.region_lease.until < now()",
        &[&slug, &cell, &LEASE_S],
    )?;
    Ok(n == 1)
}

/// Ask the other cells to write their players now; wait for them (at most
/// [`FLUSH_WAIT`]). Blocking: call from a blocking context.
pub fn flush_others(host: &Host) {
    let Some(b) = active() else { return };
    let want = b.others.lock().unwrap().len() as i64;
    let res = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut c = crate::db::connect(&b.db)?;
        let req: i64 = c
            .query_one(
                "INSERT INTO ops.cell_events (shard, from_cell, kind) VALUES ($1, $2, 'flush') RETURNING id",
                &[&host.shard(), &host.cell()],
            )?
            .get(0);
        let until = Instant::now() + FLUSH_WAIT;
        while Instant::now() < until {
            let n: i64 = c
                .query_one(
                    "SELECT count(DISTINCT from_cell) FROM ops.cell_events WHERE shard = $1 AND kind = 'ack' AND req = $2",
                    &[&host.shard(), &req],
                )?
                .get(0);
            if n >= want {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        warn!(req, want, "cells: flush not acknowledged by every cell");
        Ok(())
    })();
    if let Err(e) = res {
        warn!(error = %e, "cells: flush failed");
    }
}

/// First use of `k` (a passport, a signed call) in any cell of the shard,
/// remembered until `exp` (unix s). Without other cells: true (each check
/// keeps its own memory too). Blocks briefly on the database.
pub fn first_use(k: &str, exp: u64) -> bool {
    let Some(b) = active() else { return true };
    let run = || -> Result<bool, Box<dyn std::error::Error>> {
        let mut c = crate::db::connect(&b.db)?;
        let n = c.execute(
            "INSERT INTO ops.used_once (k, exp) VALUES ($1, to_timestamp($2)) ON CONFLICT DO NOTHING",
            &[&k, &(exp as f64)],
        )?;
        Ok(n == 1)
    };
    let res = if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(run)
    } else {
        run()
    };
    // Database down: allow (the cell's own memory still refuses repeats).
    res.unwrap_or_else(|e| {
        warn!(error = %e, "cells: used_once failed");
        true
    })
}

/// Write this cell's players now (every loaded region), then return.
fn flush_here(host: &Host) {
    let (ack, done) = crossbeam_channel::unbounded();
    let n = host.broadcast(None, || FromWeb::Flush { ack: ack.clone() });
    let until = Instant::now() + FLUSH_WAIT;
    for _ in 0..n {
        if done.recv_deadline(until).is_err() {
            break;
        }
    }
}

fn run(host: &Arc<Host>, db: &str, rx: &Receiver<Out>) {
    let bus = BUS.get().expect("bus");
    let mut conn = crate::db::Reconnecting::new("cells");
    let mut last: Option<i64> = None;
    let mut cleaned = Instant::now();
    let mut renewed: Option<Instant> = None;
    let mut polled: Option<Instant> = None;
    let mut outgoing: Vec<Out> = Vec::new();
    loop {
        // Other cells: in the region list or registered (`ops.cells`), both
        // read with the list (no query of its own: an idle database sleeps).
        let others = host.other_cells();
        let on = !others.is_empty();
        *bus.others.lock().unwrap() = others.clone();
        if bus.active.swap(on, Ordering::Relaxed) != on {
            info!(?others, "cells: bus {}", if on { "on" } else { "off" });
        }
        if !on {
            last = None;
            renewed = None;
            // Nothing to tell anyone: drop what came while switching off.
            while rx.try_recv().is_ok() {}
            outgoing.clear();
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        // Players here: twice a second. Regions loaded, nobody connected:
        // every 10 minutes (leases last 30). Nothing loaded: only to send.
        let every = if host.connected() {
            Some(POLL)
        } else if !host.loaded_slugs().is_empty() {
            Some(IDLE_POLL)
        } else {
            None
        };
        let due = every.is_some_and(|e| polled.is_none_or(|t| t.elapsed() >= e));
        if due || !outgoing.is_empty() {
            outgoing.extend(rx.try_iter());
            let res = step(
                host,
                db,
                &mut outgoing,
                &mut conn,
                &mut last,
                &mut cleaned,
                &mut renewed,
            );
            match res {
                Ok(()) => polled = Some(Instant::now()),
                Err(e) => conn.failed("poll", &e),
            }
        }
        // Wait for the next poll or something to send (local, no query).
        let wait = if every == Some(POLL) {
            POLL
        } else {
            Duration::from_secs(5)
        };
        if let Ok(o) = rx.recv_timeout(wait) {
            outgoing.push(o);
        }
    }
}

fn step(
    host: &Arc<Host>,
    db: &str,
    outgoing: &mut Vec<Out>,
    conn: &mut crate::db::Reconnecting,
    last: &mut Option<i64>,
    cleaned: &mut Instant,
    renewed: &mut Option<Instant>,
) -> Result<(), postgres::Error> {
    let Some(c) = conn.get(db) else { return Ok(()) };
    let (shard, me) = (host.shard(), host.cell());
    // Start after what is there: older events were for the regions of then.
    let from = match *last {
        Some(id) => id,
        None => c
            .query_one(
                "SELECT coalesce(max(id), 0) FROM ops.cell_events WHERE shard = $1",
                &[&shard],
            )?
            .get(0),
    };
    *last = Some(from);
    for out in outgoing.drain(..) {
        let (kind, a, b): (&str, &[u8], Option<&[u8]>) = match &out {
            Out::Evict(id) => ("evict", id, None),
            Out::Rekey(old, new) => ("rekey", old, Some(new)),
            Out::Release(slug) => {
                c.execute(
                    "DELETE FROM ops.region_lease WHERE slug = $1 AND cell = $2",
                    &[slug, &me],
                )?;
                continue;
            }
        };
        c.execute(
            "INSERT INTO ops.cell_events (shard, from_cell, kind, a, b) VALUES ($1, $2, $3, $4, $5)",
            &[&shard, &me, &kind, &a, &b],
        )?;
    }
    let rows = c.query(
        "SELECT id, kind, a, b FROM ops.cell_events \
         WHERE shard = $1 AND id > $2 AND from_cell <> $3 AND kind <> 'ack' ORDER BY id",
        &[&shard, &from, &me],
    )?;
    for r in rows {
        let id: i64 = r.get(0);
        let kind: &str = r.get(1);
        let a: Option<Vec<u8>> = r.get(2);
        let b: Option<Vec<u8>> = r.get(3);
        let key = |v: Option<Vec<u8>>| v.and_then(|v| <[u8; 32]>::try_from(v).ok());
        match (kind, key(a), key(b)) {
            ("evict", Some(id), _) => {
                host.broadcast(None, || FromWeb::Evict { id });
            }
            ("rekey", Some(old), Some(new)) => {
                host.broadcast(None, || FromWeb::Rekey { old, new });
            }
            // A region moved (docs/deploy.md, Cells): read the list now;
            // the old cell lets go at its next sweep.
            ("refresh", _, _) => host.refresh_list(),
            ("flush", _, _) => {
                flush_here(host);
                c.execute(
                    "INSERT INTO ops.cell_events (shard, from_cell, kind, req) VALUES ($1, $2, 'ack', $3)",
                    &[&shard, &me, &id],
                )?;
            }
            _ => warn!(id, kind, "cells: unknown event"),
        }
        *last = Some(id);
    }
    // Leases of the loaded regions (also the ones loaded before the shard
    // had other cells). Lost to another cell: stop here.
    if renewed.is_none_or(|t| t.elapsed() >= RENEW) {
        for slug in host.loaded_slugs() {
            if !lease(c, &slug, me)? {
                warn!(slug, "cells: lease held by another cell; unloading here");
                host.unload(&slug, "lease lost");
            }
        }
        *renewed = Some(Instant::now());
    }
    if host.is_main() && cleaned.elapsed() > Duration::from_secs(600) {
        c.execute(
            "DELETE FROM ops.cell_events WHERE shard = $1 AND at < now() - interval '1 hour'",
            &[&shard],
        )?;
        c.execute("DELETE FROM ops.used_once WHERE exp < now()", &[])?;
        c.execute(
            "DELETE FROM ops.region_lease WHERE until < now() - interval '1 hour'",
            &[],
        )?;
        *cleaned = Instant::now();
    }
    Ok(())
}

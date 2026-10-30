//! Region apps on a few worker threads instead of a thread each.
//! Loader threads read a region
//! (`crate::prepare`: database, files, tiles) and hand it to the worker with
//! the fewest junctions; the worker builds the app and owns it until it
//! stops. A worker updates an app when a message for it arrives (at most
//! `MIN_GAP` apart, like the old 64 Hz loop), otherwise every `ACTIVE` while
//! someone is connected and every `IDLE` when not. An app stays on its
//! worker: Bevy's `App` isn't `Send`.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bevy::prelude::App;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use tracing::{error, info};

use crate::gateway::FromWeb;
use crate::regions::{Region, RegionStart};

/// Fastest update after a message (the old fixed loop ran at 64 Hz).
const MIN_GAP: Duration = Duration::from_micros(15_625);
/// With someone connected: pushes (spread, overview, scores) go out this often.
const ACTIVE: Duration = Duration::from_micros(62_500);
/// Nobody connected: claims age, chapters and points tick on.
const IDLE: Duration = Duration::from_millis(500);

/// To a region's app: queues the message and wakes its worker.
#[derive(Clone)]
pub struct RegionTx {
    tx: Sender<FromWeb>,
    /// The worker and slot, once the app runs (messages wait until then).
    at: Arc<OnceLock<(Sender<Cmd>, u64)>>,
}

impl RegionTx {
    pub fn new(tx: Sender<FromWeb>) -> Self {
        Self {
            tx,
            at: Default::default(),
        }
    }

    /// The app runs (messages are handled now, not after loading).
    pub fn running(&self) -> bool {
        self.at.get().is_some()
    }

    pub fn send(&self, msg: FromWeb) -> Result<(), Closed> {
        self.tx.send(msg).map_err(|_| Closed)?;
        if let Some((w, id)) = self.at.get() {
            let _ = w.send(Cmd::Wake(*id));
        }
        Ok(())
    }
}

/// The region's app is gone.
#[derive(Debug)]
pub struct Closed;

impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("region stopped")
    }
}

impl std::error::Error for Closed {}

/// Set once a region's app has stopped (written what it held).
#[derive(Default)]
pub struct Done(Mutex<bool>, Condvar);

impl Done {
    pub fn set(&self) {
        *self.0.lock().unwrap() = true;
        self.1.notify_all();
    }

    pub fn wait(&self) {
        let mut d = self.0.lock().unwrap();
        while !*d {
            d = self.1.wait(d).unwrap();
        }
    }
}

pub enum Cmd {
    Add(Box<crate::Prepared>, Arc<Region>, usize),
    Wake(u64),
}

struct Worker {
    tx: Sender<Cmd>,
    /// Junctions of the regions it runs.
    load: Arc<AtomicUsize>,
}

struct Pool {
    loader: Sender<(RegionStart, Arc<Region>)>,
    workers: Vec<Worker>,
}

fn env_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
        .max(1)
}

/// `ELEMENTS_WORKERS` (default: cores − 1) and `ELEMENTS_LOADERS` (default
/// 4) threads, started on first use.
fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        let workers: Vec<Worker> = (0..env_count("ELEMENTS_WORKERS", cores.saturating_sub(1)))
            .map(|i| {
                let (tx, rx) = crossbeam_channel::unbounded();
                let load = Arc::new(AtomicUsize::new(0));
                let (me, l) = (tx.clone(), load.clone());
                std::thread::Builder::new()
                    .name(format!("worker-{i}"))
                    .spawn(move || work(rx, me, l))
                    .expect("worker thread");
                Worker { tx, load }
            })
            .collect();
        let (loader, jobs) = crossbeam_channel::unbounded::<(RegionStart, Arc<Region>)>();
        for i in 0..env_count("ELEMENTS_LOADERS", 4) {
            let jobs = jobs.clone();
            std::thread::Builder::new()
                .name(format!("loader-{i}"))
                .spawn(move || {
                    for (start, region) in jobs {
                        let p = crate::prepare(start);
                        let weight = p.junctions() + 1000;
                        let w = pool()
                            .workers
                            .iter()
                            .min_by_key(|w| w.load.load(Ordering::Relaxed))
                            .expect("a worker");
                        w.load.fetch_add(weight, Ordering::Relaxed);
                        let _ = w.tx.send(Cmd::Add(Box::new(p), region, weight));
                    }
                })
                .expect("loader thread");
        }
        info!(
            workers = workers.len(),
            loaders = env_count("ELEMENTS_LOADERS", 4),
            "region pool"
        );
        Pool { loader, workers }
    })
}

/// Load the region and run it on a worker; `region.done` is set once it stops.
pub fn start(start: RegionStart, region: Arc<Region>) {
    let _ = pool().loader.send((start, region));
}

struct Slot {
    app: App,
    region: Arc<Region>,
    span: tracing::Span,
    next: Instant,
    last: Instant,
    weight: usize,
}

fn work(rx: Receiver<Cmd>, me: Sender<Cmd>, load: Arc<AtomicUsize>) {
    let mut slots: HashMap<u64, Slot> = HashMap::new();
    let mut next_id = 0u64;
    loop {
        let first = match slots.values().map(|s| s.next).min() {
            Some(t) => rx.recv_timeout(t.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let cmds = match first {
            Ok(c) => std::iter::once(c).chain(rx.try_iter()).collect(),
            Err(RecvTimeoutError::Timeout) => Vec::new(),
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let now = Instant::now();
        for c in cmds {
            match c {
                Cmd::Add(p, region, weight) => {
                    let span = crate::region_span(p.slug());
                    let built = catch_unwind(AssertUnwindSafe(|| {
                        let mut app = crate::build(*p);
                        app.finish();
                        app.cleanup();
                        app
                    }));
                    let Ok(app) = built else {
                        error!(slug = %region.slug, "region app failed to build");
                        load.fetch_sub(weight, Ordering::Relaxed);
                        region.done.set();
                        continue;
                    };
                    next_id += 1;
                    let _ = region.tx.at.set((me.clone(), next_id));
                    slots.insert(
                        next_id,
                        Slot {
                            app,
                            region,
                            span,
                            next: now,
                            last: now,
                            weight,
                        },
                    );
                }
                Cmd::Wake(id) => {
                    if let Some(s) = slots.get_mut(&id) {
                        s.next = s.next.min((s.last + MIN_GAP).max(now));
                    }
                }
            }
        }
        let due: Vec<u64> = slots
            .iter()
            .filter(|(_, s)| s.next <= now)
            .map(|(&id, _)| id)
            .collect();
        for id in due {
            let s = slots.get_mut(&id).expect("due slot");
            let t = Instant::now();
            let ran = {
                let _g = s.span.enter();
                catch_unwind(AssertUnwindSafe(|| s.app.update()))
            };
            s.last = t;
            s.next = t + if s.region.conns() > 0 { ACTIVE } else { IDLE };
            let stop = match ran {
                Ok(()) => s.app.should_exit().is_some(),
                Err(_) => {
                    error!(slug = %s.region.slug, "region app panicked; unloading it");
                    true
                }
            };
            if stop {
                let s = slots.remove(&id).expect("slot");
                load.fetch_sub(s.weight, Ordering::Relaxed);
                let _g = s.span.clone().entered();
                let region = s.region.clone();
                let app = s.app;
                // A world left by a panic may panic again on drop.
                let _ = catch_unwind(AssertUnwindSafe(move || drop(app)));
                info!("region stopped");
                region.done.set();
            }
        }
    }
}

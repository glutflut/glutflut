//! Database writers shared by all regions.
//! A few threads with one connection each instead of four threads and four
//! connections per loaded region. Each region's writer of one kind is a
//! [`Lane`]: pinned to one thread (so its writes keep their order), run when
//! something was sent to it or its timer is due, dropped once its region is
//! gone (channel closed).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crossbeam_channel::{Receiver, RecvTimeoutError, SendError, Sender};

use crate::db::Reconnecting;

/// One region's writer of one kind.
pub trait Lane: Send {
    /// Handle what is queued (`try_recv` until empty); `false` once the
    /// sending side is gone.
    fn run(&mut self, db: &mut Db) -> bool;
    /// Run again at this time even without news (periodic loads).
    fn due(&self) -> Option<Instant> {
        None
    }
}

/// The connections of one writer thread, by database URL (one in practice).
pub struct Db(HashMap<String, Reconnecting>);

impl Db {
    /// The open connection to `url`; `None` without a database or if
    /// connecting fails.
    pub fn get(&mut self, url: Option<&str>) -> Option<&mut postgres::Client> {
        let url = url?;
        self.0
            .entry(url.to_string())
            .or_insert_with(|| Reconnecting::new("writer"))
            .get(url)
    }

    /// After a failed query: log it, reconnect on the next `get`.
    pub fn failed(&mut self, url: Option<&str>, doing: &str, e: &dyn std::fmt::Display) {
        if let Some(c) = url.and_then(|u| self.0.get_mut(u)) {
            c.failed(doing, e);
        }
    }
}

enum Cmd {
    Add(u64, Box<dyn Lane>),
    Wake(u64),
}

struct Pool {
    threads: Vec<Sender<Cmd>>,
    next_id: std::sync::atomic::AtomicU64,
}

/// `ELEMENTS_DB_THREADS` writer threads (default 4), started on first use.
fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::env::var("ELEMENTS_DB_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4usize)
            .max(1);
        let threads = (0..n)
            .map(|i| {
                let (tx, rx) = crossbeam_channel::unbounded();
                std::thread::Builder::new()
                    .name(format!("db-writer-{i}"))
                    .spawn(move || work(rx))
                    .expect("db writer thread");
                tx
            })
            .collect();
        Pool {
            threads,
            next_id: Default::default(),
        }
    })
}

fn work(rx: Receiver<Cmd>) {
    let mut lanes: HashMap<u64, Box<dyn Lane>> = HashMap::new();
    let mut db = Db(HashMap::new());
    loop {
        let next = lanes.values().filter_map(|l| l.due()).min();
        let cmd = match next {
            Some(t) => rx.recv_timeout(t.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match cmd {
            Ok(Cmd::Add(id, lane)) => {
                lanes.insert(id, lane);
                run(&mut lanes, id, &mut db);
            }
            Ok(Cmd::Wake(id)) => run(&mut lanes, id, &mut db),
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                let due: Vec<u64> = lanes
                    .iter()
                    .filter(|(_, l)| l.due().is_some_and(|t| t <= now))
                    .map(|(&id, _)| id)
                    .collect();
                for id in due {
                    run(&mut lanes, id, &mut db);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn run(lanes: &mut HashMap<u64, Box<dyn Lane>>, id: u64, db: &mut Db) {
    if let Some(l) = lanes.get_mut(&id) {
        if !l.run(db) {
            lanes.remove(&id);
        }
    }
}

/// Start a lane on the thread for `key` (region and kind): a sender for it
/// that wakes it, and the receiving end goes into `make`.
pub fn lane<T: Send + 'static, L: Lane + 'static>(
    key: &str,
    make: impl FnOnce(Receiver<T>) -> L,
) -> LaneTx<T> {
    let p = pool();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    let thread = p.threads[h.finish() as usize % p.threads.len()].clone();
    let id = p.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = crossbeam_channel::unbounded();
    let _ = thread.send(Cmd::Add(id, Box::new(make(rx))));
    LaneTx {
        tx: Some(tx),
        thread,
        id,
        senders: Arc::new(AtomicUsize::new(1)),
    }
}

/// Sends to a lane and wakes its thread.
pub struct LaneTx<T> {
    /// `None` only while dropping.
    tx: Option<Sender<T>>,
    thread: Sender<Cmd>,
    id: u64,
    /// Clones alive, to wake the lane once the last one goes.
    senders: Arc<AtomicUsize>,
}

impl<T> Clone for LaneTx<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            thread: self.thread.clone(),
            id: self.id,
            senders: {
                self.senders.fetch_add(1, Ordering::Relaxed);
                self.senders.clone()
            },
        }
    }
}

impl<T> LaneTx<T> {
    pub fn send(&self, msg: T) -> Result<(), SendError<T>> {
        let Some(tx) = &self.tx else {
            return Err(SendError(msg));
        };
        tx.send(msg)?;
        let _ = self.thread.send(Cmd::Wake(self.id));
        Ok(())
    }
}

impl<T> Drop for LaneTx<T> {
    fn drop(&mut self) {
        // The last sender gone: wake the lane once more so it sees the
        // closed channel and is dropped.
        drop(self.tx.take());
        let last = self.senders.fetch_sub(1, Ordering::AcqRel) == 1;
        if last {
            let _ = self.thread.send(Cmd::Wake(self.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Collect(Receiver<u32>, Arc<Mutex<Vec<u32>>>);

    impl Lane for Collect {
        fn run(&mut self, _: &mut Db) -> bool {
            loop {
                match self.0.try_recv() {
                    Ok(v) => self.1.lock().unwrap().push(v),
                    Err(crossbeam_channel::TryRecvError::Empty) => return true,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => return false,
                }
            }
        }
    }

    /// Order kept per lane, and a dropped sender ends the lane.
    #[test]
    fn lanes_keep_order() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let tx = lane("test-order", move |rx| Collect(rx, g));
        for v in 0..1000 {
            tx.send(v).unwrap();
        }
        let t0 = Instant::now();
        while got.lock().unwrap().len() < 1000 && t0.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(*got.lock().unwrap(), (0..1000).collect::<Vec<_>>());
        // Dropped: the lane sees the closed channel and goes (its Arc too).
        drop(tx);
        let t0 = Instant::now();
        while Arc::strong_count(&got) > 1 && t0.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(Arc::strong_count(&got), 1);
    }
}

//! Limits against floods and scripts: token buckets per client address, in
//! memory only (never stored or logged; forgotten once full again). Behind
//! Caddy the address comes from `X-Forwarded-For`; IPv6 counts per /64, as a
//! phone gets a whole prefix (open sockets also per /48). IPv4 is often shared (carrier NAT, a market
//! full of phones on one cell), so the bursts are generous. Calls from this
//! machine without a proxy (local tests, bots) and `ELEMENTS_LIMITS=0` are
//! not limited.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use axum::http::HeaderMap;

/// One client: an IPv4 address or an IPv6 /64.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Client(u128);

/// The client that is never limited.
const LOCAL: Client = Client(u128::MAX);

impl Client {
    /// One bucket for everyone (a global limit).
    pub const ANY: Client = Client(u128::MAX - 1);
}

static OFF: LazyLock<bool> =
    LazyLock::new(|| std::env::var("ELEMENTS_LIMITS").is_ok_and(|v| v == "0"));

impl Client {
    pub fn of(peer: SocketAddr, headers: &HeaderMap) -> Self {
        let ip = peer.ip();
        // Only a local proxy (Caddy) may name the client; Caddy replaces
        // what the client sent, so its entry is the last one.
        let ip = if ip.is_loopback() {
            let fwd = headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit(',').next())
                .and_then(|v| v.trim().parse::<IpAddr>().ok());
            match fwd {
                Some(ip) => ip,
                None => return LOCAL,
            }
        } else {
            ip
        };
        Self(match ip {
            IpAddr::V4(v) => u32::from(v) as u128,
            IpAddr::V6(v) => match v.to_ipv4_mapped() {
                Some(v) => u32::from(v) as u128,
                None => u128::from(v) & !((1u128 << 64) - 1),
            },
        })
    }

    /// The IPv6 /48 around a /64 (a site or a hoster's customer, who can
    /// spread over thousands of /64s); `None` for IPv4.
    fn site(self) -> Option<u128> {
        (self.0 > u32::MAX as u128 && self.0 < Client::ANY.0).then_some(self.0 >> 80)
    }

    /// Exempt from all limits.
    pub fn free(self) -> bool {
        self == LOCAL || *OFF
    }
}

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// `burst` at once, then `per_s` per second.
pub struct Limiter {
    per_s: f64,
    burst: f64,
    buckets: Mutex<(BTreeMap<Client, Bucket>, Option<Instant>)>,
}

impl Limiter {
    pub const fn new(per_s: f64, burst: f64) -> Self {
        Self {
            per_s,
            burst,
            buckets: Mutex::new((BTreeMap::new(), None)),
        }
    }

    /// No token left for `c` (without taking one).
    pub fn empty(&self, c: Client) -> bool {
        if c.free() {
            return false;
        }
        let g = self.buckets.lock().unwrap();
        g.0.get(&c)
            .is_some_and(|b| b.tokens + b.at.elapsed().as_secs_f64() * self.per_s < 1.0)
    }

    pub fn allow(&self, c: Client) -> bool {
        if c.free() {
            return true;
        }
        let now = Instant::now();
        let mut g = self.buckets.lock().unwrap();
        let (map, swept) = &mut *g;
        // Drop the full ones now and then.
        if swept.is_none_or(|t| now.duration_since(t).as_secs() >= 60) {
            *swept = Some(now);
            let (per_s, burst) = (self.per_s, self.burst);
            map.retain(|_, b| b.tokens + now.duration_since(b.at).as_secs_f64() * per_s < burst);
        }
        let b = map.entry(c).or_insert(Bucket {
            tokens: self.burst,
            at: now,
        });
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * self.per_s).min(self.burst);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Any HTTP request (tiles come in bursts of a hundred).
pub static HTTP: Limiter = Limiter::new(30.0, 600.0);
/// New sockets (phones reconnect often).
pub static SOCKETS: Limiter = Limiter::new(2.0, 60.0);
/// New players (unknown tokens): 120 an hour after a burst of 30.
pub static NEW_PLAYERS: Limiter = Limiter::new(1.0 / 30.0, 30.0);
/// "I want to play here": one per 10 minutes after 3.
pub static WISHES: Limiter = Limiter::new(1.0 / 600.0, 3.0);
/// Short-link hits counted per client (the redirect works regardless).
pub static LINKS: Limiter = Limiter::new(1.0 / 600.0, 2.0);
/// Feedback: one per 5 minutes after 3.
pub static FEEDBACK: Limiter = Limiter::new(1.0 / 300.0, 3.0);
/// Recovery codes tried.
pub static RECOVER: Limiter = Limiter::new(1.0 / 60.0, 5.0);
/// Presence codes tried (guessing a 50-bit code is hopeless anyway).
pub static PRESENCE: Limiter = Limiter::new(1.0 / 60.0, 10.0);
/// Wrong dashboard tokens.
pub static BAD_TOKENS: Limiter = Limiter::new(0.1, 10.0);
/// Messages on one socket: positions every second, views while panning.
pub const MSGS_PER_S: f64 = 10.0;
pub const MSGS_BURST: f64 = 60.0;
/// Messages dropped on one socket before it is closed.
pub const MSGS_DROPPED_MAX: u32 = 200;
/// Largest message from a browser (a full `view` is ~4 KB).
pub const MAX_MESSAGE: usize = 16 * 1024;

/// Open sockets per client, per IPv6 /48, and in all.
const SOCKETS_PER_CLIENT: usize = 100;
const SOCKETS_PER_SITE: usize = 1000;
const SOCKETS_TOTAL: usize = 5000;
/// A socket closes without a hello this soon after opening, or without any
/// message this long (the app sends positions every second and pings after
/// silence; a hidden tab reconnects when it comes back).
pub const HELLO_S: u64 = 15;
pub const IDLE_S: u64 = 300;

/// Memory of a loaded region per junction (graph, field, blocks; measured on
/// New York, San Francisco and Toronto: ~1.5 GB per million), and what the
/// server needs besides its regions.
pub const BYTES_PER_JUNCTION: u64 = 1536;
pub const BASE_BYTES: u64 = 400 << 20;

/// The container's memory limit (cgroup v2, else v1), if there is one.
pub fn memory_limit() -> Option<u64> {
    [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok()?.trim().parse::<u64>().ok())
    .filter(|&b| b < 1 << 50)
}

/// Junction budget of the region pool for `memory` bytes (0: none).
pub fn junction_budget(memory: Option<u64>) -> u64 {
    memory.map_or(0, |m| m.saturating_sub(BASE_BYTES) / BYTES_PER_JUNCTION)
}

/// Open sockets by client, and by /48.
#[derive(Default)]
struct Counts(HashMap<Client, usize>, HashMap<u128, usize>);
static OPEN: LazyLock<Mutex<Counts>> = LazyLock::new(Default::default);

fn release<K: Eq + std::hash::Hash>(m: &mut HashMap<K, usize>, k: K) {
    if let Some(n) = m.get_mut(&k) {
        *n -= 1;
        if *n == 0 {
            m.remove(&k);
        }
    }
}
static OPEN_TOTAL: AtomicUsize = AtomicUsize::new(0);

/// An open socket; counted until dropped.
pub struct Open(Client);

impl Open {
    pub fn try_new(c: Client) -> Option<Self> {
        let mut m = OPEN.lock().unwrap();
        if c.free() {
            OPEN_TOTAL.fetch_add(1, Ordering::Relaxed);
            return Some(Self(c));
        }
        let Counts(by_client, by_site) = &mut *m;
        let site = c.site().map(|s| by_site.get(&s).copied().unwrap_or(0));
        if by_client.get(&c).copied().unwrap_or(0) >= SOCKETS_PER_CLIENT
            || site.is_some_and(|n| n >= SOCKETS_PER_SITE)
            || OPEN_TOTAL.load(Ordering::Relaxed) >= SOCKETS_TOTAL
        {
            return None;
        }
        *by_client.entry(c).or_default() += 1;
        if let Some(s) = c.site() {
            *by_site.entry(s).or_default() += 1;
        }
        OPEN_TOTAL.fetch_add(1, Ordering::Relaxed);
        Some(Self(c))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let mut m = OPEN.lock().unwrap();
        release(&mut m.0, self.0);
        if let Some(s) = self.0.site() {
            release(&mut m.1, s);
        }
        OPEN_TOTAL.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Token bucket for one socket's messages.
pub struct Pace {
    free: bool,
    tokens: f64,
    at: Instant,
    pub dropped: u32,
}

impl Pace {
    pub fn new(c: Client) -> Self {
        Self {
            free: c.free(),
            tokens: MSGS_BURST,
            at: Instant::now(),
            dropped: 0,
        }
    }

    pub fn allow(&mut self) -> bool {
        if self.free {
            return true;
        }
        let now = Instant::now();
        self.tokens =
            (self.tokens + now.duration_since(self.at).as_secs_f64() * MSGS_PER_S).min(MSGS_BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            self.dropped += 1;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_runs_dry() {
        let l = Limiter::new(0.0, 3.0);
        let c = Client(1);
        assert!(l.allow(c) && l.allow(c) && l.allow(c));
        assert!(!l.allow(c));
        assert!(l.allow(Client(2)));
    }

    #[test]
    fn forwarded_only_from_loopback() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        let local: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let remote: SocketAddr = "9.9.9.9:9".parse().unwrap();
        assert_eq!(Client::of(local, &HeaderMap::new()), LOCAL);
        assert_eq!(
            Client::of(local, &h),
            Client(u32::from(std::net::Ipv4Addr::new(5, 6, 7, 8)) as u128)
        );
        assert_eq!(
            Client::of(remote, &h),
            Client(u32::from(std::net::Ipv4Addr::new(9, 9, 9, 9)) as u128)
        );
        let a: SocketAddr = "[2001:db8:1:2:aaaa::1]:9".parse().unwrap();
        let b: SocketAddr = "[2001:db8:1:2:bbbb::2]:9".parse().unwrap();
        assert_eq!(
            Client::of(a, &HeaderMap::new()),
            Client::of(b, &HeaderMap::new())
        );
    }

    #[test]
    fn open_sockets_counted() {
        let c = Client(42);
        let all: Vec<_> = (0..SOCKETS_PER_CLIENT)
            .map(|_| Open::try_new(c).unwrap())
            .collect();
        assert!(Open::try_new(c).is_none());
        drop(all);
        assert!(Open::try_new(c).is_some());
    }

    #[test]
    fn open_sockets_counted_per_site() {
        let site = 0x2001_0db8_0009u128 << 80;
        let all: Vec<_> = (0..SOCKETS_PER_SITE as u128)
            .map(|i| Open::try_new(Client(site | (i + 1) << 64)).unwrap())
            .collect();
        let next = Client(site | 0xffff << 64);
        assert!(Open::try_new(next).is_none());
        assert!(Open::try_new(Client((0x2001_0db8_000au128 << 80) | 1 << 64)).is_some());
        drop(all);
        assert!(Open::try_new(next).is_some());
    }
}

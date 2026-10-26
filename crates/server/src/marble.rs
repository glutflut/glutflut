//! The game state for a globe (docs/data-files.md):
//! each region's state as H3 cells (R5-R9) with 72 h of history in 15-minute
//! bins, as runs of equal values per cell, encoded as `GFL1` files plus a
//! manifest per shard. Region apps send snapshots every few minutes; the
//! collector here (on the host, so it outlives an unloaded region) keeps the
//! history; the uploader writes the files to an S3 bucket and the
//! gateway serves them at `/api/marble/…` (development).
//!
//! No personal data: territory is game state; activity (claims per cell and
//! bin) only counts with at least `K` different players, and only at R5-R7.
//! Territory changes show per bin only while at least `K` players claim in
//! the region; otherwise they wait for the next day (04:00 local), so a lone
//! player's walk can't be read back as a trail with times.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::sync::{Arc, Mutex};

/// H3 resolutions in the files.
pub const RESOLUTIONS: [u8; 5] = [5, 6, 7, 8, 9];
/// Activity only up to this resolution (coarse cells).
pub const ACT_MAX_RES: u8 = 7;
/// Different players needed before claims in a cell and bin count.
pub const K: usize = 5;
pub const BIN_MINUTES: u32 = 15;
/// 72 hours.
pub const BIN_COUNT: u32 = 288;

/// The absolute bin (15 minutes since 1970) of unix seconds `t`.
pub fn bin_of(t: f64) -> u32 {
    (t / 60.0 / f64::from(BIN_MINUTES)) as u32
}

/// One cell's values in a bin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Values {
    pub fire: u8,
    pub water: u8,
    pub front: u8,
    pub act: u16,
}

/// A run: these values from absolute bin `start` on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: u32,
    pub v: Values,
}

/// A cell's weight and history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub nodes: u16,
    pub runs: Vec<Run>,
}

impl Cell {
    /// Values in absolute bin `bin` (the latest snapshot of that bin wins).
    pub fn set(&mut self, bin: u32, v: Values) {
        match self.runs.last_mut() {
            Some(last) if last.start == bin => {
                last.v = v;
                // Merged back into the run before?
                let n = self.runs.len();
                if n >= 2 && self.runs[n - 2].v == v {
                    self.runs.pop();
                }
            }
            Some(last) if last.v == v => {}
            _ => self.runs.push(Run { start: bin, v }),
        }
    }

    /// Drop runs that ended before `from` (the one in force at `from` stays).
    pub fn trim(&mut self, from: u32) {
        let keep = self.runs.iter().rposition(|r| r.start <= from).unwrap_or(0);
        self.runs.drain(..keep);
    }
}

/// One snapshot of a region: per resolution, cells with weight and values.
pub struct Snapshot {
    pub slug: String,
    pub bin: u32,
    pub cells: Vec<(u8, u64, u16, Values)>,
    /// At least `K` different players claimed in the region in this bin.
    pub crowd: bool,
    /// Local day, changing at [`RELEASE_HOUR`] (territory held back until then).
    pub day: i64,
}

/// Local hour at which a quiet region's territory changes are published.
pub const RELEASE_HOUR: f64 = 4.0;

/// A region's cells per resolution.
#[derive(Default)]
pub struct RegionHist {
    pub res: HashMap<u8, HashMap<u64, Cell>>,
    /// Bin of the last snapshot (activity ends after it: region unloaded).
    pub last: u32,
    /// Day of the last snapshot whose territory was published.
    pub released: i64,
}

/// The shard's history of all regions (on the host), and what was published.
#[derive(Default)]
pub struct Collector {
    pub regions: Mutex<HashMap<String, RegionHist>>,
    pub published: Mutex<Published>,
}

/// Files as last published (served at `/api/marble/…` too).
#[derive(Default)]
pub struct Published {
    pub manifest: Option<Arc<str>>,
    /// `glutflut/shards.json` (eu shard only): every shard and its manifest.
    pub index: Option<Arc<str>>,
    /// Path (`<slug>/r<res>_<hash>.bin`) -> gzip bytes.
    pub files: HashMap<String, Arc<Vec<u8>>>,
    current: HashMap<(String, u8), String>,
    /// Replaced paths and when; deleted 10 minutes later.
    retired: Vec<(f64, String)>,
}

impl Collector {
    pub fn add(&self, s: Snapshot) {
        let mut all = self.regions.lock().unwrap();
        let r = all.entry(s.slug).or_default();
        let from = s.bin.saturating_sub(BIN_COUNT - 1);
        // Quiet: territory keeps its published values until the next day.
        let release = s.crowd || s.day > r.released;
        if release {
            r.released = r.released.max(s.day);
        }
        for (res, id, nodes, v) in s.cells {
            let c = r.res.entry(res).or_default().entry(id).or_default();
            c.nodes = nodes;
            let v = match c.runs.last() {
                Some(old) if !release => Values {
                    act: v.act,
                    ..old.v
                },
                _ => v,
            };
            c.set(s.bin, v);
            c.trim(from);
        }
        r.last = r.last.max(s.bin);
    }

    /// History read back after a restart, before what came since.
    pub fn restore(&self, slug: &str, d: Decoded, last: u32) {
        let mut all = self.regions.lock().unwrap();
        let r = all.entry(slug.to_string()).or_default();
        let cells = r.res.entry(d.res).or_default();
        for (id, old) in d.cells {
            let c = cells.entry(id).or_default();
            let first = c.runs.first().map_or(u32::MAX, |r| r.start);
            let mut runs: Vec<Run> = old.runs.into_iter().filter(|r| r.start < first).collect();
            // Activity of the old last bin ends with it.
            if let Some(l) = runs
                .last()
                .copied()
                .filter(|l| l.v.act != 0 && last + 1 < first)
            {
                runs.push(Run {
                    start: last + 1,
                    v: Values { act: 0, ..l.v },
                });
            }
            runs.append(&mut c.runs);
            c.runs = runs;
            if c.nodes == 0 {
                c.nodes = old.nodes;
            }
        }
        r.last = r.last.max(last);
    }
}

/// `GFL1` for one region and resolution over the 72 h ending with bin `now`;
/// `last`: the region's last snapshot (no activity after it).
pub fn encode(res: u8, cells: &HashMap<u64, Cell>, now: u32, last: u32) -> Vec<u8> {
    let start = now.saturating_sub(BIN_COUNT - 1);
    let act = res <= ACT_MAX_RES;
    let mut ids: Vec<&u64> = cells.keys().collect();
    ids.sort_unstable();
    // Runs inside the window; one in force at its start begins at 0. Bins
    // before a cell's first run: no data yet.
    let window = |c: &Cell| -> Vec<(u16, Values)> {
        let mut out: Vec<(u16, Values)> = Vec::new();
        for r in &c.runs {
            let b = r.start.saturating_sub(start) as u16;
            match out.last_mut() {
                Some(last) if last.0 == b => last.1 = r.v,
                _ => out.push((b, r.v)),
            }
        }
        if let Some(&(b, v)) = out.last() {
            let end = (last + 1).saturating_sub(start);
            if v.act != 0 && last < now && end > u32::from(b) {
                out.push((end as u16, Values { act: 0, ..v }));
            }
        }
        out
    };
    let runs: Vec<Vec<(u16, Values)>> = ids.iter().map(|id| window(&cells[id])).collect();
    let total: usize = runs.iter().map(Vec::len).sum();
    let mut b = Vec::with_capacity(24 + cells.len() * 12 + total * 8);
    b.extend_from_slice(b"GFL1");
    b.push(res);
    b.push(u8::from(act));
    b.extend_from_slice(&(BIN_MINUTES as u16).to_le_bytes());
    b.extend_from_slice(&(BIN_COUNT as u16).to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&(start * BIN_MINUTES).to_le_bytes());
    b.extend_from_slice(&(cells.len() as u32).to_le_bytes());
    b.extend_from_slice(&(total as u32).to_le_bytes());
    for (id, rs) in ids.iter().zip(&runs) {
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&cells[id].nodes.to_le_bytes());
        b.extend_from_slice(&(rs.len() as u16).to_le_bytes());
        for (bin, v) in rs {
            b.extend_from_slice(&bin.to_le_bytes());
            b.extend_from_slice(&[v.fire, v.water, v.front, 0]);
            if act {
                b.extend_from_slice(&v.act.to_le_bytes());
            }
        }
    }
    b
}

/// A decoded file: resolution, absolute start bin, cells.
#[derive(Debug, PartialEq)]
pub struct Decoded {
    pub res: u8,
    pub start: u32,
    pub cells: HashMap<u64, Cell>,
}

/// Read a `GFL1` file back (restart: the history continues from the bucket).
pub fn decode(b: &[u8]) -> Option<Decoded> {
    let mut p = 0usize;
    let mut take = |n: usize| {
        let s = b.get(p..p + n)?;
        p += n;
        Some(s)
    };
    if take(4)? != b"GFL1" {
        return None;
    }
    let h = take(2)?;
    let (res, act) = (h[0], h[1] & 1 == 1);
    let u16_ = |s: &[u8]| u16::from_le_bytes([s[0], s[1]]);
    let u32_ = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
    let minutes = u16_(take(2)?);
    let _count = u16_(take(2)?);
    take(2)?;
    let start_min = u32_(take(4)?);
    let n = u32_(take(4)?) as usize;
    take(4)?;
    let start = start_min / u32::from(minutes.max(1));
    let mut cells = HashMap::with_capacity(n);
    for _ in 0..n {
        let id = u64::from_le_bytes(take(8)?.try_into().ok()?);
        let nodes = u16_(take(2)?);
        let runs_n = u16_(take(2)?) as usize;
        let mut runs = Vec::with_capacity(runs_n);
        for _ in 0..runs_n {
            let bin = u16_(take(2)?);
            let v = take(4)?;
            let a = if act { u16_(take(2)?) } else { 0 };
            runs.push(Run {
                start: start + u32::from(bin),
                v: Values {
                    fire: v[0],
                    water: v[1],
                    front: v[2],
                    act: a,
                },
            });
        }
        cells.insert(id, Cell { nodes, runs });
    }
    Some(Decoded { res, start, cells })
}

/// gzip for upload (`Content-Encoding: gzip`).
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(data).expect("gzip to memory");
    enc.finish().expect("gzip to memory")
}

/// Short content hash for file names.
pub fn short_hash(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// --- Region side: snapshots every 5 minutes -------------------------------

/// Claims in the current bin (real players only: bots have no score slot).
#[derive(bevy::prelude::Resource, Default)]
pub struct Claims {
    bin: u32,
    list: Vec<(i64, u32)>,
}

impl Claims {
    pub fn add(&mut self, now: f64, node: i64, slot: u32) {
        let bin = bin_of(now);
        if self.bin == 0 {
            self.bin = bin;
        }
        self.list.push((node, slot));
    }
}

const SNAPSHOT_S: f64 = 300.0;

/// Every 5 minutes, and when a bin ends: the region's cells to the host.
#[allow(clippy::too_many_arguments)]
pub fn tick(
    sim: bevy::prelude::Res<crate::sim::ElementSim>,
    by_id: bevy::prelude::Res<crate::sim::NodeById>,
    nodes: bevy::prelude::Query<&elements_shared::geo::GeoNode>,
    info: Option<bevy::prelude::Res<elements_shared::geo::RegionInfo>>,
    peers: Option<bevy::prelude::Res<crate::regions::Peers>>,
    clock: Option<bevy::prelude::Res<crate::clock::Clock>>,
    mut claims: bevy::prelude::ResMut<Claims>,
    mut cells: bevy::prelude::Local<Vec<[u64; 5]>>,
    mut next: bevy::prelude::Local<f64>,
) {
    let (Some(info), Some(peers)) = (info, peers) else {
        return;
    };
    let now = crate::analytics::now_s();
    // First snapshot a minute after loading, once the field has stepped.
    if *next == 0.0 {
        *next = now + 60.0;
    }
    let bin = bin_of(now);
    if claims.bin == 0 {
        claims.bin = bin;
    }
    let rolled = bin != claims.bin;
    if !rolled && now < *next {
        return;
    }
    let state = sim.state();
    if state.is_empty() {
        return;
    }
    *next = now + SNAPSHOT_S;
    if cells.len() != state.len() {
        *cells = sim
            .nodes()
            .iter()
            .map(|&e| {
                let Some(ll) = nodes
                    .get(e)
                    .ok()
                    .and_then(|n| h3o::LatLng::new(n.lat, n.lon).ok())
                else {
                    return [0; 5];
                };
                RESOLUTIONS.map(|r| {
                    h3o::Resolution::try_from(r).map_or(0, |res| u64::from(ll.to_cell(res)))
                })
            })
            .collect();
    }
    // The ending bin gets its full activity; the new one starts empty.
    let snap_bin = claims.bin;
    let list = std::mem::take(&mut claims.list);
    let mut snap = build(&info.slug, snap_bin, state, &cells, &list, |id| {
        by_id.index(id)
    });
    snap.day = clock.map_or(0, |c| c.day(now - RELEASE_HOUR * 3600.0));
    peers.0.marble.add(snap);
    if rolled {
        claims.bin = bin;
        *next = now;
    } else {
        claims.list = list;
    }
}

/// Shares per cell from node states; activity from claims (k players).
pub fn build(
    slug: &str,
    bin: u32,
    state: &[(f32, f32)],
    cells: &[[u64; 5]],
    claims: &[(i64, u32)],
    index: impl Fn(i64) -> Option<usize>,
) -> Snapshot {
    use elements_shared::elements::ElementState;
    use elements_shared::prelude::Faction;
    // (nodes, fire, water, front) per (res, cell).
    let mut acc: HashMap<(u8, u64), [u32; 4]> = HashMap::new();
    for (i, &(fire, water)) in state.iter().enumerate() {
        let e = ElementState { fire, water };
        let k = match e.owner() {
            Some(Faction::Fire) => Some(1),
            Some(Faction::Water) => Some(2),
            None if e.contested() => Some(3),
            None => None,
        };
        for (r, &cell) in RESOLUTIONS.iter().zip(&cells[i]) {
            if cell == 0 {
                continue;
            }
            let a = acc.entry((*r, cell)).or_default();
            a[0] += 1;
            if let Some(k) = k {
                a[k] += 1;
            }
        }
    }
    let mut act: HashMap<(u8, u64), (u32, std::collections::HashSet<u32>)> = HashMap::new();
    for &(node, slot) in claims {
        let Some(i) = index(node) else { continue };
        for (r, &cell) in RESOLUTIONS.iter().zip(&cells[i]) {
            if *r <= ACT_MAX_RES && cell != 0 {
                let a = act.entry((*r, cell)).or_default();
                a.0 += 1;
                a.1.insert(slot);
            }
        }
    }
    let share = |n: u32, of: u32| ((n * 255 + of / 2) / of.max(1)).min(255) as u8;
    let cells = acc
        .into_iter()
        .map(|((r, cell), [n, f, w, c])| {
            let a = act
                .get(&(r, cell))
                .filter(|(_, who)| who.len() >= K)
                .map_or(0, |(n, _)| (*n).min(u32::from(u16::MAX)) as u16);
            let v = Values {
                fire: share(f, n),
                water: share(w, n),
                front: share(c, n),
                act: a,
            };
            (r, cell, n.min(u32::from(u16::MAX)) as u16, v)
        })
        .collect();
    let crowd = claims
        .iter()
        .map(|&(_, slot)| slot)
        .collect::<HashSet<_>>()
        .len()
        >= K;
    Snapshot {
        slug: slug.to_string(),
        bin,
        cells,
        crowd,
        day: 0,
    }
}

// --- Host side: publish every 5 minutes -----------------------------------

/// The globe's bucket (`ELEMENTS_MARBLE_S3_*`); prefix `glutflut/<shard>/`
/// for every cell: region files and `<slug>/latest.json` don't depend on the
/// cell (regions move). Only the manifest of a cell other than main goes to
/// `cells/<cell>/manifest.json` (main merges them).
pub struct S3 {
    bucket: rusty_s3::Bucket,
    creds: rusty_s3::Credentials,
    prefix: String,
    cell: String,
    agent: ureq::Agent,
}

impl S3 {
    pub fn from_env(shard: &str, cell: &str) -> Option<Self> {
        let var = |k: &str| {
            std::env::var(format!("ELEMENTS_MARBLE_S3_{k}"))
                .ok()
                .filter(|v| !v.is_empty())
        };
        let endpoint = url::Url::parse(&var("ENDPOINT")?).ok()?;
        // Hetzner: the location is the region (`nbg1.your-objectstorage.com`).
        let region = var("REGION").unwrap_or_else(|| {
            endpoint
                .host_str()
                .and_then(|h| h.strip_suffix(".your-objectstorage.com"))
                .unwrap_or("us-east-1")
                .to_string()
        });
        let bucket =
            rusty_s3::Bucket::new(endpoint, rusty_s3::UrlStyle::Path, var("BUCKET")?, region)
                .ok()?;
        // ureq has no default TLS backend (as in shards.rs).
        let tls = native_tls::TlsConnector::new()
            .map_err(|e| tracing::warn!(error = %e, "marble: no TLS"))
            .ok()?;
        Some(Self {
            bucket,
            creds: rusty_s3::Credentials::new(var("KEY_ID")?, var("SECRET")?),
            prefix: format!("glutflut/{shard}/"),
            cell: cell.to_string(),
            agent: ureq::AgentBuilder::new()
                .tls_connector(Arc::new(tls))
                .timeout(std::time::Duration::from_secs(30))
                .build(),
        })
    }

    fn url(&self, method: &str, key: &str) -> url::Url {
        self.url_at(method, &format!("{}{key}", self.prefix))
    }

    /// A key from the bucket root (the shard index lives beside the shards).
    fn url_at(&self, method: &str, key: &str) -> url::Url {
        use rusty_s3::S3Action;
        let d = std::time::Duration::from_secs(600);
        match method {
            "PUT" => self.bucket.put_object(Some(&self.creds), key).sign(d),
            "DELETE" => self.bucket.delete_object(Some(&self.creds), key).sign(d),
            _ => self.bucket.get_object(Some(&self.creds), key).sign(d),
        }
    }

    fn put(&self, key: &str, body: &[u8], ctype: &str, cache: &str) -> Result<(), String> {
        self.put_url(self.url("PUT", key), body, ctype, cache)
    }

    fn put_url(&self, url: url::Url, body: &[u8], ctype: &str, cache: &str) -> Result<(), String> {
        self.agent
            .put(url.as_str())
            .set("Content-Type", ctype)
            .set("Content-Encoding", "gzip")
            .set("Cache-Control", cache)
            .send_bytes(body)
            .map(|_| ())
            .map_err(brief)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.agent
            .delete(self.url("DELETE", key).as_str())
            .call()
            .map(|_| ())
            .map_err(brief)
    }

    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let r = self.agent.get(self.url("GET", key).as_str()).call().ok()?;
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut r.into_reader().take(64 << 20), &mut b).ok()?;
        Some(gunzip(b))
    }
}

/// An error without the request's URL (it carries the signature).
fn brief(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            format!(
                "HTTP {code}: {}",
                body.chars().take(300).collect::<String>()
            )
        }
        ureq::Error::Transport(t) => format!("{}: {}", t.kind(), t.message().unwrap_or("")),
    }
}

/// Bytes as they came (an S3 GET may hand back the stored gzip as is).
fn gunzip(b: Vec<u8>) -> Vec<u8> {
    if b.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        if std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&b[..]), &mut out).is_ok() {
            return out;
        }
    }
    b
}

use std::io::Read as _;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const MANIFEST_CACHE: &str = "public, max-age=60";

/// The publisher thread: restore from the bucket, then every 5 minutes.
pub fn spawn(host: std::sync::Arc<crate::regions::Host>) {
    let s3 = S3::from_env(host.shard(), host.cell());
    if s3.is_none() {
        tracing::info!("marble: no ELEMENTS_MARBLE_S3_*; files only at /api/marble");
    }
    let _ = std::thread::Builder::new()
        .name("marble".into())
        .spawn(move || {
            let mut restored = HashSet::new();
            loop {
                // Regions new here (start, or moved from another cell): their
                // history from the bucket first.
                if let Some(s3) = &s3 {
                    restore(&host, s3, &mut restored);
                }
                // Just after each 5-minute mark, once snapshots are in.
                let now = crate::analytics::now_s();
                let wait = SNAPSHOT_S - now.rem_euclid(SNAPSHOT_S) + 20.0;
                std::thread::sleep(std::time::Duration::from_secs_f64(wait));
                if let Some(s3) = &s3 {
                    restore(&host, s3, &mut restored);
                }
                publish(&host, s3.as_ref(), crate::analytics::now_s());
            }
        });
}

/// Regions on the globe: this shard's listed ones (beta regions too with
/// `ELEMENTS_MARBLE_UNLISTED=1`).
fn shown(host: &crate::regions::Host) -> Vec<crate::db::Listing> {
    let all = std::env::var("ELEMENTS_MARBLE_UNLISTED").is_ok_and(|v| v == "1");
    host.listings()
        .iter()
        .filter(|l| host.here(l) && (all || l.json["listed"].as_bool() != Some(false)))
        .cloned()
        .collect()
}

/// Encode all regions, upload what changed, then the manifest; delete
/// replaced files after 10 minutes.
pub fn publish(host: &crate::regions::Host, s3: Option<&S3>, now: f64) {
    let bin = bin_of(now);
    let regions = shown(host);
    let mut out: Vec<(String, u8, Vec<u8>)> = Vec::new();
    let mut last: HashMap<String, u32> = HashMap::new();
    {
        let hist = host.marble.regions.lock().unwrap();
        for l in &regions {
            let Some(h) = hist.get(&l.slug) else { continue };
            last.insert(l.slug.clone(), h.last);
            for r in RESOLUTIONS {
                if let Some(cells) = h.res.get(&r) {
                    out.push((l.slug.clone(), r, encode(r, cells, bin, h.last)));
                }
            }
        }
    }
    let mut changed = 0;
    let mut fresh: HashSet<String> = HashSet::new();
    for (slug, r, raw) in out {
        let path = format!("{slug}/r{r}_{}.bin", short_hash(&raw));
        let key = (slug, r);
        let old = host
            .marble
            .published
            .lock()
            .unwrap()
            .current
            .get(&key)
            .cloned();
        if old.as_deref() == Some(path.as_str()) {
            continue;
        }
        let gz = gzip(&raw);
        if let Some(s3) = s3 {
            if let Err(e) = s3.put(&path, &gz, "application/octet-stream", IMMUTABLE) {
                tracing::warn!(%path, error = %e, "marble: upload failed");
                continue;
            }
        }
        changed += 1;
        fresh.insert(key.0.clone());
        let mut p = host.marble.published.lock().unwrap();
        p.files.insert(path.clone(), Arc::new(gz));
        if let Some(old) = p.current.insert(key, path) {
            p.retired.push((now, old));
        }
    }
    let manifest = {
        let p = host.marble.published.lock().unwrap();
        manifest(host.shard(), &regions, &p.current, &last, bin, now)
    };
    // Each changed region's entry on its own: whichever cell has it next
    // finds its history there.
    if let Some(s3) = s3 {
        let m: serde_json::Value = serde_json::from_str(&manifest).unwrap_or_default();
        for r in m["regions"].as_array().into_iter().flatten() {
            let Some(slug) = r["slug"].as_str().filter(|s| fresh.contains(*s)) else {
                continue;
            };
            if let Err(e) = s3.put(
                &format!("{slug}/latest.json"),
                &gzip(r.to_string().as_bytes()),
                "application/json",
                MANIFEST_CACHE,
            ) {
                tracing::warn!(slug, error = %e, "marble: latest.json upload failed");
            }
        }
    }
    // Main: the shard's manifest with the other cells' regions in it.
    let main = host.is_main();
    let manifest = match s3.filter(|_| main) {
        Some(s3) => {
            let others: Vec<(String, serde_json::Value)> = host
                .other_cells()
                .into_iter()
                .filter_map(|c| {
                    let b = s3.get(&format!("cells/{c}/manifest.json"))?;
                    Some((c, serde_json::from_slice(&b).ok()?))
                })
                .collect();
            merge_cells(&manifest, &others)
        }
        None => manifest,
    };
    if let Some(s3) = s3 {
        if let Err(e) = s3.put(
            &if main {
                "manifest.json".to_string()
            } else {
                format!("cells/{}/manifest.json", s3.cell)
            },
            &gzip(manifest.as_bytes()),
            "application/json",
            MANIFEST_CACHE,
        ) {
            tracing::warn!(error = %e, "marble: manifest upload failed");
            return;
        }
    }
    // The shard index, written by eu only: the globe reads it instead of
    // a fixed list of shards. Uploaded when it changes.
    if host.shard() == "eu" && main {
        let idx = index(host.shard(), &crate::shards::get().urls);
        let same = host.marble.published.lock().unwrap().index.as_deref() == Some(idx.as_str());
        let ok = same
            || s3.is_none_or(|s3| {
                s3.put_url(
                    s3.url_at("PUT", "glutflut/shards.json"),
                    &gzip(idx.as_bytes()),
                    "application/json",
                    INDEX_CACHE,
                )
                .map_err(|e| tracing::warn!(error = %e, "marble: shard index upload failed"))
                .is_ok()
            });
        if ok {
            host.marble.published.lock().unwrap().index = Some(idx.into());
        }
        // The world ranking next to it (`world.rs`), every publish.
        if let (Some(s3), Some(json)) = (
            s3,
            host.db()
                .and_then(|db| crate::world::merged(db, host.shard(), now)),
        ) {
            if let Err(e) = s3.put_url(
                s3.url_at("PUT", "glutflut/world.json"),
                &gzip(json.as_bytes()),
                "application/json",
                INDEX_CACHE,
            ) {
                tracing::warn!(error = %e, "marble: world.json upload failed");
            }
        }
    }
    // Not main: the shard's manifest may be stale (every cell idle, nobody
    // renews the main lease); files it still names stay until it moves on.
    let named: HashSet<String> = match s3.filter(|_| !main) {
        Some(s3) => s3
            .get("manifest.json")
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .map(|m| {
                m["regions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|r| r["files"].as_object().into_iter().flatten())
                    .filter_map(|(_, f)| f.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        None => HashSet::new(),
    };
    let gone: Vec<String> = {
        let mut p = host.marble.published.lock().unwrap();
        p.manifest = Some(manifest.into());
        let (old, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut p.retired)
            .into_iter()
            .partition(|(t, path)| now - t >= 600.0 && !named.contains(path));
        p.retired = keep;
        for (_, path) in &old {
            p.files.remove(path);
        }
        old.into_iter().map(|(_, p)| p).collect()
    };
    if let Some(s3) = s3 {
        for path in &gone {
            if let Err(e) = s3.delete(path) {
                tracing::warn!(%path, error = %e, "marble: delete failed");
            }
        }
    }
    if changed > 0 {
        tracing::debug!(changed, deleted = gone.len(), "marble: published");
    }
}

/// The shard index lives longer in caches than a manifest; it changes only
/// when a shard comes or goes.
const INDEX_CACHE: &str = "public, max-age=300";

/// `glutflut/shards.json`: this shard and the others of `ELEMENTS_SHARDS`,
/// each with its manifest (relative to `glutflut/`) and game address.
pub fn index(own: &str, others: &HashMap<String, String>) -> String {
    let mut shards: Vec<(String, String)> = others
        .iter()
        .filter(|(k, _)| k.as_str() != own)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    shards.push((own.to_string(), crate::public_url()));
    shards.sort();
    let list: Vec<serde_json::Value> = shards
        .into_iter()
        .map(|(s, url)| serde_json::json!({ "shard": s, "manifest": format!("{s}/manifest.json"), "play": url }))
        .collect();
    serde_json::json!({ "format": "glutflut-shards", "version": 1, "shards": list }).to_string()
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ` of unix seconds.
fn iso(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

fn bin_secs(bin: u32) -> i64 {
    i64::from(bin) * i64::from(BIN_MINUTES) * 60
}

fn manifest(
    shard: &str,
    regions: &[crate::db::Listing],
    current: &HashMap<(String, u8), String>,
    last: &HashMap<String, u32>,
    bin: u32,
    now: f64,
) -> String {
    let base = crate::public_url();
    let list: Vec<serde_json::Value> = regions
        .iter()
        .filter_map(|l| {
            let files: serde_json::Map<String, serde_json::Value> = RESOLUTIONS
                .iter()
                .filter_map(|r| {
                    let p = current.get(&(l.slug.clone(), *r))?;
                    Some((r.to_string(), p.clone().into()))
                })
                .collect();
            if files.is_empty() {
                return None;
            }
            let b = &l.json["bounds"];
            let f = |i: usize| b[i].as_f64().unwrap_or(0.0);
            Some(serde_json::json!({
                "slug": l.slug,
                "name": l.json["city"].as_str().or(l.json["name"].as_str()).unwrap_or(&l.slug),
                "language": l.json["language"],
                "center": [(f(0) + f(2)) / 2.0, (f(1) + f(3)) / 2.0],
                "bounds": [f(0), f(1), f(2), f(3)],
                "play": format!("{base}/?region={}", l.slug),
                "updated": iso(bin_secs(last.get(&l.slug).copied().unwrap_or(bin) + 1).min(now as i64)),
                "files": files,
            }))
        })
        .collect();
    serde_json::json!({
        "format": "GFL1",
        "shard": shard,
        "generated": iso(now as i64),
        "bin_minutes": BIN_MINUTES,
        "bin_count": BIN_COUNT,
        "start": iso(bin_secs(bin.saturating_sub(BIN_COUNT - 1))),
        "resolutions": RESOLUTIONS,
        "regions": list,
    })
    .to_string()
}

/// `own` manifest plus the regions of other cells' manifests (paths are
/// relative to the shard's folder for every cell). A region in two of them
/// (just moved): the newer entry.
fn merge_cells(own: &str, others: &[(String, serde_json::Value)]) -> String {
    let Ok(mut m) = serde_json::from_str::<serde_json::Value>(own) else {
        return own.to_string();
    };
    let Some(list) = m["regions"].as_array_mut() else {
        return own.to_string();
    };
    for (_, o) in others {
        for r in o["regions"].as_array().into_iter().flatten() {
            let newer = |a: &serde_json::Value| r["updated"].as_str() > a["updated"].as_str();
            match list.iter_mut().find(|a| a["slug"] == r["slug"]) {
                Some(a) if newer(a) => *a = r.clone(),
                Some(_) => {}
                None => list.push(r.clone()),
            }
        }
    }
    m.to_string()
}

/// History of the regions new in this cell (at start, or moved here): from
/// `<slug>/latest.json`, else the shard's manifest (published before those
/// existed). Regions gone to another cell drop theirs.
fn restore(host: &crate::regions::Host, s3: &S3, done: &mut HashSet<String>) {
    let here: HashSet<String> = host
        .listings()
        .iter()
        .filter(|l| host.here(l))
        .map(|l| l.slug.clone())
        .collect();
    done.retain(|slug| {
        let keep = here.contains(slug);
        if !keep {
            host.marble.regions.lock().unwrap().remove(slug);
        }
        keep
    });
    let mut shard: Option<serde_json::Value> = None;
    let mut n = 0;
    let new: Vec<&String> = here.iter().filter(|s| !done.contains(*s)).collect();
    for slug in new {
        let entry = s3
            .get(&format!("{slug}/latest.json"))
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .or_else(|| {
                let m = shard.get_or_insert_with(|| {
                    s3.get("manifest.json")
                        .and_then(|b| serde_json::from_slice(&b).ok())
                        .unwrap_or_default()
                });
                m["regions"]
                    .as_array()?
                    .iter()
                    .find(|r| r["slug"] == slug.as_str())
                    .cloned()
            });
        done.insert(slug.clone());
        let Some(r) = entry else { continue };
        let last = r["updated"]
            .as_str()
            .and_then(parse_iso)
            .map_or(0, |t| bin_of(t as f64).saturating_sub(1));
        for (_, path) in r["files"].as_object().into_iter().flatten() {
            let Some(path) = path.as_str() else { continue };
            if let Some(d) = s3.get(path).as_deref().and_then(decode) {
                host.marble.restore(slug, d, last);
                n += 1;
            }
        }
    }
    if n > 0 {
        tracing::info!(files = n, "marble: history restored");
    }
}

/// Unix seconds of `YYYY-MM-DDTHH:MM:SSZ`.
fn parse_iso(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (hh, mm, ss) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_merges_other_cells_newest_entry_wins() {
        let own = r#"{"format":"GFL1","regions":[
            {"slug":"a","updated":"2026-10-11T10:00:00Z","files":{"6":"a/r6_x.bin"}},
            {"slug":"m","updated":"2026-10-11T10:00:00Z","files":{"6":"m/r6_old.bin"}}]}"#;
        let b = serde_json::json!({"regions":[
            {"slug":"b","updated":"2026-10-11T10:05:00Z","files":{"7":"b/r7_z.bin"}},
            {"slug":"m","updated":"2026-10-11T10:05:00Z","files":{"6":"m/r6_new.bin"}}]});
        let m: serde_json::Value =
            serde_json::from_str(&merge_cells(own, &[("bw".into(), b)])).unwrap();
        let r = m["regions"].as_array().unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0]["files"]["6"], "a/r6_x.bin");
        assert_eq!(r[1]["files"]["6"], "m/r6_new.bin");
        assert_eq!(r[2]["files"]["7"], "b/r7_z.bin");
    }

    #[test]
    fn shard_index_lists_every_shard_once() {
        let others: HashMap<String, String> = [
            ("sa".to_string(), "https://sa.glutflut.com".to_string()),
            ("na".to_string(), "https://na.glutflut.com".to_string()),
            ("eu".to_string(), "https://ignored.example".to_string()),
        ]
        .into();
        let v: serde_json::Value = serde_json::from_str(&index("eu", &others)).unwrap();
        assert_eq!(v["format"], "glutflut-shards");
        let names: Vec<&str> = v["shards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["shard"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["eu", "na", "sa"]);
        assert_eq!(v["shards"][1]["manifest"], "na/manifest.json");
        assert_eq!(v["shards"][2]["play"], "https://sa.glutflut.com");
        assert_ne!(
            v["shards"][0]["play"], "https://ignored.example",
            "own address from ELEMENTS_PUBLIC_URL"
        );
    }

    fn v(fire: u8, water: u8, act: u16) -> Values {
        Values {
            fire,
            water,
            front: 0,
            act,
        }
    }

    #[test]
    fn runs_merge_equal_values_and_the_latest_snapshot_wins() {
        let mut c = Cell::default();
        c.set(10, v(200, 0, 0));
        c.set(11, v(200, 0, 0));
        assert_eq!(c.runs.len(), 1, "same values: one run");
        c.set(12, v(100, 50, 0));
        c.set(12, v(200, 0, 0));
        assert_eq!(c.runs.len(), 1, "back to the run before");
        c.set(13, v(0, 255, 0));
        assert_eq!(c.runs.len(), 2);
        c.trim(20);
        assert_eq!(
            c.runs,
            vec![Run {
                start: 13,
                v: v(0, 255, 0)
            }],
            "the run in force stays"
        );
    }

    #[test]
    fn encode_and_decode_round_trip_within_the_window() {
        let now = 100_000;
        let mut cells = HashMap::new();
        let mut a = Cell {
            nodes: 12,
            ..Default::default()
        };
        // Before the window: in force at its start.
        a.set(now - 400, v(255, 0, 0));
        a.set(now - 10, v(0, 200, 7));
        cells.insert(0x0852_0000_0000_0fffu64, a);
        let b = encode(7, &cells, now, now);
        let d = decode(&b).unwrap();
        let start = now - (BIN_COUNT - 1);
        assert_eq!(d.res, 7);
        assert_eq!(d.start, start);
        let c = &d.cells[&0x0852_0000_0000_0fffu64];
        assert_eq!(c.nodes, 12);
        assert_eq!(
            c.runs,
            vec![
                Run {
                    start,
                    v: v(255, 0, 0)
                },
                Run {
                    start: now - 10,
                    v: v(0, 200, 7)
                }
            ]
        );
        // R8: no activity bytes.
        let b8 = encode(8, &cells, now, now);
        assert_eq!(b.len() - b8.len(), 2 * 2, "two runs, 2 bytes each");
        assert!(decode(&gzip(&b)).is_none(), "gzip is for transport only");
    }

    #[test]
    fn collector_keeps_regions_apart() {
        let col = Collector::default();
        col.add(Snapshot {
            slug: "region-a".into(),
            bin: 5,
            cells: vec![(9, 1, 3, v(255, 0, 0))],
            crowd: false,
            day: 1,
        });
        col.add(Snapshot {
            slug: "region-b".into(),
            bin: 5,
            cells: vec![(9, 1, 4, v(0, 255, 0))],
            crowd: false,
            day: 1,
        });
        let r = col.regions.lock().unwrap();
        assert_eq!(r["region-a"].res[&9][&1].runs[0].v.fire, 255);
        assert_eq!(r["region-b"].res[&9][&1].nodes, 4);
    }

    #[test]
    fn snapshot_shares_and_activity_only_with_k_players() {
        // Four nodes in one R9 cell (and its parents): fire, fire, water, contested.
        let state = [(0.9, 0.0), (0.8, 0.1), (0.0, 0.9), (0.5, 0.5)];
        let cells = vec![[5, 6, 7, 8, 9]; 4];
        let ids = [10i64, 11, 12, 13];
        let index = |id: i64| ids.iter().position(|&x| x == id);
        let few: Vec<(i64, u32)> = (0..4).map(|s| (10, s)).collect();
        let s = build("r", 1, &state, &cells, &few, index);
        let at = |res: u8| s.cells.iter().find(|c| c.0 == res).unwrap();
        let (_, _, n, v) = at(9);
        assert_eq!(*n, 4);
        assert_eq!((v.fire, v.water, v.front), (128, 64, 64));
        assert_eq!(at(7).3.act, 0, "4 players: no activity");
        let many: Vec<(i64, u32)> = (0..6).map(|s| (10, s % 5)).collect();
        let s = build("r", 1, &state, &cells, &many, index);
        let at = |res: u8| s.cells.iter().find(|c| c.0 == res).unwrap();
        assert_eq!(at(7).3.act, 6, "5 players: all 6 claims count");
        assert_eq!(at(8).3.act, 0, "never at R8/R9");
    }

    #[test]
    fn activity_ends_after_the_last_snapshot_and_survives_a_restart() {
        let now = 50_000;
        let mut cells = HashMap::new();
        let mut c = Cell {
            nodes: 3,
            ..Default::default()
        };
        c.set(now - 20, v(255, 0, 9));
        cells.insert(42u64, c);
        // Unloaded at now - 20: activity only in that bin.
        let d = decode(&encode(7, &cells, now, now - 20)).unwrap();
        assert_eq!(
            d.cells[&42]
                .runs
                .iter()
                .map(|r| (r.start, r.v.act))
                .collect::<Vec<_>>(),
            vec![(now - 20, 9), (now - 19, 0)]
        );
        // Restart: a new snapshot first, then the old history before it.
        let col = Collector::default();
        col.add(Snapshot {
            slug: "r".into(),
            bin: now + 3,
            cells: vec![(7, 42, 3, v(0, 255, 0))],
            crowd: false,
            day: 1,
        });
        col.restore("r", d, now - 20);
        let r = col.regions.lock().unwrap();
        let runs: Vec<u32> = r["r"].res[&7][&42].runs.iter().map(|r| r.start).collect();
        assert_eq!(runs, vec![now - 20, now - 19, now + 3]);
        assert_eq!(r["r"].last, now + 3);
    }

    #[test]
    fn quiet_territory_waits_for_the_next_day_a_crowd_shows_it_at_once() {
        let col = Collector::default();
        let snap = |bin, fire, crowd, day| Snapshot {
            slug: "r".into(),
            bin,
            cells: vec![(9, 1, 3, v(fire, 0, 0))],
            crowd,
            day,
        };
        let shown = |col: &Collector| {
            let r = col.regions.lock().unwrap();
            r["r"].res[&9][&1]
                .runs
                .iter()
                .map(|r| (r.start, r.v.fire))
                .collect::<Vec<_>>()
        };
        col.add(snap(10, 0, false, 1)); // first: as it is
        col.add(snap(11, 100, false, 1)); // a lone player claims: held
        col.add(snap(12, 200, false, 1));
        assert_eq!(shown(&col), vec![(10, 0)]);
        col.add(snap(13, 210, false, 2)); // 04:00: the day's result at once
        assert_eq!(shown(&col), vec![(10, 0), (13, 210)]);
        col.add(snap(14, 255, true, 2)); // a crowd: every bin
        assert_eq!(shown(&col), vec![(10, 0), (13, 210), (14, 255)]);
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_791_454_500), "2026-10-08T10:15:00Z");
        for t in [0, 951_782_400, 1_791_454_500, 4_102_444_799] {
            assert_eq!(parse_iso(&iso(t)), Some(t));
        }
    }

    /// Network: a made-up key gets an HTTP answer (TLS works), without the URL.
    /// Needs `TEST_S3_ENDPOINT` (any S3 host, e.g. `https://s3.example.com`).
    #[test]
    #[ignore]
    fn s3_answers_over_tls() {
        let Ok(endpoint) = std::env::var("TEST_S3_ENDPOINT") else {
            return;
        };
        for (k, v) in [
            ("ENDPOINT", endpoint.as_str()),
            ("BUCKET", "example-bucket"),
            ("KEY_ID", "NOTAKEY"),
            ("SECRET", "notasecret"),
        ] {
            unsafe { std::env::set_var(format!("ELEMENTS_MARBLE_S3_{k}"), v) };
        }
        let s3 = S3::from_env("test", "main").expect("config");
        let e = s3
            .put("probe.bin", b"x", "application/octet-stream", IMMUTABLE)
            .unwrap_err();
        println!("{e}");
        assert!(e.starts_with("HTTP 403"), "{e}");
        assert!(!e.contains("X-Amz-Signature"));
    }
}

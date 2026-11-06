//! Cloud-optimised GeoTIFFs read over HTTP range requests, just enough for the
//! dashboard's raster layers: classic or BigTIFF, 8-bit tiles, deflate (with or
//! without predictor 2), Web Mercator. Bands are pixel-interleaved; we keep the
//! last one, which in GeoSci's ratings is the score (a two-band file carries
//! the value first, e.g. decibels).

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex};

pub const NODATA: u8 = 255;
/// Web Mercator half extent, metres.
const E: f64 = 20_037_508.342_789_244;
/// GDAL's COGs keep every header in the first few KiB.
const HEAD: u64 = 64 << 10;
/// Offset and byte-count arrays are read in chunks of this size.
const CHUNK: u64 = 4096;
/// A compressed tile is far smaller; a bigger one is not a tile.
const BLOCK_MAX: u64 = 8 << 20;
/// Decoded tiles kept per file (one band, 256 KiB each at 512²).
const KEEP: usize = 48;
pub const TILE: usize = 256;

#[derive(Debug, PartialEq)]
pub enum Error {
    NotFound,
    Upstream,
    Format(&'static str),
}

/// Where the bytes come from.
pub trait Source: Send + Sync {
    /// Up to `len` bytes from `start`; fewer at the end of the file.
    fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, Error>;
}

pub struct Http(pub String);

impl Source for Http {
    fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, Error> {
        let agent = crate::shards::agent().ok_or(Error::Upstream)?;
        let range = format!("bytes={}-{}", start, start + len.max(1) - 1);
        let r = match agent.get(&self.0).set("Range", &range).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(403 | 404, _)) => return Err(Error::NotFound),
            Err(ureq::Error::Status(416, _)) => return Ok(Vec::new()),
            Err(_) => return Err(Error::Upstream),
        };
        // A 200 would be the whole file.
        if r.status() != 206 {
            return Err(Error::Upstream);
        }
        let mut buf = Vec::new();
        r.into_reader()
            .take(len)
            .read_to_end(&mut buf)
            .map_err(|_| Error::Upstream)?;
        Ok(buf)
    }
}

/// An offset or byte-count array: inline values, or where to find them.
struct Array {
    inline: Option<Vec<u64>>,
    pos: u64,
    size: u64,
    n: u64,
}

struct Level {
    w: u64,
    h: u64,
    tw: u64,
    th: u64,
    res: f64,
    offsets: Array,
    counts: Array,
}

type Block = Option<Arc<Vec<u8>>>;

pub struct Cog {
    src: Box<dyn Source>,
    le: bool,
    /// Finest first.
    levels: Vec<Level>,
    spp: usize,
    predictor: u16,
    ox: f64,
    oy: f64,
    chunks: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
    blocks: Mutex<(HashMap<(usize, u64), Block>, VecDeque<(usize, u64)>)>,
}

struct Entry {
    typ: u16,
    count: u64,
    /// The value field, or the offset of the values.
    field: [u8; 8],
}

fn type_size(t: u16) -> Option<u64> {
    Some(match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return None,
    })
}

struct Parser<'a> {
    src: &'a dyn Source,
    head: &'a [u8],
    le: bool,
    big: bool,
}

impl Parser<'_> {
    fn bytes(&self, pos: u64, len: u64) -> Result<Vec<u8>, Error> {
        let end = pos.checked_add(len).ok_or(Error::Format("offset"))?;
        if end <= self.head.len() as u64 {
            return Ok(self.head[pos as usize..end as usize].to_vec());
        }
        let b = self.src.read(pos, len)?;
        if (b.len() as u64) < len {
            return Err(Error::Format("short read"));
        }
        Ok(b)
    }

    fn uint(&self, b: &[u8]) -> u64 {
        let mut v = 0u64;
        for i in 0..b.len() {
            let byte = if self.le { b[b.len() - 1 - i] } else { b[i] };
            v = (v << 8) | byte as u64;
        }
        v
    }

    fn entries(&self, ifd: u64) -> Result<(HashMap<u16, Entry>, u64), Error> {
        let (count_len, esize, field) = if self.big { (8, 20, 8) } else { (2, 12, 4) };
        let n = self.uint(&self.bytes(ifd, count_len)?);
        if n > 4096 {
            return Err(Error::Format("ifd"));
        }
        let next_len = if self.big { 8 } else { 4 };
        let b = self.bytes(ifd + count_len, n * esize + next_len)?;
        let mut out = HashMap::new();
        for i in 0..n as usize {
            let e = &b[i * esize as usize..][..esize as usize];
            let tag = self.uint(&e[0..2]) as u16;
            let typ = self.uint(&e[2..4]) as u16;
            let (count, f) = if self.big {
                (self.uint(&e[4..12]), &e[12..20])
            } else {
                (self.uint(&e[4..8]), &e[8..12])
            };
            let mut fieldb = [0u8; 8];
            fieldb[..field].copy_from_slice(f);
            out.insert(
                tag,
                Entry {
                    typ,
                    count,
                    field: fieldb,
                },
            );
        }
        let next = self.uint(&b[(n * esize) as usize..]);
        Ok((out, next))
    }

    fn field_len(&self) -> u64 {
        if self.big {
            8
        } else {
            4
        }
    }

    /// Where an entry's values are, inline or not.
    fn array(&self, e: &Entry) -> Result<Array, Error> {
        let size = type_size(e.typ).ok_or(Error::Format("type"))?;
        let total = size.checked_mul(e.count).ok_or(Error::Format("count"))?;
        if total <= self.field_len() {
            let v = (0..e.count as usize)
                .map(|i| self.uint(&e.field[i * size as usize..][..size as usize]))
                .collect();
            return Ok(Array {
                inline: Some(v),
                pos: 0,
                size,
                n: e.count,
            });
        }
        Ok(Array {
            inline: None,
            pos: self.uint(&e.field[..self.field_len() as usize]),
            size,
            n: e.count,
        })
    }

    /// Small integer arrays, read in full.
    fn ints(&self, e: &Entry) -> Result<Vec<u64>, Error> {
        if !matches!(e.typ, 1 | 3 | 4 | 16) || e.count > 64 {
            return Err(Error::Format("ints"));
        }
        let a = self.array(e)?;
        if let Some(v) = a.inline {
            return Ok(v);
        }
        let b = self.bytes(a.pos, a.size * a.n)?;
        Ok(b.chunks(a.size as usize).map(|c| self.uint(c)).collect())
    }

    fn doubles(&self, e: &Entry) -> Result<Vec<f64>, Error> {
        if e.typ != 12 || e.count > 64 {
            return Err(Error::Format("doubles"));
        }
        let a = self.array(e)?;
        let raw: Vec<u64> = match a.inline {
            Some(v) => v,
            None => {
                let b = self.bytes(a.pos, 8 * a.n)?;
                b.chunks(8).map(|c| self.uint(c)).collect()
            }
        };
        Ok(raw.into_iter().map(f64::from_bits).collect())
    }
}

impl Cog {
    pub fn open(src: Box<dyn Source>) -> Result<Cog, Error> {
        Self::open_with(src, HEAD)
    }

    fn open_with(src: Box<dyn Source>, head_len: u64) -> Result<Cog, Error> {
        let head = src.read(0, head_len)?;
        if head.len() < 16 {
            return Err(Error::Format("header"));
        }
        let le = match &head[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err(Error::Format("not a tiff")),
        };
        let mut p = Parser {
            src: &*src,
            head: &head,
            le,
            big: false,
        };
        let mut ifd = match p.uint(&head[2..4]) {
            42 => p.uint(&head[4..8]),
            43 => {
                p.big = true;
                p.uint(&head[8..16])
            }
            _ => return Err(Error::Format("not a tiff")),
        };
        let mut levels: Vec<Level> = Vec::new();
        let (mut spp, mut predictor, mut ox, mut oy) = (0usize, 1u16, 0.0, 0.0);
        let mut seen = 0;
        while ifd != 0 && seen < 32 {
            seen += 1;
            let (t, next) = p.entries(ifd)?;
            ifd = next;
            let one = |tag: u16, default: Option<u64>| -> Result<u64, Error> {
                match t.get(&tag) {
                    Some(e) => p.ints(e)?.first().copied().ok_or(Error::Format("tag")),
                    None => default.ok_or(Error::Format("missing tag")),
                }
            };
            // Masks are not levels.
            if one(254, Some(0))? & 4 != 0 {
                continue;
            }
            let compression = one(259, Some(1))?;
            if compression != 8 && compression != 32946 {
                return Err(Error::Format("compression"));
            }
            if one(284, Some(1))? != 1 || one(339, Some(1))? != 1 {
                return Err(Error::Format("layout"));
            }
            let bits = t.get(&258).map(|e| p.ints(e)).transpose()?;
            if bits.is_some_and(|b| b.iter().any(|&b| b != 8)) {
                return Err(Error::Format("bits"));
            }
            let s = one(277, Some(1))? as usize;
            let pr = one(317, Some(1))? as u16;
            if pr != 1 && pr != 2 {
                return Err(Error::Format("predictor"));
            }
            let (w, h, tw, th) = (
                one(256, None)?,
                one(257, None)?,
                one(322, None)?,
                one(323, None)?,
            );
            if w == 0 || h == 0 || tw == 0 || th == 0 || tw > 2048 || th > 2048 {
                return Err(Error::Format("size"));
            }
            let offsets = p.array(t.get(&324).ok_or(Error::Format("offsets"))?)?;
            let counts = p.array(t.get(&325).ok_or(Error::Format("counts"))?)?;
            let tiles = w.div_ceil(tw) * h.div_ceil(th);
            if offsets.n < tiles || counts.n < tiles {
                return Err(Error::Format("tiles"));
            }
            let res = if let Some(first) = levels.first() {
                if s != spp || pr != predictor {
                    return Err(Error::Format("levels differ"));
                }
                first.res * first.w as f64 / w as f64
            } else {
                let scale = p.doubles(t.get(&33550).ok_or(Error::Format("scale"))?)?;
                let tie = p.doubles(t.get(&33922).ok_or(Error::Format("tiepoint"))?)?;
                if scale.len() < 2 || tie.len() < 6 || scale[0] <= 0.0 {
                    return Err(Error::Format("georef"));
                }
                ox = tie[3] - tie[0] * scale[0];
                oy = tie[4] + tie[1] * scale[1];
                spp = s;
                predictor = pr;
                scale[0]
            };
            levels.push(Level {
                w,
                h,
                tw,
                th,
                res,
                offsets,
                counts,
            });
        }
        if levels.is_empty() || spp == 0 {
            return Err(Error::Format("no image"));
        }
        Ok(Cog {
            src,
            le,
            levels,
            spp,
            predictor,
            ox,
            oy,
            chunks: Mutex::new(HashMap::new()),
            blocks: Mutex::new((HashMap::new(), VecDeque::new())),
        })
    }

    /// Bytes through the chunk cache (array entries).
    fn cached(&self, pos: u64, len: u64) -> Result<Vec<u8>, Error> {
        let mut out = Vec::with_capacity(len as usize);
        let mut at = pos;
        while at < pos + len {
            let start = at / CHUNK * CHUNK;
            let hit = self.chunks.lock().unwrap().get(&start).cloned();
            let chunk = match hit {
                Some(c) => c,
                None => {
                    let c = Arc::new(self.src.read(start, CHUNK)?);
                    let mut m = self.chunks.lock().unwrap();
                    if m.len() > 4096 {
                        m.clear();
                    }
                    m.insert(start, c.clone());
                    c
                }
            };
            let from = (at - start) as usize;
            let take = ((pos + len - at) as usize).min(chunk.len().saturating_sub(from));
            if take == 0 {
                return Err(Error::Format("short read"));
            }
            out.extend_from_slice(&chunk[from..from + take]);
            at += take as u64;
        }
        Ok(out)
    }

    fn entry(&self, a: &Array, i: u64) -> Result<u64, Error> {
        if i >= a.n {
            return Err(Error::Format("index"));
        }
        if let Some(v) = &a.inline {
            return Ok(v[i as usize]);
        }
        let b = self.cached(a.pos + i * a.size, a.size)?;
        let mut v = 0u64;
        for k in 0..b.len() {
            let byte = if self.le { b[b.len() - 1 - k] } else { b[k] };
            v = (v << 8) | byte as u64;
        }
        Ok(v)
    }

    /// One tile of a level, last band only; `None` where the file has none.
    fn block(&self, li: usize, idx: u64) -> Result<Block, Error> {
        if let Some(b) = self.blocks.lock().unwrap().0.get(&(li, idx)) {
            return Ok(b.clone());
        }
        let l = &self.levels[li];
        let (off, len) = (self.entry(&l.offsets, idx)?, self.entry(&l.counts, idx)?);
        let got = if off == 0 || len == 0 {
            None
        } else if len > BLOCK_MAX {
            return Err(Error::Format("block size"));
        } else {
            let raw = self.src.read(off, len)?;
            let want = (l.tw * l.th) as usize * self.spp;
            let mut px = Vec::with_capacity(want);
            flate2::read::ZlibDecoder::new(&raw[..])
                .take(want as u64)
                .read_to_end(&mut px)
                .map_err(|_| Error::Format("deflate"))?;
            if px.len() != want {
                return Err(Error::Format("block"));
            }
            let row = l.tw as usize * self.spp;
            if self.predictor == 2 {
                for r in px.chunks_mut(row) {
                    for i in self.spp..r.len() {
                        r[i] = r[i].wrapping_add(r[i - self.spp]);
                    }
                }
            }
            let last = self.spp - 1;
            Some(Arc::new(
                px.chunks(self.spp).map(|p| p[last]).collect::<Vec<u8>>(),
            ))
        };
        let mut m = self.blocks.lock().unwrap();
        let (map, order) = &mut *m;
        if map.insert((li, idx), got.clone()).is_none() {
            order.push_back((li, idx));
            while order.len() > KEEP {
                if let Some(k) = order.pop_front() {
                    map.remove(&k);
                }
            }
        }
        Ok(got)
    }

    /// The web map tile `z/x/y` as 256² values (`NODATA` outside), or `None`
    /// when the file has nothing there.
    pub fn render(&self, z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>, Error> {
        let size = 2.0 * E / f64::from(1u32 << z.min(30));
        let r = size / TILE as f64;
        // The coarsest level that is still at least as fine as the tile.
        let li = self
            .levels
            .iter()
            .rposition(|l| l.res <= r * 1.001)
            .unwrap_or(0);
        let l = &self.levels[li];
        let x0 = -E + f64::from(x) * size;
        let y0 = E - f64::from(y) * size;
        // Pixel-centre coordinates in the level.
        let fx = |px: usize| (x0 + (px as f64 + 0.5) * r - self.ox) / l.res - 0.5;
        let fy = |py: usize| (self.oy - (y0 - (py as f64 + 0.5) * r)) / l.res - 0.5;
        let (c0, c1) = (fx(0).floor() as i64, fx(TILE - 1).floor() as i64 + 1);
        let (r0, r1) = (fy(0).floor() as i64, fy(TILE - 1).floor() as i64 + 1);
        let (w, h) = (l.w as i64, l.h as i64);
        if c1 < 0 || r1 < 0 || c0 >= w || r0 >= h {
            return Ok(None);
        }
        let across = l.w.div_ceil(l.tw);
        let (bc0, bc1) = (c0.max(0) as u64 / l.tw, c1.min(w - 1) as u64 / l.tw);
        let (br0, br1) = (r0.max(0) as u64 / l.th, r1.min(h - 1) as u64 / l.th);
        let want: Vec<u64> = (br0..=br1)
            .flat_map(|br| (bc0..=bc1).map(move |bc| br * across + bc))
            .collect();
        if want.len() > 16 {
            return Err(Error::Format("too many blocks"));
        }
        let got: Vec<Result<Block, Error>> = std::thread::scope(|s| {
            let hs: Vec<_> = want
                .iter()
                .map(|&i| s.spawn(move || self.block(li, i)))
                .collect();
            hs.into_iter()
                .map(|h| h.join().unwrap_or(Err(Error::Upstream)))
                .collect()
        });
        let mut blocks = HashMap::new();
        for (i, b) in want.iter().zip(got) {
            blocks.insert(*i, b?);
        }
        if blocks.values().all(Option::is_none) {
            return Ok(None);
        }
        let at = |c: i64, r: i64| -> Option<u8> {
            if c < 0 || r < 0 || c >= w || r >= h {
                return None;
            }
            let (c, r) = (c as u64, r as u64);
            let b = blocks.get(&((r / l.th) * across + c / l.tw))?.as_ref()?;
            let v = b[((r % l.th) * l.tw + c % l.tw) as usize];
            (v != NODATA).then_some(v)
        };
        let mut out = vec![NODATA; TILE * TILE];
        let mut any = false;
        for py in 0..TILE {
            let sy = fy(py);
            let (ry, ty) = (sy.floor() as i64, sy - sy.floor());
            for px in 0..TILE {
                let sx = fx(px);
                let (rx, tx) = (sx.floor() as i64, sx - sx.floor());
                let q = [
                    at(rx, ry),
                    at(rx + 1, ry),
                    at(rx, ry + 1),
                    at(rx + 1, ry + 1),
                ];
                let v = match q {
                    [Some(a), Some(b), Some(c), Some(d)] => {
                        let top = f64::from(a) * (1.0 - tx) + f64::from(b) * tx;
                        let bot = f64::from(c) * (1.0 - tx) + f64::from(d) * tx;
                        Some((top * (1.0 - ty) + bot * ty).round().min(254.0) as u8)
                    }
                    _ => q[usize::from(tx >= 0.5) + 2 * usize::from(ty >= 0.5)],
                };
                if let Some(v) = v {
                    out[py * TILE + px] = v;
                    any = true;
                }
            }
        }
        Ok(any.then_some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made with gdal_translate -of COG (DEFLATE, PREDICTOR=2, BLOCKSIZE=512,
    /// two overviews) from a 1100×700 two-band raster at 20 m in EPSG:3857,
    /// top left 1021400/6049700. Band 2 is (col/5 + row/7) % 250, nodata in
    /// the top left 100×100.
    const BIG: &[u8] = include_bytes!("testdata/cog-big.tif");
    const CLASSIC: &[u8] = include_bytes!("testdata/cog-classic.tif");

    struct Mem(&'static [u8], Arc<std::sync::atomic::AtomicUsize>);
    impl Source for Mem {
        fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, Error> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let s = (start as usize).min(self.0.len());
            let e = (start + len).min(self.0.len() as u64) as usize;
            Ok(self.0[s..e].to_vec())
        }
    }

    fn open(b: &'static [u8], head: u64) -> Cog {
        Cog::open_with(Box::new(Mem(b, Default::default())), head).unwrap()
    }

    /// The tile holding a point, and the point's pixel in it.
    fn locate(z: u32, mx: f64, my: f64) -> (u32, u32, usize) {
        let size = 2.0 * E / f64::from(1u32 << z);
        let (tx, ty) = (((mx + E) / size) as u32, ((E - my) / size) as u32);
        let px = ((mx + E - f64::from(tx) * size) / (size / 256.0)) as usize;
        let py = ((E - my - f64::from(ty) * size) / (size / 256.0)) as usize;
        (tx, ty, py * 256 + px)
    }

    fn band2(c: u64, r: u64) -> u8 {
        if c < 100 && r < 100 {
            NODATA
        } else {
            ((c / 5 + r / 7) % 250) as u8
        }
    }

    #[test]
    fn levels_and_georef() {
        for b in [BIG, CLASSIC] {
            // A short head forces reads past it.
            for head in [HEAD, 300] {
                let c = open(b, head);
                assert_eq!(c.levels.len(), 3);
                assert_eq!((c.levels[0].w, c.levels[0].h), (1100, 700));
                assert_eq!(c.levels[2].w, 275);
                assert_eq!(c.levels[2].res, 80.0);
                assert_eq!((c.ox, c.oy, c.spp), (1021400.0, 6049700.0, 2));
            }
        }
    }

    #[test]
    fn blocks_decode_to_band_two() {
        for b in [BIG, CLASSIC] {
            let c = open(b, 300);
            // Block 4 is row 1, col 1: pixels 512.. in both directions.
            let blk = c.block(0, 4).unwrap().unwrap();
            for (cc, rr) in [(0u64, 0u64), (37, 5), (511, 187)] {
                assert_eq!(blk[(rr * 512 + cc) as usize], band2(512 + cc, 512 + rr));
            }
            let first = c.block(0, 0).unwrap().unwrap();
            assert_eq!(first[0], NODATA);
            assert_eq!(first[150 * 512 + 150], band2(150, 150));
        }
    }

    #[test]
    fn render_lands_on_the_right_pixels() {
        let c = open(BIG, HEAD);
        // Centre of pixel (600, 400); z14 is 9.55 m a pixel, so level 0.
        let (mx, my) = (1021400.0 + 600.5 * 20.0, 6049700.0 - 400.5 * 20.0);
        let (tx, ty, i) = locate(14, mx, my);
        let got = c.render(14, tx, ty).unwrap().unwrap()[i];
        let want = band2(600, 400);
        assert!(got.abs_diff(want) <= 1, "{got} vs {want}");
        // In the nodata corner: transparent.
        let (tx, ty, i) = locate(14, 1021400.0 + 50.0 * 20.0, 6049700.0 - 50.0 * 20.0);
        assert_eq!(c.render(14, tx, ty).unwrap().unwrap()[i], NODATA);
        // Far away: nothing.
        assert_eq!(c.render(14, 0, 0).unwrap(), None);
        // Zoomed out, an overview does it.
        let (tx, ty, _) = locate(9, mx, my);
        assert!(c.render(9, tx, ty).unwrap().is_some());
    }

    #[test]
    fn rejects_what_is_not_a_cog() {
        let e = Cog::open_with(
            Box::new(Mem(b"GIF89a.................", Default::default())),
            HEAD,
        );
        assert!(matches!(e, Err(Error::Format(_))));
    }

    #[test]
    fn caches_blocks_and_chunks() {
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = Cog::open_with(Box::new(Mem(BIG, n.clone())), 300).unwrap();
        let count = || n.load(std::sync::atomic::Ordering::Relaxed);
        c.block(0, 1).unwrap();
        let after_one = count();
        c.block(0, 1).unwrap();
        assert_eq!(count(), after_one);
        // The neighbour's offsets sit in the same chunk: only its data is read.
        c.block(0, 2).unwrap();
        assert_eq!(count(), after_one + 1);
    }
}

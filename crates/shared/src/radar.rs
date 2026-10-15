//! Rain radar values (published weather files, format RAD2-VALUES;
//! docs/data-files.md "Weather"): a list of H3 R4 blocks; a
//! 15-minute frame holds one byte per R8 child of every block, the children in
//! ascending H3 order. Byte codes: 0 dry, 1..253 rain
//! (mm/h = 0.1 * 10^((v-1)/72)), 255 no data.

use std::collections::HashMap;
use std::sync::Arc;

const BLOCK_RES: h3o::Resolution = h3o::Resolution::Four;
/// Cell resolution unless the manifest says otherwise (`cell_res`).
pub const CELL_RES: u8 = 8;

/// mm/h of a byte code; `None` without data.
pub fn mm_h(v: u8) -> Option<f32> {
    match v {
        0 => Some(0.0),
        1..=253 => Some(0.1 * 10f32.powf((f32::from(v) - 1.0) / 72.0)),
        _ => None,
    }
}

/// A block's place in a frame and its R8 children, sorted.
struct Block {
    offset: usize,
    children: Vec<u64>,
}

/// The blocks of `blocks_<hash>.bin` and a frame's expected length.
#[derive(Clone)]
pub struct Blocks {
    map: Arc<HashMap<u64, Block>>,
    res: h3o::Resolution,
    pub cells: usize,
}

impl Blocks {
    /// `u32` count, then that many `u64` R4 indices (little-endian); cells
    /// at resolution `res` (5..=15).
    pub fn parse(b: &[u8], res: u8) -> Option<Self> {
        let res = h3o::Resolution::try_from(res)
            .ok()
            .filter(|r| *r > BLOCK_RES)?;
        let n = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
        let mut map = HashMap::with_capacity(n);
        let mut offset = 0;
        for i in 0..n {
            let at = 4 + i * 8;
            let id = u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?);
            let cell = h3o::CellIndex::try_from(id).ok()?;
            let mut children: Vec<u64> = cell.children(res).map(u64::from).collect();
            children.sort_unstable();
            let len = children.len();
            map.insert(id, Block { offset, children });
            offset += len;
        }
        Some(Self {
            map: Arc::new(map),
            res,
            cells: offset,
        })
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The cell resolution.
    pub fn res(&self) -> u8 {
        u8::from(self.res)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// A point's byte offset in every frame; `None` outside the blocks.
    pub fn offset(&self, lat: f64, lon: f64) -> Option<usize> {
        self.offset_of(Self::cell(lat, lon, self.res())?)
    }

    /// A point's cell at resolution `res` (cache it: many points share one).
    pub fn cell(lat: f64, lon: f64, res: u8) -> Option<u64> {
        let res = h3o::Resolution::try_from(res).ok()?;
        Some(u64::from(h3o::LatLng::new(lat, lon).ok()?.to_cell(res)))
    }

    /// A cell's byte offset (cells at this index's resolution).
    pub fn offset_of(&self, cell: u64) -> Option<usize> {
        let c = h3o::CellIndex::try_from(cell).ok()?;
        let block = self.map.get(&u64::from(c.parent(BLOCK_RES)?))?;
        let k = block.children.binary_search(&cell).ok()?;
        Some(block.offset + k)
    }
}

/// Unix seconds of a manifest time (`2026-10-09T14:30:00Z`, seconds optional).
pub fn epoch(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, m, d, hh, mm) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?);
    let ss = n(17, 19).unwrap_or(0);
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// One frame with its blocks.
pub struct Frame {
    /// Valid time (UTC, as in the manifest).
    pub time: String,
    pub blocks: Blocks,
    pub values: Vec<u8>,
}

impl Frame {
    /// Rain at a point (mm/h, 0 when dry); `None` outside the blocks or without data.
    pub fn at(&self, lat: f64, lon: f64) -> Option<f32> {
        mm_h(*self.values.get(self.blocks.offset(lat, lon)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_codes() {
        assert_eq!(mm_h(0), Some(0.0));
        assert!((mm_h(1).unwrap() - 0.1).abs() < 1e-6);
        assert!((mm_h(73).unwrap() - 1.0).abs() < 1e-4, "72 steps a decade");
        assert_eq!(mm_h(254), None);
        assert_eq!(mm_h(255), None);
    }

    #[test]
    fn a_point_finds_its_byte() {
        // One block around Konstanz; every child dry except the one under the Marktstätte.
        let cell = h3o::LatLng::new(47.66038, 9.17639)
            .unwrap()
            .to_cell(h3o::Resolution::Eight);
        let block = u64::from(cell.parent(BLOCK_RES).unwrap());
        let mut b = 1u32.to_le_bytes().to_vec();
        b.extend(block.to_le_bytes());
        assert_eq!(Blocks::parse(&b, 7).unwrap().cells, 343);
        assert!(Blocks::parse(&b, 4).is_none());
        let blocks = Blocks::parse(&b, CELL_RES).unwrap();
        assert_eq!(blocks.cells, 2401);
        let mut values = vec![0u8; blocks.cells];
        values[blocks.offset(47.66038, 9.17639).unwrap()] = 73;
        let f = Frame {
            time: "t".into(),
            blocks,
            values,
        };
        assert!((f.at(47.66038, 9.17639).unwrap() - 1.0).abs() < 1e-4);
        assert_eq!(f.at(47.70, 9.30), Some(0.0), "same block, dry");
        assert_eq!(f.at(52.52, 13.40), None, "outside the blocks");
    }
}

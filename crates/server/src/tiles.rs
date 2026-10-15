//! The street graph cut into web-mercator tiles (zoom [`TILE_Z`]) so the app
//! loads only the area in view: a whole city doesn't fit on a phone. Built once
//! from the loaded graph; the gateway serves the tile index and tiles, and the
//! Bevy side sends node state only for the tiles a browser holds.

use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::sync::{Arc, RwLock};

use elements_shared::geo::{GeoEdge, GeoNode, RegionInfo};
use serde_json::json;

/// ~700 m at 53° N, ~1.2 km at the equator.
pub const TILE_Z: u32 = 15;

/// Tile column and row at [`TILE_Z`].
pub type TileKey = (u32, u32);

pub fn tile_of(lat: f64, lon: f64) -> TileKey {
    let n = f64::from(1u32 << TILE_Z);
    let x = ((lon + 180.0) / 360.0 * n).floor();
    let r = lat.to_radians();
    let y = ((1.0 - (r.tan() + 1.0 / r.cos()).ln() / PI) / 2.0 * n).floor();
    (x.clamp(0.0, n - 1.0) as u32, y.clamp(0.0, n - 1.0) as u32)
}

/// Round to 6 decimals (~10 cm) to keep tiles small.
fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

pub struct Tile {
    /// `{"n": [[id, lon, lat, elevation_m], ..], "e": [[id, source, target, [lon, lat, ..]], ..],
    /// "b": [[id, [border node ids], [[lon, lat, ..], ..rings]], ..],
    /// "s": [[id, kind], ..]}` (natural sources of the tile's own nodes, kind 0 fire,
    /// 1 water, 2 spring), gzipped (served as is;
    /// a sixth of the JSON in memory). A block's border nodes come with its tile.
    pub gz: Arc<[u8]>,
    /// Every node the tile carries (its own plus far ends of its edges).
    pub nodes: Vec<i64>,
}

pub struct TileSet {
    /// Region settings plus `tiles: {z, index: [[x, y, nodes], ..]}`.
    pub meta: Arc<str>,
    pub tiles: HashMap<TileKey, Tile>,
    /// Sorted node ids and the tile each lies in.
    ids: Vec<i64>,
    home: Vec<TileKey>,
    /// Tiles with nodes of their own, sorted (the overview's tiles).
    pub home_keys: Vec<TileKey>,
    /// Further tiles that carry a node (far ends of edges at tile borders).
    extra: HashMap<i64, Vec<TileKey>>,
}

/// Filled once the region is loaded.
pub type SharedTiles = Arc<RwLock<Option<Arc<TileSet>>>>;

impl TileSet {
    /// `meta` is the region object the app reads (name, bounds, language, ..);
    /// `lines` are the edge polylines as (lat, lon); blocks with an outline go
    /// to the tile of their outline's mean point. `area_share`: `[area] share`
    /// when blocks are on (the app fills held ones).
    pub fn build(
        region: &RegionInfo,
        meta: serde_json::Value,
        nodes: &[GeoNode],
        edges: &[GeoEdge],
        lines: &[(i64, Vec<(f64, f64)>)],
        blocks: &[crate::area::Block],
        area_share: Option<f32>,
    ) -> Self {
        let pos: HashMap<i64, (f64, f64)> = nodes.iter().map(|n| (n.id, (n.lat, n.lon))).collect();
        let elev: HashMap<i64, f64> = nodes.iter().map(|n| (n.id, n.elevation_m)).collect();
        let natural: HashMap<i64, u8> = nodes
            .iter()
            .filter_map(|n| {
                let k = match n.node_type.as_str() {
                    "fire_source" => 0,
                    "water_source" => 1,
                    "spring_source" => 2,
                    _ => return None,
                };
                Some((n.id, k))
            })
            .collect();
        let home: HashMap<i64, TileKey> = nodes
            .iter()
            .map(|n| (n.id, tile_of(n.lat, n.lon)))
            .collect();
        let line: HashMap<i64, &Vec<(f64, f64)>> = lines.iter().map(|(id, l)| (*id, l)).collect();

        let mut own: HashMap<TileKey, Vec<i64>> = HashMap::new();
        for n in nodes {
            own.entry(home[&n.id]).or_default().push(n.id);
        }
        // An edge belongs to the tile of its middle vertex; its end nodes come along.
        let mut tile_edges: HashMap<TileKey, Vec<&GeoEdge>> = HashMap::new();
        for e in edges {
            let (Some(a), Some(b)) = (pos.get(&e.source), pos.get(&e.target)) else {
                continue;
            };
            let mid = match line.get(&e.id) {
                Some(l) if !l.is_empty() => l[l.len() / 2],
                _ => ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0),
            };
            tile_edges.entry(tile_of(mid.0, mid.1)).or_default().push(e);
        }

        let mut tile_blocks: HashMap<TileKey, Vec<&crate::area::Block>> = HashMap::new();
        if area_share.is_some() {
            for b in blocks {
                let pts: Vec<f64> = b.rings.first().cloned().unwrap_or_default();
                if pts.len() < 2 || !b.nodes.iter().all(|id| pos.contains_key(id)) {
                    continue;
                }
                let k = (pts.len() / 2) as f64;
                let lon = pts.iter().step_by(2).sum::<f64>() / k;
                let lat = pts.iter().skip(1).step_by(2).sum::<f64>() / k;
                tile_blocks.entry(tile_of(lat, lon)).or_default().push(b);
            }
        }

        let keys: HashSet<TileKey> = own
            .keys()
            .chain(tile_edges.keys())
            .chain(tile_blocks.keys())
            .copied()
            .collect();
        let mut tiles = HashMap::with_capacity(keys.len());
        let mut extra: HashMap<i64, Vec<TileKey>> = HashMap::new();
        for key in keys {
            let mut ids: Vec<i64> = own.get(&key).cloned().unwrap_or_default();
            let es = tile_edges.get(&key).map(Vec::as_slice).unwrap_or_default();
            ids.extend(es.iter().flat_map(|e| [e.source, e.target]));
            let bs = tile_blocks.get(&key).map(Vec::as_slice).unwrap_or_default();
            ids.extend(bs.iter().flat_map(|b| b.nodes.iter().copied()));
            ids.sort_unstable();
            ids.dedup();
            let n: Vec<_> = ids
                .iter()
                .map(|id| {
                    let (lat, lon) = pos[id];
                    // Height for the app's flow direction (fire uphill, water downhill).
                    json!([id, r6(lon), r6(lat), (elev[id] * 10.0).round() / 10.0])
                })
                .collect();
            let e: Vec<_> = es
                .iter()
                .map(|e| {
                    let coords: Vec<f64> = match line.get(&e.id) {
                        Some(l) if l.len() >= 2 => l
                            .iter()
                            .flat_map(|&(lat, lon)| [r6(lon), r6(lat)])
                            .collect(),
                        _ => [pos[&e.source], pos[&e.target]]
                            .iter()
                            .flat_map(|&(lat, lon)| [r6(lon), r6(lat)])
                            .collect(),
                    };
                    json!([e.id, e.source, e.target, coords])
                })
                .collect();
            for &id in &ids {
                if home.get(&id) != Some(&key) {
                    extra.entry(id).or_default().push(key);
                }
            }
            let b: Vec<_> = bs
                .iter()
                .map(|b| {
                    let rings: Vec<Vec<f64>> = b
                        .rings
                        .iter()
                        .map(|r| r.iter().map(|&v| r6(v)).collect())
                        .collect();
                    json!([b.id, b.nodes, rings])
                })
                .collect();
            let src: Vec<_> = own
                .get(&key)
                .into_iter()
                .flatten()
                .filter_map(|id| natural.get(id).map(|k| json!([id, k])))
                .collect();
            let mut tile = json!({ "n": n, "e": e });
            if !b.is_empty() {
                tile["b"] = json!(b);
            }
            if !src.is_empty() {
                tile["s"] = json!(src);
            }
            tiles.insert(key, (tile.to_string(), ids));
        }

        let mut index: Vec<_> = own.iter().map(|(&(x, y), n)| (x, y, n.len())).collect();
        index.extend(
            tiles
                .keys()
                .filter(|k| !own.contains_key(k))
                .map(|&(x, y)| (x, y, 0)),
        );
        index.sort_unstable();
        // Tiles are cached by browsers; a new graph (re-import, region swap)
        // gets new tile URLs through this version.
        let mut keys: Vec<_> = tiles.keys().copied().collect();
        keys.sort_unstable();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a
        for k in &keys {
            for b in format!("{k:?}").bytes().chain(tiles[k].0.bytes()) {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        let tiles = tiles
            .into_iter()
            .map(|(k, (json, nodes))| {
                (
                    k,
                    Tile {
                        gz: gzip(json.as_bytes()).into(),
                        nodes,
                    },
                )
            })
            .collect();
        let mut by_id: Vec<(i64, TileKey)> = home.into_iter().collect();
        by_id.sort_unstable();
        let mut home_keys: Vec<TileKey> = own.keys().copied().collect();
        home_keys.sort_unstable();
        let meta = json!({
            "region": meta,
            "nodes": nodes.len(),
            "tiles": { "z": TILE_Z, "index": index, "v": format!("{:012x}", h >> 16) },
            "hotspot": region.hotspot_center.map(|(lat, lon)| [lon, lat]),
            "area": area_share.map(|s| (f64::from(s) * 1000.0).round() / 1000.0),
        });
        Self {
            meta: meta.to_string().into(),
            tiles,
            ids: by_id.iter().map(|p| p.0).collect(),
            home: by_id.iter().map(|p| p.1).collect(),
            home_keys,
            extra,
        }
    }

    /// The tile node `id` lies in.
    pub fn home_of(&self, id: i64) -> Option<TileKey> {
        self.ids.binary_search(&id).ok().map(|i| self.home[i])
    }

    /// Every tile that carries node `id`.
    pub fn tiles_of(&self, id: i64) -> impl Iterator<Item = TileKey> + '_ {
        self.home_of(id)
            .into_iter()
            .chain(self.extra.get(&id).into_iter().flatten().copied())
    }
}

fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).expect("gzip to memory");
    enc.finish().expect("gzip to memory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_of_matches_slippy_map() {
        // Konstanz Münster at z15 (checked against the OSM tile formula).
        assert_eq!(tile_of(47.6633, 9.1757), (17219, 11436));
        assert_eq!(tile_of(0.0, 0.0), (16384, 16384));
    }

    #[test]
    fn a_block_brings_its_border_nodes_to_its_tile() {
        let node = |id, lat, lon| GeoNode {
            id,
            lat,
            lon,
            elevation_m: 400.0,
            node_type: "crossing".into(),
            control_state: String::new(),
            affinity: 0.0,
        };
        // Node 3 lies two tiles north of the block's middle.
        let nodes = [
            node(1, 47.6633, 9.1757),
            node(2, 47.6633, 9.1767),
            node(3, 47.6800, 9.1762),
        ];
        let region = RegionInfo {
            slug: "region-test".into(),
            name: "Test".into(),
            centroid_lat: 47.66,
            centroid_lon: 9.17,
            bounds_min_lat: 47.6,
            bounds_min_lon: 9.1,
            bounds_max_lat: 47.7,
            bounds_max_lon: 9.2,
            hotspot_center: None,
            hotspot_area_m: None,
            timezone: None,
        };
        let block = crate::area::Block {
            id: 5,
            area: 1000.0,
            nodes: vec![1, 2, 3],
            rings: vec![vec![
                9.1757, 47.6633, 9.1767, 47.6633, 9.1762, 47.6640, 9.1757, 47.6633,
            ]],
        };
        let set = TileSet::build(
            &region,
            json!({}),
            &nodes,
            &[],
            &[],
            std::slice::from_ref(&block),
            Some(0.9),
        );
        let home = tile_of(47.6635, 9.1761);
        assert!(set.tiles[&home].nodes.contains(&3));
        assert!(set.tiles_of(3).any(|k| k == home));
        let mut json = String::new();
        std::io::Read::read_to_string(
            &mut flate2::read::GzDecoder::new(&set.tiles[&home].gz[..]),
            &mut json,
        )
        .unwrap();
        let t: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(t["b"][0][0], 5);
        assert_eq!(t["b"][0][1], json!([1, 2, 3]));
        assert!(set.meta.contains("\"area\":0.9"));
        // Off: no blocks in tiles.
        let off = TileSet::build(&region, json!({}), &nodes, &[], &[], &[block], None);
        assert!(!off.tiles[&home].nodes.contains(&3));
    }
}

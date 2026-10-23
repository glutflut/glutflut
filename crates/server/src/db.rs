//! One-shot load of a region's road graph from PostGIS (`geo.*`) into a plain
//! [`RegionGraph`]. The Bevy spawn step lives in `main`; this module is just the
//! database boundary.
//!
//! Synchronous on purpose: it runs once at `Startup`, the server has no async
//! runtime, and the blocking `postgres` client owns its own connection thread
//! for the brief load. Geometry never crosses into Rust — nodes store
//! denormalized `lat`/`lon`, and the region centroid is read with `ST_Y`/`ST_X`
//! as plain doubles — so there is no WKB decoding and no client-side PostGIS
//! dependency.

use std::error::Error;

use elements_shared::geo::{GeoEdge, GeoNode, RegionGraph, RegionInfo};
use postgres::Client;
use postgres_native_tls::MakeTlsConnector;
use tracing::{error, warn};

/// Connect to `conn_str` and load the active region identified by `slug`,
/// together with all of its active nodes and edges.
///
/// Returns an empty [`RegionGraph`] (with `region == None`) if no active region
/// matches the slug — distinct from a connection/query error, which is surfaced
/// to the caller.
pub fn load_region(conn_str: &str, slug: &str) -> Result<RegionGraph, Box<dyn Error>> {
    let connector = MakeTlsConnector::new(native_tls::TlsConnector::new()?);
    let mut client = Client::connect(conn_str, connector)?;

    let region = client
        .query_opt(
            "SELECT slug, name, \
                    ST_Y(centroid_geom) AS centroid_lat, \
                    ST_X(centroid_geom) AS centroid_lon, \
                    bounds_min_lat, bounds_min_lon, bounds_max_lat, bounds_max_lon, \
                    hotspot_lat, hotspot_lon, hotspot_area_m, timezone \
             FROM geo.regions \
             WHERE slug = $1 AND active = true",
            &[&slug],
        )?
        .map(|r| RegionInfo {
            slug: r.get("slug"),
            name: r.get("name"),
            centroid_lat: r.get("centroid_lat"),
            centroid_lon: r.get("centroid_lon"),
            bounds_min_lat: r.get("bounds_min_lat"),
            bounds_min_lon: r.get("bounds_min_lon"),
            bounds_max_lat: r.get("bounds_max_lat"),
            bounds_max_lon: r.get("bounds_max_lon"),
            hotspot_center: r
                .get::<_, Option<f64>>("hotspot_lat")
                .zip(r.get::<_, Option<f64>>("hotspot_lon")),
            hotspot_area_m: r.get("hotspot_area_m"),
            timezone: r.get("timezone"),
        });

    // No such active region: hand back an empty graph for the caller to log.
    if region.is_none() {
        return Ok(RegionGraph::default());
    }

    let nodes = client
        .query(
            "SELECT n.id, n.lat, n.lon, n.elevation_m, n.node_type, n.control_state, n.affinity \
             FROM geo.nodes n \
             JOIN geo.regions r ON r.id = n.region_id \
             WHERE r.slug = $1 AND n.is_active = true \
             ORDER BY n.id",
            &[&slug],
        )?
        .into_iter()
        .map(|r| GeoNode {
            id: r.get("id"),
            lat: r.get("lat"),
            lon: r.get("lon"),
            elevation_m: r.get("elevation_m"),
            node_type: r.get("node_type"),
            control_state: r.get("control_state"),
            affinity: r.get("affinity"),
        })
        .collect();

    let edges = client
        .query(
            "SELECT e.id, e.source, e.target, e.length_m, e.road_type, e.one_way, \
                    COALESCE(e.fire_cost_forward,  e.length_m) AS fcf, \
                    COALESCE(e.fire_cost_reverse,  e.length_m) AS fcr, \
                    COALESCE(e.water_cost_forward, e.length_m) AS wcf, \
                    COALESCE(e.water_cost_reverse, e.length_m) AS wcr \
             FROM geo.edges e \
             JOIN geo.regions r ON r.id = e.region_id \
             WHERE r.slug = $1 AND e.is_active = true \
             ORDER BY e.id",
            &[&slug],
        )?
        .into_iter()
        .map(|r| GeoEdge {
            id: r.get("id"),
            source: r.get("source"),
            target: r.get("target"),
            length_m: r.get("length_m"),
            road_type: r.get("road_type"),
            one_way: r.get("one_way"),
            // Columns are `double precision`; the wire/sim use f32.
            fire_cost_forward: r.get::<_, f64>("fcf") as f32,
            fire_cost_reverse: r.get::<_, f64>("fcr") as f32,
            water_cost_forward: r.get::<_, f64>("wcf") as f32,
            water_cost_reverse: r.get::<_, f64>("wcr") as f32,
        })
        .collect();

    Ok(RegionGraph {
        region,
        nodes,
        edges,
    })
}

/// Everything a region app loads from the database; `None` (logged) if the
/// graph can't be read or the region isn't active. Lines, settings and zones
/// degrade to empty.
pub fn load_region_data(conn_str: &str, slug: &str) -> Option<crate::RegionData> {
    let mut graph = match load_region(conn_str, slug) {
        Ok(g) => g,
        Err(e) => {
            error!(error = %e, region = %slug, "failed to load region graph; starting empty");
            return None;
        }
    };
    let Some(region) = graph.region.take() else {
        warn!(region = %slug, "no active region with that slug — starting empty");
        return None;
    };
    let lines = load_edge_lines(conn_str, slug).unwrap_or_else(|e| {
        error!(error = %e, "failed to load edge polylines; analytics disabled");
        Vec::new()
    });
    let meta = load_region_meta(conn_str, slug)
        .map_err(|e| error!(error = %e, "failed to load region settings for the web gateway"))
        .ok();
    let places = connect(conn_str)
        .and_then(|mut c| Ok(crate::places::load(&mut c, slug)?))
        .unwrap_or_else(|e| {
            warn!(error = %e, "no zones or hotspot sites; flows disabled");
            Default::default()
        });
    let finds = crate::finds::Finds::load(conn_str, &region);
    let blocks = load_blocks(conn_str, slug).unwrap_or_else(|e| {
        warn!(error = %e, "no blocks");
        Vec::new()
    });
    let tide = load_tide(conn_str, slug).unwrap_or_else(|e| {
        warn!(error = %e, "no tide data");
        None
    });
    Some(crate::RegionData {
        graph,
        region,
        lines,
        meta,
        places,
        finds,
        blocks,
        tiles: None,
        tide,
    })
}

/// The region's gauge and its junctions near tidal water; `None`
/// without a gauge.
fn load_tide(conn_str: &str, slug: &str) -> Result<Option<crate::tide::Setup>, Box<dyn Error>> {
    let mut c = connect(conn_str)?;
    let Some(r) = c.query_opt(
        "SELECT source, station, mlw, mhw FROM geo.region_tide WHERE region = $1",
        &[&slug],
    )?
    else {
        return Ok(None);
    };
    let dist = c
        .query(
            "SELECT t.node_id, t.dist_m FROM geo.node_tide t JOIN geo.nodes n ON n.id = t.node_id \
             JOIN geo.regions g ON g.id = n.region_id WHERE g.slug = $1 AND n.is_active",
            &[&slug],
        )?
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, f32>(1)))
        .collect();
    Ok(Some(crate::tide::Setup {
        source: r.get(0),
        station: r.get(1),
        mlw: r.get(2),
        mhw: r.get(3),
        dist,
    }))
}

/// Region metadata plus node and edge `FeatureCollection`s as one JSON document
/// for the web client, built entirely in PostGIS.
pub fn load_region_json(conn_str: &str, slug: &str) -> Result<String, Box<dyn Error>> {
    let connector = MakeTlsConnector::new(native_tls::TlsConnector::new()?);
    let mut client = Client::connect(conn_str, connector)?;
    let row = client.query_one(
        "WITH r AS (SELECT * FROM geo.regions WHERE slug = $1 AND active = true) \
         SELECT json_build_object( \
           'region', (SELECT json_build_object( \
               'slug', slug, 'name', name, 'demo_label', demo_label, 'demo_day', demo_day::text, 'language', language, \
               'centroid', json_build_array(ST_X(centroid_geom), ST_Y(centroid_geom)), \
               'bounds', json_build_array(bounds_min_lon, bounds_min_lat, bounds_max_lon, bounds_max_lat)) \
             FROM r), \
           'nodes', json_build_object('type', 'FeatureCollection', 'features', COALESCE( \
             (SELECT json_agg(json_build_object('type', 'Feature', 'id', n.id, \
                 'geometry', ST_AsGeoJSON(n.geom, 6)::json, \
                 'properties', json_build_object('id', n.id, 'elev', round(n.elevation_m::numeric, 1)))) \
              FROM geo.nodes n JOIN r ON r.id = n.region_id WHERE n.is_active), '[]'::json)), \
           'edges', json_build_object('type', 'FeatureCollection', 'features', COALESCE( \
             (SELECT json_agg(json_build_object('type', 'Feature', 'id', e.id, \
                 'geometry', ST_AsGeoJSON(e.geom, 6)::json, \
                 'properties', json_build_object('id', e.id, 'source', e.source, 'target', e.target))) \
              FROM geo.edges e JOIN r ON r.id = e.region_id WHERE e.is_active), '[]'::json)) \
         )::text",
        &[&slug],
    )?;
    Ok(row.get(0))
}

/// The region object of [`load_region_json`] alone (settings the pages read).
pub fn load_region_meta(conn_str: &str, slug: &str) -> Result<serde_json::Value, Box<dyn Error>> {
    let mut client = connect(conn_str)?;
    let row = client.query_one(
        "SELECT json_build_object( \
           'slug', slug, 'name', name, 'demo_label', demo_label, 'demo_day', demo_day::text, 'language', language, \
           'centroid', json_build_array(ST_X(centroid_geom), ST_Y(centroid_geom)), \
           'bounds', json_build_array(bounds_min_lon, bounds_min_lat, bounds_max_lon, bounds_max_lat))::text \
         FROM geo.regions WHERE slug = $1 AND active = true",
        &[&slug],
    )?;
    Ok(serde_json::from_str(row.get(0))?)
}

/// An active region as the region list shows it.
#[derive(Clone)]
pub struct Listing {
    pub slug: String,
    pub shard: String,
    /// Home cell within the shard (`geo.regions.cell`).
    pub cell: String,
    /// Rule set name (file in the rules directory), if not the default.
    pub rules: Option<String>,
    pub demo_day: Option<String>,
    /// `{slug, name, city, language, demo_label, listed, city_in, bounds, outline}` for `/api/regions`.
    pub json: serde_json::Value,
}

/// All active regions (every shard), outlines simplified to ~50 m.
/// The other cells of `shard` registered in `ops.cells`.
pub fn registered_cells(
    conn_str: &str,
    shard: &str,
    cell: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut c = connect(conn_str)?;
    Ok(c.query(
        "SELECT cell FROM ops.cells WHERE shard = $1 AND cell <> $2",
        &[&shard, &cell],
    )?
    .iter()
    .map(|r| r.get(0))
    .collect())
}

pub fn list_regions(conn_str: &str) -> Result<Vec<Listing>, Box<dyn Error>> {
    let mut client = connect(conn_str)?;
    let rows = client.query(
        "SELECT slug, shard, cell, rules, demo_day::text, json_build_object( \
           'slug', slug, 'name', name, 'city', split_part(name, ' – ', 1), \
           'language', language, 'demo_label', demo_label, 'listed', listed, 'city_in', city_in, \
           'bounds', json_build_array(bounds_min_lon, bounds_min_lat, bounds_max_lon, bounds_max_lat), \
           'outline', ST_AsGeoJSON(ST_SimplifyPreserveTopology( \
               COALESCE(boundary_geom, ST_MakeEnvelope(bounds_min_lon, bounds_min_lat, \
                                                       bounds_max_lon, bounds_max_lat, 4326)), \
               0.0005), 5)::json)::text \
         FROM geo.regions WHERE active = true ORDER BY name",
        &[],
    )?;
    rows.into_iter()
        .map(|r| {
            Ok(Listing {
                slug: r.get(0),
                shard: r.get(1),
                cell: r.get(2),
                rules: r.get(3),
                demo_day: r.get(4),
                json: serde_json::from_str(r.get(5))?,
            })
        })
        .collect()
}

/// Active junctions per region (the pool's junction budget), for these slugs.
pub fn junction_counts(
    conn_str: &str,
    slugs: &[String],
) -> Result<Vec<(String, u64)>, Box<dyn Error>> {
    let mut client = connect(conn_str)?;
    let rows = client.query(
        "SELECT r.slug, count(n.id) FROM geo.regions r \
         LEFT JOIN geo.nodes n ON n.region_id = r.id AND n.is_active \
         WHERE r.slug = ANY($1) GROUP BY r.slug",
        &[&slugs],
    )?;
    Ok(rows
        .iter()
        .map(|r| (r.get(0), r.get::<_, i64>(1) as u64))
        .collect())
}

/// One of the region's finds as stored (`geo.finds`).
#[derive(Clone, Debug)]
pub struct FindRow {
    pub id: i64,
    pub collection: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// `{lang: text}`: a letter shown when found.
    pub text: Option<serde_json::Value>,
    /// Plays only while this campaign runs.
    pub campaign: Option<String>,
}

/// The region's visible finds.
pub fn find_rows(c: &mut Client, slug: &str) -> Result<Vec<FindRow>, postgres::Error> {
    Ok(c.query(
        "SELECT id, collection, name, lat, lon, text::text, campaign \
         FROM geo.finds WHERE region = $1 AND NOT hidden",
        &[&slug],
    )?
    .into_iter()
    .map(|r| FindRow {
        id: r.get(0),
        collection: r.get(1),
        name: r.get(2),
        lat: r.get(3),
        lon: r.get(4),
        text: r
            .get::<_, Option<String>>(5)
            .and_then(|s| serde_json::from_str(&s).ok()),
        campaign: r.get(6),
    })
    .collect())
}

/// The region's blocks (`geo.blocks`), outlines simplified to ~1 m.
pub fn load_blocks(conn_str: &str, slug: &str) -> Result<Vec<crate::area::Block>, Box<dyn Error>> {
    let mut c = connect(conn_str)?;
    let rows = c.query(
        "SELECT id, area_m2, nodes, \
                (ST_AsGeoJSON(ST_SimplifyPreserveTopology(geom, 0.00001), 6)::json -> 'coordinates')::text \
         FROM geo.blocks WHERE region = $1 ORDER BY id",
        &[&slug],
    )?;
    Ok(rows
        .iter()
        .map(|r| {
            let coords: serde_json::Value =
                serde_json::from_str(r.get(3)).unwrap_or(serde_json::Value::Null);
            // MultiPolygon: polygons -> rings -> [lon, lat]; the simplified
            // single polygon comes back as a Polygon.
            let coords = if coords[0][0][0].is_number() {
                serde_json::Value::Array(vec![coords])
            } else {
                coords
            };
            let rings = coords
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_array())
                .flatten()
                .filter_map(|ring| ring.as_array())
                .map(|ring| {
                    ring.iter()
                        .filter_map(|pt| Some([pt.get(0)?.as_f64()?, pt.get(1)?.as_f64()?]))
                        .flatten()
                        .collect()
                })
                .collect();
            crate::area::Block {
                id: r.get(0),
                area: r.get(1),
                nodes: r.get(2),
                rings,
            }
        })
        .collect())
}

pub fn load_finds(conn_str: &str, slug: &str) -> Result<Vec<FindRow>, Box<dyn Error>> {
    let mut c = connect(conn_str)?;
    Ok(find_rows(&mut c, slug)?)
}

/// Open a connection (TLS per `sslmode` in the string).
pub fn connect(conn_str: &str) -> Result<Client, Box<dyn Error>> {
    let connector = MakeTlsConnector::new(native_tls::TlsConnector::new()?);
    let mut cfg: postgres::Config = conn_str.parse()?;
    // A connection that died silently (Neon restarting its compute) fails
    // within about a minute instead of blocking its thread forever.
    cfg.connect_timeout(std::time::Duration::from_secs(20))
        .keepalives(true)
        .keepalives_idle(std::time::Duration::from_secs(30))
        .keepalives_interval(std::time::Duration::from_secs(10))
        .keepalives_retries(3)
        .tcp_user_timeout(std::time::Duration::from_secs(60));
    Ok(cfg.connect(connector)?)
}

/// A worker thread's connection: opened when needed, dropped after an error
/// so the next use reconnects. Errors are logged as "`what`: ...".
pub struct Reconnecting {
    what: &'static str,
    client: Option<Client>,
}

impl Reconnecting {
    pub const fn new(what: &'static str) -> Self {
        Self { what, client: None }
    }

    /// The open connection to `url`; `None` if connecting fails.
    pub fn get(&mut self, url: &str) -> Option<&mut Client> {
        if self.client.as_ref().is_none_or(|c| c.is_closed()) {
            self.client = connect(url)
                .map_err(|e| tracing::error!(error = %e, "{}: connect failed", self.what))
                .ok();
        }
        self.client.as_mut()
    }

    /// The connection if one is open; never opens one (health checks).
    pub fn open(&mut self) -> Option<&mut Client> {
        self.client.as_mut().filter(|c| !c.is_closed())
    }

    /// After a failed query: log it, reconnect on the next `get`.
    pub fn failed(&mut self, doing: &str, e: &dyn std::fmt::Display) {
        tracing::error!(error = %e, "{}: {doing} failed", self.what);
        self.client = None;
    }
}

/// Demo day when neither the region nor `ELEMENTS_DEMO_DAY` sets one.
pub const DEFAULT_DEMO_DAY: &str = "2025-12-06";

/// The region's demo day (`geo.regions.demo_day`), if set.
pub fn region_demo_day(conn_str: &str, slug: &str) -> Option<String> {
    let mut c = connect(conn_str).ok()?;
    c.query_opt(
        "SELECT demo_day::text FROM geo.regions WHERE slug = $1",
        &[&slug],
    )
    .ok()??
    .get(0)
}

/// Places need this many ratings in the period to be published.
pub const MIN_RATINGS: i64 = 3;

/// Cell-key noise (as the Australian Bureau of Statistics does it): every
/// published person count gets a fixed offset in -2..=2, derived from the
/// row's key and the secret `ELEMENTS_NOISE_KEY`. The same row always gets the
/// same offset, so repeated queries don't average it out; subtracting nested
/// or overlapping counts only yields a range. Sums over many rows stay close.
/// Stored counts stay exact; noise is added on the way out.
pub(crate) fn noisy(col: &str, key: &str) -> String {
    format!(
        "({col} + (ARRAY[-2,-1,-1,0,0,0,0,1,1,2])[1 + ((('x' || substr(md5({key} || \
         current_setting('elements.noise_key')), 1, 8))::bit(32)::int % 10) + 10) % 10])"
    )
}

/// `col` (seconds of `persons`) scaled like the noisy person count.
fn scaled(col: &str, persons: &str, noisy_persons: &str) -> String {
    format!("round({col}::numeric * {noisy_persons} / {persons})")
}

/// Row keys for [`noisy`]; `t` is a table alias prefix like `"d."` or `""`.
/// Pooled leftovers add their span, so window rows keep their noise.
fn dwell_key(t: &str) -> String {
    format!(
        "'d|' || {t}region || '|' || {t}synthetic || '|' || {t}edge_id || '|' || \
         extract(epoch FROM {t}window_start)::bigint || \
         CASE WHEN {t}span_s >= 3600 THEN '|' || {t}span_s ELSE '' END"
    )
}
const HEX_KEY: &str = "'h|' || region || '|' || synthetic || '|' || res || '|' || cell || '|' || \
     extract(epoch FROM hour_start)::bigint || CASE WHEN span_s >= 86400 THEN '|' || span_s ELSE '' END";
/// Routes per hour and pooled days as one noisy set: region,
/// from_zone, to_zone, `t` (start), span_s, synthetic, persons.
pub(crate) fn flows_all() -> String {
    const DAY_KEY: &str =
        "'fd|' || region || '|' || synthetic || '|' || from_zone || '|' || to_zone || '|' || day";
    format!(
        "(SELECT region, from_zone, to_zone, hour_start AS t, 3600 AS span_s, synthetic, {} AS persons \
           FROM analytics.flow \
         UNION ALL SELECT region, from_zone, to_zone, day::timestamptz, 86400, synthetic, {} \
           FROM analytics.flow_day)",
        noisy("persons", FLOW_KEY),
        noisy("persons", DAY_KEY)
    )
}

/// Visitors at a focus place (`analytics.place_visits`).
pub(crate) const PLACE_KEY: &str =
    "'p|' || region || '|' || synthetic || '|' || zone_id || '|' || \
     extract(epoch FROM start)::bigint || '|' || span_s";
const FLOW_KEY: &str =
    "'f|' || region || '|' || synthetic || '|' || from_zone || '|' || to_zone || '|' || \
     extract(epoch FROM hour_start)::bigint";

/// Session setting read by [`noisy`]. Without `ELEMENTS_NOISE_KEY` the noise
/// is predictable (fine for dev, not for published data).
pub(crate) fn set_noise_key(c: &mut Client) -> Result<(), Box<dyn Error>> {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let key = KEY.get_or_init(|| {
        std::env::var("ELEMENTS_NOISE_KEY").unwrap_or_else(|_| {
            tracing::warn!("ELEMENTS_NOISE_KEY not set: published noise is predictable");
            String::new()
        })
    });
    c.execute("SELECT set_config('elements.noise_key', $1, false)", &[key])?;
    Ok(())
}

/// Suppress marginal cells below k, and at least two if any, so a hidden
/// cell can't be recovered from the total.
fn suppress_marginal(cells: &mut [(String, Option<i64>)], k: i64) {
    let small = cells.iter().filter(|c| c.1.is_some_and(|n| n < k)).count();
    if small == 0 {
        return;
    }
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by_key(|&i| cells[i].1.unwrap_or(0));
    for (rank, &i) in order.iter().enumerate() {
        if cells[i].1.is_some_and(|n| n < k) || rank < 2 {
            cells[i].1 = None;
        }
    }
}

/// Edge polylines as (edge id, [(lat, lon)]) for position snapping.
pub fn load_edge_lines(
    conn_str: &str,
    slug: &str,
) -> Result<Vec<(i64, Vec<(f64, f64)>)>, Box<dyn Error>> {
    let mut client = connect(conn_str)?;
    let rows = client.query(
        "SELECT e.id, ST_AsGeoJSON(e.geom, 7) FROM geo.edges e \
         JOIN geo.regions r ON r.id = e.region_id \
         WHERE r.slug = $1 AND e.is_active = true",
        &[&slug],
    )?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let geo: serde_json::Value = serde_json::from_str(r.get(1))?;
        let pts = geo["coordinates"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| Some((c[1].as_f64()?, c[0].as_f64()?)))
                    .collect()
            })
            .unwrap_or_default();
        out.push((r.get(0), pts));
    }
    Ok(out)
}

/// Time range of a dashboard query: the last N days, or one UTC day
/// (`YYYY-MM-DD`, e.g. the demo day). Bound as `$2` (text).
#[derive(Clone, Debug)]
pub enum Period {
    Days(i32),
    Day(String),
}

/// Longest period of the `crowdflow.json` export, in days.
const CROWDFLOW_MAX_DAYS: i32 = 7;

/// Start of today in the region's time zone (the session's, see
/// `set_region_tz`), as timestamptz and as date.
const TODAY: &str = "(date_trunc('day', now() AT TIME ZONE current_setting('TimeZone')) \
     AT TIME ZONE current_setting('TimeZone'))";
const TODAY_DATE: &str = "(now() AT TIME ZONE current_setting('TimeZone'))::date";

/// The session's time zone = the region's (`geo.regions.timezone`), so local
/// hours and days follow the region.
fn set_region_tz(c: &mut Client, region: &str) -> Result<(), Box<dyn Error>> {
    c.execute(
        "SELECT set_config('TimeZone', coalesce( \
           (SELECT timezone FROM geo.regions WHERE slug = $1), 'Europe/Berlin'), false)",
        &[&region],
    )?;
    Ok(())
}

impl Period {
    /// A `YYYY-MM-DD` day, if well-formed.
    pub fn day(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        let ok = b.len() == 10
            && b.iter().enumerate().all(|(i, c)| {
                if i == 4 || i == 7 {
                    *c == b'-'
                } else {
                    c.is_ascii_digit()
                }
            });
        ok.then(|| Period::Day(s.to_string()))
    }

    fn param(&self) -> String {
        match self {
            Period::Days(n) => n.to_string(),
            Period::Day(d) => d.clone(),
        }
    }

    /// Condition on a timestamptz column.
    /// `released`: real data is only shown for complete past days (region time),
    /// so nobody can watch today's crowd live; `Days(n)` are then the n days
    /// before today.
    fn ts(&self, col: &str, released: bool) -> String {
        match self {
            Period::Days(_) if released => format!(
                "{col} >= {TODAY} - make_interval(days => $2::text::int) AND {col} < {TODAY}"
            ),
            Period::Days(_) => format!("{col} > now() - make_interval(days => $2::text::int)"),
            // A local day: the session runs in the region's time zone.
            Period::Day(_) => format!(
                "{col} >= ($2::text::date)::timestamptz \
                 AND {col} < ($2::text::date + 1)::timestamptz{}",
                if released {
                    format!(" AND {col} < {TODAY}")
                } else {
                    String::new()
                }
            ),
        }
    }

    /// Condition on a date column.
    fn date(&self, col: &str, released: bool) -> String {
        match self {
            Period::Days(_) if released => {
                format!("{col} >= {TODAY_DATE} - $2::text::int AND {col} < {TODAY_DATE}")
            }
            Period::Days(_) => {
                format!("{col} > (now() - make_interval(days => $2::text::int))::date")
            }
            Period::Day(_) => format!(
                "{col} = $2::text::date{}",
                if released {
                    format!(" AND {col} < {TODAY_DATE}")
                } else {
                    String::new()
                }
            ),
        }
    }
}

/// Aggregates for the city dashboard.
pub fn analytics_summary(
    conn_str: &str,
    region: &str,
    period: &Period,
    synthetic: &[bool],
) -> Result<serde_json::Value, Box<dyn Error>> {
    // Real data (not only synthetic) is released the day after.
    let rel = synthetic.contains(&false);
    use serde_json::json;
    let mut c = connect(conn_str)?;
    set_noise_key(&mut c)?;
    set_region_tz(&mut c, region)?;
    let k = crate::analytics::k_min() as i64;
    let p = period.param();
    let win = period.ts("window_start", rel);
    let dk = dwell_key("");
    let persons = noisy("persons", &dk);
    let dwell: Vec<_> = c
        .query(
            &format!(
                "SELECT edge_id, sum({persons})::bigint, sum({})::bigint, sum({})::bigint \
                 FROM analytics.dwell \
                 WHERE region = $1 AND {win} AND synthetic = ANY($3) GROUP BY edge_id",
                scaled("dwell_s", "persons", &persons),
                scaled("hotspot_dwell_s", "persons", &persons),
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| {
            json!([
                r.get::<_, i64>(0),
                r.get::<_, i64>(1),
                r.get::<_, i64>(2),
                r.get::<_, i64>(3)
            ])
        })
        .collect();
    let hourly: Vec<_> = c
        .query(
            &format!(
                "SELECT extract(hour FROM window_start AT TIME ZONE current_setting('TimeZone'))::int, sum({persons})::bigint \
                 FROM analytics.dwell WHERE region = $1 AND {win} AND synthetic = ANY($3) AND span_s < 86400 \
                 GROUP BY 1 ORDER BY 1"
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| json!([r.get::<_, i32>(0), r.get::<_, i64>(1)]))
        .collect();
    let perception: Vec<_> = c
        .query(
            &format!(
                "SELECT node_id, category, n FROM ( \
                   SELECT node_id, category, sum(count)::bigint n, \
                          sum(sum(count)) OVER (PARTITION BY node_id) total \
                   FROM analytics.perception \
                   WHERE region = $1 AND {} AND synthetic = ANY($3) \
                   GROUP BY node_id, category) t WHERE total >= {MIN_RATINGS}",
                period.date("day", rel)
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| {
            json!([
                r.get::<_, i64>(0),
                r.get::<_, String>(1),
                r.get::<_, i64>(2)
            ])
        })
        .collect();
    // By local hour (rows with hour -1, from before hours were kept, are left out); a place
    // needs MIN_RATINGS in that hour, the city-wide profile per hour too.
    const LOCAL_HOUR: &str =
        "extract(hour FROM ((day + make_interval(hours => hour)) AT TIME ZONE 'UTC') \
         AT TIME ZONE current_setting('TimeZone'))::int";
    let perception_hourly: Vec<_> = c
        .query(
            &format!(
                "SELECT node_id, category, h, n FROM ( \
                   SELECT node_id, category, {LOCAL_HOUR} h, sum(count)::bigint n, \
                          sum(sum(count)) OVER (PARTITION BY node_id, {LOCAL_HOUR}) total \
                   FROM analytics.perception \
                   WHERE region = $1 AND {} AND synthetic = ANY($3) AND hour >= 0 \
                   GROUP BY node_id, category, h) t WHERE total >= {MIN_RATINGS}",
                period.date("day", rel)
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| {
            json!([
                r.get::<_, i64>(0),
                r.get::<_, String>(1),
                r.get::<_, i32>(2),
                r.get::<_, i64>(3)
            ])
        })
        .collect();
    let perception_by_hour: Vec<_> = c
        .query(
            &format!(
                "SELECT h, category, n FROM ( \
                   SELECT {LOCAL_HOUR} h, category, sum(count)::bigint n, \
                          sum(sum(count)) OVER (PARTITION BY {LOCAL_HOUR}) total \
                   FROM analytics.perception \
                   WHERE region = $1 AND {} AND synthetic = ANY($3) AND hour >= 0 \
                   GROUP BY h, category) t WHERE total >= {MIN_RATINGS} ORDER BY 1",
                period.date("day", rel)
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| {
            json!([
                r.get::<_, i32>(0),
                r.get::<_, String>(1),
                r.get::<_, i64>(2)
            ])
        })
        .collect();
    let slot = period.ts("slot_start", rel);
    let experiment: Vec<_> = c
        .query(
            &format!(
                "SELECT active, count(*)::bigint, sum(nearby)::bigint, sum(reached)::bigint, sum(reached_nearby)::bigint \
                 FROM analytics.site_visits \
                 WHERE region = $1 AND {slot} AND synthetic = ANY($3) GROUP BY active \
                 HAVING sum(nearby) >= {k} AND sum(reached) >= {k}"
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| {
            json!({"active": r.get::<_, bool>(0), "slots": r.get::<_, i64>(1),
                   "nearby": r.get::<_, i64>(2), "reached": r.get::<_, i64>(3),
                   "reached_nearby": r.get::<_, i64>(4)})
        })
        .collect();
    let hourly_edges: Vec<_> = c
        .query(
            &format!(
                "SELECT edge_id, extract(hour FROM window_start AT TIME ZONE current_setting('TimeZone'))::int, sum({persons})::bigint \
                 FROM analytics.dwell WHERE region = $1 AND {win} AND synthetic = ANY($3) AND span_s < 86400 GROUP BY 1, 2"
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| json!([r.get::<_, i64>(0), r.get::<_, i32>(1), r.get::<_, i64>(2)]))
        .collect();
    let zones: Vec<_> = c
        .query("SELECT id, name, lat, lon, focus, coalesce(radius_m, 400)::float8 FROM geo.zones WHERE region = $1 ORDER BY id", &[&region])?
        .iter()
        .map(|r| json!({"id": r.get::<_, String>(0), "name": r.get::<_, String>(1), "lat": r.get::<_, f64>(2), "lon": r.get::<_, f64>(3), "focus": r.get::<_, bool>(4), "radius_m": r.get::<_, f64>(5)}))
        .collect();
    let flows: Vec<_> = c
        .query(
            &format!(
                "SELECT from_zone, to_zone, sum(persons)::bigint FROM {} f \
                 WHERE region = $1 AND {} AND synthetic = ANY($3) GROUP BY 1, 2 ORDER BY 3 DESC",
                flows_all(),
                period.ts("t", rel)
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| {
            json!([
                r.get::<_, String>(0),
                r.get::<_, String>(1),
                r.get::<_, i64>(2)
            ])
        })
        .collect();
    let daily: Vec<_> = c
        .query(
            &format!(
                "SELECT day::text, sum(players)::bigint, sum(newcomers)::bigint, sum(flagged)::bigint FROM analytics.daily \
                 WHERE region = $1 AND {} AND synthetic = ANY($3) \
                 GROUP BY 1 ORDER BY 1",
                period.date("day", rel)
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| json!([r.get::<_, String>(0), r.get::<_, i64>(1), r.get::<_, i64>(2), r.get::<_, i64>(3)]))
        .collect();
    // Player mix: person-days per answer, one marginal at a time (never the
    // cross table), cells below k suppressed.
    let mut marginal = |col: &str, keys: &[&str]| -> Result<serde_json::Value, Box<dyn Error>> {
        let rows = c.query(
            &format!(
                "SELECT {col}, sum(players)::bigint FROM analytics.daily_profile \
                 WHERE region = $1 AND {} AND synthetic = ANY($3) GROUP BY 1",
                period.date("day", rel)
            ),
            &[&region, &p, &synthetic],
        )?;
        let mut cells: Vec<(String, Option<i64>)> = keys
            .iter()
            .map(|key| {
                let n = rows
                    .iter()
                    .find(|r| r.get::<_, String>(0) == *key)
                    .map_or(0, |r| r.get::<_, i64>(1));
                (key.to_string(), Some(n))
            })
            .collect();
        if cells.iter().any(|c| c.1 != Some(0)) {
            suppress_marginal(&mut cells, k);
        }
        Ok(json!(cells))
    };
    let profiles = json!({
        "age": marginal("age", &["u30", "30_59", "60p", "na"])?,
        "local": marginal("local", &["yes", "no", "na"])?,
    });
    let sup = c.query_one(
        &format!(
            "SELECT coalesce(sum(kept), 0)::bigint, coalesce(sum(dropped), 0)::bigint FROM analytics.suppressed \
             WHERE region = $1 AND {win} AND synthetic = ANY($3)"
        ),
        &[&region, &p, &synthetic],
    )?;
    let hexes = hex_summary(&mut c, region, period, synthetic)?;
    // Positions of rated places and the extent of segments with data, so the
    // dashboard needs no whole graph (a city loads by tiles).
    let ids = |rows: &[serde_json::Value]| -> Vec<i64> {
        let mut v: Vec<i64> = rows.iter().filter_map(|r| r[0].as_i64()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let places: serde_json::Map<String, serde_json::Value> = c
        .query(
            "SELECT id, lon, lat FROM geo.nodes WHERE id = ANY($1)",
            &[&ids(&perception)],
        )?
        .iter()
        .map(|r| {
            (
                r.get::<_, i64>(0).to_string(),
                json!([r.get::<_, f64>(1), r.get::<_, f64>(2)]),
            )
        })
        .collect();
    let b = c.query_one(
        "SELECT ST_XMin(x), ST_YMin(x), ST_XMax(x), ST_YMax(x) \
         FROM (SELECT ST_Extent(geom)::geometry x FROM geo.edges WHERE id = ANY($1)) q",
        &[&ids(&dwell)],
    )?;
    let dwell_bounds = b.get::<_, Option<f64>>(0).map(|w| {
        json!([
            w,
            b.get::<_, f64>(1),
            b.get::<_, f64>(2),
            b.get::<_, f64>(3)
        ])
    });
    // Growth (counts only, not about places or hours, so also today) and the
    // most persons at once (a quarter hour, released like the rest).
    let growth: Vec<_> = c
        .query(
            &format!(
                "SELECT to_char(day, 'YYYY-MM-DD'), players, new, multi, total FROM analytics.growth_day \
                 WHERE region = $1 AND ({}) AND false = ANY($3) ORDER BY day",
                period.date("day", false)
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| {
            json!([
                r.get::<_, String>(0),
                r.get::<_, i32>(1),
                r.get::<_, i32>(2),
                r.get::<_, i32>(3),
                r.get::<_, i32>(4)
            ])
        })
        .collect();
    let peak = c
        .query_opt(
            &format!(
                "SELECT persons, extract(epoch FROM window_start)::float8 FROM analytics.suppressed \
                 WHERE region = $1 AND {win} AND synthetic = ANY($3) AND persons IS NOT NULL \
                 ORDER BY persons DESC, window_start DESC LIMIT 1"
            ),
            &[&region, &p, &synthetic],
        )?
        .map(|r| json!([r.get::<_, i32>(0), r.get::<_, f64>(1)]));
    // Short links are not per region (a sticker works everywhere).
    let links: Vec<_> = c
        .query(
            &format!(
                "SELECT code, sum(hits)::bigint FROM analytics.link_day \
                 WHERE ({}) AND $1::text IS NOT NULL AND false = ANY($3) GROUP BY code ORDER BY 2 DESC",
                period.date("day", false)
            ),
            &[&region, &p, &synthetic],
        )?
        .iter()
        .map(|r| json!([r.get::<_, String>(0), r.get::<_, i64>(1)]))
        .collect();
    let (days, day) = match period {
        Period::Days(n) => (*n, None),
        Period::Day(d) => (1, Some(d.clone())),
    };
    Ok(
        json!({ "growth": growth, "peak": peak, "links": links, "days": days, "day": day, "k": crate::analytics::k_min(), "dwell": dwell, "hourly": hourly,
               "perception": perception, "perception_hourly": perception_hourly,
               "perception_by_hour": perception_by_hour,
               "experiment": experiment, "min_ratings": MIN_RATINGS,
               "hourly_edges": hourly_edges, "zones": zones, "flows": flows, "daily": daily,
               "profiles": profiles,
               "suppressed": {"kept": sup.get::<_, i64>(0), "dropped": sup.get::<_, i64>(1)},
               "hexes": hexes, "places": places, "dwell_bounds": dwell_bounds }),
    )
}

/// Downloads for the city (`crowdflow.json`: Smart Data Models
/// `CrowdFlowObserved`, key-values).
pub const EXPORTS: [&str; 4] = [
    "dwell.csv",
    "flows.csv",
    "perception.geojson",
    "crowdflow.json",
];

/// One of [`EXPORTS`]: (content type, body).
pub fn export(
    conn_str: &str,
    region: &str,
    name: &str,
    period: &Period,
    synthetic: &[bool],
) -> Result<(&'static str, String), Box<dyn Error>> {
    // Real data (not only synthetic) is released the day after.
    let rel = synthetic.contains(&false);
    use std::fmt::Write as _;
    let mut c = connect(conn_str)?;
    set_noise_key(&mut c)?;
    set_region_tz(&mut c, region)?;
    let dk = dwell_key("");
    let persons = noisy("persons", &dk);
    let p = period.param();
    let iso =
        |col: &str| format!("to_char({col} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')");
    match name {
        "dwell.csv" => {
            let mut out = String::from(
                "edge_id,window_start,persons,dwell_s,hotspot_dwell_s,synthetic,span_s\n",
            );
            let q = format!(
                "SELECT edge_id, {}, {persons}::int, {}::int, synthetic, {}::int, span_s FROM analytics.dwell \
                 WHERE region = $1 AND {} AND synthetic = ANY($3) ORDER BY window_start, edge_id",
                iso("window_start"),
                scaled("dwell_s", "persons", &persons),
                scaled("hotspot_dwell_s", "persons", &persons),
                period.ts("window_start", rel)
            );
            for r in c.query(&q, &[&region, &p, &synthetic])? {
                let _ = writeln!(
                    out,
                    "{},{},{},{},{},{},{}",
                    r.get::<_, i64>(0),
                    r.get::<_, String>(1),
                    r.get::<_, i32>(2),
                    r.get::<_, i32>(3),
                    r.get::<_, i32>(5),
                    r.get::<_, bool>(4),
                    r.get::<_, i32>(6)
                );
            }
            Ok(("text/csv; charset=utf-8", out))
        }
        "flows.csv" => {
            let mut out = String::from("from_zone,to_zone,hour_start,persons,synthetic,span_s\n");
            let q = format!(
                "SELECT from_zone, to_zone, {}, persons::int, synthetic, span_s FROM {} f \
                 WHERE region = $1 AND {} AND synthetic = ANY($3) ORDER BY t, span_s, from_zone, to_zone",
                iso("t"),
                flows_all(),
                period.ts("t", rel)
            );
            for r in c.query(&q, &[&region, &p, &synthetic])? {
                let _ = writeln!(
                    out,
                    "{},{},{},{},{},{}",
                    r.get::<_, String>(0),
                    r.get::<_, String>(1),
                    r.get::<_, String>(2),
                    r.get::<_, i32>(3),
                    r.get::<_, bool>(4),
                    r.get::<_, i32>(5)
                );
            }
            Ok(("text/csv; charset=utf-8", out))
        }
        "perception.geojson" => {
            let row = c.query_one(
                &format!(
                    "SELECT json_build_object('type', 'FeatureCollection', 'features', coalesce(json_agg(json_build_object( \
                       'type', 'Feature', 'geometry', ST_AsGeoJSON(n.geom, 6)::json, \
                       'properties', json_build_object('node_id', p.node_id, 'category', p.category, 'count', p.cnt))), '[]'::json))::text \
                     FROM (SELECT node_id, category, sum(count)::int cnt, \
                                  sum(sum(count)) OVER (PARTITION BY node_id) total \
                           FROM analytics.perception \
                           WHERE region = $1 AND {} AND synthetic = ANY($3) \
                           GROUP BY 1, 2) p JOIN geo.nodes n ON n.id = p.node_id \
                     WHERE p.total >= {MIN_RATINGS}",
                    period.date("day", rel)
                ),
                &[&region, &p, &synthetic],
            )?;
            Ok(("application/geo+json", row.get(0)))
        }
        "crowdflow.json" => {
            // One entity per segment and 15 min, geometry included: 90 days were
            // 108 MB. At most 7 days; platforms fetch daily (`day=YYYY-MM-DD`).
            let p = match period {
                Period::Days(n) if *n > CROWDFLOW_MAX_DAYS => CROWDFLOW_MAX_DAYS.to_string(),
                _ => p.clone(),
            };
            let source = if synthetic == [true] {
                "simulation"
            } else {
                "players"
            };
            let q = format!(
                "SELECT coalesce(json_agg(json_build_object( \
                   'id', 'urn:ngsi-ld:CrowdFlowObserved:' || $1 || ':' || d.edge_id || ':' || extract(epoch FROM d.window_start)::bigint \
                     || CASE WHEN d.span_s >= 3600 THEN ':' || d.span_s ELSE '' END, \
                   'type', 'CrowdFlowObserved', \
                   'refRoadSegment', 'urn:ngsi-ld:RoadSegment:' || $1 || ':' || d.edge_id, \
                   'dateObserved', {end}, \
                   'dateObservedFrom', {start}, \
                   'dateObservedTo', {end}, \
                   'peopleCount', {dp}, \
                   'occupancy', round(least(1.0, {dd} / d.span_s::numeric), 3), \
                   'location', ST_AsGeoJSON(e.geom, 6)::json, \
                   'dataProvider', 'Glut & Flut (glutflut.com)', \
                   'source', $4::text \
                 ) ORDER BY d.window_start, d.edge_id), '[]'::json)::text \
                 FROM analytics.dwell d JOIN geo.edges e ON e.id = d.edge_id \
                 WHERE d.region = $1 AND {cond} AND d.synthetic = ANY($3)",
                start = iso("d.window_start"),
                end = iso("(d.window_start + make_interval(secs => d.span_s))"),
                cond = period.ts("d.window_start", rel),
                dp = noisy("d.persons", &dwell_key("d.")),
                dd = scaled("d.dwell_s", "d.persons", &noisy("d.persons", &dwell_key("d."))),
            );
            let row = c.query_one(&q, &[&region, &p, &synthetic, &source])?;
            Ok(("application/json", row.get(0)))
        }
        _ => Err("unknown export".into()),
    }
}

/// Per H3 resolution: cells as GeoJSON (boundary from the index; id = hex
/// string, JS can't hold 64-bit ints), totals, persons per local hour, and
/// kept/dropped cell-hours.
fn hex_summary(
    c: &mut Client,
    region: &str,
    period: &Period,
    synthetic: &[bool],
) -> Result<serde_json::Value, Box<dyn Error>> {
    // Real data (not only synthetic) is released the day after.
    let rel = synthetic.contains(&false);
    use serde_json::json;
    let p = period.param();
    let hr = period.ts("hour_start", rel);
    let hp = noisy("persons", HEX_KEY);
    let mut out = serde_json::Map::new();
    for res in crate::analytics::HEX_RES {
        let r = res as i16;
        let mut index = std::collections::HashMap::new();
        let mut features = Vec::new();
        for row in c.query(
            &format!(
                "SELECT cell, sum({hp})::bigint, sum({})::bigint, sum({})::bigint \
                 FROM analytics.hex_dwell \
                 WHERE region = $1 AND {hr} AND synthetic = ANY($3) AND res = $4 GROUP BY cell",
                scaled("dwell_s", "persons", &hp),
                scaled("hotspot_dwell_s", "persons", &hp),
            ),
            &[&region, &p, &synthetic, &r],
        )? {
            let cell: i64 = row.get(0);
            let Ok(ci) = h3o::CellIndex::try_from(cell as u64) else {
                continue;
            };
            let mut ring: Vec<[f64; 2]> = ci
                .boundary()
                .iter()
                .map(|ll| [ll.lng(), ll.lat()])
                .collect();
            ring.push(ring[0]);
            index.insert(cell, features.len());
            features.push(json!({
                "type": "Feature",
                "id": features.len(),
                "properties": {"cell": ci.to_string(), "persons": row.get::<_, i64>(1),
                               "dwell_s": row.get::<_, i64>(2), "hotspot_dwell_s": row.get::<_, i64>(3)},
                "geometry": {"type": "Polygon", "coordinates": [ring]}
            }));
        }
        let hourly: Vec<_> = c
            .query(
                &format!(
                    "SELECT cell, extract(hour FROM hour_start AT TIME ZONE current_setting('TimeZone'))::int, \
                     sum({hp})::bigint FROM analytics.hex_dwell \
                     WHERE region = $1 AND {hr} AND synthetic = ANY($3) AND res = $4 AND span_s < 86400 GROUP BY 1, 2"
                ),
                &[&region, &p, &synthetic, &r],
            )?
            .iter()
            .filter_map(|row| {
                let i = index.get(&row.get::<_, i64>(0))?;
                Some(json!([i, row.get::<_, i32>(1), row.get::<_, i64>(2)]))
            })
            .collect();
        let sup = c.query_one(
            &format!(
                "SELECT coalesce(sum(kept), 0)::bigint, coalesce(sum(dropped), 0)::bigint \
                 FROM analytics.hex_suppressed \
                 WHERE region = $1 AND {hr} AND synthetic = ANY($3) AND res = $4"
            ),
            &[&region, &p, &synthetic, &r],
        )?;
        out.insert(
            res.to_string(),
            json!({"cells": {"type": "FeatureCollection", "features": features},
                   "hourly": hourly,
                   "kept": sup.get::<_, i64>(0), "dropped": sup.get::<_, i64>(1)}),
        );
    }
    Ok(serde_json::Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::Period;

    #[test]
    fn day_format() {
        assert!(Period::day("2025-12-06").is_some());
        assert!(Period::day("2025-12-6").is_none());
        assert!(Period::day("2025/12/06").is_none());
        assert!(Period::day("x025-12-06").is_none());
    }

    #[test]
    fn marginal_hides_at_least_two() {
        let cell = |k: &str, n: i64| (k.to_string(), Some(n));
        let mut m = vec![
            cell("u30", 40),
            cell("30_59", 20),
            cell("60p", 2),
            cell("na", 9),
        ];
        super::suppress_marginal(&mut m, 5);
        let hidden: Vec<&str> = m
            .iter()
            .filter(|c| c.1.is_none())
            .map(|c| c.0.as_str())
            .collect();
        assert_eq!(hidden, vec!["60p", "na"]);
        let mut m = vec![cell("yes", 40), cell("no", 20), cell("na", 9)];
        super::suppress_marginal(&mut m, 5);
        assert!(m.iter().all(|c| c.1.is_some()), "nothing below k");
    }
}

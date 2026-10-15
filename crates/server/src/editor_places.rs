//! Authoring tools for cities, stage 2: places.
//! Zones (flows, Entdecken; focus places for measuring) with their own
//! radius, and hotspot candidates on junctions. A token with role `editor`
//! lists and changes them; each change goes to `ops.audit_log` and, if the
//! region is loaded, to the region (`FromWeb::Places`). Hotspot sites list
//! the gastronomy within 50 m (OSM): the editor warns when it piles up there
//! (who plays would be skewed).

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::{audit_entity, bad};
use super::{who, AppState, Client, FromWeb, RegionQ, Role};

const NAME_MAX: usize = 60;
const RADIUS: (f32, f32) = (30.0, 1000.0);
/// Default zone radius (flows, `analytics::ZONE_R_M`; Entdecken `zone_r_m`).
const RADIUS_DEFAULT: f32 = 400.0;
const ZONES_MAX: i64 = 100;
const SITES_MAX: i64 = 30;
/// A hotspot site goes to the nearest junction within this distance.
const SNAP_M: f64 = 150.0;
/// Gastronomy counted around a hotspot site.
const FOOD_M: f64 = 50.0;
/// Finds must be this close to a junction (as the data import).
const FIND_REACH_M: f64 = 40.0;
const FINDS_MAX: i64 = 400;
/// Planned hotspots: 10 min to 12 h, at most this many not over yet.
const PLAN_S: (f64, f64) = (600.0, 43_200.0);
const PLANS_MAX: i64 = 50;
/// Collections the app knows (`finds.c.*`).
const COLLECTIONS: [&str; 6] = ["fountains", "trees", "gates", "views", "art", "stones"];
const FOOD: [&str; 7] = [
    "restaurant",
    "cafe",
    "bar",
    "pub",
    "biergarten",
    "fast_food",
    "ice_cream",
];

/// A change from the editor.
#[derive(Deserialize, Debug, Clone)]
pub struct PlaceIn {
    /// `zone`, `site` or `find`.
    pub kind: String,
    /// Absent: a new one. Zones: the id; sites and finds: the number.
    pub id: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub lat: f64,
    #[serde(default)]
    pub lon: f64,
    pub radius_m: Option<f32>,
    #[serde(default)]
    pub focus: bool,
    #[serde(default)]
    pub delete: bool,
    /// Finds: the collection.
    pub collection: Option<String>,
    /// Finds: hidden (doesn't play; players keep it).
    #[serde(default)]
    pub hidden: bool,
    /// Plans: the hotspot site (its number) and the window (unix seconds).
    pub site: Option<String>,
    pub starts: Option<f64>,
    pub ends: Option<f64>,
}

/// Checks that need no database.
pub fn check(p: &PlaceIn) -> Result<(), &'static str> {
    if !["zone", "site", "find", "plan"].contains(&p.kind.as_str()) {
        return Err("kind: zone, site, find or plan");
    }
    if p.kind != "zone" && p.id.as_ref().is_some_and(|i| i.parse::<i64>().is_err()) {
        return Err("id: a number");
    }
    if p.delete {
        return p.id.as_ref().map(|_| ()).ok_or("delete: id missing");
    }
    if p.kind == "plan" {
        if p.id.is_some() {
            return Err("plans: delete and plan anew");
        }
        if p.site
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
            .is_none()
        {
            return Err("plan: hotspot site missing");
        }
        let (Some(a), Some(b)) = (p.starts, p.ends) else {
            return Err("plan: start and end missing");
        };
        if !(a.is_finite() && b.is_finite()) || !(PLAN_S.0..=PLAN_S.1).contains(&(b - a)) {
            return Err("plan: 10 minutes to 12 hours");
        }
        return Ok(());
    }
    let n = p.name.trim().chars().count();
    if n == 0 || n > NAME_MAX {
        return Err("name: 1-60 characters");
    }
    if !(p.lat.is_finite() && p.lon.is_finite())
        || !(-90.0..=90.0).contains(&p.lat)
        || !(-180.0..=180.0).contains(&p.lon)
    {
        return Err("position invalid");
    }
    if let Some(r) = p.radius_m {
        if !(RADIUS.0..=RADIUS.1).contains(&r) {
            return Err("radius: 30-1000 m");
        }
    }
    if p.kind == "find"
        && !p
            .collection
            .as_deref()
            .is_some_and(|c| COLLECTIONS.contains(&c))
    {
        return Err("collection unknown");
    }
    Ok(())
}

/// A zone id from its name: lowercase ASCII, `-`, unique among `taken`.
pub fn zone_id(name: &str, taken: &[String]) -> String {
    let mut base = String::new();
    for c in name.trim().to_lowercase().chars() {
        let s = match c {
            'ä' => "ae",
            'ö' => "oe",
            'ü' => "ue",
            'ß' => "ss",
            c if c.is_ascii_alphanumeric() => {
                base.push(c);
                continue;
            }
            _ => "-",
        };
        base.push_str(s);
    }
    let mut base = base
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    base.truncate(32);
    let base = base.trim_end_matches('-').to_string();
    let base = if base.is_empty() { "ort".into() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

fn may_read_food(db: &mut postgres::Client) -> bool {
    db.query_one(
        "SELECT to_regclass('osm.pois_all') IS NOT NULL \
         AND has_table_privilege('osm.pois_all', 'SELECT')",
        &[],
    )
    .is_ok_and(|r| r.get(0))
}

/// `GET /api/ops/places?region=` (editor): zones, hotspot sites (with
/// gastronomy nearby), the automatic hotspot area and the limits.
pub(super) async fn list(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !who(&s, c, &headers).await.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let zones: Vec<Value> = db
                .query(
                    "SELECT id, name, lat, lon, radius_m, focus, created_by FROM geo.zones \
                     WHERE region = $1 ORDER BY focus DESC, name",
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<_, String>(0),
                        "name": r.get::<_, String>(1),
                        "lat": r.get::<_, f64>(2),
                        "lon": r.get::<_, f64>(3),
                        "radius_m": r.get::<_, Option<f32>>(4),
                        "focus": r.get::<_, bool>(5),
                        "by": r.get::<_, Option<String>>(6),
                    })
                })
                .collect();
            let food = may_read_food(db);
            let food_sql = if food {
                format!(
                    "(SELECT array_agg(DISTINCT coalesce(p.name, p.value)) FROM osm.pois_all p \
                     WHERE p.kind = 'amenity' AND p.value = ANY($2) \
                     AND p.geometry && ST_Expand(ST_SetSRID(ST_MakePoint(s.lon, s.lat), 4326), 0.001) \
                     AND ST_DWithin(p.geometry::geography, ST_MakePoint(s.lon, s.lat)::geography, {FOOD_M}))"
                )
            } else {
                "NULL::text[]".into()
            };
            let food_kinds: Vec<String> = FOOD.iter().map(|s| s.to_string()).collect();
            let sql = format!(
                "SELECT s.id, s.name, s.lat, s.lon, s.node_id, s.created_by, {food_sql} \
                 FROM geo.hotspot_sites s WHERE s.region = $1 ORDER BY s.id"
            );
            let rows = if food {
                db.query(&sql, &[&slug, &food_kinds])?
            } else {
                db.query(&sql, &[&slug])?
            };
            let sites: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<_, i64>(0).to_string(),
                        "name": r.get::<_, String>(1),
                        "lat": r.get::<_, f64>(2),
                        "lon": r.get::<_, f64>(3),
                        "node": r.get::<_, i64>(4),
                        "by": r.get::<_, Option<String>>(5),
                        "food": r.get::<_, Option<Vec<Option<String>>>>(6)
                            .map(|v| v.into_iter().flatten().collect::<Vec<_>>()),
                    })
                })
                .collect();
            let r = db.query_one(
                "SELECT coalesce(hotspot_lat, ST_Y(centroid_geom)), coalesce(hotspot_lon, ST_X(centroid_geom)), \
                 coalesce(hotspot_area_m, 700)::float8, \
                 bounds_min_lon, bounds_min_lat, bounds_max_lon, bounds_max_lat \
                 FROM geo.regions WHERE slug = $1",
                &[&slug],
            )?;
            let finds: Vec<Value> = db
                .query(
                    "SELECT f.id, f.collection, f.name, f.lat, f.lon, f.hidden, f.osm LIKE 'city/%', \
                     f.created_by, (SELECT count(*) FROM game.player_finds p WHERE p.find = f.id) \
                     FROM geo.finds f WHERE f.region = $1 ORDER BY f.collection, f.name",
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<_, i64>(0).to_string(),
                        "collection": r.get::<_, String>(1),
                        "name": r.get::<_, String>(2),
                        "lat": r.get::<_, f64>(3),
                        "lon": r.get::<_, f64>(4),
                        "hidden": r.get::<_, bool>(5),
                        "own": r.get::<_, bool>(6),
                        "by": r.get::<_, Option<String>>(7),
                        "found": r.get::<_, i64>(8),
                    })
                })
                .collect();
            // Plans not over, and those of the last week.
            let plans: Vec<Value> = db
                .query(
                    "SELECT id, name, extract(epoch FROM starts)::float8, extract(epoch FROM ends)::float8, \
                     created_by, lat, lon FROM game.hotspot_plans \
                     WHERE region = $1 AND ends > now() - interval '7 days' ORDER BY starts",
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    json!({
                        "id": r.get::<_, i64>(0).to_string(),
                        "name": r.get::<_, String>(1),
                        "starts": r.get::<_, f64>(2),
                        "ends": r.get::<_, f64>(3),
                        "by": r.get::<_, Option<String>>(4),
                        "lat": r.get::<_, f64>(5),
                        "lon": r.get::<_, f64>(6),
                    })
                })
                .collect();
            Ok(json!({
                "zones": zones,
                "sites": sites,
                "finds": finds,
                "plans": plans,
                "collections": COLLECTIONS,
                "food_checked": food,
                "auto": { "lat": r.get::<_, f64>(0), "lon": r.get::<_, f64>(1), "radius_m": r.get::<_, f64>(2) },
                "bounds": [r.get::<_, f64>(3), r.get::<_, f64>(4), r.get::<_, f64>(5), r.get::<_, f64>(6)],
                "limits": {
                    "radius": RADIUS, "radius_default": RADIUS_DEFAULT,
                    "zones": ZONES_MAX, "sites": SITES_MAX, "food_m": FOOD_M,
                    "finds": FINDS_MAX, "find_reach_m": FIND_REACH_M,
                },
            }))
        })
    })
    .await
    .ok()
    .flatten();
    match out {
        Some(v) => Json(v).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /api/ops/places?region=` (editor): add, change or delete a zone or
/// hotspot site. A loaded region takes the new places at once (hotspot sites
/// from its next slot).
pub(super) async fn save(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<PlaceIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Err(e) = check(&req) {
        return bad(e);
    }
    let st = s.clone();
    let by = w.label;
    let region = slug.clone();
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            let r = if req.kind == "find" {
                write_find(db, &region, &req, &by)?
            } else if req.kind == "plan" {
                write_plan(db, &region, &req, &by, crate::analytics::now_s())?
            } else {
                write(db, &region, &req, &by)?
            };
            let msg = if req.kind == "find" {
                let rows = db
                    .query(
                        "SELECT id, collection, name, lat, lon FROM geo.finds WHERE region = $1 AND NOT hidden",
                        &[&region],
                    )?
                    .iter()
                    .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
                    .collect();
                FromWeb::Finds(rows)
            } else {
                FromWeb::Places(crate::places::load(db, &region)?)
            };
            Ok(r.map(|id| (id, msg)))
        })
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(Ok((id, msg))) => {
            if let Some(r) = s.host.peek(&slug) {
                let _ = r.tx.send(msg);
            }
            Json(json!({ "id": id })).into_response()
        }
        Some(Err(e)) => bad(e),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// The write in one transaction; `Ok(Err)`: refused (message for the editor).
fn write(
    db: &mut postgres::Client,
    region: &str,
    p: &PlaceIn,
    by: &str,
) -> Result<Result<String, &'static str>, postgres::Error> {
    let mut t = db.transaction()?;
    let entity = |id: &str| format!("{} {region}/{id}", p.kind);
    if p.delete {
        let id = p.id.clone().unwrap_or_default();
        let n = if p.kind == "zone" {
            t.execute(
                "DELETE FROM geo.zones WHERE region = $1 AND id = $2",
                &[&region, &id],
            )?
        } else {
            let n: i64 = id.parse().unwrap_or(-1);
            t.execute(
                "DELETE FROM geo.hotspot_sites WHERE region = $1 AND id = $2",
                &[&region, &n],
            )?
        };
        if n == 0 {
            return Ok(Err("no such place"));
        }
        audit_entity(&mut t, &entity(&id), "delete", by, Value::Null)?;
        t.commit()?;
        return Ok(Ok(id));
    }
    let name = p.name.trim().to_string();
    let inside: Option<bool> = t
        .query_opt(
            "SELECT ST_Contains(COALESCE(boundary_geom, ST_MakeEnvelope(bounds_min_lon, bounds_min_lat, \
             bounds_max_lon, bounds_max_lat, 4326)), ST_SetSRID(ST_MakePoint($2, $3), 4326)) \
             FROM geo.regions WHERE slug = $1",
            &[&region, &p.lon, &p.lat],
        )?
        .map(|r| r.get(0));
    if inside != Some(true) {
        return Ok(Err("outside the region"));
    }
    let id = if p.kind == "zone" {
        let taken: Vec<String> = t
            .query("SELECT id FROM geo.zones WHERE region = $1", &[&region])?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let id = match &p.id {
            Some(id) if taken.contains(id) => id.clone(),
            Some(_) => return Ok(Err("no such zone")),
            None if taken.len() as i64 >= ZONES_MAX => return Ok(Err("too many zones (100)")),
            None => zone_id(&name, &taken),
        };
        t.execute(
            "INSERT INTO geo.zones (region, id, name, lat, lon, radius_m, focus, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (region, id) DO UPDATE SET name = EXCLUDED.name, lat = EXCLUDED.lat, \
             lon = EXCLUDED.lon, radius_m = EXCLUDED.radius_m, focus = EXCLUDED.focus, \
             created_by = EXCLUDED.created_by, updated_at = now()",
            &[
                &region,
                &id,
                &name,
                &p.lat,
                &p.lon,
                &p.radius_m,
                &p.focus,
                &by,
            ],
        )?;
        id
    } else {
        // On the nearest junction.
        let Some(node) = t.query_opt(
            "SELECT n.id, n.lat, n.lon FROM geo.nodes n JOIN geo.regions r ON r.id = n.region_id \
             WHERE r.slug = $1 AND n.is_active \
             AND ST_DWithin(n.geom::geography, ST_MakePoint($2, $3)::geography, $4) \
             ORDER BY n.geom <-> ST_SetSRID(ST_MakePoint($2, $3), 4326) LIMIT 1",
            &[&region, &p.lon, &p.lat, &SNAP_M],
        )?
        else {
            return Ok(Err("no junction within 150 m"));
        };
        let (node, lat, lon): (i64, f64, f64) = (node.get(0), node.get(1), node.get(2));
        let clash: Option<i64> = t
            .query_opt(
                "SELECT id FROM geo.hotspot_sites WHERE region = $1 AND node_id = $2",
                &[&region, &node],
            )?
            .map(|r| r.get(0));
        match &p.id {
            Some(id) => {
                let n: i64 = id.parse().unwrap_or(-1);
                if clash.is_some_and(|c| c != n) {
                    return Ok(Err("another site is on that junction"));
                }
                let k = t.execute(
                    "UPDATE geo.hotspot_sites SET name = $3, node_id = $4, lat = $5, lon = $6, \
                     created_by = $7, updated_at = now() WHERE region = $1 AND id = $2",
                    &[&region, &n, &name, &node, &lat, &lon, &by],
                )?;
                if k == 0 {
                    return Ok(Err("no such site"));
                }
                n.to_string()
            }
            None => {
                if clash.is_some() {
                    return Ok(Err("another site is on that junction"));
                }
                let count: i64 = t
                    .query_one(
                        "SELECT count(*) FROM geo.hotspot_sites WHERE region = $1",
                        &[&region],
                    )?
                    .get(0);
                if count >= SITES_MAX {
                    return Ok(Err("too many hotspot sites (30)"));
                }
                t.query_one(
                    "INSERT INTO geo.hotspot_sites (region, name, node_id, lat, lon, created_by) \
                     VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
                    &[&region, &name, &node, &lat, &lon, &by],
                )?
                .get::<_, i64>(0)
                .to_string()
            }
        }
    };
    audit_entity(
        &mut t,
        &entity(&id),
        if p.id.is_some() { "change" } else { "add" },
        by,
        json!({ "name": name, "lat": p.lat, "lon": p.lon, "radius_m": p.radius_m, "focus": p.focus }),
    )?;
    t.commit()?;
    Ok(Ok(id))
}

/// A planned hotspot: from one of the city's hotspot sites (name and junction
/// copied), or deleted (a running one stays on until its end).
fn write_plan(
    db: &mut postgres::Client,
    region: &str,
    p: &PlaceIn,
    by: &str,
    now: f64,
) -> Result<Result<String, &'static str>, postgres::Error> {
    let mut t = db.transaction()?;
    let id: Option<i64> = p.id.as_deref().and_then(|i| i.parse().ok());
    if p.delete {
        let n = t.execute(
            "DELETE FROM game.hotspot_plans WHERE region = $1 AND id = $2 AND ends > now()",
            &[&region, &id],
        )?;
        if n == 0 {
            return Ok(Err("no such plan, or it is over"));
        }
        audit_entity(
            &mut t,
            &format!("plan {region}/{}", id.unwrap_or(0)),
            "delete",
            by,
            Value::Null,
        )?;
        t.commit()?;
        return Ok(Ok(id.unwrap_or(0).to_string()));
    }
    let (starts, ends) = (p.starts.unwrap_or(0.0), p.ends.unwrap_or(0.0));
    if starts < now - 300.0 {
        return Ok(Err("plan: the start is in the past"));
    }
    let site: i64 = p.site.as_deref().and_then(|s| s.parse().ok()).unwrap_or(-1);
    let Some(row) = t.query_opt(
        "SELECT name, node_id, lat, lon FROM geo.hotspot_sites WHERE region = $1 AND id = $2",
        &[&region, &site],
    )?
    else {
        return Ok(Err("no such hotspot site"));
    };
    let (name, node, lat, lon): (String, i64, f64, f64) =
        (row.get(0), row.get(1), row.get(2), row.get(3));
    let open: i64 = t
        .query_one(
            "SELECT count(*) FROM game.hotspot_plans WHERE region = $1 AND ends > now()",
            &[&region],
        )?
        .get(0);
    if open >= PLANS_MAX {
        return Ok(Err("too many planned hotspots (50)"));
    }
    let new: i64 = t
        .query_one(
            "INSERT INTO game.hotspot_plans (region, name, node_id, lat, lon, starts, ends, created_by) \
             VALUES ($1, $2, $3, $4, $5, to_timestamp($6), to_timestamp($7), $8) RETURNING id",
            &[&region, &name, &node, &lat, &lon, &starts, &ends, &by],
        )?
        .get(0);
    audit_entity(
        &mut t,
        &format!("plan {region}/{new}"),
        "add",
        by,
        json!({ "site": site, "name": name, "starts": starts, "ends": ends }),
    )?;
    t.commit()?;
    Ok(Ok(new.to_string()))
}

/// A find: add (own, `osm = 'city/…'`), change (also hide or show OSM ones;
/// an edited one keeps its text on re-import) or delete (own ones nobody has
/// found; otherwise hide, so players keep what they found).
fn write_find(
    db: &mut postgres::Client,
    region: &str,
    p: &PlaceIn,
    by: &str,
) -> Result<Result<String, &'static str>, postgres::Error> {
    let mut t = db.transaction()?;
    let id: Option<i64> = p.id.as_deref().and_then(|i| i.parse().ok());
    if p.delete {
        let Some(row) = t.query_opt(
            "SELECT osm LIKE 'city/%', (SELECT count(*) FROM game.player_finds WHERE find = f.id) \
             FROM geo.finds f WHERE region = $1 AND id = $2",
            &[&region, &id],
        )?
        else {
            return Ok(Err("no such find"));
        };
        let (own, found): (bool, i64) = (row.get(0), row.get(1));
        if !own || found > 0 {
            return Ok(Err(
                "only own finds nobody has found yet can be deleted; hide it instead",
            ));
        }
        t.execute(
            "DELETE FROM geo.finds WHERE region = $1 AND id = $2",
            &[&region, &id],
        )?;
        audit_entity(
            &mut t,
            &format!("find {region}/{}", id.unwrap_or(0)),
            "delete",
            by,
            Value::Null,
        )?;
        t.commit()?;
        return Ok(Ok(id.unwrap_or(0).to_string()));
    }
    let name = p.name.trim().to_string();
    // Reachable: near a junction of the region (as the import keeps them).
    let near: bool = t
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM geo.nodes n JOIN geo.regions r ON r.id = n.region_id \
             WHERE r.slug = $1 AND n.is_active \
             AND n.geom && ST_Expand(ST_SetSRID(ST_MakePoint($2, $3), 4326), 0.001) \
             AND ST_DWithin(n.geom::geography, ST_MakePoint($2, $3)::geography, $4))",
            &[&region, &p.lon, &p.lat, &FIND_REACH_M],
        )?
        .get(0);
    if !near {
        return Ok(Err("no street within 40 m"));
    }
    let coll = p.collection.clone().unwrap_or_default();
    let out = match id {
        Some(id) => {
            let n = t.execute(
                "UPDATE geo.finds SET name = $3, collection = $4, lat = $5, lon = $6, hidden = $7, \
                 created_by = $8, updated_at = now() WHERE region = $1 AND id = $2",
                &[&region, &id, &name, &coll, &p.lat, &p.lon, &p.hidden, &by],
            )?;
            if n == 0 {
                return Ok(Err("no such find"));
            }
            id
        }
        None => {
            let count: i64 = t
                .query_one(
                    "SELECT count(*) FROM geo.finds WHERE region = $1",
                    &[&region],
                )?
                .get(0);
            if count >= FINDS_MAX {
                return Ok(Err("too many finds (400)"));
            }
            t.query_one(
                "INSERT INTO geo.finds (region, collection, name, osm, lat, lon, hidden, created_by, updated_at) \
                 VALUES ($1, $2, $3, 'city/' || gen_random_uuid(), $4, $5, $6, $7, now()) RETURNING id",
                &[&region, &coll, &name, &p.lat, &p.lon, &p.hidden, &by],
            )?
            .get(0)
        }
    };
    audit_entity(
        &mut t,
        &format!("find {region}/{out}"),
        if id.is_some() { "change" } else { "add" },
        by,
        json!({ "name": name, "collection": coll, "lat": p.lat, "lon": p.lon, "hidden": p.hidden }),
    )?;
    t.commit()?;
    Ok(Ok(out.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(kind: &str) -> PlaceIn {
        PlaceIn {
            kind: kind.into(),
            id: None,
            name: "Marktstätte".into(),
            lat: 47.66,
            lon: 9.17,
            radius_m: Some(200.0),
            focus: true,
            delete: false,
            collection: None,
            hidden: false,
            site: None,
            starts: None,
            ends: None,
        }
    }

    #[test]
    fn checks() {
        assert!(check(&place("zone")).is_ok());
        assert!(check(&place("site")).is_ok());
        assert!(check(&place("shop")).is_err());
        let mut p = place("zone");
        p.radius_m = Some(10.0);
        assert!(check(&p).is_err(), "radius too small");
        p.radius_m = None;
        assert!(check(&p).is_ok(), "default radius");
        p.name = " ".into();
        assert!(check(&p).is_err(), "empty name");
        let mut p = place("site");
        p.id = Some("abc".into());
        assert!(check(&p).is_err(), "site ids are numbers");
        let mut p = place("zone");
        p.delete = true;
        assert!(check(&p).is_err(), "delete needs an id");
        p.id = Some("x".into());
        p.name.clear();
        assert!(check(&p).is_ok());
        let mut p = place("zone");
        p.lat = f64::NAN;
        assert!(check(&p).is_err());
        let mut p = place("find");
        assert!(check(&p).is_err(), "a find needs a collection");
        p.collection = Some("fountains".into());
        assert!(check(&p).is_ok());
        p.collection = Some("shops".into());
        assert!(check(&p).is_err(), "only the app's collections");
        let mut p = place("plan");
        p.name.clear();
        assert!(check(&p).is_err(), "a plan needs a site");
        p.site = Some("3".into());
        p.starts = Some(1000.0);
        p.ends = Some(1000.0 + 3600.0);
        assert!(check(&p).is_ok(), "no name needed: it comes from the site");
        p.ends = Some(1000.0 + 300.0);
        assert!(check(&p).is_err(), "at least 10 minutes");
        p.ends = Some(1000.0 + 13.0 * 3600.0);
        assert!(check(&p).is_err(), "at most 12 hours");
    }

    #[test]
    fn zone_ids_from_names() {
        assert_eq!(zone_id("Marktstätte", &[]), "marktstaette");
        assert_eq!(zone_id("  Hafen / Konzil ", &[]), "hafen-konzil");
        assert_eq!(
            zone_id("Hafen / Konzil", &["hafen-konzil".into()]),
            "hafen-konzil-2"
        );
        assert_eq!(zone_id("Straße am Münster", &[]), "strasse-am-muenster");
        assert_eq!(zone_id("!!!", &[]), "ort");
    }
}

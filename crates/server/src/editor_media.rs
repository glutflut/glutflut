//! Images for finds, zones and chapters in the editor.
//! An editor uploads one or picks a Commons suggestion; a second person of
//! the city approves it (not the uploader), then the app shows it with its
//! credit line (`GET /api/media`). One approved image per target; approving
//! a new one removes the old. Files in the media bucket (`media.rs`), the
//! database is the index; each change goes to `ops.audit_log`. A child module
//! of the gateway.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};

use super::editor::{audit_entity, bad, primary};
use super::{who, AppState, Client, RegionQ, Role};
use crate::media::{self, Licence, Processed};

const AUTHOR_MAX: usize = 120;
const ALT_MAX: usize = 200;
/// Pending images per region (a queue for the second person, not a store).
const PENDING_MAX: i64 = 60;

#[derive(Deserialize)]
pub(super) struct UploadIn {
    kind: String,
    target: String,
    /// The file, base64 (a `data:` URL is fine too).
    data: String,
    author: String,
    /// `own`, `cc-by-4.0`, `cc-by-sa-4.0` or `cc0-1.0`.
    licence: String,
    #[serde(default)]
    alt: String,
    source_url: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct PickIn {
    find: String,
    title: String,
    #[serde(default)]
    alt: String,
}

#[derive(Deserialize)]
pub(super) struct DecideIn {
    id: i64,
    approve: bool,
}

#[derive(Deserialize)]
pub(super) struct IdQ {
    region: Option<String>,
    id: i64,
}

/// What goes into a new row besides the file.
struct Meta {
    kind: String,
    target: String,
    author: String,
    licence: Licence,
    licence_url: Option<String>,
    source_url: Option<String>,
    commons_title: Option<String>,
    alt: Option<String>,
}

/// A text field: trimmed, no control characters, at most `max` characters.
pub fn clean_text(s: &str, max: usize, what: &'static str) -> Result<String, &'static str> {
    let t = s.trim();
    if t.chars().count() > max || t.chars().any(char::is_control) {
        return Err(what);
    }
    Ok(t.to_string())
}

/// The stored licence code as a [`Licence`].
fn stored_licence(code: &str, version: Option<String>) -> Licence {
    let code = ["own", "cc-by", "cc-by-sa", "cc0", "pd"]
        .into_iter()
        .find(|c| *c == code)
        .unwrap_or("own");
    Licence { code, version }
}

/// The target exists in this region: its name (chapters: the key).
fn target_name(
    db: &mut postgres::Client,
    slug: &str,
    kind: &str,
    target: &str,
) -> Result<Option<String>, postgres::Error> {
    Ok(match kind {
        "find" => match target.parse::<i64>() {
            Ok(id) => db
                .query_opt(
                    "SELECT name FROM geo.finds WHERE region = $1 AND id = $2",
                    &[&slug, &id],
                )?
                .map(|r| r.get(0)),
            Err(_) => None,
        },
        "zone" => db
            .query_opt(
                "SELECT name FROM geo.zones WHERE region = $1 AND id = $2",
                &[&slug, &target],
            )?
            .map(|r| r.get(0)),
        "challenge" => db
            .query_opt(
                "SELECT key FROM game.chapters WHERE region = $1 AND key = $2 AND kind IS NULL",
                &[&slug, &target],
            )?
            .map(|r| r.get(0)),
        _ => None,
    })
}

/// Store a processed image as pending (the master token's: approved at once,
/// replacing the target's image): bucket first, then the row (blocking).
fn store_new(
    st: &AppState,
    slug: &str,
    p: Processed,
    m: Meta,
    by: &str,
    master: bool,
) -> Result<i64, String> {
    let store = media::store().ok_or("no media bucket configured")?;
    let slug_s = slug.to_string();
    let (kind, target) = (m.kind.clone(), m.target.clone());
    let pre = st
        .with_db(move |db| {
            if target_name(db, &slug_s, &kind, &target)?.is_none() {
                return Ok(Err("unknown target"));
            }
            let pending: i64 = db
                .query_one(
                    "SELECT count(*) FROM geo.media WHERE region = $1 AND status = 'pending'",
                    &[&slug_s],
                )?
                .get(0);
            if pending >= PENDING_MAX {
                return Ok(Err("too many images waiting for approval"));
            }
            let id: i64 = db
                .query_one("SELECT nextval('geo.media_id_seq')", &[])?
                .get(0);
            Ok(Ok(id))
        })
        .ok_or("database unavailable")??;
    // A new key per image (immutable caching); the suffix keeps ids unguessable.
    let key = store.key(&format!(
        "{slug}/{}/{pre}-{:08x}.jpg",
        m.kind,
        rand::random::<u32>()
    ));
    store.put(&key, &p.jpeg).map_err(|e| {
        tracing::warn!(error = %e, "media: upload failed");
        "storing the image failed".to_string()
    })?;
    let (slug_s, by_s, key_s) = (slug.to_string(), by.to_string(), key.clone());
    let ok = st.with_db(move |db| {
        let mut t = db.transaction()?;
        t.execute(
            "INSERT INTO geo.media (id, region, kind, target, s3_key, width, height, bytes, author, \
             licence, licence_version, licence_url, source_url, commons_title, alt, uploaded_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
            &[
                &pre,
                &slug_s,
                &m.kind,
                &m.target,
                &key_s,
                &(p.width as i32),
                &(p.height as i32),
                &(p.jpeg.len() as i32),
                &m.author,
                &m.licence.code,
                &m.licence.version,
                &m.licence_url,
                &m.source_url,
                &m.commons_title,
                &m.alt,
                &by_s,
            ],
        )?;
        audit_entity(
            &mut t,
            &format!("media {slug_s}/{pre}"),
            "upload",
            &by_s,
            json!({ "kind": m.kind, "target": m.target, "licence": m.licence.code, "commons": m.commons_title }),
        )?;
        let mut gone: Vec<String> = Vec::new();
        if master {
            for r in t.query(
                "DELETE FROM geo.media WHERE region = $1 AND kind = $2 AND target = $3 \
                 AND status = 'approved' RETURNING s3_key",
                &[&slug_s, &m.kind, &m.target],
            )? {
                gone.push(r.get(0));
            }
            t.execute(
                "UPDATE geo.media SET status = 'approved', approved_by = $2, decided_at = now() WHERE id = $1",
                &[&pre, &by_s],
            )?;
            audit_entity(
                &mut t,
                &format!("media {slug_s}/{pre}"),
                "approve",
                &by_s,
                json!({ "kind": m.kind, "target": m.target, "direct": true }),
            )?;
        }
        t.commit()?;
        Ok(gone)
    });
    let Some(gone) = ok else {
        let _ = store.delete(&key);
        return Err("database unavailable".into());
    };
    for k in gone {
        let _ = store.delete(&k);
    }
    Ok(pre)
}

/// `GET /api/ops/media?region=` (editor): images (all states), Commons
/// suggestions per find and the targets (finds, zones, chapters).
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
        let store = media::store();
        st.with_db(|db| {
            let lang = primary(db, &slug)?;
            let images: Vec<Value> = db
                .query(
                    "SELECT id, kind, target, s3_key, width, height, author, licence, licence_version, \
                     licence_url, source_url, alt, status, uploaded_by, approved_by, \
                     extract(epoch FROM created_at)::float8, bytes \
                     FROM geo.media WHERE region = $1 ORDER BY created_at DESC",
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    let (id, key, status): (i64, String, String) = (r.get(0), r.get(3), r.get(12));
                    let lic = stored_licence(r.get(7), r.get(8));
                    // Rejected ones have no file any more.
                    let url = (status != "rejected")
                        .then(|| store.map(|s| if s.private { s.presigned(&key) } else { s.url(&key, id) }))
                        .flatten();
                    json!({
                        "id": id,
                        "kind": r.get::<_, String>(1),
                        "target": r.get::<_, String>(2),
                        "url": url,
                        "width": r.get::<_, i32>(4),
                        "height": r.get::<_, i32>(5),
                        "author": r.get::<_, String>(6),
                        "licence": lic.code,
                        "licence_label": lic.label(),
                        "licence_url": r.get::<_, Option<String>>(9),
                        "source_url": r.get::<_, Option<String>>(10),
                        "alt": r.get::<_, Option<String>>(11),
                        "status": status,
                        "by": r.get::<_, String>(13),
                        "approved_by": r.get::<_, Option<String>>(14),
                        "created": r.get::<_, f64>(15),
                        "bytes": r.get::<_, i32>(16),
                    })
                })
                .collect();
            let mut suggestions = serde_json::Map::new();
            for r in db.query(
                "SELECT find, title, thumb_url, page_url, author, licence, licence_url, distance_m \
                 FROM geo.commons_suggestions WHERE region = $1 ORDER BY find, distance_m",
                &[&slug],
            )? {
                let f = r.get::<_, i64>(0).to_string();
                let e = suggestions.entry(f).or_insert_with(|| Value::Array(vec![]));
                if let Value::Array(a) = e {
                    a.push(json!({
                        "title": r.get::<_, String>(1),
                        "thumb": r.get::<_, String>(2),
                        "page": r.get::<_, String>(3),
                        "author": r.get::<_, String>(4),
                        "licence": r.get::<_, String>(5),
                        "licence_url": r.get::<_, Option<String>>(6),
                        "distance_m": r.get::<_, f32>(7),
                    }));
                }
            }
            let finds: Vec<Value> = db
                .query(
                    "SELECT id, name, collection, hidden FROM geo.finds WHERE region = $1 ORDER BY collection, name",
                    &[&slug],
                )?
                .iter()
                .map(|r| json!({ "id": r.get::<_, i64>(0).to_string(), "name": r.get::<_, String>(1), "collection": r.get::<_, String>(2), "hidden": r.get::<_, bool>(3) }))
                .collect();
            let zones: Vec<Value> = db
                .query("SELECT id, name, focus FROM geo.zones WHERE region = $1 ORDER BY focus DESC, name", &[&slug])?
                .iter()
                .map(|r| json!({ "id": r.get::<_, String>(0), "name": r.get::<_, String>(1), "focus": r.get::<_, bool>(2) }))
                .collect();
            let chapters: Vec<Value> = db
                .query(
                    "SELECT key, coalesce(names -> $2 ->> 'title', names -> 'de' ->> 'title', key) \
                     FROM game.chapters WHERE region = $1 AND kind IS NULL ORDER BY starts DESC",
                    &[&slug, &lang],
                )?
                .iter()
                .map(|r| json!({ "id": r.get::<_, String>(0), "name": r.get::<_, String>(1) }))
                .collect();
            Ok(json!({
                "store": store.is_some(),
                "images": images,
                "suggestions": suggestions,
                "targets": { "find": finds, "zone": zones, "challenge": chapters },
                "limits": { "bytes": media::MAX_IN, "side": media::MAX_SIDE },
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

/// `POST /api/ops/media?region=` (editor): upload an image (base64 JSON,
/// at most 8 MB); it waits for a second person's approval.
pub(super) async fn upload(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<UploadIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !media::KINDS.contains(&req.kind.as_str()) {
        return bad("kind: find, zone or challenge");
    }
    let author = match clean_text(&req.author, AUTHOR_MAX, "author: 1-120 characters") {
        Ok(a) if !a.is_empty() => a,
        _ => return bad("author: 1-120 characters"),
    };
    let alt = match clean_text(&req.alt, ALT_MAX, "alt text: at most 200 characters") {
        Ok(a) => Some(a).filter(|a| !a.is_empty()),
        Err(e) => return bad(e),
    };
    let licence = match media::upload_licence(&req.licence) {
        Ok(l) => l,
        Err(e) => return bad(e),
    };
    let source_url = req
        .source_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(String::from);
    if source_url
        .as_deref()
        .is_some_and(|u| u.len() > 500 || !u.starts_with("https://"))
    {
        return bad("source: an https:// link");
    }
    let b64 = req
        .data
        .split_once("base64,")
        .map_or(req.data.as_str(), |(_, d)| d);
    let bytes = match base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
        Ok(b) => b,
        Err(_) => return bad("file unreadable"),
    };
    let st = s.clone();
    let master = w.master;
    let by = w.label;
    let res = tokio::task::spawn_blocking(move || {
        let p = media::process(&bytes).map_err(String::from)?;
        let licence_url = licence.url();
        store_new(
            &st,
            &slug,
            p,
            Meta {
                kind: req.kind,
                target: req.target,
                author,
                licence,
                licence_url,
                source_url,
                commons_title: None,
                alt,
            },
            &by,
            master,
        )
    })
    .await;
    answer(res)
}

/// `POST /api/ops/media/commons?region=` (editor): take a Commons suggestion
/// of a find; the server fetches it at ~1280 px with author and licence.
pub(super) async fn pick(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<PickIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let alt = match clean_text(&req.alt, ALT_MAX, "alt text: at most 200 characters") {
        Ok(a) => Some(a).filter(|a| !a.is_empty()),
        Err(e) => return bad(e),
    };
    let Ok(find) = req.find.parse::<i64>() else {
        return bad("find: a number");
    };
    let st = s.clone();
    let master = w.master;
    let by = w.label;
    let res = tokio::task::spawn_blocking(move || {
        // Only titles the job suggested for this find (no free fetching).
        let (sl, title) = (slug.clone(), req.title.clone());
        let known = st
            .with_db(move |db| {
                db.query_opt(
                    "SELECT 1 FROM geo.commons_suggestions WHERE region = $1 AND find = $2 AND title = $3",
                    &[&sl, &find, &title],
                )
                .map(|r| r.is_some())
            })
            .ok_or("database unavailable")?;
        if !known {
            return Err("not a suggestion for this find".to_string());
        }
        let photo = media::fetch_commons(&req.title).map_err(|e| {
            tracing::info!(error = %e, "media: commons fetch");
            format!("Commons: {e}")
        })?;
        let p = media::process(&photo.bytes).map_err(String::from)?;
        let licence_url = photo.licence_url.or_else(|| photo.licence.url());
        store_new(
            &st,
            &slug,
            p,
            Meta {
                kind: "find".into(),
                target: find.to_string(),
                author: photo.author,
                licence: photo.licence,
                licence_url,
                source_url: Some(photo.page).filter(|p| p.starts_with("https://")),
                commons_title: Some(req.title),
                alt,
            },
            &by,
            master,
        )
    })
    .await;
    answer(res)
}

fn answer(res: Result<Result<i64, String>, tokio::task::JoinError>) -> Response {
    match res {
        Ok(Ok(id)) => Json(json!({ "id": id })).into_response(),
        Ok(Err(e)) if e == "database unavailable" => {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Ok(Err(e)) => bad(&e),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /api/ops/media/decide?region=` (editor): approve or reject a
/// pending image; not by the one who uploaded it. Approving replaces the
/// target's image; rejecting deletes the file (the row stays for the record).
pub(super) async fn decide(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<DecideIn>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let st = s.clone();
    let master = w.master;
    let by = w.label;
    let res = tokio::task::spawn_blocking(move || {
        let gone = st.with_db(|db| {
            let mut t = db.transaction()?;
            let Some(r) = t.query_opt(
                "SELECT status, uploaded_by, kind, target, s3_key FROM geo.media \
                 WHERE region = $1 AND id = $2 FOR UPDATE",
                &[&slug, &req.id],
            )?
            else {
                return Ok(Err("no such image"));
            };
            let (status, uploaded_by, kind, target, key): (String, String, String, String, String) =
                (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4));
            if status != "pending" {
                return Ok(Err("already decided"));
            }
            if let Err(e) = media::may_decide(&uploaded_by, &by, master) {
                return Ok(Err(e));
            }
            let mut gone: Vec<String> = Vec::new();
            if req.approve {
                for r in t.query(
                    "DELETE FROM geo.media WHERE region = $1 AND kind = $2 AND target = $3 \
                     AND status = 'approved' RETURNING s3_key",
                    &[&slug, &kind, &target],
                )? {
                    gone.push(r.get(0));
                }
                t.execute(
                    "UPDATE geo.media SET status = 'approved', approved_by = $2, decided_at = now() WHERE id = $1",
                    &[&req.id, &by],
                )?;
            } else {
                // approved_by: who decided.
                t.execute(
                    "UPDATE geo.media SET status = 'rejected', approved_by = $2, decided_at = now() WHERE id = $1",
                    &[&req.id, &by],
                )?;
                gone.push(key);
            }
            audit_entity(
                &mut t,
                &format!("media {slug}/{}", req.id),
                if req.approve { "approve" } else { "reject" },
                &by,
                json!({ "kind": kind, "target": target }),
            )?;
            t.commit()?;
            Ok(Ok(gone))
        });
        match gone {
            Some(Ok(keys)) => {
                if let Some(store) = media::store() {
                    for k in keys {
                        if let Err(e) = store.delete(&k) {
                            tracing::warn!(error = %e, "media: delete failed");
                        }
                    }
                }
                Some(Ok(()))
            }
            Some(Err(e)) => Some(Err(e)),
            None => None,
        }
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Some(Err(e)) => bad(e),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `DELETE /api/ops/media?region=&id=` (editor): remove an image (any state)
/// with its file.
pub(super) async fn remove(
    State(s): State<AppState>,
    Query(q): Query<IdQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let w = who(&s, c, &headers).await;
    if !w.may(&slug, Role::Editor) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let st = s.clone();
    let (by, id) = (w.label, q.id);
    let res = tokio::task::spawn_blocking(move || {
        let row = st.with_db(|db| {
            let mut t = db.transaction()?;
            let r = t.query_opt(
                "DELETE FROM geo.media WHERE region = $1 AND id = $2 RETURNING s3_key, status",
                &[&slug, &id],
            )?;
            if r.is_some() {
                audit_entity(
                    &mut t,
                    &format!("media {slug}/{id}"),
                    "delete",
                    &by,
                    Value::Null,
                )?;
            }
            t.commit()?;
            Ok(r.map(|r| (r.get::<_, String>(0), r.get::<_, String>(1))))
        })?;
        if let (Some((key, status)), Some(store)) = (&row, media::store()) {
            if status != "rejected" {
                if let Err(e) = store.delete(key) {
                    tracing::warn!(error = %e, "media: delete failed");
                }
            }
        }
        Some(row.is_some())
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(true) => StatusCode::NO_CONTENT.into_response(),
        Some(false) => bad("no such image"),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `GET /api/media?region=` (public): the region's approved images with
/// credit, for the find card, the challenge card and chapter cards.
pub(super) async fn public(State(s): State<AppState>, Query(q): Query<RegionQ>) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let Some(store) = media::store() else {
        return Json(json!([])).into_response();
    };
    let st = s.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            Ok(db
                .query(
                    "SELECT m.id, m.kind, m.target, m.s3_key, m.width, m.height, m.author, m.licence, \
                     m.licence_version, m.licence_url, m.source_url, m.alt, z.name \
                     FROM geo.media m LEFT JOIN geo.zones z \
                       ON m.kind = 'zone' AND z.region = m.region AND z.id = m.target \
                     WHERE m.region = $1 AND m.status = 'approved' ORDER BY m.id",
                    &[&slug],
                )?
                .iter()
                .map(|r| {
                    let lic = stored_licence(r.get(7), r.get(8));
                    json!({
                        "kind": r.get::<_, String>(1),
                        "target": r.get::<_, String>(2),
                        "url": store.url(r.get(3), r.get(0)),
                        "w": r.get::<_, i32>(4),
                        "h": r.get::<_, i32>(5),
                        "author": r.get::<_, String>(6),
                        "licence": lic.label(),
                        "licence_url": r.get::<_, Option<String>>(9),
                        "source": r.get::<_, Option<String>>(10),
                        "alt": r.get::<_, Option<String>>(11),
                        "name": r.get::<_, Option<String>>(12),
                    })
                })
                .collect::<Vec<_>>())
        })
    })
    .await
    .ok()
    .flatten();
    match out {
        Some(v) => ([(header::CACHE_CONTROL, "public, max-age=300")], Json(v)).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `GET /api/media/{id}` (public, `ELEMENTS_MEDIA_PRIVATE=1` only): a
/// redirect to a presigned URL of an approved image, cached half an hour.
pub(super) async fn file(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let Some(store) = media::store().filter(|s| s.private) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let st = s.clone();
    let key = tokio::task::spawn_blocking(move || {
        st.with_db(|db| {
            Ok(db
                .query_opt(
                    "SELECT s3_key FROM geo.media WHERE id = $1 AND status = 'approved'",
                    &[&id],
                )?
                .map(|r| r.get::<_, String>(0)))
        })
    })
    .await
    .ok()
    .flatten();
    match key {
        Some(Some(k)) => (
            StatusCode::FOUND,
            [
                (header::LOCATION, store.presigned(&k)),
                (header::CACHE_CONTROL, "private, max-age=1800".into()),
            ],
        )
            .into_response(),
        Some(None) => StatusCode::NOT_FOUND.into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_and_licences() {
        assert_eq!(
            clean_text("  Stadt Konstanz ", 120, "x"),
            Ok("Stadt Konstanz".into())
        );
        assert!(clean_text("a\nb", 120, "x").is_err());
        assert!(clean_text(&"x".repeat(121), 120, "x").is_err());
        let l = stored_licence("cc-by", Some("4.0".into()));
        assert_eq!(l.label().as_deref(), Some("CC BY 4.0"));
        assert_eq!(stored_licence("weird", None).code, "own");
    }
}

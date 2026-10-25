//! Web gateway: HTTP + WebSocket/JSON for browser clients. Runs its own tokio
//! runtime in a thread and talks to Bevy through channels (see `web.rs`).
//!
//! Protocol: JSON text frames tagged by `t`, typed in `protocol.rs` (the app's
//! `web/src/protocol.ts` is generated from it).
//!
//! Every region-bound path takes `?region=<slug>` (default: `ELEMENTS_REGION`);
//! `GET /api/regions` lists the active regions (outline, language, and `url`
//! when another shard runs one). The socket is `/ws?region=<slug>`.
//!
//! HTTP: `GET /api/region/meta` (settings and tile index), `GET /api/tile/{x}/{y}`
//! (zoom `tiles::TILE_Z`), `GET /api/overview` (territory per tile; `&peek=1`: 204 unless loaded), `GET /api/region`
//! (whole graph as GeoJSON, built on first use; dashboard and bots),
//! `GET /api/leaderboard`, `GET /api/version` (public; also `city`, `demo_label`, `source`),
//! `GET /api/analytics?days=&data=real|synthetic|all` (public and cached 10 min for
//! the demo day, `day=demo&data=synthetic`)
//! `POST /api/ops/hotspot {node, minutes}` and `POST /api/ops/reload` (all need a token: `ELEMENTS_OPS_TOKEN`
//! for all regions, or one from `ops.tokens` for its region; `GET /api/access` says which).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Extension, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};

use crate::limits::{self, Client};
use crate::protocol::{ClientMsg, ServerMsg};
use crate::regions::{ConnGuard, Host, Region, Unavailable};
use crate::tiles::TileKey;

/// Most tiles one browser may watch at once.
const MAX_VIEW_TILES: usize = 256;

/// Per-connection outbound queue of JSON text frames.
pub type Outbox = mpsc::UnboundedSender<String>;

/// A JSON document the Bevy side fills and the gateway serves.
pub type SharedJson = Arc<RwLock<Option<Arc<str>>>>;

/// Side of every junction (`/api/ops/nodes`): built by the region only after
/// a request set `wanted`, so nobody pays for it otherwise.
#[derive(Clone, Default)]
pub struct NodesShare {
    pub json: SharedJson,
    pub wanted: Arc<std::sync::atomic::AtomicBool>,
}

/// Events from browser connections into Bevy.
pub enum FromWeb {
    Hello {
        conn: u64,
        token: Option<String>,
        /// The stored player of `token` (read here, so the region is current).
        global: Option<crate::score::Global>,
        bot: bool,
        /// Loads by tiles: state only for the tiles of its `view`.
        tiles: bool,
        /// A valid proof of work came along (needed for a new identity).
        pow_ok: bool,
        /// Only watching: no player row, nothing counted.
        watch: bool,
        out: Outbox,
    },
    View {
        conn: u64,
        tiles: Vec<TileKey>,
    },
    Overview {
        conn: u64,
        on: bool,
    },
    Rate {
        conn: u64,
        node: i64,
        cat: String,
    },
    OpsHotspot {
        node: i64,
        minutes: f64,
    },
    /// Zones and hotspot sites changed (editor).
    Places(crate::places::Places),
    /// The visible finds changed (editor).
    Finds(Vec<crate::db::FindRow>),
    Pos {
        conn: u64,
        lat: f64,
        lon: f64,
        speed: Option<f32>,
        /// Reported accuracy in m (plausibility check).
        acc: Option<f32>,
    },
    Claim {
        conn: u64,
        node: i64,
        via: Option<String>,
    },
    Rename {
        conn: u64,
    },
    Hide {
        conn: u64,
        on: bool,
    },
    Forget {
        conn: u64,
    },
    Profile {
        conn: u64,
        age: Option<String>,
        local: Option<bool>,
    },
    Code {
        conn: u64,
    },
    Ability {
        conn: u64,
        name: String,
    },
    /// A recovery code moved a player to `token` (after `Rekey`).
    Recovered {
        conn: u64,
        token: String,
    },
    /// Every region: a player's id changed (recovery code).
    Rekey {
        old: [u8; 32],
        new: [u8; 32],
    },
    /// Every other region: a player was deleted.
    Evict {
        id: [u8; 32],
    },
    /// Every region: write the players now, then `ack` (from the writer).
    Flush {
        ack: crossbeam_channel::Sender<()>,
    },
    /// Test servers only: switch this player to `faction`.
    Side {
        conn: u64,
        faction: String,
    },
    /// Cheer for a side (`cheer.rs`).
    Cheer {
        conn: u64,
        side: String,
    },
    /// Test servers only: set this player's level.
    Level {
        conn: u64,
        level: u8,
    },
    /// A tag line for an own permanent source.
    Motto {
        conn: u64,
        node: i64,
        ticket: String,
        motto: Option<String>,
    },
    /// Link two own permanent sources, or unlink one.
    Link {
        conn: u64,
        node: i64,
        ticket: String,
        to: Option<i64>,
        to_ticket: Option<String>,
    },
    /// Opt in to (or out of) collecting finds.
    Collect {
        conn: u64,
        on: bool,
    },
    /// Count in the analytics for the city, or not (objection, Art. 21 GDPR).
    Stats {
        conn: u64,
        on: bool,
    },
    /// A presence code of this region (looked up here): confirm it if the
    /// player is within `radius_m` of its place.
    Presence {
        conn: u64,
        code: String,
        lat: f64,
        lon: f64,
        radius_m: f32,
    },
    /// Place a permanent source on an own claim, or top one up (`keep.rs`).
    Keep {
        conn: u64,
        node: i64,
        ticket: Option<String>,
    },
    Redeem {
        conn: u64,
        ticket: String,
    },
    /// What the card of a tapped junction shows (`info.rs`).
    Info {
        conn: u64,
        node: i64,
    },
    /// Free the permanent-source slots whose tickets the app lacks (lost device).
    Release {
        conn: u64,
        tickets: Vec<String>,
    },
    Bye {
        conn: u64,
    },
}

/// Settings for the HTTP side.
#[derive(Clone)]
pub struct GatewayConfig {
    pub web_dir: Option<PathBuf>,
    /// Served before `web_dir`: basemap and media kept on the host.
    pub static_dir: Option<PathBuf>,
    pub ops_token: Option<String>,
    /// `ELEMENTS_DB_READ`: the dashboard's reads (none: the main connection).
    pub db_read: Option<String>,
}

#[derive(Clone)]
struct AppState {
    cfg: GatewayConfig,
    host: Arc<Host>,
    next_conn: Arc<AtomicU64>,
    /// For player lookups and recovery (one connection, reopened on error).
    db: Arc<std::sync::Mutex<crate::db::Reconnecting>>,
    /// The dashboard's reads ([`AppState::with_read`]).
    db_read: Arc<std::sync::Mutex<crate::db::Reconnecting>>,
}

impl AppState {
    /// Run `f` on the gateway's database connection (blocking; call from
    /// `spawn_blocking`). `None` without a database or on error.
    fn with_db<T>(
        &self,
        f: impl FnOnce(&mut postgres::Client) -> Result<T, postgres::Error>,
    ) -> Option<T> {
        self.run_on(&self.db, self.host.db()?, f)
    }

    /// The read-only connection string (`ELEMENTS_DB_READ`, else the main one).
    pub fn read_db(&self) -> Option<&str> {
        self.cfg.db_read.as_deref().or(self.host.db())
    }

    /// Like [`AppState::with_db`], on the read-only connection: dashboard
    /// reads (geo and analytics only).
    pub fn with_read<T>(
        &self,
        f: impl FnOnce(&mut postgres::Client) -> Result<T, postgres::Error>,
    ) -> Option<T> {
        self.run_on(&self.db_read, self.read_db()?, f)
    }

    fn run_on<T>(
        &self,
        db: &std::sync::Mutex<crate::db::Reconnecting>,
        conn: &str,
        f: impl FnOnce(&mut postgres::Client) -> Result<T, postgres::Error>,
    ) -> Option<T> {
        let mut db = db.lock().unwrap();
        match f(db.get(conn)?) {
            Ok(v) => Some(v),
            Err(e) => {
                // The server's message, not just "db error" (no DETAIL: values).
                match e.as_db_error() {
                    Some(d) => db.failed("query", &format!("{}: {}", d.code().code(), d.message())),
                    None => db.failed("query", &e),
                }
                None
            }
        }
    }
}

/// `?region=<slug>`; none = the default region.
#[derive(Deserialize, Default)]
struct RegionQ {
    region: Option<String>,
    /// Only if loaded: never wakes the region (neighbours in the map).
    peek: Option<String>,
}

// Err is the ready answer (404/503), as axum handlers return it.
#[allow(clippy::result_large_err)]
impl AppState {
    /// The slug asked for (or the default), if it is an active region.
    fn slug(&self, asked: Option<&str>) -> Result<String, Response> {
        self.host
            .resolve(asked)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "unknown region").into_response())
    }

    /// The running region, started if needed.
    fn region(&self, asked: Option<&str>) -> Result<Arc<Region>, Response> {
        let slug = self.slug(asked)?;
        self.host.get(&slug).map_err(|u| match u {
            Unavailable::Unknown => (StatusCode::NOT_FOUND, "unknown region").into_response(),
            Unavailable::Elsewhere(url) => (
                StatusCode::MISDIRECTED_REQUEST,
                Json(serde_json::json!({ "url": url })),
            )
                .into_response(),
            Unavailable::Busy => not_loaded(),
            // Mid-move the proxy lags a few seconds behind: the app asks again.
            Unavailable::OtherCell => not_loaded(),
        })
    }

    /// Like [`AppState::region`], and loaded (tiles there).
    fn ready(&self, asked: Option<&str>) -> Result<Arc<Region>, Response> {
        let r = self.region(asked)?;
        if r.ready() {
            Ok(r)
        } else {
            Err(not_loaded())
        }
    }
}

/// Content policy for the pages: scripts only from here, connections to here,
/// the other shards and the weather host if set (`web/src/radar.ts`; the
/// basemap fallback to OpenFreeMap exists in dev builds only, which the Vite
/// server serves without this header).
fn csp(shards: &[String]) -> Arc<str> {
    let mut connect = String::from("'self'");
    if let Some(w) = crate::radar::base() {
        connect.push(' ');
        connect.push_str(w);
    }
    for u in shards {
        connect.push(' ');
        connect.push_str(u.trim_end_matches('/'));
        if let Some(host) = u.strip_prefix("https://") {
            connect.push_str(&format!(" wss://{}", host.trim_end_matches('/')));
        }
    }
    // Images: the media bucket, and Commons thumbnails in the editor.
    let mut img =
        String::from("'self' data: blob: https://upload.wikimedia.org https://thumb.wikimedia.org");
    if let Some(o) = crate::media::store().and_then(|s| s.origin()) {
        img.push(' ');
        img.push_str(&o);
    }
    format!(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src {img}; font-src 'self'; connect-src {connect}; \
         worker-src 'self' blob:; child-src 'self' blob:; media-src 'self' blob:; \
         object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'self'"
    )
    .into()
}

/// Every request: the client's HTTP budget (429 when spent), the client for
/// the handlers, and security headers.
async fn guard(
    State(csp): State<Arc<str>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let c = Client::of(peer, req.headers());
    if !limits::HTTP.allow(c) {
        return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "10")]).into_response();
    }
    // A socket on the page's own host, for browsers whose 'self' misses wss.
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| {
            h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
        })
        .map(String::from);
    req.extensions_mut().insert(c);
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    let set = |h: &mut HeaderMap, k: &'static str, v: &str| {
        if let Ok(v) = header::HeaderValue::from_str(v) {
            h.insert(k, v);
        }
    };
    set(h, "x-content-type-options", "nosniff");
    set(h, "referrer-policy", "no-referrer");
    set(h, "x-frame-options", "SAMEORIGIN");
    set(h, "strict-transport-security", "max-age=31536000");
    set(
        h,
        "permissions-policy",
        "geolocation=(self), camera=(self), accelerometer=(self), gyroscope=(self), \
         magnetometer=(self), microphone=(), payment=(), usb=()",
    );
    let html = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if html {
        let csp = match host {
            Some(host) => csp.replacen("connect-src ", &format!("connect-src wss://{host} "), 1),
            None => csp.to_string(),
        };
        set(h, "content-security-policy", &csp);
    }
    res
}

/// Pages are revalidated on every visit so a deploy shows at once; built
/// assets carry a content hash in their name and may be kept for a year.
/// API answers set their own headers.
async fn cache_headers(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_owned();
    let mut res = next.run(req).await;
    let value = if path.starts_with("/api/") || path == "/ws" {
        return res;
    } else if path.starts_with("/assets/") {
        "public, max-age=31536000, immutable"
    } else if path.starts_with("/basemap/") || path.starts_with("/media/") {
        "public, max-age=86400"
    } else {
        "no-cache"
    };
    if !res.headers().contains_key(header::CACHE_CONTROL) {
        res.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static(value),
        );
    }
    res
}

/// Set on SIGTERM / Ctrl-C; Bevy flushes analytics and exits.
pub static SHUTDOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn wait_for_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    tracing::info!("shutdown requested");
    SHUTDOWN.store(true, Ordering::Relaxed);
}

pub fn spawn(bind: SocketAddr, cfg: GatewayConfig, host: Arc<Host>) {
    let web_dir = cfg.web_dir.clone();
    let static_dir = cfg.static_dir.clone();
    let shard_urls: Vec<String> = crate::shards::get().urls.values().cloned().collect();
    let state = AppState {
        cfg,
        host,
        next_conn: Arc::default(),
        db: Arc::new(std::sync::Mutex::new(crate::db::Reconnecting::new(
            "gateway",
        ))),
        db_read: Arc::new(std::sync::Mutex::new(crate::db::Reconnecting::new(
            "gateway read",
        ))),
    };
    std::thread::Builder::new()
        .name("web-gateway".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                tokio::spawn(wait_for_signal());
                let mut app = Router::new()
                    .route("/api/regions", get(regions_handler))
                    .route("/api/marble/manifest.json", get(marble_manifest))
                    .route("/api/marble/shards.json", get(marble_index))
                    .route("/api/marble/{slug}/{file}", get(marble_file))
                    .route("/api/wish", post(wish_handler))
                    .route("/api/wishes", get(wishes_handler))
                    .route(
                        "/api/feedback",
                        post(feedback_handler).get(feedback_list_handler),
                    )
                    .route("/api/access", get(access_handler))
                    .route("/api/region", get(region_handler))
                    .route("/api/region/meta", get(meta_handler))
                    .route("/api/tile/{x}/{y}", get(tile_handler))
                    .route("/api/overview", get(overview_handler))
                    .route("/api/leaderboard", get(board_handler))
                    .route("/api/version", get(version_handler))
                    .route("/api/health", get(health_handler))
                    .route("/api/session", post(session_handler))
                    .route("/api/passport", post(passport_handler))
                    .route("/api/shard/event", post(shard_event_handler))
                    .route("/api/shard/recover", post(shard_recover_handler))
                    .route("/ws", get(ws_handler))
                    .route("/api/analytics", get(analytics_handler))
                    .route("/api/report", get(report::report))
                    .route("/api/areas", get(areas::areas))
                    .route("/api/export/{name}", get(export_handler))
                    .route("/api/ops/hotspot", post(ops_hotspot))
                    .route("/api/ops/reload", post(ops_reload))
                    .route("/api/ops/nodes", get(ops_nodes))
                    .route(
                        "/api/ops/chapters",
                        get(editor::list).post(editor::save).delete(editor::delete),
                    )
                    .route("/api/ops/chapters/suggest", get(editor::suggest))
                    .route(
                        "/api/ops/mottos",
                        get(editor_mottos::list)
                            .post(editor_mottos::save)
                            .delete(editor_mottos::delete),
                    )
                    .route(
                        "/api/ops/places",
                        get(editor_places::list).post(editor_places::save),
                    )
                    .route(
                        "/api/ops/media",
                        get(editor_media::list)
                            .post(editor_media::upload)
                            .delete(editor_media::remove)
                            // base64 of 8 MB
                            .layer(axum::extract::DefaultBodyLimit::max(12 << 20)),
                    )
                    .route("/api/ops/media/commons", post(editor_media::pick))
                    .route("/api/ops/media/decide", post(editor_media::decide))
                    .route(
                        "/api/ops/notices",
                        get(editor_notices::list)
                            .post(editor_notices::save)
                            .delete(editor_notices::delete),
                    )
                    .route("/api/chronicle", get(chronicle::chronicle))
                    .route("/api/media", get(editor_media::public))
                    .route("/api/media/{id}", get(editor_media::file));
                for code in LINKS {
                    app = app.route(&format!("/{code}"), get(link_handler));
                }
                let mut app = app.with_state(state);
                let web = web_dir.map(|dir| {
                    let index = ServeFile::new(dir.join("index.html"));
                    ServeDir::new(dir).fallback(index)
                });
                app = match (static_dir, web) {
                    (Some(st), Some(web)) => app.fallback_service(ServeDir::new(st).fallback(web)),
                    (None, Some(web)) => app.fallback_service(web),
                    (Some(st), None) => app.fallback_service(ServeDir::new(st)),
                    (None, None) => app,
                };
                let csp = csp(&shard_urls);
                let app = app
                    .layer(axum::middleware::from_fn(cache_headers))
                    .layer(axum::middleware::from_fn_with_state(csp, guard))
                    .layer(CompressionLayer::new());
                let listener = tokio::net::TcpListener::bind(bind)
                    .await
                    .unwrap_or_else(|e| panic!("web gateway bind {bind}: {e}"));
                tracing::info!(%bind, "web gateway listening");
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                .expect("web gateway");
            });
        })
        .expect("gateway thread");
}

/// `ELEMENTS_VERSION` (commit + date, set by the image build), else "dev".
pub fn version() -> String {
    std::env::var("ELEMENTS_VERSION").unwrap_or_else(|_| "dev".into())
}

/// What the pages show per region, from the region (the env only where it has
/// none): the city (region name up to " – ", `ELEMENTS_CITY`), the name and
/// day of the demo (`demo_label`/`demo_day`, `ELEMENTS_DEMO_LABEL`/
/// `ELEMENTS_DEMO_DAY`). The label is a
/// key (e.g. `christmas-saturday`) the pages translate.
#[derive(Clone, Default)]
struct Instance {
    city: String,
    /// Fallback language of the app (the region's).
    language: String,
    demo_label: String,
    demo_day: String,
}

fn instance(s: &AppState, slug: &str) -> Instance {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let l = s.host.listing(slug);
    let r = l.as_ref().map(|l| &l.json);
    let get = |k: &str| r.and_then(|r| r[k].as_str()).map(String::from);
    Instance {
        // The region's own values first; the env only fills gaps (one
        // container serves many regions).
        city: get("city")
            .filter(|v| !v.is_empty())
            .or_else(|| env("ELEMENTS_CITY"))
            .unwrap_or_default(),
        language: get("language").unwrap_or_else(|| "de".into()),
        demo_label: get("demo_label")
            .filter(|v| !v.is_empty())
            .or_else(|| env("ELEMENTS_DEMO_LABEL"))
            .unwrap_or_else(|| "christmas-saturday".into()),
        demo_day: l
            .as_ref()
            .and_then(|l| l.demo_day.clone())
            .or_else(|| s.host.demo_day().map(String::from))
            .unwrap_or_else(|| crate::db::DEFAULT_DEMO_DAY.into()),
    }
}

/// The source code of this server (`ELEMENTS_SOURCE_URL`, https only); the
/// app's footers link it. Unset: no link.
fn source_url() -> Option<&'static str> {
    static URL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    URL.get_or_init(|| source_from(&std::env::var("ELEMENTS_SOURCE_URL").ok()?))
        .as_deref()
}

fn source_from(u: &str) -> Option<String> {
    let u = u.trim();
    (u.len() > "https://".len() && u.starts_with("https://") && !u.contains(char::is_whitespace))
        .then(|| u.to_string())
}

async fn version_handler(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
) -> Json<serde_json::Value> {
    // `shard`: this server's shard (its weather values); `weather`: the data
    // host for the map's weather layers, absent when off.
    let mut v =
        serde_json::json!({ "version": version(), "shard": s.host.shard(), "cell": s.host.cell() });
    if let Some(w) = crate::radar::base() {
        v["weather"] = w.into();
    }
    // `source`: where the running code is published (AGPL), absent when unset.
    if let Some(u) = source_url() {
        v["source"] = u.into();
    }
    if let Ok(slug) = s.slug(q.region.as_deref()) {
        let i = instance(&s, &slug);
        v["region"] = slug.clone().into();
        v["city"] = i.city.into();
        // "im Furttal" instead of "in Furttal" (per language, if the region sets it).
        v["city_in"] = s
            .host
            .listing(&slug)
            .map(|l| l.json["city_in"].clone())
            .unwrap_or_default();
        v["demo_label"] = i.demo_label.into();
        v["demo_day"] = i.demo_day.into();
        v["language"] = i.language.into();
    }
    Json(v)
}

/// Last database check: (when, ok). Checked at most every 30 s.
static DB_OK: std::sync::Mutex<Option<(std::time::Instant, bool)>> = std::sync::Mutex::new(None);

/// `GET /api/health` (external monitoring): 200 if the database answers and
/// the default region is loaded, else 503; `{ok, db, region, version}`.
async fn health_handler(State(s): State<AppState>) -> Response {
    // Quiet server: the last answer, no query (a monitor mustn't keep the
    // database awake; it scales to zero).
    let quiet = s.host.quiet();
    let cached = DB_OK
        .lock()
        .unwrap()
        .filter(|(t, _)| quiet || t.elapsed().as_secs() < 30)
        .map(|(_, ok)| ok)
        .or(quiet.then_some(true));
    let db = match cached {
        Some(ok) => ok,
        None => {
            let st = s.clone();
            let ok = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                // Only over a connection already open: never one just for
                // the check (no connection: ok).
                tokio::task::spawn_blocking(move || {
                    let mut db = st.db.lock().unwrap();
                    match db.open() {
                        Some(c) => c.simple_query("SELECT 1").is_ok(),
                        None => true,
                    }
                }),
            )
            .await
            .is_ok_and(|r| r.unwrap_or(false));
            *DB_OK.lock().unwrap() = Some((std::time::Instant::now(), ok));
            ok
        }
    };
    // Known is enough: health doesn't load (or wake) the default region.
    let region = s.host.default_known();
    let ok = db && region;
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({ "ok": ok, "db": db, "region": region, "version": version() })),
    )
        .into_response()
}

/// The active regions (all shards).
/// `?v=2`: outlines packed (`regions::pack_outline`). An ETag lets the app
/// revalidate the list instead of loading it again.
async fn regions_handler(
    State(s): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let (body, tag) = s.host.list_json(q.get("v").is_some_and(|v| v == "2"));
    // Caddy's compression marks the tag ("-gzip", weak); compare the core.
    let core = |t: &str| {
        t.trim_start_matches("W/")
            .trim_matches('"')
            .split('-')
            .next()
            .unwrap_or("")
            .to_owned()
    };
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| core(t.trim()) == core(&tag)));
    if fresh {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, tag.as_ref())]).into_response();
    }
    let mut r = json_response(&body, "max-age=300");
    if let Ok(v) = header::HeaderValue::from_str(&tag) {
        r.headers_mut().insert(header::ETAG, v);
    }
    r
}

/// Globe files (`marble.rs`), as published; CORS for the origins in
/// `ELEMENTS_MARBLE_ORIGINS` and local development.
async fn marble_manifest(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let m = s.host.marble.published.lock().unwrap().manifest.clone();
    match m {
        Some(m) => marble_cors(&headers, json_response(&m, "public, max-age=60")),
        None => (StatusCode::SERVICE_UNAVAILABLE, "not published yet").into_response(),
    }
}

/// The shard index (eu only; `marble::index`).
async fn marble_index(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let m = s.host.marble.published.lock().unwrap().index.clone();
    match m {
        Some(m) => marble_cors(&headers, json_response(&m, "public, max-age=300")),
        None => (StatusCode::NOT_FOUND, "no shard index here").into_response(),
    }
}

async fn marble_file(
    State(s): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path((slug, file)): axum::extract::Path<(String, String)>,
) -> Response {
    let f = s
        .host
        .marble
        .published
        .lock()
        .unwrap()
        .files
        .get(&format!("{slug}/{file}"))
        .cloned();
    match f {
        Some(gz) => marble_cors(
            &headers,
            (
                [
                    (header::CONTENT_TYPE, "application/octet-stream"),
                    (header::CONTENT_ENCODING, "gzip"),
                    (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                ],
                axum::body::Bytes::from_owner((*gz).clone()),
            )
                .into_response(),
        ),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Allowed origins for the globe files, comma or space separated.
static MARBLE_ORIGINS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    std::env::var("ELEMENTS_MARBLE_ORIGINS")
        .unwrap_or_default()
        .split([',', ' '])
        .map(|o| o.trim().trim_end_matches('/').to_owned())
        .filter(|o| !o.is_empty())
        .collect()
});

fn marble_cors(req: &HeaderMap, mut res: Response) -> Response {
    let origin = req
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .unwrap_or("");
    let ok = MARBLE_ORIGINS.iter().any(|o| o == origin)
        || origin.starts_with("http://localhost:")
        || origin.starts_with("http://127.0.0.1:");
    if ok {
        if let Ok(v) = header::HeaderValue::from_str(origin) {
            res.headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
            res.headers_mut()
                .insert(header::VARY, header::HeaderValue::from_static("Origin"));
        }
    }
    res
}

async fn board_handler(State(s): State<AppState>, Query(q): Query<RegionQ>) -> Response {
    let r = match s.region(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let v = r.board.read().unwrap().clone();
    match v {
        Some(json) => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "max-age=10"),
            ],
            json.to_string(),
        )
            .into_response(),
        None => (StatusCode::SERVICE_UNAVAILABLE, "no scores yet").into_response(),
    }
}

fn json_response(json: &str, cache: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, cache),
        ],
        json.to_string(),
    )
        .into_response()
}

/// The region is loading (or unloading): ask again in a moment.
fn not_loaded() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "2")],
        "region loading",
    )
        .into_response()
}

/// Whole graph as GeoJSON (dashboard, bots, tools). Big for a whole city, so
/// built only on first use.
async fn region_handler(State(s): State<AppState>, Query(q): Query<RegionQ>) -> Response {
    let r = match s.ready(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let Some(db) = s.host.db().map(String::from) else {
        return not_loaded();
    };
    let region = r.slug.clone();
    let json = r
        .full
        .get_or_try_init(|| async move {
            tokio::task::spawn_blocking(move || {
                crate::db::load_region_json(&db, &region).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())?
            .map(Arc::<str>::from)
        })
        .await;
    match json {
        Ok(json) => json_response(json, "no-cache"),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// Region settings and the tile index.
async fn meta_handler(State(s): State<AppState>, Query(q): Query<RegionQ>) -> Response {
    let r = match s.region(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let v = r.tiles.read().unwrap().clone();
    match v {
        Some(t) => json_response(&t.meta, "no-cache"),
        None => not_loaded(),
    }
}

/// One tile of the graph; fixed until the next start (new region data), so
/// browsers keep it for an hour.
async fn tile_handler(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    axum::extract::Path((x, y)): axum::extract::Path<(u32, u32)>,
) -> Response {
    let r = match s.ready(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let Some(t) = r.tiles.read().unwrap().clone() else {
        return not_loaded();
    };
    match t.tiles.get(&(x, y)) {
        Some(tile) => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CONTENT_ENCODING, "gzip"),
                (header::CACHE_CONTROL, "public, max-age=3600"),
                (header::VARY, "Accept-Encoding"),
            ],
            axum::body::Bytes::from_owner(tile.gz.clone()),
        )
            .into_response(),
        None => json_response(r#"{"n":[],"e":[]}"#, "public, max-age=3600"),
    }
}

/// Territory per tile, for the zoomed-out map; with `peek`, 204 unless the
/// region is loaded anyway.
async fn overview_handler(State(s): State<AppState>, Query(q): Query<RegionQ>) -> Response {
    let r = if q.peek.is_some() {
        let slug = match s.slug(q.region.as_deref()) {
            Ok(v) => v,
            Err(e) => return e,
        };
        match s.host.peek(&slug) {
            Some(r) => r,
            None => return StatusCode::NO_CONTENT.into_response(),
        }
    } else {
        match s.region(q.region.as_deref()) {
            Ok(r) => r,
            Err(e) => return e,
        }
    };
    let v = r.overview.read().unwrap().clone();
    match v {
        Some(json) => json_response(&json, "max-age=5"),
        None if q.peek.is_some() => StatusCode::NO_CONTENT.into_response(),
        None => not_loaded(),
    }
}

/// Name of the first-party cookie that keeps the session token when the
/// browser drops script storage (Safari after 7 days without a visit).
const TOKEN_COOKIE: &str = "elements_token";

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == TOKEN_COOKIE)
        .map(|(_, v)| v.to_string())
        .filter(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A request from a page of this site, or from no browser at all (bots,
/// curl). Another site's page could otherwise set our cookie (session
/// fixation) or open a socket in the visitor's name.
fn same_origin(headers: &HeaderMap) -> bool {
    let h = |k| headers.get(k).and_then(|v| v.to_str().ok());
    if h("sec-fetch-site").is_some_and(|v| v != "same-origin") {
        return false;
    }
    match h("origin") {
        None => true,
        Some(o) => o.split_once("://").map(|(_, a)| a) == h("host"),
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let busy = || (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "10")]).into_response();
    if !limits::SOCKETS.allow(c) {
        return busy();
    }
    let Some(open) = limits::Open::try_new(c) else {
        return busy();
    };
    let cookie = cookie_token(&headers);
    // A loading region queues the messages until its app is up.
    let r = match s.region(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    ws.max_message_size(limits::MAX_MESSAGE)
        .max_frame_size(limits::MAX_MESSAGE)
        .on_upgrade(move |socket| handle_socket(socket, s, r, cookie, c, open))
}

/// `POST /api/session` with the token as body: sets the token cookie
/// (strictly necessary for the game, § 25(2) no. 2 TDDDG); an empty body
/// deletes it.
async fn session_handler(headers: HeaderMap, body: String) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token = body.trim();
    let valid = token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit());
    let cookie = if valid {
        format!(
            "{TOKEN_COOKIE}={token}; Path=/; Max-Age=34560000; HttpOnly; Secure; SameSite=Strict"
        )
    } else if token.is_empty() {
        format!("{TOKEN_COOKIE}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Strict")
    } else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

/// `POST /api/passport` with the token as body (or the cookie): a signed
/// passport for another shard, `{"p": ...}`; 404 without one.
async fn passport_handler(State(s): State<AppState>, headers: HeaderMap, body: String) -> Response {
    let shards = crate::shards::get();
    let token = Some(body.trim().to_string())
        .filter(|t| t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .or_else(|| cookie_token(&headers));
    let Some(token) = token.filter(|_| shards.enabled()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let id = crate::score::token_id(&token);
    let st = s.clone();
    let p = tokio::task::spawn_blocking(move || {
        // Unwritten changes first (a new player's row, a rename).
        flush_players(&st.host);
        st.with_db(|c| crate::shards::passport_of(c, &shards.me, &id))
    })
    .await
    .ok()
    .flatten()
    .flatten();
    match p.and_then(|p| shards.issue(&p)) {
        Some(p) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({ "p": p })),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Have every loaded region write its players now (a new row, a fresh
/// recovery code), waiting at most 3 s (blocking).
fn flush_players(host: &Host) {
    let (ack, done) = crossbeam_channel::unbounded();
    let n = host.broadcast(None, || FromWeb::Flush { ack: ack.clone() });
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    for _ in 0..n {
        if done.recv_deadline(until).is_err() {
            break;
        }
    }
    crate::cells::flush_others(host);
}

/// The body of a call from another shard, if signed with the shared key.
fn shard_body(headers: &HeaderMap, body: &str) -> Option<()> {
    let sig = headers.get("x-elements-sig")?.to_str().ok()?;
    crate::shards::get().check(sig, body).then_some(())
}

/// `POST /api/shard/event` (signed): a deletion or a badge from another shard.
async fn shard_event_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if shard_body(&headers, &body).is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(ev) = serde_json::from_str::<crate::shards::Event>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let st = s.clone();
    let res = tokio::task::spawn_blocking(move || {
        st.with_db(|c| crate::shards::apply(c, &crate::shards::get().me, &ev))
    })
    .await
    .ok()
    .flatten();
    match res {
        Some(gone) => {
            for id in gone {
                s.host.broadcast(None, || FromWeb::Evict { id });
                crate::cells::evict(id);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
struct CodeBody {
    code: String,
}

/// `POST /api/shard/recover {code}` (signed): redeem a code of a player of
/// this shard for another one; `{"p": passport}` or 404.
async fn shard_recover_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if shard_body(&headers, &body).is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(CodeBody { code }) = serde_json::from_str(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let shards = crate::shards::get();
    let st = s.clone();
    let res = tokio::task::spawn_blocking(move || {
        flush_players(&st.host);
        st.with_db(|c| crate::shards::redeem(c, &shards.me, &code))
    })
    .await
    .ok()
    .flatten()
    .flatten();
    let Some((p, old, new)) = res else {
        return StatusCode::NOT_FOUND.into_response();
    };
    s.host.broadcast(None, || FromWeb::Rekey { old, new });
    crate::cells::rekey(old, new);
    match shards.issue(&p) {
        Some(p) => Json(serde_json::json!({ "p": p })).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A code unknown here, tried on the other shards (blocking). On success the
/// player is taken in here under `token`; `Some(rekey)`.
#[allow(clippy::type_complexity)]
fn recover_elsewhere(
    s: &AppState,
    code: &str,
    token: &str,
    region: &str,
) -> Option<Option<([u8; 32], [u8; 32])>> {
    let shards = crate::shards::get();
    let body = serde_json::json!({ "code": code }).to_string();
    let p = shards.urls.keys().find_map(|name| {
        let answer = shards.call(name, "/api/shard/recover", &body)?;
        let v: serde_json::Value = serde_json::from_str(&answer).ok()?;
        shards.verify(v["p"].as_str()?)
    })?;
    s.with_db(|c| crate::shards::arrive(c, &shards.me, &p, None, token, region))
        .map(|(_, rekey)| rekey)
}

async fn handle_socket(
    socket: WebSocket,
    s: AppState,
    r: Arc<Region>,
    cookie: Option<String>,
    client: Client,
    _open: limits::Open,
) {
    let conn = s.next_conn.fetch_add(1, Ordering::Relaxed);
    let tx = r.tx.clone();
    let slug = r.slug.clone();
    let region = r.clone();
    let _guard = ConnGuard::new(r);
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<String>();

    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut pace = limits::Pace::new(client);
    // Hellos per socket: one, or two with a proof of work in between. Each
    // costs a row lookup, and the app only ever says it once per socket.
    let mut hellos = 0u8;
    // A watcher's socket passes on only what watching needs.
    let mut watching = false;
    // Sockets that never say hello, or go quiet, don't hold a slot for long.
    let opened = std::time::Instant::now();
    let mut last_rx = opened;
    // Region unloaded under the socket (ops reload): close, the app reconnects.
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        let msg = tokio::select! {
            m = stream.next() => m,
            _ = tick.tick() => {
                if region.stopping()
                    || (hellos == 0 && opened.elapsed().as_secs() > limits::HELLO_S)
                    || last_rx.elapsed().as_secs() > limits::IDLE_S
                {
                    break;
                }
                continue;
            }
        };
        let Some(Ok(msg)) = msg else { break };
        last_rx = std::time::Instant::now();
        let Message::Text(text) = msg else { continue };
        if !pace.allow() {
            if pace.dropped >= limits::MSGS_DROPPED_MAX {
                break;
            }
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<ClientMsg>(&text) else {
            continue;
        };
        if watching
            && !matches!(
                parsed,
                ClientMsg::Ping
                    | ClientMsg::View { .. }
                    | ClientMsg::Overview { .. }
                    | ClientMsg::Info { .. }
                    | ClientMsg::Cheer { .. }
            )
        {
            continue;
        }
        let ev = match parsed {
            ClientMsg::Ping => {
                let _ = out.send(ServerMsg::Pong.to_json());
                continue;
            }
            ClientMsg::Hello {
                watch: true, tiles, ..
            } => {
                hellos += 1;
                if hellos > 1 {
                    break;
                }
                watching = true;
                FromWeb::Hello {
                    conn,
                    token: None,
                    global: None,
                    bot: false,
                    tiles,
                    pow_ok: true,
                    watch: true,
                    out: out.clone(),
                }
            }
            ClientMsg::Hello {
                token,
                bot,
                tiles,
                passport,
                pow,
                ..
            } => {
                hellos += 1;
                if hellos > 2 {
                    break;
                }
                let mut token = token.or_else(|| cookie.clone());
                // With a passport: the player's row here first (see shards.rs).
                let shards = crate::shards::get();
                if let Some(p) = passport
                    .filter(|_| !bot)
                    .and_then(|p| shards.redeem_passport(&p))
                {
                    let (st, t0, slug) = (s.clone(), token.clone(), slug.clone());
                    let fresh = format!("{:032x}", rand::random::<u128>());
                    let arrived = tokio::task::spawn_blocking(move || {
                        st.with_db(|c| {
                            crate::shards::arrive(c, &shards.me, &p, t0.as_deref(), &fresh, &slug)
                        })
                    })
                    .await
                    .ok()
                    .flatten();
                    if let Some((t, rekey)) = arrived {
                        if let Some((old, new)) = rekey {
                            s.host.broadcast(None, || FromWeb::Rekey { old, new });
                            crate::cells::rekey(old, new);
                        }
                        token = Some(t);
                    }
                }
                let global = match token.clone().filter(|_| !bot) {
                    Some(t) => {
                        let st = s.clone();
                        tokio::task::spawn_blocking(move || {
                            st.with_db(|c| crate::score::lookup(c, &t)).flatten()
                        })
                        .await
                        .ok()
                        .flatten()
                    }
                    None => None,
                };
                // An unknown token makes a new player: limited per client.
                if global.is_none() && !limits::NEW_PLAYERS.allow(client) {
                    let _ = out.send(ServerMsg::Busy.to_json());
                    break;
                }
                let pow_ok =
                    crate::pow::bits() == 0 || pow.is_some_and(|p| crate::pow::check(&p.c, &p.n));
                FromWeb::Hello {
                    conn,
                    token,
                    global,
                    bot,
                    tiles,
                    pow_ok,
                    watch: false,
                    out: out.clone(),
                }
            }
            ClientMsg::Overview { on } => FromWeb::Overview { conn, on },
            ClientMsg::View { mut tiles } => {
                tiles.truncate(MAX_VIEW_TILES);
                FromWeb::View { conn, tiles }
            }
            ClientMsg::Rate { node, cat } => FromWeb::Rate { conn, node, cat },
            ClientMsg::Pos { lat, lon, spd, acc } => FromWeb::Pos {
                conn,
                lat,
                lon,
                speed: spd,
                acc,
            },
            ClientMsg::Claim { node, via } => FromWeb::Claim { conn, node, via },
            ClientMsg::Keep { node, ticket } => FromWeb::Keep { conn, node, ticket },
            ClientMsg::Redeem { ticket } => FromWeb::Redeem { conn, ticket },
            ClientMsg::Release { mut tickets } => {
                tickets.truncate(32);
                FromWeb::Release { conn, tickets }
            }
            ClientMsg::Info { node } => FromWeb::Info { conn, node },
            ClientMsg::Rename => FromWeb::Rename { conn },
            ClientMsg::Hide { on } => FromWeb::Hide { conn, on },
            ClientMsg::Forget => FromWeb::Forget { conn },
            ClientMsg::Profile { age, local } => FromWeb::Profile { conn, age, local },
            ClientMsg::Code => FromWeb::Code { conn },
            ClientMsg::Ability { name } => FromWeb::Ability { conn, name },
            ClientMsg::Recover { code } => {
                // In the database: the player may come from any region.
                let allowed =
                    limits::RECOVER.allow(client) && RECOVER_LIMIT.lock().unwrap().allow(conn);
                let token = format!("{:032x}", rand::random::<u128>());
                let old = if allowed {
                    let (st, t) = (s.clone(), token.clone());
                    let code = code.clone();
                    tokio::task::spawn_blocking(move || {
                        // A code made minutes ago may not be written yet.
                        flush_players(&st.host);
                        st.with_db(|c| {
                            let old = crate::score::recover(c, &code, &t)?;
                            if let Some(old) = &old {
                                crate::shards::queue_rekey(c, old, &crate::score::token_id(&t))?;
                            }
                            Ok(old)
                        })
                        .flatten()
                    })
                    .await
                    .ok()
                    .flatten()
                } else {
                    None
                };
                // Unknown here: maybe a player of another shard.
                if old.is_none() && allowed {
                    let (st, t, slug) = (s.clone(), token.clone(), slug.clone());
                    let away = tokio::task::spawn_blocking(move || {
                        recover_elsewhere(&st, &code, &t, &slug)
                    })
                    .await
                    .ok()
                    .flatten();
                    if let Some(rekey) = away {
                        if let Some((old, new)) = rekey {
                            s.host.broadcast(None, || FromWeb::Rekey { old, new });
                            crate::cells::rekey(old, new);
                        }
                        if tx.send(FromWeb::Recovered { conn, token }).is_err() {
                            break;
                        }
                        continue;
                    }
                }
                let Some(old) = old else {
                    let reason = if allowed { "unknown" } else { "wait" };
                    let _ = out.send(ServerMsg::Recover { ok: false, reason }.to_json());
                    continue;
                };
                let new = crate::score::token_id(&token);
                s.host.broadcast(None, || FromWeb::Rekey { old, new });
                crate::cells::rekey(old, new);
                FromWeb::Recovered { conn, token }
            }
            ClientMsg::Side { faction } => FromWeb::Side { conn, faction },
            ClientMsg::Cheer { side } => FromWeb::Cheer { conn, side },
            ClientMsg::Level { level } => FromWeb::Level { conn, level },
            ClientMsg::Motto {
                node,
                ticket,
                motto,
            } => FromWeb::Motto {
                conn,
                node,
                ticket,
                motto,
            },
            ClientMsg::Link {
                node,
                ticket,
                to,
                to_ticket,
            } => FromWeb::Link {
                conn,
                node,
                ticket,
                to,
                to_ticket,
            },
            ClientMsg::Collect { on } => FromWeb::Collect { conn, on },
            ClientMsg::Stats { on } => FromWeb::Stats { conn, on },
            ClientMsg::Presence { code } => {
                let refuse = |reason: &'static str| {
                    let msg = ServerMsg::Presence {
                        ok: false,
                        reason: Some(reason),
                        n: None,
                    };
                    let _ = out.send(msg.to_json());
                };
                let valid = code.len() <= 16 && code.bytes().all(|b| b.is_ascii_alphanumeric());
                if !valid || !limits::PRESENCE.allow(client) {
                    refuse("unknown");
                    continue;
                }
                let st = s.clone();
                let c2 = code.clone();
                let row = tokio::task::spawn_blocking(move || {
                    st.with_db(|c| {
                        c.query_opt(
                            "SELECT region, lat, lon, radius_m FROM game.presence_codes \
                             WHERE code = $1 AND active",
                            &[&c2],
                        )
                    })
                    .flatten()
                })
                .await
                .ok()
                .flatten();
                let Some(row) = row else {
                    refuse("unknown");
                    continue;
                };
                if row.get::<_, &str>(0) != slug {
                    refuse("region");
                    continue;
                }
                FromWeb::Presence {
                    conn,
                    code,
                    lat: row.get(1),
                    lon: row.get(2),
                    radius_m: row.get(3),
                }
            }
        };
        if tx.send(ev).is_err() {
            break;
        }
    }

    let _ = tx.send(FromWeb::Bye { conn });
    drop(out);
    // A last word (`busy`) still goes out.
    let mut writer = writer;
    if tokio::time::timeout(std::time::Duration::from_millis(500), &mut writer)
        .await
        .is_err()
    {
        writer.abort();
    }
}

/// Recovery attempts: at most 5 per connection and 60 in all per minute.
/// With 60-bit codes guessing is hopeless anyway; this keeps it cheap.
#[derive(Default)]
struct RecoverLimit {
    window: Option<std::time::Instant>,
    total: u32,
    per_conn: std::collections::HashMap<u64, u32>,
}

impl RecoverLimit {
    fn allow(&mut self, conn: u64) -> bool {
        if self.window.is_none_or(|w| w.elapsed().as_secs() >= 60) {
            *self = Self {
                window: Some(std::time::Instant::now()),
                ..Default::default()
            };
        }
        let c = self.per_conn.entry(conn).or_default();
        if *c >= 5 || self.total >= 60 {
            return false;
        }
        *c += 1;
        self.total += 1;
        true
    }
}

static RECOVER_LIMIT: std::sync::LazyLock<std::sync::Mutex<RecoverLimit>> =
    std::sync::LazyLock::new(Default::default);

#[derive(Deserialize)]
struct Wish {
    lat: f64,
    lon: f64,
}

/// Wishes per hour from everyone together, so a script can't flood the table.
const WISHES_PER_HOUR: u32 = 600;
static WISH_LIMIT: std::sync::Mutex<(u64, u32)> = std::sync::Mutex::new((0, 0));

/// Short links on stickers, cards and posts:
/// sticker, card, mystery card, play-day flyer, Instagram, lecture hall, press,
/// the ARG ("Der Plan"), the QR code of a screen in kiosk mode.
pub const LINKS: [&str; 9] = ["s", "k", "m", "t", "ig", "h", "p", "plan", "tv"];

/// `/<code>`: count the hit for the day (no cookie, nothing per person), then
/// the game.
async fn link_handler(
    State(s): State<AppState>,
    Extension(c): Extension<Client>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response {
    let code = uri.path().trim_start_matches('/').to_string();
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bot = ua.is_empty()
        || [
            "bot",
            "crawl",
            "spider",
            "preview",
            "facebookexternalhit",
            "curl",
            "wget",
            "python",
        ]
        .iter()
        .any(|b| ua.contains(b));
    if !bot && limits::LINKS.allow(c) {
        tokio::task::spawn_blocking(move || {
            s.with_db(|c| {
                c.execute(
                    "INSERT INTO analytics.link_day (day, code, hits) VALUES (current_date, $1, 1) \
                     ON CONFLICT (day, code) DO UPDATE SET hits = analytics.link_day.hits + 1",
                    &[&code],
                )
            })
        });
    }
    (
        StatusCode::FOUND,
        [(header::LOCATION, "/"), (header::CACHE_CONTROL, "no-store")],
    )
        .into_response()
}

/// `POST /api/wish {lat, lon}` ("I want to play here"): counts the H3 cell at
/// resolution 6 for today. Only the cell is stored.
async fn wish_handler(
    State(s): State<AppState>,
    Extension(c): Extension<Client>,
    Json(w): Json<Wish>,
) -> Response {
    if !limits::WISHES.allow(c) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(ll) = h3o::LatLng::new(w.lat, w.lon) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let cell = ll.to_cell(h3o::Resolution::Six).to_string();
    {
        let hour = crate::analytics::now_s() as u64 / 3600;
        let mut l = WISH_LIMIT.lock().unwrap();
        if l.0 != hour {
            *l = (hour, 0);
        }
        if l.1 >= WISHES_PER_HOUR {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        l.1 += 1;
    }
    let ok = tokio::task::spawn_blocking(move || {
        s.with_db(|c| {
            c.execute(
                "INSERT INTO game.wishes (cell, day, count) VALUES ($1, current_date, 1) \
                 ON CONFLICT (cell, day) DO UPDATE SET count = game.wishes.count + 1",
                &[&cell],
            )
        })
    })
    .await
    .ok()
    .flatten()
    .is_some();
    if ok {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

#[derive(Deserialize)]
struct Feedback {
    mood: i16,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

/// Feedback per hour from everyone together.
const FEEDBACK_PER_HOUR: u32 = 300;
static FEEDBACK_LIMIT: std::sync::Mutex<(u64, u32)> = std::sync::Mutex::new((0, 0));
/// Kept this long.
const FEEDBACK_DAYS: i32 = 365;

/// Text as stored: control characters out, at most 1000 characters, `None` if empty.
fn feedback_text(t: Option<String>) -> Option<String> {
    let t: String = t?
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(1000)
        .collect();
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// `POST /api/feedback?region= {mood 1-3, text?, lang?, version?}` (settings →
/// Feedback): stored with region and day, without player or position.
async fn feedback_handler(
    State(s): State<AppState>,
    Extension(c): Extension<Client>,
    Query(q): Query<RegionQ>,
    Json(f): Json<Feedback>,
) -> Response {
    if !limits::FEEDBACK.allow(c) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(slug) = s.slug(q.region.as_deref()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !(1..=3).contains(&f.mood) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    {
        let hour = crate::analytics::now_s() as u64 / 3600;
        let mut l = FEEDBACK_LIMIT.lock().unwrap();
        if l.0 != hour {
            *l = (hour, 0);
        }
        if l.1 >= FEEDBACK_PER_HOUR {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        l.1 += 1;
    }
    let text = feedback_text(f.text);
    let lang = f
        .lang
        .filter(|l| l.len() == 2 && l.bytes().all(|b| b.is_ascii_lowercase()));
    let version: Option<String> = f.version.map(|v| v.chars().take(60).collect());
    let ok = tokio::task::spawn_blocking(move || {
        s.with_db(|c| {
            c.execute(
                "INSERT INTO game.feedback (region, mood, text, lang, version) VALUES ($1, $2, $3, $4, $5)",
                &[&slug, &f.mood, &text, &lang, &version],
            )?;
            c.execute(
                "DELETE FROM game.feedback WHERE day < current_date - $1::int",
                &[&FEEDBACK_DAYS],
            )
        })
    })
    .await
    .ok()
    .flatten()
    .is_some();
    if ok {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

/// `GET /api/feedback[?region=]` (ops token for all regions): the latest 500, newest first.
async fn feedback_list_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Query(q): Query<AnalyticsQuery>,
) -> Response {
    if !matches!(access(&s, c, &headers).await, Access::All) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let region = q.region.clone();
    let rows = tokio::task::spawn_blocking(move || {
        s.with_db(|c| {
            c.query(
                "SELECT region, day::text, mood, text, lang, version FROM game.feedback \
                 WHERE $1::text IS NULL OR region = $1 ORDER BY id DESC LIMIT 500",
                &[&region],
            )
        })
    })
    .await
    .ok()
    .flatten();
    let Some(rows) = rows else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "region": r.get::<_, &str>(0),
                "day": r.get::<_, &str>(1),
                "mood": r.get::<_, i16>(2),
                "text": r.get::<_, Option<&str>>(3),
                "lang": r.get::<_, Option<&str>>(4),
                "version": r.get::<_, Option<&str>>(5),
            })
        })
        .collect();
    Json(items).into_response()
}

/// `GET /api/wishes` (ops token): cells with their wishes, most first.
async fn wishes_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    if !matches!(access(&s, c, &headers).await, Access::All) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let rows = tokio::task::spawn_blocking(move || {
        s.with_db(|c| {
            c.query(
                "SELECT cell, sum(count)::bigint, max(day)::text FROM game.wishes \
                 GROUP BY cell ORDER BY 2 DESC LIMIT 1000",
                &[],
            )
        })
    })
    .await
    .ok()
    .flatten();
    let Some(rows) = rows else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let cells: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let cell: &str = r.get(0);
            let center = cell
                .parse::<h3o::CellIndex>()
                .map(|c| {
                    let ll = h3o::LatLng::from(c);
                    [ll.lng(), ll.lat()]
                })
                .ok();
            serde_json::json!({ "cell": cell, "wishes": r.get::<_, i64>(1), "last": r.get::<_, &str>(2), "center": center })
        })
        .collect();
    Json(cells).into_response()
}

/// What a dashboard token opens: everything (`ELEMENTS_OPS_TOKEN`, or an
/// `ops.tokens` row without region) or some regions.
enum Access {
    None,
    All,
    Regions(Vec<String>),
}

impl Access {
    fn allows(&self, slug: &str) -> bool {
        match self {
            Access::All => true,
            Access::Regions(r) => r.iter().any(|x| x == slug),
            Access::None => false,
        }
    }
}

/// What a token or login may do: read the dashboard, also edit chapters, also
/// hotspots and reloads.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Role {
    Read,
    Editor,
    Admin,
}

/// A token's or login's grants (region, `None`: all; role) and label (for
/// the audit log).
struct Who {
    pub grants: Vec<(Option<String>, Role)>,
    pub label: String,
    /// `ELEMENTS_OPS_TOKEN`: may approve directly (own images too).
    pub master: bool,
    /// Came through a login (`login.rs`), not a token.
    pub user: Option<String>,
}

impl Who {
    fn none() -> Self {
        Who {
            grants: Vec::new(),
            label: String::new(),
            master: false,
            user: None,
        }
    }

    /// Allowed for `slug` with at least `role`.
    fn may(&self, slug: &str, role: Role) -> bool {
        self.grants
            .iter()
            .any(|(r, g)| *g >= role && r.as_deref().is_none_or(|r| r == slug))
    }

    /// The regions with any grant.
    fn access(&self) -> Access {
        if self.grants.is_empty() {
            Access::None
        } else if self.grants.iter().any(|(r, _)| r.is_none()) {
            Access::All
        } else {
            Access::Regions(self.grants.iter().filter_map(|(r, _)| r.clone()).collect())
        }
    }

    /// The highest role it has anywhere.
    fn role(&self) -> Role {
        self.grants
            .iter()
            .map(|(_, g)| *g)
            .max()
            .unwrap_or(Role::Read)
    }
}

/// The token or login of a request: regions only.
async fn access(s: &AppState, c: Client, headers: &HeaderMap) -> Access {
    who(s, c, headers).await.access()
}

/// The token from `Authorization: Bearer …`, checked against
/// `ELEMENTS_OPS_TOKEN` and `ops.tokens` (hash only); without one, a login
/// from the proxy (`login.rs`). Header only: a token in the URL would end up
/// in histories, bookmarks and screenshots.
async fn who(s: &AppState, c: Client, headers: &HeaderMap) -> Who {
    let none = Who::none;
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = bearer.map(str::trim).filter(|t| !t.is_empty()) else {
        // "@name" in the audit log, so a login never matches a token label.
        return crate::login::config()
            .and_then(|cfg| crate::login::from(cfg, headers))
            .map(|l| Who {
                grants: l.grants,
                label: format!("@{}", l.user),
                master: false,
                user: Some(l.user),
            })
            .unwrap_or_else(none);
    };
    let hash = crate::score::token_id(token);
    // Compared as hashes, in constant time.
    if s.cfg
        .ops_token
        .as_deref()
        .is_some_and(|t| crate::shards::same(&crate::score::token_id(t), &hash))
    {
        return Who {
            grants: vec![(None, Role::Admin)],
            label: "ops".into(),
            master: true,
            user: None,
        };
    }
    // Guessing: a few misses, then nothing is looked up for a while.
    if limits::BAD_TOKENS.empty(c) {
        return none();
    }
    let st = s.clone();
    let rows = tokio::task::spawn_blocking(move || {
        st.with_db(|c| {
            c.query(
                "SELECT region, role, label FROM ops.tokens WHERE hash = $1",
                &[&&hash[..]],
            )
        })
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    let Some(row) = rows.first() else {
        limits::BAD_TOKENS.allow(c);
        return none();
    };
    let role = match row.get::<_, &str>(1) {
        "admin" => Role::Admin,
        "editor" => Role::Editor,
        _ => Role::Read,
    };
    Who {
        grants: rows.iter().map(|r| (r.get(0), role)).collect(),
        label: row.get(2),
        master: false,
        user: None,
    }
}

/// `GET /api/access` (token): which regions the token opens.
async fn access_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    let w = who(&s, c, &headers).await;
    let name = |r: Role| match r {
        Role::Admin => "admin",
        Role::Editor => "editor",
        Role::Read => "read",
    };
    let (all, regions) = match w.access() {
        // A login without any of our groups: say who, so the page can tell.
        Access::None if w.user.is_none() => return StatusCode::UNAUTHORIZED.into_response(),
        Access::None => (false, Vec::new()),
        Access::All => (true, Vec::new()),
        Access::Regions(r) => (false, r),
    };
    let mut v = serde_json::json!({ "all": all, "regions": regions, "role": name(w.role()) });
    // Per region where a login's roles differ (`may` decides on the server).
    v["roles"] = w
        .grants
        .iter()
        .map(|(r, g)| serde_json::json!({ "region": r, "role": name(*g) }))
        .collect();
    if let Some(u) = w.user {
        v["user"] = u.into();
        if let Some(l) = crate::login::config().and_then(|c| c.logout.as_deref()) {
            v["logout"] = l.into();
        }
    }
    Json(v).into_response()
}

#[derive(Deserialize)]
struct OpsHotspot {
    node: i64,
    minutes: f64,
}

async fn ops_hotspot(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Json(req): Json<OpsHotspot>,
) -> Response {
    let r = match s.region(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !who(&s, c, &headers).await.may(&r.slug, Role::Admin) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let minutes = req.minutes.clamp(1.0, 240.0);
    let _ = r.tx.send(FromWeb::OpsHotspot {
        node: req.node,
        minutes,
    });
    StatusCode::ACCEPTED.into_response()
}

/// `POST /api/ops/reload?region=` (token): unload the region and load it again
/// from the database; open sockets close and reconnect.
/// Side of every junction now, for pictures of the game state.
/// The first call asks the region to build it: 503 until it is fresher than
/// the request, the client asks again.
async fn ops_nodes(
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
    let r = match s.region(Some(&slug)) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let json = r.nodes.json.read().unwrap().clone();
    let fresh = json.as_deref().is_some_and(|j| {
        serde_json::from_str::<serde_json::Value>(j)
            .ok()
            .and_then(|v| v["t"].as_f64())
            .is_some_and(|t| crate::analytics::now_s() - t < 30.0)
    });
    if !fresh {
        r.nodes
            .wanted
            .store(true, std::sync::atomic::Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "2")],
            "building",
        )
            .into_response();
    }
    json_response(&json.unwrap_or_default(), "no-store")
}

async fn ops_reload(
    State(s): State<AppState>,
    Query(q): Query<RegionQ>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
) -> Response {
    let slug = match s.slug(q.region.as_deref()) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !who(&s, c, &headers).await.may(&slug, Role::Admin) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let host = s.host.clone();
    let sl = slug.clone();
    match tokio::task::spawn_blocking(move || host.reload(&sl))
        .await
        .ok()
        .flatten()
    {
        Some(was_loaded) => {
            tracing::info!(slug, was_loaded, "ops: region reload");
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "region": slug, "was_loaded": was_loaded })),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not a region of this shard").into_response(),
    }
}

#[derive(Deserialize)]
struct AnalyticsQuery {
    days: Option<i32>,
    /// One UTC day (`YYYY-MM-DD`, or `demo` for the demo day); overrides `days`.
    day: Option<String>,
    data: Option<String>,
    region: Option<String>,
}

impl AnalyticsQuery {
    fn period(&self, demo_day: &str) -> Option<crate::db::Period> {
        match self.day.as_deref() {
            Some("demo") => crate::db::Period::day(demo_day),
            Some(d) => crate::db::Period::day(d),
            None => Some(crate::db::Period::Days(
                self.days.unwrap_or(7).clamp(1, 365),
            )),
        }
    }

    /// The simulated demo day: public, no real people behind it.
    fn is_demo(&self) -> bool {
        self.day.as_deref() == Some("demo") && self.data.as_deref() == Some("synthetic")
    }

    fn synthetic(&self) -> Vec<bool> {
        match self.data.as_deref() {
            Some("synthetic") => vec![true],
            Some("all") => vec![false, true],
            _ => vec![false],
        }
    }
}

/// Public demo answers, kept for `DEMO_CACHE_S` so open access doesn't turn
/// into database load: key -> (made at, content type, body).
type DemoCache =
    std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, String, String)>>;
static DEMO_CACHE: std::sync::OnceLock<DemoCache> = std::sync::OnceLock::new();
const DEMO_CACHE_S: u64 = 600;

fn demo_cached(key: &str) -> Option<(String, String)> {
    let cache = DEMO_CACHE.get_or_init(Default::default).lock().unwrap();
    cache
        .get(key)
        .filter(|(at, ..)| at.elapsed().as_secs() < DEMO_CACHE_S)
        .map(|(_, t, b)| (t.clone(), b.clone()))
}

fn demo_store(key: &str, ctype: &str, body: &str) {
    DEMO_CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(
            key.into(),
            (std::time::Instant::now(), ctype.into(), body.into()),
        );
}

async fn analytics_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    Query(q): Query<AnalyticsQuery>,
) -> Response {
    let demo = q.is_demo();
    let region = match s.slug(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !demo && !access(&s, c, &headers).await.allows(&region) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let key = format!("{region}|summary");
    if demo {
        if let Some((ctype, body)) = demo_cached(&key) {
            return ([(header::CONTENT_TYPE, ctype)], body).into_response();
        }
    }
    let Some(db) = s.read_db().map(String::from) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "no database").into_response();
    };
    let demo_day = instance(&s, &region).demo_day;
    let Some(period) = q.period(&demo_day) else {
        return (StatusCode::BAD_REQUEST, "day must be YYYY-MM-DD").into_response();
    };
    let synthetic = q.synthetic();
    let res = tokio::task::spawn_blocking(move || {
        crate::db::analytics_summary(&db, &region, &period, &synthetic).map_err(|e| e.to_string())
    })
    .await;
    match res {
        Ok(Ok(mut v)) => {
            v["demo_day"] = demo_day.into();
            if demo {
                demo_store(&key, "application/json", &v.to_string());
            }
            Json(v).into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn export_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Extension(c): Extension<Client>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Query(q): Query<AnalyticsQuery>,
) -> Response {
    // Before anything costs a database connection.
    if !crate::db::EXPORTS.contains(&name.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let demo = q.is_demo();
    let region = match s.slug(q.region.as_deref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !demo && !access(&s, c, &headers).await.allows(&region) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let key = format!("{region}|{name}");
    let attachment = format!("attachment; filename=\"elements-{name}\"");
    if demo {
        if let Some((ctype, body)) = demo_cached(&key) {
            return (
                [
                    (header::CONTENT_TYPE, ctype),
                    (header::CONTENT_DISPOSITION, attachment),
                ],
                body,
            )
                .into_response();
        }
    }
    let Some(db) = s.read_db().map(String::from) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "no database").into_response();
    };
    let Some(period) = q.period(&instance(&s, &region).demo_day) else {
        return (StatusCode::BAD_REQUEST, "day must be YYYY-MM-DD").into_response();
    };
    let synthetic = q.synthetic();
    let file = name.clone();
    let res = tokio::task::spawn_blocking(move || {
        crate::db::export(&db, &region, &file, &period, &synthetic).map_err(|e| e.to_string())
    })
    .await;
    match res {
        Ok(Ok((ctype, body))) => {
            if demo {
                demo_store(&key, ctype, &body);
            }
            (
                [
                    (header::CONTENT_TYPE, ctype.to_string()),
                    (header::CONTENT_DISPOSITION, attachment),
                ],
                body,
            )
                .into_response()
        }
        Ok(Err(e)) if e == "unknown export" => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_url_https_only() {
        assert_eq!(
            source_from(" https://example.org/x \n").as_deref(),
            Some("https://example.org/x")
        );
        assert_eq!(source_from("http://example.org/x"), None);
        assert_eq!(source_from("https://"), None);
        assert_eq!(source_from("https://a b"), None);
        assert_eq!(source_from("javascript:alert(1)"), None);
    }

    fn h(kv: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in kv {
            m.insert(*k, v.parse().unwrap());
        }
        m
    }

    #[test]
    fn same_origin_only() {
        assert!(same_origin(&h(&[])));
        assert!(same_origin(&h(&[
            ("host", "glutflut.com"),
            ("origin", "https://glutflut.com")
        ])));
        assert!(same_origin(&h(&[
            ("host", "localhost:5173"),
            ("origin", "http://localhost:5173")
        ])));
        assert!(!same_origin(&h(&[
            ("host", "glutflut.com"),
            ("origin", "https://evil.example")
        ])));
        assert!(!same_origin(&h(&[
            ("host", "glutflut.com"),
            ("origin", "null")
        ])));
        assert!(!same_origin(&h(&[("sec-fetch-site", "cross-site")])));
        assert!(same_origin(&h(&[("sec-fetch-site", "same-origin")])));
    }
}

/// Held areas for the dashboard.
#[path = "areas.rs"]
mod areas;
/// The campaigns' chronicle (public).
#[path = "chronicle.rs"]
mod chronicle;
/// Authoring tools (chapters), with this module's state and token checks.
#[path = "editor.rs"]
mod editor;
/// Images for finds, zones and chapters (editor and app).
#[path = "editor_media.rs"]
mod editor_media;
/// Campaign phrases (editor).
#[path = "editor_mottos.rs"]
mod editor_mottos;
/// News flashes (editor).
#[path = "editor_notices.rs"]
mod editor_notices;
/// Authoring tools stage 2 (places).
#[path = "editor_places.rs"]
mod editor_places;
/// Before/after report per focus place.
#[path = "report.rs"]
mod report;

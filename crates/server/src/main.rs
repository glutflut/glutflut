//! Headless authoritative server for Elements.
//!
//! One process serves many regions (see `regions.rs`): a web gateway (HTTP +
//! WebSocket) in front, and per loaded region a Bevy `App` on a worker thread
//! (`pool.rs`), running the simulation as the source of truth.

use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use bevy::ecs::schedule::{ExecutorKind, Schedules};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use clap::Parser;

use elements_shared::prelude::*;

mod actions;
mod analytics;
mod area;
mod bench;
mod campaign;
mod cells;
mod challenge;
mod cheat;
mod cheer;
mod clock;
mod db;
mod demo;
mod finds;
mod gateway;
mod hinterland;
mod hotspot;
mod info;
mod keep;
mod limits;
mod live;
mod live_rain;
mod login;
mod marble;
mod media;
mod motion;
mod names;
mod places;
mod plausible;
mod pool;
mod pow;
mod protocol;
mod radar;
mod regions;
mod score;
mod shards;
mod siege;
mod sim;
#[cfg(test)]
mod testkit;
mod tide;
mod tiles;
mod weather;
mod web;
mod world;
mod writers;

use actions::tick_cooldowns;

#[derive(Parser, Debug, Clone)]
#[command(name = "elements-server", about = "Elements AR game server")]
struct Args {
    /// PostgreSQL/PostGIS connection string for the geo world. When omitted the
    /// server has no regions (the web app still starts).
    #[arg(long, env = "ELEMENTS_DB")]
    db: Option<String>,

    /// Read-only connection (role `elements_read`: geo and analytics, nothing
    /// about players) for the dashboard: analytics, exports, report, areas.
    /// Default: `ELEMENTS_DB`.
    #[arg(long, env = "ELEMENTS_DB_READ")]
    db_read: Option<String>,

    /// Default region: for requests without `?region=` (older app versions,
    /// tools).
    #[arg(long, env = "ELEMENTS_REGION")]
    region: Option<String>,

    /// Server group: this process runs the regions with this `geo.regions.shard`.
    #[arg(long, env = "ELEMENTS_SHARD", default_value = "eu")]
    shard: String,

    /// Cell within the shard: runs the regions with this `geo.regions.cell`;
    /// `main` also the shard-wide jobs (globe, outbox).
    #[arg(long, env = "ELEMENTS_CELL", default_value = "main")]
    cell: String,

    /// Other shards' base URLs, `name=https://host,name2=https://host2`.
    #[arg(long, env = "ELEMENTS_SHARDS", default_value = "")]
    shards: String,

    /// Secret shared by all shards: signs passports and calls between them.
    #[arg(long, env = "ELEMENTS_SHARD_KEY", hide_env_values = true)]
    shard_key: Option<String>,

    /// Regions kept loaded from the start (comma-separated slugs).
    #[arg(long, env = "ELEMENTS_PRELOAD", default_value = "")]
    preload: String,

    /// Unload a region after this many minutes without players. Long, so
    /// leftovers below k (analytics) can still reach k with the next player.
    #[arg(long, env = "ELEMENTS_IDLE_MIN", default_value_t = 180)]
    idle_min: u64,

    /// Most regions loaded at once.
    #[arg(long, env = "ELEMENTS_MAX_REGIONS", default_value_t = 12)]
    max_regions: usize,

    /// Most junctions loaded at once (0: no limit). Default: from the
    /// container's memory limit (`limits::junction_budget`).
    #[arg(long, env = "ELEMENTS_MAX_JUNCTIONS")]
    max_junctions: Option<u64>,

    /// Address of the web gateway (HTTP + WebSocket for browser clients).
    #[arg(long, env = "ELEMENTS_WEB_BIND", default_value = "0.0.0.0:5080")]
    web_bind: SocketAddr,

    /// Built web app to serve at `/` (e.g. `web/dist`).
    #[arg(long, env = "ELEMENTS_WEB_DIR")]
    web_dir: Option<std::path::PathBuf>,

    /// Files served before the web app (host volume): `basemap/`, `media/`.
    #[arg(long, env = "ELEMENTS_STATIC_DIR")]
    static_dir: Option<std::path::PathBuf>,

    /// Bearer token for `/api/ops/*` and `/api/analytics`; unset disables them.
    #[arg(long, env = "ELEMENTS_OPS_TOKEN", hide_env_values = true)]
    ops_token: Option<String>,

    /// UTC day of the simulated demo dataset shown in the dashboard for
    /// regions without their own `demo_day` (default 2025-12-06).
    #[arg(long, env = "ELEMENTS_DEMO_DAY")]
    demo_day: Option<String>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(clap::Subcommand, Debug, Clone)]
enum Cmd {
    /// Write the demo dataset: one simulated day through the live aggregation.
    Demo(demo::DemoArgs),
    /// Many regions from graph files, no database: load, rest and play
    /// costs.
    Bench(bench::BenchArgs),
    /// Dashboard tokens (ops.tokens): per region, or for all regions.
    #[command(subcommand)]
    Token(TokenCmd),
    /// Presence codes (game.presence_codes) for the game link on posters and stands.
    #[command(subcommand)]
    Code(CodeCmd),
}

#[derive(clap::Subcommand, Debug, Clone)]
enum CodeCmd {
    /// Make a code for a place and print the game link with it (`?p=`).
    Add {
        #[arg(long)]
        region: String,
        #[arg(long)]
        lat: f64,
        #[arg(long)]
        lon: f64,
        /// Counts within this distance of the place (m).
        #[arg(long, default_value_t = 150.0)]
        radius: f32,
        /// Where it hangs, e.g. "Stand Stadt, Weihnachtsmarkt".
        #[arg(long)]
        label: String,
    },
    /// Codes by region with their place and link.
    List,
    /// Switch a code off (confirmations made with it stay).
    Revoke { code: String },
}

/// Base of the game links printed for codes (`ELEMENTS_PUBLIC_URL`).
fn public_url() -> String {
    std::env::var("ELEMENTS_PUBLIC_URL").unwrap_or_else(|_| "https://glutflut.com".into())
}

fn code_cmd(db: &str, cmd: &CodeCmd) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = db::connect(db)?;
    let link = |code: &str, region: &str| format!("{}/?region={region}&p={code}", public_url());
    match cmd {
        CodeCmd::Add {
            region,
            lat,
            lon,
            radius,
            label,
        } => {
            if c.query_opt("SELECT 1 FROM geo.regions WHERE slug = $1", &[region])?
                .is_none()
            {
                return Err(format!("no region {region}").into());
            }
            // 10 characters without look-alikes (~50 bits).
            const ABC: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
            let code: String = (0..10)
                .map(|_| ABC[rand::random_range(0..ABC.len())] as char)
                .collect();
            c.execute(
                "INSERT INTO game.presence_codes (code, region, lat, lon, radius_m, label) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
                &[&code, region, lat, lon, radius, label],
            )?;
            println!("{}", link(&code, region));
            eprintln!("code {code} for {label} ({region}, {radius} m around {lat}, {lon})");
        }
        CodeCmd::List => {
            for r in c.query(
                "SELECT code, region, label, active, lat, lon, radius_m FROM game.presence_codes \
                 ORDER BY region, label",
                &[],
            )? {
                let (code, region): (&str, &str) = (r.get(0), r.get(1));
                println!(
                    "{}\t{}\t{}\t{:.5},{:.5} {} m\t{}",
                    if r.get::<_, bool>(3) { "on " } else { "off" },
                    r.get::<_, &str>(2),
                    code,
                    r.get::<_, f64>(4),
                    r.get::<_, f64>(5),
                    r.get::<_, f32>(6),
                    link(code, region)
                );
            }
        }
        CodeCmd::Revoke { code } => {
            let n = c.execute(
                "UPDATE game.presence_codes SET active = false WHERE code = $1",
                &[code],
            )?;
            println!("{n} switched off");
        }
    }
    Ok(())
}

#[derive(clap::Subcommand, Debug, Clone)]
enum TokenCmd {
    /// Make a token and print it once (only its hash is stored).
    Add {
        /// Region slug; without it the token opens every region.
        #[arg(long)]
        region: Option<String>,
        /// Who it is for, e.g. "Stadt Hannover" (unique).
        #[arg(long)]
        label: String,
        /// read (dashboard), editor (also chapters, editor.html) or admin
        /// (also hotspots and reloads).
        #[arg(long, default_value = "read")]
        role: String,
    },
    /// Tokens by label and region.
    List,
    /// Delete the token with this label.
    Revoke { label: String },
}

fn token_cmd(db: &str, cmd: &TokenCmd) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = db::connect(db)?;
    match cmd {
        TokenCmd::Add {
            region,
            label,
            role,
        } => {
            if !["read", "editor", "admin"].contains(&role.as_str()) {
                return Err("role: read, editor or admin".into());
            }
            if let Some(r) = region {
                if c.query_opt("SELECT 1 FROM geo.regions WHERE slug = $1", &[r])?
                    .is_none()
                {
                    return Err(format!("no region {r}").into());
                }
            }
            let token = format!(
                "{:032x}{:08x}",
                rand::random::<u128>(),
                rand::random::<u32>()
            );
            c.execute(
                "INSERT INTO ops.tokens (hash, region, label, role) VALUES ($1, $2, $3, $4)",
                &[&&score::token_id(&token)[..], region, label, role],
            )?;
            println!("{token}");
            eprintln!(
                "for {label} ({}); shown only now",
                region.as_deref().unwrap_or("all regions")
            );
        }
        TokenCmd::List => {
            for r in c.query(
                "SELECT label, coalesce(region, '(all)'), role, created::text FROM ops.tokens ORDER BY label",
                &[],
            )? {
                println!(
                    "{}\t{}\t{}\t{}",
                    r.get::<_, &str>(0),
                    r.get::<_, &str>(1),
                    r.get::<_, &str>(2),
                    r.get::<_, &str>(3)
                );
            }
        }
        TokenCmd::Revoke { label } => {
            let n = c.execute("DELETE FROM ops.tokens WHERE label = $1", &[label])?;
            println!("{n} revoked");
        }
    }
    Ok(())
}

fn split_list(s: &str) -> impl Iterator<Item = &str> {
    s.split(',').map(str::trim).filter(|v| !v.is_empty())
}

fn main() {
    let args = Args::parse();
    let version = gateway::version();
    if let Some(Cmd::Token(t)) = &args.cmd {
        let Some(db) = args.db.as_deref() else {
            eprintln!("token: ELEMENTS_DB is required");
            std::process::exit(2);
        };
        if let Err(e) = token_cmd(db, t) {
            eprintln!("token: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(Cmd::Code(cmd)) = &args.cmd {
        let Some(db) = args.db.as_deref() else {
            eprintln!("code: ELEMENTS_DB is required");
            std::process::exit(2);
        };
        if let Err(e) = code_cmd(db, cmd) {
            eprintln!("code: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(Cmd::Demo(d)) = &args.cmd {
        let (Some(db), Some(region)) = (args.db.as_deref(), args.region.as_deref()) else {
            eprintln!("demo: ELEMENTS_DB and ELEMENTS_REGION are required");
            std::process::exit(2);
        };
        let day = d
            .day
            .clone()
            .or_else(|| args.demo_day.clone())
            .or_else(|| db::region_demo_day(db, region))
            .unwrap_or_else(|| db::DEFAULT_DEMO_DAY.into());
        if let Err(e) = demo::run(db, region, &day, d) {
            eprintln!("demo: {e}");
            std::process::exit(1);
        }
        return;
    }

    // One subscriber for all region apps (their LogPlugin is left out).
    let bench = matches!(args.cmd, Some(Cmd::Bench(_)));
    let filter =
        bevy::log::tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            if bench {
                "warn"
            } else {
                "info,wgpu=error,naga=warn"
            }
            .into()
        });
    bevy::log::tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();
    if let Some(Cmd::Bench(b)) = &args.cmd {
        if let Err(e) = bench::run(b) {
            eprintln!("bench: {e}");
            std::process::exit(1);
        }
        return;
    }
    info!(%version, "elements-server");
    // Parse now, so a bad ELEMENTS_CHEAT stops the start, not the first fix.
    let _ = cheat::get();
    if std::env::var_os("ELEMENTS_CHEAT").is_some() {
        info!("cheat thresholds from ELEMENTS_CHEAT");
    }

    let shard_urls: std::collections::HashMap<String, String> = split_list(&args.shards)
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.trim_end_matches('/').to_string()))
        .collect();
    shards::init(
        args.shard.clone(),
        shard_urls.clone(),
        args.shard_key.clone(),
    );
    // In every cell: rows are locked while sent (`SKIP LOCKED`), so a
    // standby or a second main can't deliver an event twice.
    if let Some(db) = args.db.clone() {
        shards::spawn_outbox(db);
    }
    let max_junctions = args
        .max_junctions
        .unwrap_or_else(|| limits::junction_budget(limits::memory_limit()));
    info!(max_junctions, "region pool: junction budget");
    let host = regions::Host::new(regions::HostConfig {
        db: args.db.clone(),
        shard: args.shard.clone(),
        cell: args.cell.clone(),
        shard_urls,
        default_region: args.region.clone(),
        preload: split_list(&args.preload).map(String::from).collect(),
        idle: std::time::Duration::from_secs(args.idle_min * 60),
        max_loaded: args.max_regions.max(1),
        max_junctions,
        demo_day: args.demo_day.clone(),
    });
    gateway::spawn(
        args.web_bind,
        gateway::GatewayConfig {
            web_dir: args.web_dir.clone(),
            static_dir: args.static_dir.clone(),
            ops_token: args.ops_token.clone(),
            db_read: args.db_read.clone().filter(|v| !v.is_empty()),
        },
        host.clone(),
    );
    // Every cell its own regions; main merges them into the shard's manifest.
    marble::spawn(host.clone());
    if let Some(db) = args.db.clone() {
        cells::spawn(host.clone(), db);
    }
    radar::spawn(host.shard());
    host.preload();
    host.supervise();
    info!("all regions stopped");
}

/// Set to unload this region app (see `regions::Host`).
#[derive(Resource, Clone)]
pub struct Stop(pub Arc<AtomicBool>);

/// What a region app is built from: everything that needs the database or
/// the disk (rules, scores, keeps, the graph, tiles), read on a loader thread
/// so a worker only builds the app (`pool.rs`).
pub struct Prepared {
    slug: String,
    stop: Arc<AtomicBool>,
    host: Arc<regions::Host>,
    rx: crossbeam_channel::Receiver<gateway::FromWeb>,
    tiles: tiles::SharedTiles,
    overview: gateway::SharedJson,
    nodes: gateway::NodesShare,
    rules: elements_shared::rules::Rules,
    analytics: analytics::AnalyticsTx,
    scores: score::Scores,
    keeps: keep::Keeps,
    live: live::Live,
    campaign: campaign::Campaign,
    data: Option<RegionData>,
}

impl Prepared {
    /// Junctions, for spreading regions over the workers.
    pub fn junctions(&self) -> usize {
        self.data.as_ref().map_or(0, |d| d.graph.nodes.len())
    }

    pub fn slug(&self) -> &str {
        &self.slug
    }
}

/// The span a region's logs go under.
pub fn region_span(slug: &str) -> tracing::Span {
    info_span!("region", slug = %slug.trim_start_matches("region-"))
}

/// Read what the region needs (loader thread).
pub fn prepare(start: regions::RegionStart) -> Prepared {
    let _span = region_span(&start.slug).entered();
    let rules = actions::load_rules(start.rules.as_deref());
    let analytics = analytics::spawn_writer(start.db.clone(), start.slug.clone());
    let keeps = keep::Keeps::new(start.db.clone(), start.slug.clone(), rules.keep.enabled);
    let live = live::Live::new(start.db.clone(), start.slug.clone());
    let campaign = campaign::Campaign::spawn(start.db.clone(), start.slug.clone());
    let mut data = match (&start.graph_file, start.db.as_deref()) {
        (Some(path), _) => bench::load_file(path, &start.slug)
            .map_err(
                |e| error!(error = %e, path = %path.display(), "bench graph file; starting empty"),
            )
            .ok(),
        (None, Some(conn_str)) => db::load_region_data(conn_str, &start.slug),
        (None, None) => {
            warn!("ELEMENTS_DB not set — starting with an empty world (no geo graph loaded)");
            None
        }
    };
    // Tiles for the web app (area loading).
    if let Some(d) = data.as_mut() {
        if let Some(meta) = d.meta.clone() {
            let share = rules.area.enabled.then_some(rules.area.share);
            let set = tiles::TileSet::build(
                &d.region,
                meta,
                &d.graph.nodes,
                &d.graph.edges,
                &d.lines,
                &d.blocks,
                share,
            );
            info!(tiles = set.tiles.len(), z = tiles::TILE_Z, "web tiles");
            d.tiles = Some(Arc::new(set));
        }
    }
    // Scores count local days from the first load on (`clock.rs`).
    let clock = clock::Clock::new(data.as_ref().and_then(|d| d.region.timezone.as_deref()));
    let mut scores = score::Scores::new(start.db.clone(), start.slug.clone(), start.board, clock);
    scores.prog = rules.progression.clone();
    scores.points = rules.score.clone();
    scores.streak_rules = rules.streak.clone();
    Prepared {
        slug: start.slug,
        stop: start.stop,
        host: start.host,
        rx: start.rx,
        tiles: start.tiles,
        overview: start.overview,
        nodes: start.nodes,
        rules,
        analytics,
        scores,
        keeps,
        live,
        campaign,
        data,
    }
}

/// The region's app; a `pool.rs` worker calls `update()` on it.
pub fn build(p: Prepared) -> App {
    let _span = region_span(&p.slug).entered();
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    // Updated a few times a second when nobody plays: frame time must not be
    // clamped (sources age by `Time`).
    app.insert_resource(Time::<Virtual>::from_max_delta(
        std::time::Duration::from_secs(10),
    ));

    app.insert_resource(Stop(p.stop));
    app.insert_resource(regions::Peers(p.host));

    app.insert_resource(p.analytics);
    app.insert_resource(web::WebInbox(p.rx));
    app.insert_resource(web::SharedTilesRes(p.tiles));
    app.insert_resource(web::OverviewJson(p.overview));
    app.insert_resource(web::NodesJson(p.nodes));
    let rules = p.rules;
    app.init_resource::<web::WebClients>();
    app.init_resource::<web::Views>();
    app.init_resource::<web::Overview>();
    app.init_resource::<web::Sessions>();
    app.init_resource::<web::Parked>();
    app.init_resource::<cheer::Cheers>();
    app.init_resource::<hinterland::Supply>();
    app.init_resource::<hinterland::SparkNews>();
    app.insert_resource(motion::AllowSim::from_env());
    app.insert_resource(p.scores);
    app.insert_resource(p.keeps);
    app.insert_resource(p.live);
    app.insert_resource(weather::BaseRules(rules.clone()));
    app.init_resource::<live_rain::LiveRain>();
    app.init_resource::<live_rain::LiveSun>();
    app.init_resource::<tide::LiveTide>();
    app.insert_resource(sim::GameRules(rules));
    app.init_resource::<area::Blocks>();
    app.init_resource::<marble::Claims>();
    app.insert_resource(p.campaign);

    app.init_resource::<NextPlayerId>();
    app.insert_resource(Loaded(p.data));
    app.add_systems(Startup, load_geo);
    // Element sim (S3): build the diffusion graph after the geo spawns are
    // applied, then step it server-side each FixedUpdate.
    app.add_systems(PostStartup, sim::setup_elements);
    app.add_systems(
        FixedUpdate,
        sim::element_tick.run_if(resource_exists::<sim::ElementSim>),
    );
    app.add_systems(
        Update,
        (
            tick_cooldowns,
            sim::expire_sources,
            analytics::shutdown_system,
            marble::tick
                .run_if(resource_exists::<sim::ElementSim>)
                .run_if(resource_exists::<sim::NodeById>),
            (
                web::inbox,
                hotspot::tick,
                hinterland::tick.run_if(resource_exists::<sim::ElementSim>),
                web::broadcast,
                web::overview_tick.run_if(resource_exists::<sim::ElementSim>),
                web::nodes_tick.run_if(resource_exists::<sim::ElementSim>),
                analytics::flush_system,
                score::tick.run_if(resource_exists::<sim::ElementSim>),
                siege::tick.run_if(resource_exists::<sim::ElementSim>),
                web::challenge_tick.run_if(resource_exists::<challenge::Challenges>),
                web::campaign_tick,
                web::tide_tick,
                cheer::tick,
                live_rain::update.run_if(resource_exists::<live_rain::LiveRain>),
                tide::update.run_if(resource_exists::<tide::LiveTide>),
                weather::apply,
                (keep::restore, live::restore).chain(),
                (keep::tick, live::save_on_stop).chain(),
                web::send_scores,
            )
                .chain()
                .run_if(resource_exists::<sim::NodeById>)
                .run_if(resource_exists::<analytics::Aggregator>)
                .run_if(resource_exists::<hotspot::Hotspots>),
        ),
    );

    for (_, schedule) in app.world_mut().resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor_kind(ExecutorKind::SingleThreaded);
    }
    app
}

/// Monotonic id handed to each new player avatar (avoids depending on the
/// internal `PeerId` representation).
#[derive(Resource, Default)]
pub struct NextPlayerId(u64);

impl NextPlayerId {
    pub fn take(&mut self) -> u64 {
        self.0 += 1;
        self.0 - 1
    }
}

/// Resident memory in MB (Linux).
pub fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096 / 1_048_576)
}

extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

/// Hand the load's temporary memory back to the system (glibc keeps it
/// otherwise) and log what stays.
pub fn trim_memory(what: &str) {
    // SAFETY: glibc's malloc_trim only releases free heap pages.
    unsafe {
        malloc_trim(0);
    }
    info!(rss_mb = rss_mb(), "{what}");
}

/// The region's data from `prepare`, taken by `load_geo`.
#[derive(Resource)]
struct Loaded(Option<RegionData>);

/// What a region app is built from (database or, for the bench, a file).
pub struct RegionData {
    pub graph: RegionGraph,
    pub region: RegionInfo,
    /// Edge polylines as (lat, lon).
    pub lines: Vec<(i64, Vec<(f64, f64)>)>,
    /// Region settings for the tiles; `None`: no tiles.
    pub meta: Option<serde_json::Value>,
    /// Zones and hotspot sites (`places.rs`).
    pub places: places::Places,
    pub finds: finds::Finds,
    /// Blocks enclosed by streets (`geo.blocks`).
    pub blocks: Vec<area::Block>,
    /// Built from `meta` on the loader thread.
    pub tiles: Option<Arc<tiles::TileSet>>,
    /// Gauge and junction distances to tidal water (`tide.rs`), if any.
    pub tide: Option<tide::Setup>,
}

/// Spawn the region's graph from `prepare` as ECS entities (S2), with its
/// tiles, zones, finds and hotspots. Without data (no `ELEMENTS_DB`, or a
/// failed load, both logged) the region runs an empty world.
fn load_geo(
    mut commands: Commands,
    mut loaded: ResMut<Loaded>,
    shared_tiles: Res<web::SharedTilesRes>,
    analytics_tx: Res<analytics::AnalyticsTx>,
    mut scores: ResMut<score::Scores>,
    mut keeps: ResMut<keep::Keeps>,
) {
    let Some(data) = loaded.0.take() else {
        return;
    };
    let RegionData {
        graph,
        region,
        lines,
        meta,
        places,
        finds,
        blocks,
        tiles,
        tide,
    } = data;
    // Automatic hotspots never at the waterline (`tide.rs`).
    let shore = tide.as_ref().map(tide::Setup::shore).unwrap_or_default();
    commands.insert_resource(tide::LiveTide::new(tide));
    // Days in local time (`clock.rs`).
    let clock = scores.clock.clone();
    keeps.clock = clock.clone();
    commands.insert_resource(clock.clone());
    info!(blocks = blocks.len(), "blocks");
    commands.insert_resource(area::Blocks::new(&blocks));
    drop(blocks);

    // New player names in the region's language.
    if let Some(l) = meta.as_ref().and_then(|m| m["language"].as_str()) {
        scores.lang = l.into();
    }
    if let Some(set) = tiles {
        *shared_tiles.0.write().unwrap() = Some(set.clone());
        commands.insert_resource(web::Tiles(set));
    }
    // Analytics: snap index over the real street polylines, hotspot sites.
    let lines: Vec<_> = lines
        .into_iter()
        .map(|(id, pts)| {
            let pts = pts
                .into_iter()
                .map(|(lat, lon)| {
                    let p = project_enu(lat, lon, region.centroid_lat, region.centroid_lon);
                    Vec2::new(p.x, p.z)
                })
                .collect();
            (id, pts)
        })
        .collect();
    let zones = places::project(&places.zones, &region);
    info!(zones = zones.len(), "flow zones");
    commands.insert_resource(challenge::Challenges::new(
        places::for_challenges(&zones),
        clock.clone(),
    ));
    let mut agg = analytics::Aggregator::new(
        analytics::Snapper::new(&lines),
        places::for_flows(&zones),
        analytics_tx.clone(),
    )
    .with_hexes(region.centroid_lat, region.centroid_lon);
    agg.set_focus(places::for_focus(&places.zones, &region));
    agg.clock = clock;
    commands.insert_resource(agg);
    commands.insert_resource(finds);
    let inland: std::borrow::Cow<[GeoNode]> = if shore.is_empty() {
        std::borrow::Cow::Borrowed(&graph.nodes)
    } else {
        graph
            .nodes
            .iter()
            .filter(|n| !shore.contains(&n.id))
            .cloned()
            .collect()
    };
    let mut hotspots =
        hotspot::Hotspots::new(&inland, &region, &places.sites, analytics_tx.clone());
    drop(inland);
    hotspots.set_plans(&places.plans, &region);
    commands.insert_resource(hotspots);

    let (node_count, edge_count) = (graph.nodes.len(), graph.edges.len());
    // Elevation span (terrain load) — confirms non-zero ground heights reached ECS.
    let (elev_min, elev_max) = graph
        .nodes
        .iter()
        .map(|n| n.elevation_m)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), e| {
            (lo.min(e), hi.max(e))
        });

    // Element state starts neutral; `sim::setup_elements` seeds sources.
    // Lean entities: no names, no edge entities, no unused strings (a city
    // has a million of each).
    let mut lean = Vec::new();
    // Dense index in id order from the start: added later, it would move
    // every node to a new table and leave the old one's memory behind.
    let mut nodes = graph.nodes;
    nodes.sort_by_key(|n| n.id);
    for (i, mut node) in nodes.into_iter().enumerate() {
        if !node.node_type.ends_with("_source") {
            node.node_type = String::new();
        }
        node.control_state = String::new();
        lean.push((node, ElementState::default(), sim::ElemIdx(i)));
    }
    // One command for all (single spawns leave a huge command buffer behind).
    commands.spawn_batch(lean);
    commands.insert_resource(sim::PendingEdges(graph.edges));

    info!(
        slug = %region.slug,
        name = %region.name,
        nodes = node_count,
        edges = edge_count,
        elev_min_m = elev_min,
        elev_max_m = elev_max,
        "loaded region graph into ECS",
    );
    commands.insert_resource(region);
}

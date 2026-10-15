//! Rain radar values for the game (published weather files, format RAD2-VALUES;
//! docs/data-files.md "Weather"): per shard a list of H3 R4
//! blocks; each 15-minute frame holds one byte per R8 child of every block,
//! the children in ascending H3 order. A host thread reads the newest measured
//! frame every minute; `rain_at` answers "how much rain here, now".
//! Byte codes: 0 dry, 1..253 rain (mm/h = 0.1 * 10^((v-1)/72)), 255 no data.
//! Cloud cover (CLD1-VALUES, `clouds()`) comes the same way: 0..100 %.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use elements_shared::radar::{Blocks, Frame};
use tracing::{info, warn};

/// The host writes a frame every 15 min; a minute keeps the card close to the map layer.
const EVERY: Duration = Duration::from_secs(60);
/// Rain and cloud values below the weather host.
const RAIN: &str = "radar/v2/values";
const CLOUDS: &str = "clouds/v1/values";

/// The weather data host (`ELEMENTS_WEATHER_URL`, e.g.
/// `https://weather.example.org`); no default: unset, the server reads no
/// weather and the app shows no weather buttons.
pub fn base() -> Option<&'static str> {
    static BASE: OnceLock<Option<String>> = OnceLock::new();
    BASE.get_or_init(|| {
        let u = std::env::var("ELEMENTS_WEATHER_URL").ok()?;
        let u = u.trim().trim_end_matches('/');
        (u.starts_with("https://") || u.starts_with("http://")).then(|| u.to_string())
    })
    .as_deref()
}

type Latest = OnceLock<RwLock<Option<Arc<Frame>>>>;
static LATEST: Latest = OnceLock::new();
static LATEST_CLOUDS: Latest = OnceLock::new();

fn latest(which: &'static Latest) -> &'static RwLock<Option<Arc<Frame>>> {
    which.get_or_init(|| RwLock::new(None))
}

/// The newest rain frame, if the radar is on and has delivered one.
pub fn frame() -> Option<Arc<Frame>> {
    latest(&LATEST).read().ok()?.clone()
}

/// The newest cloud frame (values are % cover).
pub fn clouds() -> Option<Arc<Frame>> {
    latest(&LATEST_CLOUDS).read().ok()?.clone()
}

fn cell_res() -> u8 {
    elements_shared::radar::CELL_RES
}

#[derive(serde::Deserialize)]
struct Manifest {
    blocks: String,
    #[serde(default = "cell_res")]
    cell_res: u8,
    #[serde(default)]
    frames: Vec<ManifestFrame>,
}

#[derive(serde::Deserialize)]
struct ManifestFrame {
    time: String,
    kind: String,
    values: String,
}

fn agent() -> Option<ureq::Agent> {
    let tls = native_tls::TlsConnector::new()
        .map_err(|e| warn!(error = %e, "radar: no TLS"))
        .ok()?;
    Some(
        ureq::AgentBuilder::new()
            .tls_connector(Arc::new(tls))
            .timeout(Duration::from_secs(30))
            .user_agent("glutflut.com")
            .build(),
    )
}

fn get(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let r = agent.get(url).call().ok()?;
    let mut b = Vec::new();
    std::io::Read::read_to_end(&mut r.into_reader().take(32 << 20), &mut b).ok()?;
    // The bucket keeps files gzip-encoded; decode here (ureq doesn't).
    if b.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&b[..]), &mut out).ok()?;
        return Some(out);
    }
    Some(b)
}

use std::io::Read as _;

/// Start the readers for this shard (only with `ELEMENTS_WEATHER_URL`).
pub fn spawn(shard: &str) {
    let Some(base) = base() else {
        info!("weather: off (no ELEMENTS_WEATHER_URL)");
        return;
    };
    read(format!("{base}/{RAIN}/{shard}"), "radar", &LATEST);
    read(format!("{base}/{CLOUDS}/{shard}"), "clouds", &LATEST_CLOUDS);
}

fn read(dir: String, name: &'static str, which: &'static Latest) {
    let _ = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let Some(agent) = agent() else { return };
            let mut index: Option<(String, Blocks)> = None;
            let mut shown = String::new();
            loop {
                if let Some(m) = get(&agent, &format!("{dir}/manifest.json"))
                    .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
                {
                    if index
                        .as_ref()
                        .is_none_or(|(name, b)| *name != m.blocks || b.res() != m.cell_res)
                    {
                        index = get(&agent, &format!("{dir}/{}", m.blocks))
                            .and_then(|b| Blocks::parse(&b, m.cell_res))
                            .map(|b| (m.blocks.clone(), b));
                        if let Some((_, b)) = &index {
                            info!(blocks = b.len(), cells = b.cells, "{name}: blocks");
                        }
                    }
                    let f = m.frames.iter().rfind(|f| f.kind == "obs");
                    if let (Some(f), Some((_, blocks))) = (f, &index) {
                        if f.values != shown {
                            match get(&agent, &format!("{dir}/{}", f.values)) {
                                Some(values) if values.len() == blocks.cells => {
                                    shown = f.values.clone();
                                    *latest(which).write().unwrap() = Some(Arc::new(Frame {
                                        time: f.time.clone(),
                                        blocks: blocks.clone(),
                                        values,
                                    }));
                                }
                                Some(values) => {
                                    warn!(
                                        got = values.len(),
                                        want = blocks.cells,
                                        "{name}: frame size"
                                    )
                                }
                                None => warn!("{name}: frame not loaded"),
                            }
                        }
                    }
                }
                std::thread::sleep(EVERY);
            }
        });
}

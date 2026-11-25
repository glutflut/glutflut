//! Wind now (Open-Meteo, DWD ICON, CC BY 4.0) at the region's centre, for
//! carrying (web `spill.ts`: an ember burns down walking into the wind). Only
//! while players are connected; a host thread asks every 30 minutes, all
//! wanted regions of this process in one request. Never a player's position.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use bevy::prelude::*;

use crate::analytics::now_s;

const EVERY: Duration = Duration::from_secs(1800);
/// Older than this, the wind is unknown (fetch down).
const MAX_AGE_S: f64 = 3.0 * 3600.0;
/// Regions without players for this long are not fetched.
const IDLE_S: f64 = 3600.0;

/// What the app gets (`ServerMsg::Wind`).
#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, ts_rs::TS)]
#[ts(rename = "WindShown")]
pub struct Shown {
    /// Where it blows from (degrees, 0 = north) and its speed, gusts (m/s, 10 m).
    pub from_deg: f32,
    pub speed: f32,
    pub gusts: f32,
    /// When it was measured / modelled (unix s).
    #[ts(type = "number")]
    pub at: i64,
}

/// Rounded centre (1/1000°) -> last asked (unix s), last value.
#[derive(Default)]
struct Hub {
    wanted: HashMap<(i32, i32), f64>,
    last: HashMap<(i32, i32), Shown>,
}

fn hub() -> &'static Mutex<Hub> {
    static H: OnceLock<Mutex<Hub>> = OnceLock::new();
    H.get_or_init(Mutex::default)
}

fn key(lat: f64, lon: f64) -> (i32, i32) {
    ((lat * 1000.0).round() as i32, (lon * 1000.0).round() as i32)
}

/// The region's wind (`centre` lat, lon).
#[derive(Resource, Default)]
pub struct LiveWind {
    centre: Option<(f64, f64)>,
    shown: Option<Shown>,
    /// Changes whenever `shown` does.
    pub version: u32,
}

impl LiveWind {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            centre: Some((lat, lon)),
            ..Default::default()
        }
    }

    /// Fresh wind, if known.
    pub fn shown(&self) -> Option<Shown> {
        self.shown.filter(|s| now_s() - (s.at as f64) < MAX_AGE_S)
    }
}

/// Ask for the wind while players are here; take what the thread fetched.
pub fn update(
    mut wind: ResMut<LiveWind>,
    clients: Res<crate::web::WebClients>,
    mut next: Local<f64>,
) {
    let now = now_s();
    if now < *next {
        return;
    }
    *next = now + 30.0;
    let Some((lat, lon)) = wind.centre else {
        return;
    };
    let k = key(lat, lon);
    let got = {
        let mut h = hub().lock().unwrap();
        if !clients.is_empty() {
            h.wanted.insert(k, now);
        }
        h.last.get(&k).copied()
    };
    if got.is_some() && got != wind.shown {
        wind.shown = got;
        wind.version = wind.version.wrapping_add(1);
    }
    if !clients.is_empty() {
        static THREAD: OnceLock<()> = OnceLock::new();
        THREAD.get_or_init(|| {
            let _ = std::thread::Builder::new().name("wind".into()).spawn(run);
        });
    }
}

fn run() {
    let agent = match native_tls::TlsConnector::new() {
        Ok(tls) => ureq::AgentBuilder::new()
            .tls_connector(std::sync::Arc::new(tls))
            .timeout(Duration::from_secs(20))
            .user_agent("glutflut.com")
            .build(),
        Err(e) => {
            warn!(error = %e, "wind: no TLS");
            return;
        }
    };
    loop {
        let keys: Vec<(i32, i32)> = {
            let mut h = hub().lock().unwrap();
            let now = now_s();
            h.wanted.retain(|_, at| now - *at < IDLE_S);
            h.wanted.keys().copied().collect()
        };
        if !keys.is_empty() {
            match fetch(&agent, &keys) {
                Ok(v) => {
                    let mut h = hub().lock().unwrap();
                    for (k, s) in keys.into_iter().zip(v) {
                        if let Some(s) = s {
                            h.last.insert(k, s);
                        }
                    }
                }
                Err(e) => warn!(error = %e, "wind: fetch failed"),
            }
        }
        std::thread::sleep(EVERY);
    }
}

#[derive(serde::Deserialize)]
struct Current {
    time: i64,
    wind_speed_10m: Option<f32>,
    wind_direction_10m: Option<f32>,
    wind_gusts_10m: Option<f32>,
}

#[derive(serde::Deserialize)]
struct Answer {
    current: Current,
}

/// One request for all centres (Open-Meteo takes lists; one answer per place).
fn fetch(
    agent: &ureq::Agent,
    keys: &[(i32, i32)],
) -> Result<Vec<Option<Shown>>, Box<dyn std::error::Error>> {
    let list = |f: fn(&(i32, i32)) -> i32| {
        keys.iter()
            .map(|k| format!("{:.3}", f64::from(f(k)) / 1000.0))
            .collect::<Vec<_>>()
            .join(",")
    };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}\
         &current=wind_speed_10m,wind_direction_10m,wind_gusts_10m\
         &wind_speed_unit=ms&timeformat=unixtime&models=icon_seamless",
        list(|k| k.0),
        list(|k| k.1)
    );
    let body = agent.get(&url).call()?.into_string()?;
    let answers: Vec<Answer> = if keys.len() == 1 {
        vec![serde_json::from_str(&body)?]
    } else {
        serde_json::from_str(&body)?
    };
    Ok(answers.into_iter().map(|a| shown(a.current)).collect())
}

fn shown(c: Current) -> Option<Shown> {
    Some(Shown {
        from_deg: c.wind_direction_10m?,
        speed: c.wind_speed_10m?,
        gusts: c.wind_gusts_10m.unwrap_or(c.wind_speed_10m?),
        at: c.time,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_and_many() {
        let one = r#"{"latitude":47.66,"current":{"time":1764000000,"interval":900,"wind_speed_10m":4.2,"wind_direction_10m":250,"wind_gusts_10m":9.1}}"#;
        let a: Answer = serde_json::from_str(one).unwrap();
        let s = shown(a.current).unwrap();
        assert_eq!(
            (s.from_deg, s.speed, s.gusts, s.at),
            (250.0, 4.2, 9.1, 1_764_000_000)
        );
        let many = format!("[{one},{}]", one.replace("4.2", "null"));
        let v: Vec<Answer> = serde_json::from_str(&many).unwrap();
        let v: Vec<_> = v.into_iter().map(|a| shown(a.current)).collect();
        assert!(v[0].is_some() && v[1].is_none());
    }
}

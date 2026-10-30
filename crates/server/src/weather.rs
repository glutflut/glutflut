//! Weather days: today's
//! forecast for the region (Open-Meteo, DWD ICON, CC BY 4.0) decides a race
//! for the day. The side the weather favours gets the higher target. "Warm"
//! and "cold" are relative to the region's last 30 days (Bogotá is always ~20 °C).

use bevy::prelude::*;
use serde::Deserialize;

use elements_shared::rules::Rules;
use elements_shared::weather::WeatherDay;

use crate::sim::{ElementSim, ElementSource, GameRules, NaturalSource, Spring};

/// The rules as loaded; `GameRules` is these on today's weather.
#[derive(Resource)]
pub struct BaseRules(pub Rules);

/// Weather as a rule (`[weather] enabled`): when the day's weather is known or
/// changes, today's rules replace `GameRules`, natural sources take their new
/// strength and the field is built again if frost changed it. With live rain
/// (`live_rain.rs`) the day loses its rain part and natural fire sources
/// weaken where it rains now.
#[allow(clippy::type_complexity)]
pub fn apply(
    campaign: Res<crate::campaign::Campaign>,
    base: Res<BaseRules>,
    live: Option<Res<crate::live_rain::LiveRain>>,
    sun: Option<Res<crate::live_rain::LiveSun>>,
    tide: Option<Res<crate::tide::LiveTide>>,
    mut rules: ResMut<GameRules>,
    mut sim: ResMut<ElementSim>,
    mut naturals: Query<(
        &mut NaturalSource,
        Option<&mut ElementSource>,
        Has<Spring>,
        Option<&crate::sim::ElemIdx>,
    )>,
    mut seen: Local<Option<(Option<WeatherDay>, u32, bool, u32, bool, u32)>>,
) {
    // Tide alone (`tide.rs`) also needs the natural sources set here.
    if !base.0.weather.enabled && !base.0.tide.enabled() {
        return;
    }
    let live_on = live.as_ref().is_some_and(|l| l.on);
    let sun_on = sun.as_ref().is_some_and(|l| l.on);
    let now = (
        campaign.weather,
        live.as_ref().map_or(0, |l| l.version),
        live_on,
        sun.as_ref().map_or(0, |l| l.version),
        sun_on,
        tide.as_ref().map_or(0, |t| t.version),
    );
    if *seen == Some(now) {
        return;
    }
    let day_changed = seen.is_none_or(|s| s.0 != now.0 || s.2 != now.2 || s.4 != now.4);
    *seen = Some(now);
    let w = &base.0.weather;
    let used = campaign
        .weather
        .filter(|_| w.enabled)
        .map(|day| live_day(w, day, live_on, sun_on));
    let new = match used {
        Some(day) => base.0.for_weather(&day),
        None => base.0.clone(),
    };
    for (mut n, src, spring, idx) in &mut naturals {
        let at = |l: Option<&crate::live_rain::LiveField>| match (l, idx) {
            (Some(l), Some(i)) => l.at(i.0),
            _ => 0.0,
        };
        let (k, s) = (
            at(live.as_deref().map(|l| &l.0)),
            at(sun.as_deref().map(|l| &l.0)),
        );
        let t = match (&tide, idx) {
            (Some(t), Some(i)) => t.at(i.0),
            _ => 0.0,
        };
        n.0.strength = new.natural_strength_at(n.0.faction, spring)
            * w.live_natural(n.0.faction, k)
            * w.sun_natural(n.0.faction, s)
            * base.0.tide.natural(n.0.faction, t);
        // Not claimed right now: the natural source itself.
        if let Some(mut s) = src.filter(|s| s.life.is_none()) {
            s.strength = n.0.strength;
        }
    }
    if new.field != rules.0.field {
        sim.rebuild_field(&new.field);
    }
    if day_changed {
        if let (Some(day), Some(used)) = (campaign.weather, used) {
            info!(effect = ?used.effect(&new.weather), ?day, live_rain = live_on, live_sun = sun_on, "weather: today's rules");
        }
    }
    rules.0 = new;
}

/// What the app shows: today's main effect (`None`: calm) and the day.
#[derive(serde::Serialize, PartialEq, Debug, ts_rs::TS)]
#[ts(rename = "WeatherShown")]
pub struct Shown {
    #[ts(type = "\"rain\" | \"heat\" | \"frost\" | null")]
    pub effect: Option<&'static str>,
    /// Side it helps.
    #[ts(as = "Option<crate::protocol::Faction>")]
    pub side: Option<&'static str>,
    pub level: f32,
    pub tmax: f32,
    pub tmin: f32,
    pub precip_mm: f32,
}

/// The day as the rules use it: live rain takes its rain, live sun its sun.
pub fn live_day(
    w: &elements_shared::weather::Weather,
    day: WeatherDay,
    rain: bool,
    sun: bool,
) -> WeatherDay {
    let day = if rain { day.dry(w) } else { day };
    if sun {
        day.sunless()
    } else {
        day
    }
}

/// With live rain or sun those parts of the day don't count (the maps show them).
pub fn shown(rules: &Rules, day: Option<WeatherDay>, live: bool, sun: bool) -> Option<Shown> {
    let day = day.filter(|_| rules.weather.enabled)?;
    let day = live_day(&rules.weather, day, live, sun);
    let e = day.effect(&rules.weather);
    Some(Shown {
        effect: e.map(|e| e.0),
        side: e.map(|e| if e.0 == "heat" { "fire" } else { "water" }),
        level: e.map_or(0.0, |e| (e.1 * 100.0).round() / 100.0),
        tmax: day.tmax.round(),
        tmin: day.tmin.round(),
        precip_mm: day.precip_mm.round(),
    })
}

/// Today's weather in a region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Day {
    pub tmax: f64,
    pub tmin: f64,
    pub precip_mm: f64,
    pub sun_h: f64,
    /// Mean daily maximum of the last 30 days.
    pub usual_tmax: f64,
    pub wind_kmh: f64,
    pub wind_from: f64,
}

#[derive(Deserialize)]
struct Answer {
    daily: Daily,
}

#[derive(Deserialize)]
struct Daily {
    temperature_2m_max: Vec<Option<f64>>,
    temperature_2m_min: Vec<Option<f64>>,
    precipitation_sum: Vec<Option<f64>>,
    sunshine_duration: Vec<Option<f64>>,
    #[serde(default)]
    wind_speed_10m_max: Vec<Option<f64>>,
    #[serde(default)]
    wind_direction_10m_dominant: Vec<Option<f64>>,
}

/// Today (last entry) and the 30 days before, from Open-Meteo.
pub fn fetch(lat: f64, lon: f64) -> Result<Day, Box<dyn std::error::Error>> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat:.3}&longitude={lon:.3}\
         &daily=temperature_2m_max,temperature_2m_min,precipitation_sum,sunshine_duration,\
         wind_speed_10m_max,wind_direction_10m_dominant\
         &models=icon_seamless&timezone=auto&past_days=30&forecast_days=1"
    );
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(native_tls::TlsConnector::new()?))
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("glutflut.com")
        .build();
    let a: Answer = agent.get(&url).call()?.into_json()?;
    let d = a.daily;
    let last = |v: &[Option<f64>]| v.last().copied().flatten().ok_or("no data for today");
    let past: Vec<f64> = d
        .temperature_2m_max
        .iter()
        .rev()
        .skip(1)
        .flatten()
        .copied()
        .collect();
    let usual_tmax = if past.is_empty() {
        last(&d.temperature_2m_max)?
    } else {
        past.iter().sum::<f64>() / past.len() as f64
    };
    Ok(Day {
        tmax: last(&d.temperature_2m_max)?,
        tmin: last(&d.temperature_2m_min)?,
        precip_mm: last(&d.precipitation_sum)?,
        sun_h: last(&d.sunshine_duration)? / 3600.0,
        usual_tmax,
        // Optional: without wind the day still counts.
        wind_kmh: last(&d.wind_speed_10m_max).unwrap_or(0.0),
        wind_from: last(&d.wind_direction_10m_dominant).unwrap_or(0.0),
    })
}

impl From<Day> for elements_shared::weather::WeatherDay {
    fn from(d: Day) -> Self {
        Self {
            tmax: d.tmax as f32,
            tmin: d.tmin as f32,
            precip_mm: d.precip_mm as f32,
            sun_h: d.sun_h as f32,
            usual_tmax: d.usual_tmax as f32,
            wind_kmh: d.wind_kmh as f32,
            wind_from: d.wind_from as f32,
        }
    }
}

/// The race of the day: kind (`weather:<kind>`), goal and the sum of both
/// sides' targets (split by [`targets`]).
#[derive(Debug, PartialEq)]
pub struct Plan {
    pub kind: &'static str,
    pub goal: &'static str,
    pub total: i32,
}

/// What kind of day, in this order: rain, frost, hot, cold, warm, sunny, grey, mild.
pub fn plan(d: &Day) -> Plan {
    let p = |kind, goal, total| Plan { kind, goal, total };
    if d.precip_mm >= 5.0 {
        p("weather:rain", "claims", 80)
    } else if d.tmin <= 0.0 || d.tmax <= 3.0 {
        p("weather:frost", "claims", 80)
    } else if d.tmax >= 28.0 || d.tmax >= d.usual_tmax + 6.0 {
        p("weather:hot", "claims", 80)
    } else if d.tmax <= d.usual_tmax - 3.0 {
        p("weather:cold", "claims", 90)
    } else if d.tmax >= d.usual_tmax + 3.0 {
        p("weather:warm", "claims", 90)
    } else if d.sun_h >= 8.0 {
        p("weather:sunny", "points", 700)
    } else if d.sun_h < 1.0 {
        // Grey and hidden: a day for finds.
        p("weather:grey", "finds", 20)
    } else {
        p("weather:mild", "fronts", 40)
    }
}

/// Targets (fire, water): `total` split by fire's share of the last weeks
/// (claims or points; one side often has more to claim), so a
/// usual day is even and the weather rule tips it. Even without enough data;
/// the share is kept within 25-75 %.
pub fn targets(total: i32, fire_share: Option<f64>) -> (i32, i32) {
    let f = fire_share.map_or(0.5, |x| x.clamp(0.25, 0.75));
    let fire = ((total as f64 * f).round() as i32).max(1);
    (fire, (total - fire).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(tmax: f64, tmin: f64, precip_mm: f64, sun_h: f64, usual_tmax: f64) -> Day {
        Day {
            tmax,
            tmin,
            precip_mm,
            sun_h,
            usual_tmax,
            wind_kmh: 0.0,
            wind_from: 0.0,
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(plan(&day(15.0, 8.0, 12.0, 0.0, 15.0)).kind, "weather:rain");
        assert_eq!(plan(&day(2.0, -3.0, 0.0, 5.0, 4.0)).kind, "weather:frost");
        assert_eq!(plan(&day(30.0, 18.0, 0.0, 12.0, 27.0)).kind, "weather:hot");
        // Bogotá: 24 °C is hot for a city that is usually 19 °C.
        assert_eq!(plan(&day(25.0, 11.0, 0.0, 6.0, 19.0)).kind, "weather:hot");
        assert_eq!(plan(&day(16.0, 9.0, 1.0, 4.0, 20.0)).kind, "weather:cold");
        assert_eq!(plan(&day(18.0, 9.0, 0.0, 9.0, 17.0)).kind, "weather:sunny");
        assert_eq!(plan(&day(12.0, 8.0, 0.5, 0.2, 12.0)).kind, "weather:grey");
        assert_eq!(plan(&day(14.0, 7.0, 0.0, 4.0, 14.0)).kind, "weather:mild");
    }

    #[test]
    fn targets_follow_the_measured_share() {
        assert_eq!(targets(80, None), (40, 40), "no data: even");
        assert_eq!(targets(80, Some(0.6)), (48, 32));
        assert_eq!(targets(80, Some(0.95)), (60, 20), "at most 75 %");
    }
}

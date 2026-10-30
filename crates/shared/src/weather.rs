//! Weather as a rule: today's
//! weather tilts the existing per-element knobs. Rain helps water, warmth and
//! sun help fire, frost lets water creep uphill. Off unless `[weather] enabled`
//! (server: `weather::apply`, with the day stored in `game.weather_days`).

use serde::{Deserialize, Serialize};

use crate::elements::Faction;
use crate::rules::Rules;

/// Effect sizes at full intensity (0 = that effect off).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Weather {
    pub enabled: bool,
    /// Rain: water sources age this much slower, fire ones this much faster.
    pub rain_decay: f32,
    /// Rain: natural fire sources this much weaker.
    pub rain_natural: f32,
    /// Warm or sunny: the same the other way round (water ages faster, natural water weaker).
    pub heat_decay: f32,
    pub heat_natural: f32,
    /// Frost: water climbs this much better (`field.water_climb_m` × (1 + x)).
    pub frost_climb: f32,
    /// Full intensity from: this much rain (mm/day), this many K above the
    /// usual maximum (+2 K is still normal), this many degrees below zero (minimum).
    pub rain_full_mm: f32,
    pub heat_full_k: f32,
    pub frost_full_c: f32,
    /// Live rain (radar, per junction and quarter hour): where
    /// it rains now, water sources age this much slower, fire ones faster,
    /// natural fire sources are this much weaker. Where radar data exists it
    /// replaces the day's rain effect; without data nothing changes. 0 = off.
    pub live_decay: f32,
    pub live_natural: f32,
    /// Live intensity 0 at `live_min_mm_h`, 1 from `live_full_mm_h` (log scale).
    pub live_min_mm_h: f32,
    pub live_full_mm_h: f32,
    /// Live sun (cloud cover per junction and quarter hour, by day): where the sky is clear now, fire sources age this much slower,
    /// water ones faster, natural water sources are this much weaker. Where
    /// cloud data exists it replaces the day's sun. 0 = off.
    pub sun_decay: f32,
    pub sun_natural: f32,
    /// Sun intensity 1 at or below `sun_clear_pct` cloud cover, 0 from
    /// `sun_cloudy_pct`; times daylight, full from `sun_elev_full` degrees.
    pub sun_clear_pct: f32,
    pub sun_cloudy_pct: f32,
    pub sun_elev_full: f32,
}

impl Default for Weather {
    fn default() -> Self {
        Self {
            enabled: false,
            rain_decay: 0.0,
            rain_natural: 0.0,
            heat_decay: 0.0,
            heat_natural: 0.0,
            frost_climb: 0.0,
            rain_full_mm: 10.0,
            heat_full_k: 6.0,
            frost_full_c: 5.0,
            live_decay: 0.0,
            live_natural: 0.0,
            live_min_mm_h: 0.2,
            live_full_mm_h: 4.0,
            sun_decay: 0.0,
            sun_natural: 0.0,
            sun_clear_pct: 20.0,
            sun_cloudy_pct: 80.0,
            sun_elev_full: 10.0,
        }
    }
}

/// One day's weather, as the server's `weather::Day` (Open-Meteo).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherDay {
    pub tmax: f32,
    pub tmin: f32,
    pub precip_mm: f32,
    pub sun_h: f32,
    /// Mean daily maximum of the last 30 days.
    pub usual_tmax: f32,
    /// Strongest wind of the day (km/h, 10 m) and where it mostly came from
    /// (degrees, 0 = north): sparks fly downwind (`[keep] ember_*`).
    pub wind_kmh: f32,
    pub wind_from: f32,
}

impl Default for WeatherDay {
    /// A mild December day in Konstanz: no effect.
    fn default() -> Self {
        Self {
            tmax: 5.0,
            tmin: 1.0,
            precip_mm: 0.0,
            sun_h: 2.0,
            usual_tmax: 5.0,
            wind_kmh: 0.0,
            wind_from: 0.0,
        }
    }
}

impl WeatherDay {
    /// Presets (December in Konstanz, usual maximum 5 °C).
    pub fn preset(name: &str) -> Option<Self> {
        let d = |tmax, tmin, precip_mm, sun_h| Self {
            tmax,
            tmin,
            precip_mm,
            sun_h,
            usual_tmax: 5.0,
            ..Self::default()
        };
        Some(match name {
            "mild" => d(6.0, 2.0, 0.5, 2.0),
            "drizzle" => d(6.0, 3.0, 4.0, 0.5),
            "rain" => d(7.0, 3.0, 12.0, 0.0),
            "warm" => d(11.0, 4.0, 0.0, 6.0),
            "foehn" => d(14.0, 6.0, 0.0, 7.0),
            "frost" => d(1.0, -6.0, 0.0, 5.0),
            _ => return None,
        })
    }

    /// Intensities 0..1: (rain, heat, frost).
    pub fn intensity(&self, w: &Weather) -> (f32, f32, f32) {
        let rain = ((self.precip_mm - 1.0) / (w.rain_full_mm - 1.0).max(0.1)).clamp(0.0, 1.0);
        let warm = ((self.tmax - self.usual_tmax - 2.0) / w.heat_full_k.max(0.1)).clamp(0.0, 1.0);
        let hot = if self.tmax >= 28.0 { 1.0 } else { 0.0 };
        let frost = if self.tmax <= 0.0 {
            1.0
        } else {
            (-self.tmin / w.frost_full_c.max(0.1)).clamp(0.0, 1.0)
        };
        // Sun counts half, and only on a dry day without frost.
        let sun = 0.5 * ((self.sun_h - 4.0) / 6.0).clamp(0.0, 1.0) * (1.0 - rain) * (1.0 - frost);
        let heat = warm.max(hot).max(sun);
        (rain, heat, frost)
    }
}

impl Weather {
    /// Live rain is a rule here.
    pub fn live(&self) -> bool {
        self.enabled && (self.live_decay > 0.0 || self.live_natural > 0.0)
    }

    /// Intensity 0..1 of `mm_h` rain now.
    pub fn live_intensity(&self, mm_h: f32) -> f32 {
        if mm_h <= self.live_min_mm_h {
            return 0.0;
        }
        let span = (self.live_full_mm_h / self.live_min_mm_h).max(1.01).ln();
        ((mm_h / self.live_min_mm_h).ln() / span).clamp(0.0, 1.0)
    }

    /// Ageing factor of a source of `f` at intensity `k`.
    pub fn live_age(&self, f: Faction, k: f32) -> f32 {
        match f {
            Faction::Water => (1.0 - k * self.live_decay).max(0.1),
            Faction::Fire => 1.0 + k * self.live_decay,
        }
    }

    /// Live sun is a rule here.
    pub fn live_sun(&self) -> bool {
        self.enabled && (self.sun_decay > 0.0 || self.sun_natural > 0.0)
    }

    /// Sun intensity 0..1 at `cloud_pct` cover with the sun `elev` degrees up.
    pub fn sun_intensity(&self, cloud_pct: f32, elev: f32) -> f32 {
        let span = (self.sun_cloudy_pct - self.sun_clear_pct).max(1.0);
        let clear = ((self.sun_cloudy_pct - cloud_pct) / span).clamp(0.0, 1.0);
        clear * (elev / self.sun_elev_full.max(0.1)).clamp(0.0, 1.0)
    }

    /// Ageing factor of a source of `f` in sun of intensity `s`.
    pub fn sun_age(&self, f: Faction, s: f32) -> f32 {
        match f {
            Faction::Fire => (1.0 - s * self.sun_decay).max(0.1),
            Faction::Water => 1.0 + s * self.sun_decay,
        }
    }

    /// Strength factor of a natural source of `f` in sun of intensity `s`.
    pub fn sun_natural(&self, f: Faction, s: f32) -> f32 {
        match f {
            Faction::Water => 1.0 - (s * self.sun_natural).min(0.9),
            Faction::Fire => 1.0,
        }
    }

    /// Strength factor of a natural source of `f` at intensity `k`.
    pub fn live_natural(&self, f: Faction, k: f32) -> f32 {
        match f {
            Faction::Fire => 1.0 - (k * self.live_natural).min(0.9),
            Faction::Water => 1.0,
        }
    }
}

impl WeatherDay {
    /// The same day without its rain (live rain takes that part over). The
    /// sun keeps the weight the rain left it, so a wet sunny day doesn't turn
    /// into a sunny one.
    pub fn dry(&self, w: &Weather) -> Self {
        let (rain, _, _) = self.intensity(w);
        let sun = ((self.sun_h - 4.0) / 6.0).clamp(0.0, 1.0) * (1.0 - rain);
        Self {
            precip_mm: 0.0,
            sun_h: self.sun_h.min(4.0 + 6.0 * sun),
            ..*self
        }
    }

    /// The same day without its sun (live sun takes that part over); warmth stays.
    pub fn sunless(&self) -> Self {
        Self {
            sun_h: 0.0,
            ..*self
        }
    }

    /// The day's main effect for the app: `rain`, `heat` or `frost` and its
    /// intensity (only effects the rules use; `None` below 0.1, a calm day).
    pub fn effect(&self, w: &Weather) -> Option<(&'static str, f32)> {
        let (rain, heat, frost) = self.intensity(w);
        let on = |x: f32, sizes: &[f32]| {
            if sizes.iter().any(|&s| s > 0.0) {
                x
            } else {
                0.0
            }
        };
        [
            ("rain", on(rain, &[w.rain_decay, w.rain_natural])),
            ("heat", on(heat, &[w.heat_decay, w.heat_natural])),
            ("frost", on(frost, &[w.frost_climb])),
        ]
        .into_iter()
        .filter(|&(_, x)| x >= 0.1)
        .max_by(|a, b| a.1.total_cmp(&b.1))
    }
}

impl Rules {
    /// These rules on a day with weather `d` (unchanged unless `[weather] enabled`).
    pub fn for_weather(&self, d: &WeatherDay) -> Rules {
        let w = &self.weather;
        if !w.enabled {
            return self.clone();
        }
        let (rain, heat, frost) = d.intensity(w);
        let mut r = self.clone();
        // Positive: water favoured, negative: fire.
        let decay = rain * w.rain_decay - heat * w.heat_decay;
        r.water_decay = (r.water_decay * (1.0 - decay)).max(0.1);
        r.fire_decay = (r.fire_decay * (1.0 + decay)).max(0.1);
        // Only the other side's natural sources weaken (water is already at 1).
        let natural = rain * w.rain_natural - heat * w.heat_natural;
        if natural > 0.0 {
            r.natural_fire_strength *= 1.0 - natural.min(0.9);
        } else {
            r.natural_water_strength *= 1.0 + natural.max(-0.9);
            r.natural_spring_strength *= 1.0 + natural.max(-0.9);
        }
        if r.field.water_climb_m > 0.0 {
            r.field.water_climb_m *= 1.0 + frost * w.frost_climb;
        }
        r
    }
}

/// The sun's elevation in degrees at a place and time (unix s); a short
/// almanac formula, good to a fraction of a degree.
pub fn sun_elevation(lat: f64, lon: f64, unix_s: f64) -> f32 {
    let d = unix_s / 86_400.0 - 10_957.5; // days since J2000.0
    let rad = std::f64::consts::PI / 180.0;
    let g = (357.529 + 0.985_600_28 * d) * rad;
    let q = 280.459 + 0.985_647_36 * d;
    let l = (q + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()) * rad;
    let e = (23.439 - 0.000_000_36 * d) * rad;
    let ra = (e.cos() * l.sin()).atan2(l.cos());
    let dec = (e.sin() * l.sin()).asin();
    let gmst = (18.697_374_558 + 24.065_709_824_419_08 * d).rem_euclid(24.0);
    let ha = (gmst * 15.0 + lon) * rad - ra;
    let (la, de) = (lat * rad, dec);
    ((la.sin() * de.sin() + la.cos() * de.cos() * ha.cos()).asin() / rad) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> Rules {
        Rules {
            weather: Weather {
                enabled: true,
                rain_decay: 0.3,
                rain_natural: 0.3,
                heat_decay: 0.3,
                heat_natural: 0.3,
                frost_climb: 1.0,
                ..Default::default()
            },
            natural_fire_strength: 0.6,
            natural_water_strength: 0.8,
            ..Default::default()
        }
    }

    #[test]
    fn sun_over_konstanz() {
        // 21 Dec 2026, 11:20 UTC (local noon): about 19° up; midnight below.
        let noon = 1_797_852_000.0;
        let e = sun_elevation(47.66, 9.18, noon);
        assert!((e - 19.0).abs() < 1.0, "{e}");
        assert!(sun_elevation(47.66, 9.18, noon + 12.0 * 3600.0) < -40.0);
        let w = Weather {
            sun_decay: 0.3,
            ..Default::default()
        };
        assert_eq!(w.sun_intensity(10.0, 19.0), 1.0);
        assert_eq!(w.sun_intensity(90.0, 19.0), 0.0);
        assert_eq!(w.sun_intensity(10.0, -5.0), 0.0);
        assert!((w.sun_intensity(50.0, 5.0) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn dry_keeps_the_suns_weight() {
        let w = on().weather;
        let d = WeatherDay {
            precip_mm: 15.0,
            sun_h: 11.0,
            ..Default::default()
        };
        assert_eq!(d.dry(&w).intensity(&w).1, d.intensity(&w).1);
        assert_eq!(d.dry(&w).intensity(&w).0, 0.0);
    }

    #[test]
    fn off_changes_nothing() {
        let r = Rules::default();
        assert_eq!(r.for_weather(&WeatherDay::preset("rain").unwrap()), r);
    }

    #[test]
    fn mild_day_changes_nothing() {
        let r = on();
        assert_eq!(r.for_weather(&WeatherDay::preset("mild").unwrap()), r);
    }

    #[test]
    fn rain_helps_water_heat_helps_fire() {
        let r = on();
        let rain = r.for_weather(&WeatherDay::preset("rain").unwrap());
        assert!(rain.water_decay < 1.0 && rain.fire_decay > 1.0);
        assert!(rain.natural_water_strength == 0.8 && rain.natural_fire_strength < 0.6);
        let warm = r.for_weather(&WeatherDay::preset("foehn").unwrap());
        assert!(warm.fire_decay < 1.0 && warm.water_decay > 1.0);
        assert!(warm.natural_fire_strength == 0.6 && warm.natural_water_strength < 0.8);
    }

    #[test]
    fn main_effect() {
        let w = on().weather;
        assert_eq!(WeatherDay::preset("mild").unwrap().effect(&w), None);
        assert_eq!(
            WeatherDay::preset("rain").unwrap().effect(&w).map(|e| e.0),
            Some("rain")
        );
        assert_eq!(
            WeatherDay::preset("foehn").unwrap().effect(&w).map(|e| e.0),
            Some("heat")
        );
        assert_eq!(
            WeatherDay::preset("frost").unwrap().effect(&w).map(|e| e.0),
            Some("frost")
        );
    }

    #[test]
    fn frost_lets_water_climb() {
        let mut r = on();
        r.field.water_climb_m = 5.0;
        let f = r.for_weather(&WeatherDay::preset("frost").unwrap());
        assert!(f.field.water_climb_m > 5.0);
        assert_eq!(f.fire_decay, r.fire_decay);
    }

    #[test]
    fn live_rain() {
        let w = Weather {
            enabled: true,
            live_decay: 0.5,
            live_natural: 0.5,
            ..Default::default()
        };
        assert!(w.live());
        assert_eq!(w.live_intensity(0.1), 0.0);
        assert_eq!(w.live_intensity(4.0), 1.0);
        assert!((w.live_intensity((0.2f32 * 4.0).sqrt()) - 0.5).abs() < 1e-4);
        assert_eq!(w.live_age(Faction::Water, 1.0), 0.5);
        assert_eq!(w.live_age(Faction::Fire, 1.0), 1.5);
        assert_eq!(w.live_natural(Faction::Fire, 1.0), 0.5);
        assert_eq!(w.live_natural(Faction::Water, 1.0), 1.0);
        assert!(!Weather::default().live());
    }
}

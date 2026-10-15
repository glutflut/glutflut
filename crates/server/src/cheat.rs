//! Thresholds of the cheat checks (`motion.rs`, `plausible.rs`). The defaults
//! work; an instance can set its own with `ELEMENTS_CHEAT="key=value; …"`
//! (keys as below), so the live values need not be the published ones.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cheat {
    /// Top plausible speed (m/s; ~29 km/h: running, slow cycling).
    pub max_speed_ms: f32,
    /// Extra metres for a GPS jump between two fixes.
    pub jump_slack_m: f32,
    /// Extra metres over the speed window.
    pub window_slack_m: f32,
    pub window_min_s: f64,
    pub window_max_s: f64,
    /// How long an implausible fix locks claims.
    pub lock_s: f64,
    /// One reported speed above this locks (m/s, ~40 km/h).
    pub drive_ms: f32,
    /// Two reported speeds above this within `fast_window_s` lock (~25 km/h).
    pub fast_ms: f32,
    pub fast_window_s: f64,
    /// Fixes looked at for a made-up track.
    pub track_fixes: usize,
    /// Path within those fixes: only moving tracks are judged.
    pub track_min_path_m: f32,
    /// Median deviation from the neighbours' line below this: no noise.
    pub smooth_m: f32,
    /// ... below this: computed, not measured.
    pub exact_m: f32,
    /// Median distance to the street axis below this: on the axis.
    pub axis_m: f32,
    /// Fixes that must have the value for the accuracy and axis signals.
    pub min_with: usize,
}

impl Default for Cheat {
    fn default() -> Self {
        Self {
            max_speed_ms: 8.0,
            jump_slack_m: 80.0,
            window_slack_m: 40.0,
            window_min_s: 20.0,
            window_max_s: 60.0,
            lock_s: 60.0,
            drive_ms: 11.0,
            fast_ms: 7.0,
            fast_window_s: 30.0,
            track_fixes: 60,
            track_min_path_m: 60.0,
            // A smoothing Android phone walking the Altstadt: 4 cm (2026-10-07).
            smooth_m: 0.015,
            exact_m: 0.005,
            // The same phone: 49 cm in narrow streets; route players: ~0.
            axis_m: 0.15,
            min_with: 50,
        }
    }
}

impl Cheat {
    /// Parse `key=value; other=value` on top of the defaults.
    pub fn parse(s: &str) -> Result<Self, String> {
        let lines: Vec<&str> = s
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let c: Self = toml::from_str(&lines.join("\n")).map_err(|e| e.message().to_string())?;
        if c.track_fixes < 3 || c.min_with > c.track_fixes || c.window_min_s > c.window_max_s {
            return Err(
                "track_fixes >= 3, min_with <= track_fixes, window_min_s <= window_max_s".into(),
            );
        }
        Ok(c)
    }
}

/// The instance's thresholds; a bad `ELEMENTS_CHEAT` stops the server at startup.
pub fn get() -> &'static Cheat {
    static C: std::sync::LazyLock<Cheat> = std::sync::LazyLock::new(|| {
        std::env::var("ELEMENTS_CHEAT").map_or_else(
            |_| Cheat::default(),
            |s| Cheat::parse(&s).unwrap_or_else(|e| panic!("ELEMENTS_CHEAT: {e}")),
        )
    });
    &C
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_on_top_of_defaults() {
        let c = Cheat::parse("max_speed_ms = 7.5; lock_s=90 ;").unwrap();
        assert_eq!((c.max_speed_ms, c.lock_s, c.drive_ms), (7.5, 90.0, 11.0));
        assert_eq!(Cheat::parse("").unwrap().track_fixes, 60);
    }

    #[test]
    fn rejects_typos_and_nonsense() {
        assert!(Cheat::parse("max_speed=7").is_err());
        assert!(Cheat::parse("min_with=80").is_err());
    }
}

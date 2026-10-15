//! Days in the region's local time: points per day, the
//! leaderboard "today", streaks and daily limits change at local midnight, not
//! at UTC midnight (Toronto 20:00, Bogotá 19:00). Day numbers count from
//! 1970-01-01 as before, so stored dates and `week_of` keep their meaning.

use bevy::prelude::*;
use jiff::tz::TimeZone;
use jiff::Timestamp;

#[derive(Resource, Clone)]
pub struct Clock {
    tz: TimeZone,
}

impl Default for Clock {
    fn default() -> Self {
        Self { tz: TimeZone::UTC }
    }
}

impl Clock {
    /// The region's clock (`geo.regions.timezone`); UTC if unknown.
    pub fn new(name: Option<&str>) -> Self {
        let tz = name.and_then(|n| match TimeZone::get(n) {
            Ok(tz) => Some(tz),
            Err(e) => {
                warn!(zone = n, error = %e, "clock: unknown time zone, using UTC");
                None
            }
        });
        Self {
            tz: tz.unwrap_or(TimeZone::UTC),
        }
    }

    /// The local day of `unix` seconds (days since 1970-01-01).
    pub fn day(&self, unix: f64) -> i64 {
        let s = unix.floor() as i64;
        let offset = Timestamp::from_second(s)
            .map(|t| self.tz.to_offset(t).seconds())
            .unwrap_or(0);
        (s + i64::from(offset)).div_euclid(86_400)
    }

    /// Unix seconds at which local `day` begins; a day is 23 or 25 hours long
    /// around a clock change (pooling analytics into days).
    pub fn day_start(&self, day: i64) -> f64 {
        let date = jiff::civil::date(1970, 1, 1).checked_add(jiff::Span::new().days(day));
        date.ok()
            .and_then(|d| d.to_zoned(self.tz.clone()).ok())
            .map_or(day as f64 * 86_400.0, |z| z.timestamp().as_second() as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn at(y: i16, m: i8, d: i8, h: i8, min: i8) -> jiff::civil::DateTime {
        jiff::civil::datetime(y, m, d, h, min, 0, 0)
    }

    fn unix(dt: jiff::civil::DateTime, zone: &str) -> f64 {
        dt.in_tz(zone).unwrap().timestamp().as_second() as f64
    }

    #[test]
    fn days_change_at_local_midnight() {
        for zone in [
            "Europe/Berlin",
            "America/Toronto",
            "America/Bogota",
            "Atlantic/Canary",
        ] {
            let c = Clock::new(Some(zone));
            let before = c.day(unix(at(2026, 12, 12, 23, 59), zone));
            let after = c.day(unix(at(2026, 12, 13, 0, 0), zone));
            assert_eq!(after, before + 1, "{zone}");
            assert_eq!(
                c.day_start(after),
                unix(at(2026, 12, 13, 0, 0), zone),
                "{zone}"
            );
        }
        // Toronto at 20:00 local is already the next day in UTC, not here.
        let t = Clock::new(Some("America/Toronto"));
        let evening = unix(at(2026, 10, 10, 20, 30), "America/Toronto");
        assert_eq!(t.day(evening) + 1, Clock::default().day(evening));
    }

    #[test]
    fn clock_changes_make_long_and_short_days() {
        let c = Clock::new(Some("Europe/Berlin"));
        // 25 Oct 2026: clocks go back, the day has 25 hours.
        let d = c.day(unix(at(2026, 10, 25, 12, 0), "Europe/Berlin"));
        assert_eq!(c.day_start(d + 1) - c.day_start(d), 25.0 * 3600.0);
        // 28 Mar 2027: forward, 23 hours.
        let d = c.day(unix(at(2027, 3, 28, 12, 0), "Europe/Berlin"));
        assert_eq!(c.day_start(d + 1) - c.day_start(d), 23.0 * 3600.0);
        assert_eq!(
            Clock::new(Some("Mars/Olympus")).day(86_400.0 * 3.5),
            3,
            "unknown: UTC"
        );
    }
}

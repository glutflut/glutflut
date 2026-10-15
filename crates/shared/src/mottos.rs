//! Tag lines for permanent sources: predefined
//! phrases, chosen by key, shown to everyone in their own language. No free
//! text, so nothing to moderate. More phrases with each level; some need a
//! badge. A source stores the phrase's number (1-based; 0 = none).

use crate::elements::Faction;

pub struct Motto {
    pub key: &'static str,
    pub side: Faction,
    /// From this level (2-5).
    pub level: u8,
    /// Needs a badge starting with this (e.g. `chapter-win:`).
    pub badge: Option<&'static str>,
}

const fn m(key: &'static str, side: Faction, level: u8, badge: Option<&'static str>) -> Motto {
    Motto {
        key,
        side,
        level,
        badge,
    }
}

use Faction::{Fire as F, Water as W};

/// Keys are stable (stored); texts live in the app's locale files (`motto.<key>`).
pub const MOTTOS: &[Motto] = &[
    m("f-glows-on", F, 2, None),
    m("f-come-closer", F, 2, None),
    m("f-warm-here", F, 2, None),
    m("f-one-spark", F, 2, None),
    m("f-dont-douse", F, 3, None),
    m("f-fire-free", F, 3, None),
    m("f-hot-stays", F, 3, None),
    m("f-ember-watch", F, 3, None),
    m("f-air-burns", F, 4, None),
    m("f-flames-dance", F, 4, None),
    m("f-from-ashes", F, 4, None),
    m("f-still-blazing", F, 4, None),
    m("f-firestorm", F, 5, None),
    m("f-unquenchable", F, 5, None),
    m("f-sun-lives", F, 5, None),
    m("f-never-flood", F, 5, None),
    m("f-won", F, 2, Some("chapter-win:")),
    m("f-found-all", F, 2, Some("coll:")),
    m("w-flood-comes", W, 2, None),
    m("w-nothing-dry", W, 2, None),
    m("w-cool-here", W, 2, None),
    m("w-one-drop", W, 2, None),
    m("w-dont-thaw", W, 3, None),
    m("w-make-waves", W, 3, None),
    m("w-ice-cold", W, 3, None),
    m("w-spring-watch", W, 3, None),
    m("w-current", W, 4, None),
    m("w-waves-break", W, 4, None),
    m("w-steam-rain", W, 4, None),
    m("w-still-rising", W, 4, None),
    m("w-deluge", W, 5, None),
    m("w-unstoppable", W, 5, None),
    m("w-lake-lives", W, 5, None),
    m("w-never-ember", W, 5, None),
    m("w-won", W, 2, Some("chapter-win:")),
    m("w-found-all", W, 2, Some("coll:")),
];

/// The phrase's number for a stored key (1-based).
pub fn index(key: &str) -> Option<u16> {
    MOTTOS
        .iter()
        .position(|m| m.key == key)
        .map(|i| i as u16 + 1)
}

/// The key of a stored number (0: none).
pub fn key(i: u16) -> Option<&'static str> {
    (i as usize)
        .checked_sub(1)
        .and_then(|i| MOTTOS.get(i))
        .map(|m| m.key)
}

/// A city's campaign phrase (`game.campaign_mottos`) is stored as this plus
/// its number (1-999) and keyed `c-<number>`; its texts come from the city.
pub const CITY_BASE: u16 = 1000;

/// The number for a stored key: built-in phrases and city phrases.
pub fn parse(key: &str) -> Option<u16> {
    match key.strip_prefix("c-") {
        Some(n) => n
            .parse::<u16>()
            .ok()
            .filter(|v| (1..=999).contains(v) && v.to_string() == n)
            .map(|v| CITY_BASE + v),
        None => index(key),
    }
}

/// The key of a stored number, built-in or city (0: none).
pub fn key_of(i: u16) -> Option<std::borrow::Cow<'static, str>> {
    if i > CITY_BASE && i < CITY_BASE + 1000 {
        return Some(format!("c-{}", i - CITY_BASE).into());
    }
    key(i).map(Into::into)
}

/// May a player of `side` at `level` with these badges choose phrase `i`?
pub fn allowed<'a>(
    i: u16,
    side: Faction,
    level: u8,
    mut badges: impl Iterator<Item = &'a str>,
) -> bool {
    let Some(m) = (i as usize).checked_sub(1).and_then(|i| MOTTOS.get(i)) else {
        return false;
    };
    m.side == side && level >= m.level && m.badge.is_none_or(|b| badges.any(|x| x.starts_with(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_unlock_by_side_level_and_badge() {
        let i = index("f-glows-on").unwrap();
        assert_eq!(key(i), Some("f-glows-on"));
        assert_eq!(key(0), None);
        assert!(allowed(i, F, 2, std::iter::empty()));
        assert!(!allowed(i, F, 1, std::iter::empty()), "level 1: none");
        assert!(!allowed(i, W, 5, std::iter::empty()), "the other side's");
        let storm = index("f-firestorm").unwrap();
        assert!(!allowed(storm, F, 4, std::iter::empty()));
        assert!(allowed(storm, F, 5, std::iter::empty()));
        let won = index("w-won").unwrap();
        assert!(!allowed(won, W, 5, ["claims-10"].into_iter()));
        assert!(allowed(won, W, 2, ["chapter-win:advent-1"].into_iter()));
        // Keys are unique (they are stored).
        let mut keys: Vec<_> = MOTTOS.iter().map(|m| m.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), MOTTOS.len());
        assert!(
            keys.iter().all(|k| !k.starts_with("c-")),
            "c- is the city's"
        );
    }

    #[test]
    fn city_phrases() {
        assert_eq!(parse("c-7"), Some(CITY_BASE + 7));
        assert_eq!(key_of(CITY_BASE + 7).as_deref(), Some("c-7"));
        assert_eq!(parse("f-glows-on"), index("f-glows-on"));
        assert_eq!(key_of(index("w-won").unwrap()).as_deref(), Some("w-won"));
        for bad in ["c-0", "c-1000", "c-07", "c-", "c-x", "c--1"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert!(
            !allowed(CITY_BASE + 7, F, 5, std::iter::empty()),
            "city phrases are checked by the region"
        );
    }
}

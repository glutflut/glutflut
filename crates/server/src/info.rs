//! What the app shows when a junction is tapped (`info` → `info`): strength,
//! source, whether the player can take it (and when), terrain and
//! surroundings. Computed on request; names of owners are never sent.

use serde::Serialize;

use elements_shared::rules::Rules;

use crate::sim::{ElementSource, SourceKind};

/// The source on a junction as the card shows it.
#[derive(Serialize, ts_rs::TS)]
#[ts(rename = "NodeSource")]
pub struct Src {
    /// natural, claim, anchor, spark, keep
    #[ts(type = "\"natural\" | \"claim\" | \"anchor\" | \"spark\" | \"keep\"")]
    pub kind: &'static str,
    #[ts(as = "crate::protocol::Faction")]
    pub faction: &'static str,
    /// Seconds until it runs out (claims, sparks) or has faded to its floor (keep).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub left: Option<u32>,
    /// Keep: seconds until it is gone without a top-up.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub gone: Option<u32>,
    /// Seconds of build-up left.
    pub build: u32,
    /// Placed by this player (claims; the app knows its own permanent source).
    pub yours: bool,
}

/// Effective ageing speed of a source at a node with `affinity`.
fn speed(s: &ElementSource, rules: &Rules, affinity: f32) -> f32 {
    match s.kind {
        SourceKind::Spark => 1.0,
        SourceKind::Keep => rules.source_age(s.faction, 1.0, affinity),
        _ => rules.source_age(s.faction, 1.0, affinity) * s.supply,
    }
    .max(1e-3)
}

/// Strength of `s` after `t` more seconds (`None`: gone).
fn strength_after(
    s: &ElementSource,
    rules: &Rules,
    affinity: f32,
    wear: f32,
    t: f32,
) -> Option<f32> {
    let age = s.age + t * speed(s, rules, affinity);
    match s.kind {
        SourceKind::Keep => Some(rules.keep.strength(age, wear) * s.mult),
        SourceKind::Spark => Some(0.0),
        _ => rules.source_strength(age, s.life?).map(|x| x * s.mult),
    }
}

/// Seconds until an enemy source is weak enough to take over with a claim of
/// strength `attack` (0 = now), `None` if not within two days.
pub fn weak_in(
    s: &ElementSource,
    rules: &Rules,
    affinity: f32,
    wear: f32,
    attack: f32,
) -> Option<u32> {
    let below = if s.newcomer {
        rules.progression.newcomer_takeover_below
    } else {
        rules.takeover_below
    };
    let limit = attack * below;
    if !rules.takeover_stronger || s.kind == SourceKind::Spark {
        return Some(0);
    }
    let weak =
        |t: u32| strength_after(s, rules, affinity, wear, t as f32).is_none_or(|x| x < limit);
    // Coarse steps over two days, then the exact second within the last step
    // (the card counts down between answers; a rounded value would jump).
    let coarse = (0..=5760u32).map(|k| k * 30).find(|&t| weak(t))?;
    Some(
        (coarse.saturating_sub(29)..=coarse)
            .find(|&t| weak(t))
            .unwrap_or(coarse),
    )
}

pub fn source(
    s: &ElementSource,
    natural: bool,
    rules: &Rules,
    affinity: f32,
    me: Option<u32>,
) -> Src {
    let kind = match s.kind {
        _ if natural && s.life.is_none() => "natural",
        SourceKind::Claim => "claim",
        SourceKind::Anchor => "anchor",
        SourceKind::Spark => "spark",
        SourceKind::Keep => "keep",
    };
    let v = speed(s, rules, affinity);
    let left = match (s.kind, s.life) {
        (SourceKind::Keep, _) => Some(((rules.keep.fade_s - s.age).max(0.0) / v) as u32),
        (_, Some(life)) => Some(((life - s.age).max(0.0) / v) as u32),
        _ => None,
    };
    Src {
        kind,
        faction: s.faction.as_str(),
        left,
        gone: (s.kind == SourceKind::Keep).then(|| (rules.keep.expire_s - s.built).max(0.0) as u32),
        build: (s.ramp - s.built).max(0.0).ceil() as u32,
        yours: me.is_some() && s.by == me,
    }
}

/// Terrain around a junction: `peak` (higher than its neighbours on average
/// by at least 1 m: fire gathers), `sink` (lower: water gathers), `slope`
/// (a neighbour 1 m or more apart) or `flat`; and the height difference to
/// the neighbours' mean.
pub fn terrain(elev: f64, neighbours: &[f64]) -> (&'static str, f32) {
    if neighbours.is_empty() {
        return ("flat", 0.0);
    }
    let mean = neighbours.iter().sum::<f64>() / neighbours.len() as f64;
    let dh = (elev - mean) as f32;
    let steep = neighbours.iter().any(|n| (n - elev).abs() >= 1.0);
    let kind = if dh >= 1.0 {
        "peak"
    } else if dh <= -1.0 {
        "sink"
    } else if steep {
        "slope"
    } else {
        "flat"
    };
    (kind, (dh * 10.0).round() / 10.0)
}

/// Surroundings (POI affinity, -1 water .. +1 fire).
pub fn lean(affinity: f32) -> &'static str {
    if affinity >= 0.2 {
        "fire"
    } else if affinity <= -0.2 {
        "water"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elements_shared::elements::Faction;
    use elements_shared::rules::Decay;

    fn claim(age: f32) -> ElementSource {
        ElementSource {
            faction: Faction::Fire,
            life: Some(540.0),
            age,
            strength: 0.8,
            ramp: 0.0,
            built: 100.0,
            by: Some(3),
            mult: 1.0,
            newcomer: false,
            kind: SourceKind::Claim,
            supply: 1.0,
            motto: 0,
            sieged: false,
        }
    }

    #[test]
    fn weak_in_follows_the_decay() {
        let r = Rules {
            claim_ttl_s: 540.0,
            claim_strength: 0.8,
            decay: Decay::Linear,
            takeover_stronger: true,
            takeover_below: 0.5,
            ..Default::default()
        };
        // Below 0.4 just after half the lifetime (270 s), exact to the second.
        assert_eq!(weak_in(&claim(0.0), &r, 0.0, 0.0, 0.8), Some(271));
        assert_eq!(weak_in(&claim(100.0), &r, 0.0, 0.0, 0.8), Some(171));
        assert_eq!(weak_in(&claim(400.0), &r, 0.0, 0.0, 0.8), Some(0));
        assert_eq!(source(&claim(0.0), false, &r, 0.0, Some(3)).left, Some(540));
        assert!(source(&claim(0.0), false, &r, 0.0, Some(3)).yours);
    }

    #[test]
    fn terrain_kinds() {
        assert_eq!(terrain(400.0, &[398.0, 398.5]).0, "peak");
        assert_eq!(terrain(396.0, &[398.0, 397.5]).0, "sink");
        assert_eq!(terrain(398.0, &[397.0, 399.0]).0, "slope");
        assert_eq!(terrain(398.0, &[398.2, 397.9]).0, "flat");
        assert_eq!(lean(0.5), "fire");
    }
}

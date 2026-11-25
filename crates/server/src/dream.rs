//! The dreaming city (`[dream]`): at night (local time) every claim also
//! plants a dream on its junction; at dawn each comes true with
//! `come_true`: the junction flares up once more for the side that dreamt
//! it (a plain claim, where no source stands). Dreams live in memory only.

use bevy::prelude::*;
use elements_shared::elements::Faction;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::analytics::now_s;
use crate::sim::{ElementSource, GameRules, NodeById, SourceKind};

const EVERY_S: f64 = 30.0;

/// What the app gets (`ServerMsg::Dream`).
#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, ts_rs::TS)]
#[ts(rename = "DreamShown")]
pub struct Shown {
    /// The city dreams now.
    pub on: bool,
    /// The last dawn: dreams, how many came true, when (unix s; 0: none yet).
    pub dreamt: u32,
    pub came_true: u32,
    #[ts(type = "number")]
    pub dawn: i64,
}

#[derive(Resource)]
pub struct Dreams {
    /// Junction id and side, one per pair.
    seeds: Vec<(i64, Faction)>,
    pub shown: Shown,
    /// Changes whenever `shown` does.
    pub version: u32,
    next: f64,
    rng: StdRng,
}

impl Default for Dreams {
    fn default() -> Self {
        Self {
            seeds: Vec::new(),
            shown: Shown {
                on: false,
                dreamt: 0,
                came_true: 0,
                dawn: 0,
            },
            version: 0,
            next: 0.0,
            rng: StdRng::from_os_rng(),
        }
    }
}

impl Dreams {
    /// A claim at night: a dream on `node` for `f`.
    pub fn plant(&mut self, node: i64, f: Faction) {
        if self.shown.on && !self.seeds.contains(&(node, f)) {
            self.seeds.push((node, f));
        }
    }

    /// Dawn: which dreams come true (and forget the rest).
    fn wake(&mut self, come_true: f32) -> Vec<(i64, Faction)> {
        let seeds = std::mem::take(&mut self.seeds);
        let n = seeds.len() as u32;
        let rng = &mut self.rng;
        let real: Vec<_> = seeds
            .into_iter()
            .filter(|_| rng.random::<f32>() < come_true)
            .collect();
        self.shown.dreamt = n;
        self.shown.came_true = real.len() as u32;
        real
    }
}

/// Every 30 s: night begins or ends (local time); at dawn dreams come true.
pub fn tick(
    mut commands: Commands,
    mut dreams: ResMut<Dreams>,
    rules: Res<GameRules>,
    clock: Option<Res<crate::clock::Clock>>,
    node_by_id: Option<Res<NodeById>>,
    sources: Query<&ElementSource>,
) {
    let now = now_s();
    if now < dreams.next {
        return;
    }
    dreams.next = now + EVERY_S;
    let r = &rules.0.dream;
    let night = r.enabled && clock.is_some_and(|c| r.night(c.hour(now)));
    if night == dreams.shown.on {
        return;
    }
    dreams.shown.on = night;
    dreams.version = dreams.version.wrapping_add(1);
    if night {
        info!("dream: the city dreams");
        return;
    }
    let real = dreams.wake(r.come_true);
    dreams.shown.dawn = now as i64;
    let Some(nodes) = node_by_id else { return };
    let rules = &rules.0;
    for &(id, f) in &real {
        let Some(&e) = nodes.get(&id) else { continue };
        if sources.get(e).is_ok() {
            continue;
        }
        let src = ElementSource::claim(
            rules,
            f,
            SourceKind::Claim,
            rules.claim_ttl_s,
            None,
            1.0,
            false,
            0.0,
        );
        commands.entity(e).insert(src);
    }
    info!(
        dreamt = dreams.shown.dreamt,
        came_true = dreams.shown.came_true,
        "dream: dawn"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_half_come_true() {
        let mut d = Dreams::default();
        d.shown.on = true;
        for id in 0..1000 {
            d.plant(id, Faction::Fire);
        }
        d.plant(0, Faction::Fire);
        let real = d.wake(0.5);
        assert_eq!(d.shown.dreamt, 1000);
        assert!((400..600).contains(&real.len()), "{}", real.len());
        assert!(d.seeds.is_empty());
    }
}

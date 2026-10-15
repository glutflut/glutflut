//! A source pins an element at a node: natural ones from the import, claims
//! by players, anchors on own ground, sparks of the other element, and a
//! player's permanent source. Its life (strength over age, when it falls,
//! who may take it over) is here; the server keeps it as a component.

use bevy::prelude::*;

use crate::elements::{ElementState, Faction};
use crate::hinterland::{Sparks, Supply};
use crate::rules::Rules;

/// How many fire (lowest) and water (highest) natural sources to seed when
/// the import marked none.
pub const NATURAL_COUNT: usize = 10;

/// What a source is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// Natural or claimed at the front.
    #[default]
    Claim,
    /// Claimed on own ground: supplies the front (`Rules::supply`).
    Anchor,
    /// Ownerless, of the other element, grows until put out (`Rules::sparks`).
    Spark,
    /// A player's permanent source (`Rules::keep`): fades to a floor, `built`
    /// counts real seconds since placing or the last top-up.
    Keep,
}

#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub faction: Faction,
    /// Lifetime in effective seconds (see `Rules::source_age`); `None` for
    /// natural sources, which never age.
    pub life: Option<f32>,
    /// Effective age so far.
    pub age: f32,
    /// Pinned intensity as last evaluated (see [`Source::current`]).
    pub strength: f32,
    /// Score slot that placed it.
    pub by: Option<u32>,
    /// Build-up time (`Rules::claim_ramp`) and real seconds since the claim.
    pub ramp: f32,
    pub built: f32,
    /// Level bonus on the strength (1 for natural sources and without levels).
    pub mult: f32,
    /// Claimed on the player's first day (Rückenwind: harder to take over).
    pub newcomer: bool,
    pub kind: Kind,
    /// Ageing factor from supply (`Rules::supply`; 1 without).
    pub supply: f32,
    /// Permanent sources: the tag line (`mottos::MOTTOS`, 1-based; 0 none).
    pub motto: u16,
    /// Inside a ring of the other side's players (`Rules::siege`, server).
    pub sieged: bool,
}

impl Source {
    pub fn natural(faction: Faction, rules: &Rules) -> Self {
        Self {
            faction,
            life: None,
            age: 0.0,
            strength: rules.natural_strength(faction),
            by: None,
            ramp: 0.0,
            built: 0.0,
            mult: 1.0,
            newcomer: false,
            kind: Kind::Claim,
            supply: 1.0,
            motto: 0,
            sieged: false,
        }
    }

    /// A player's claim (or anchor): full strength at once, or building up
    /// over `ramp` seconds.
    pub fn claim(
        rules: &Rules,
        faction: Faction,
        kind: Kind,
        life: f32,
        by: Option<u32>,
        mult: f32,
        newcomer: bool,
        ramp: f32,
    ) -> Self {
        let s = rules.claim_strength * mult;
        Self {
            faction,
            life: Some(life),
            age: 0.0,
            strength: if ramp > 0.0 { 0.0 } else { s },
            by,
            ramp,
            built: 0.0,
            mult,
            newcomer,
            kind,
            supply: 1.0,
            motto: 0,
            sieged: false,
        }
    }

    /// A spark of `faction` (the element the player's side does not own).
    pub fn spark(faction: Faction, sp: &Sparks) -> Self {
        Self {
            faction,
            life: Some(sp.life_s),
            age: 0.0,
            strength: sp.start,
            by: None,
            ramp: 0.0,
            built: 0.0,
            mult: 1.0,
            newcomer: false,
            kind: Kind::Spark,
            supply: 1.0,
            motto: 0,
            sieged: false,
        }
    }

    /// Age by `dt` real seconds at a node with `affinity` (-1 water .. +1
    /// fire): sparks just get older, the rest by element and surroundings,
    /// claims also by supply.
    pub fn age_by(&mut self, rules: &Rules, dt: f32, affinity: f32) {
        self.age += match self.kind {
            Kind::Spark => dt,
            Kind::Keep => rules.source_age(self.faction, dt, affinity),
            Kind::Claim | Kind::Anchor => {
                rules.source_age(self.faction, dt, affinity) * self.supply
            }
        };
    }

    /// `dt` real seconds pass: [`Source::age_by`] and the build-up clock.
    pub fn tick(&mut self, rules: &Rules, dt: f32, affinity: f32) {
        self.tick_slowed(rules, dt, affinity, 1.0);
    }

    /// [`Source::tick`], ageing `slow` times as fast (held blocks, `[area]`);
    /// sparks and the build-up clock keep real time.
    pub fn tick_slowed(&mut self, rules: &Rules, dt: f32, affinity: f32, slow: f32) {
        let aged = if self.kind == Kind::Spark {
            dt
        } else {
            dt * slow
        };
        self.age_by(rules, aged, affinity);
        if self.kind != Kind::Spark {
            self.built += dt;
        }
    }

    /// Strength now; `None` once it is over (a natural source never is).
    /// `wear`: what enemy claims took off a permanent source.
    pub fn current(&self, rules: &Rules, wear: f32) -> Option<f32> {
        let ramp = Rules::ramp_factor(self.ramp, self.built);
        match (self.kind, self.life) {
            (Kind::Spark, _) => rules.sparks.strength(self.age),
            (Kind::Keep, _) => Some(rules.keep.strength(self.age, wear) * self.mult * ramp),
            (_, None) => Some(self.strength),
            (_, Some(life)) => rules
                .source_strength(self.age, life)
                .map(|s| s * self.mult * ramp),
        }
    }

    /// An enemy source falls only once it has weakened enough against a
    /// claim of `attack` strength (`claim_strength` times the level factor);
    /// a newcomer's holds longer. `strength`: its strength now.
    pub fn holds(&self, rules: &Rules, strength: f32, attack: f32) -> bool {
        let below = if self.newcomer {
            rules.progression.newcomer_takeover_below
        } else {
            rules.takeover_below
        };
        self.life.is_some()
            && self.kind != Kind::Spark
            && rules.takeover_stronger
            && strength >= attack * below
    }

    /// Topped up by its owner (permanent sources): fresh, at the owner's
    /// strength now, and full at once.
    pub fn top_up(&mut self, mult: f32) {
        self.age = 0.0;
        self.built = 0.0;
        self.ramp = 0.0;
        self.mult = mult;
    }

    /// Feeds `f`'s supply: an anchor, or a natural source if they count.
    pub fn supplies(&self, f: Faction, sup: &Supply) -> bool {
        self.faction == f
            && match self.kind {
                Kind::Anchor => true,
                Kind::Claim => sup.natural && self.life.is_none(),
                Kind::Spark | Kind::Keep => false,
            }
    }

    /// Ages by supply (claims at the front).
    pub fn supplied(&self) -> bool {
        self.kind == Kind::Claim && self.life.is_some()
    }
}

/// Natural sources: the marked nodes (`fire_source` / `water_source` from the
/// import) if both elements have some, else the `NATURAL_COUNT` lowest nodes
/// for fire and highest for water.
pub fn natural_seeds(marked: Vec<(usize, Faction)>, elev: &[f64]) -> Vec<(usize, Faction)> {
    if marked.iter().any(|s| s.1 == Faction::Fire) && marked.iter().any(|s| s.1 == Faction::Water) {
        return marked;
    }
    let n = elev.len();
    let mut by: Vec<usize> = (0..n).collect();
    by.sort_by(|&a, &b| elev[a].total_cmp(&elev[b]));
    let k = NATURAL_COUNT.min(n / 2);
    let mut seeds: Vec<(usize, Faction)> = by[..k].iter().map(|&i| (i, Faction::Fire)).collect();
    seeds.extend(by[n - k..].iter().map(|&i| (i, Faction::Water)));
    seeds
}

impl Sparks {
    /// One for a player whose side owns `mine` of `total` owned nodes, with
    /// `against` sparks already burning against it: not while they are
    /// losing, not too many, not with something to do nearby (`busy`), and
    /// by chance (`roll` in 0..1, drawn only when it comes to that; checked
    /// every 10 s).
    pub fn due(
        &self,
        mine: f32,
        total: f32,
        against: u32,
        busy: impl FnOnce() -> bool,
        roll: impl FnOnce() -> f32,
    ) -> bool {
        !(mine < self.share * total
            || against >= self.max
            || busy()
            || roll() >= self.per_min / 6.0)
    }
}

/// Nodes owned through a source: `(node, source)` for every owned node whose
/// element traces back (`Field::step_traced`) to a source `at(origin)` knows.
pub fn held<'a, S: 'a>(
    state: &'a [(f32, f32)],
    origin: &'a [(u32, u32)],
    at: impl Fn(u32) -> Option<S> + 'a,
) -> impl Iterator<Item = (usize, S)> + 'a {
    state
        .iter()
        .zip(origin)
        .enumerate()
        .filter_map(move |(i, (&(fire, water), &(of, ow)))| {
            let o = match (ElementState { fire, water }).owner()? {
                Faction::Fire => of,
                Faction::Water => ow,
            };
            Some((i, at(o)?))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Decay;

    fn rules() -> Rules {
        Rules {
            claim_ttl_s: 100.0,
            claim_strength: 0.8,
            decay: Decay::Linear,
            takeover_stronger: true,
            takeover_below: 0.5,
            fire_decay: 1.0,
            water_decay: 2.0,
            decay_affinity: 0.5,
            ..Default::default()
        }
    }

    #[test]
    fn claims_fade_along_the_decay_and_fall_at_the_end() {
        let r = rules();
        let mut s = Source::claim(
            &r,
            Faction::Fire,
            Kind::Claim,
            100.0,
            Some(1),
            1.0,
            false,
            0.0,
        );
        assert_eq!(s.current(&r, 0.0), Some(0.8));
        s.tick(&r, 50.0, 0.0);
        assert!((s.current(&r, 0.0).unwrap() - 0.4).abs() < 1e-6);
        s.tick(&r, 50.0, 0.0);
        assert_eq!(s.current(&r, 0.0), None);
        // level bonus scales it; a build-up starts at 0 and is full after `ramp`
        let mut b = Source::claim(
            &r,
            Faction::Fire,
            Kind::Claim,
            100.0,
            None,
            1.5,
            false,
            20.0,
        );
        assert_eq!(b.strength, 0.0);
        assert_eq!(b.current(&r, 0.0), Some(0.0));
        b.tick(&r, 10.0, 0.0);
        assert!((b.current(&r, 0.0).unwrap() - 0.8 * 1.5 * 0.9 * 0.5).abs() < 1e-6);
    }

    #[test]
    fn ageing_follows_element_surroundings_and_supply() {
        let r = rules();
        let mut fire = Source::claim(&r, Faction::Fire, Kind::Claim, 100.0, None, 1.0, false, 0.0);
        let mut water = Source::claim(
            &r,
            Faction::Water,
            Kind::Claim,
            100.0,
            None,
            1.0,
            false,
            0.0,
        );
        // fire ages at 1x, slower on fiery ground (+1 affinity: 0.5x); water 2x, faster there (1.5x)
        fire.age_by(&r, 10.0, 1.0);
        water.age_by(&r, 10.0, 1.0);
        assert!((fire.age - 5.0).abs() < 1e-6);
        assert!((water.age - 30.0).abs() < 1e-6);
        // cut off from supply: 1.5x
        fire.supply = 1.5;
        fire.age_by(&r, 10.0, 0.0);
        assert!((fire.age - 20.0).abs() < 1e-6);
        // sparks and keeps ignore supply
        let mut sp = Source::spark(Faction::Water, &r.sparks);
        sp.supply = 1.5;
        sp.tick(&r, 10.0, 1.0);
        assert_eq!((sp.age, sp.built), (10.0, 0.0));
        let mut k = fire;
        k.kind = Kind::Keep;
        k.age = 0.0;
        k.age_by(&r, 10.0, 0.0);
        assert!((k.age - 10.0).abs() < 1e-6);
    }

    #[test]
    fn takeover_needs_a_weak_enough_source() {
        let r = rules();
        let s = Source::claim(&r, Faction::Fire, Kind::Claim, 100.0, None, 1.0, false, 0.0);
        // a fresh claim (0.8) holds against 0.8 (needs < 0.4)
        assert!(s.holds(&r, 0.8, 0.8));
        assert!(!s.holds(&r, 0.39, 0.8));
        // a level-3 attacker at 1.2 takes it once below 0.6
        assert!(!s.holds(&r, 0.59, 1.2));
        // newcomers' sources hold longer
        let mut n = s;
        n.newcomer = true;
        assert!(n.holds(&r, 0.39, 0.8));
        // sparks, natural sources and rule sets without the rule never hold
        assert!(!Source::spark(Faction::Water, &r.sparks).holds(&r, 0.9, 0.8));
        assert!(!Source::natural(Faction::Fire, &r).holds(&r, 1.0, 0.8));
        let mut off = r.clone();
        off.takeover_stronger = false;
        assert!(!s.holds(&off, 0.8, 0.8));
    }

    #[test]
    fn keeps_fade_to_the_floor_and_a_top_up_restores_them() {
        let mut r = rules();
        r.keep.strength = 0.8;
        r.keep.floor = 0.2;
        r.keep.fade_s = 100.0;
        let mut k = Source::claim(
            &r,
            Faction::Water,
            Kind::Keep,
            f32::INFINITY,
            None,
            1.0,
            false,
            0.0,
        );
        k.tick(&r, 50.0, 0.0);
        // water ages 2x: 100 s effective = fully faded
        assert!((k.current(&r, 0.0).unwrap() - 0.2).abs() < 1e-6);
        assert!((k.current(&r, 0.1).unwrap() - 0.1).abs() < 1e-6);
        k.top_up(1.5);
        assert!((k.current(&r, 0.0).unwrap() - 1.2).abs() < 1e-6);
    }

    #[test]
    fn natural_seeds_by_mark_or_by_elevation() {
        let elev: Vec<f64> = (0..30).map(|i| i as f64).collect();
        let marked = vec![(3, Faction::Fire), (7, Faction::Water)];
        assert_eq!(natural_seeds(marked.clone(), &elev), marked);
        // only one element marked: by elevation instead
        let seeds = natural_seeds(vec![(3, Faction::Fire)], &elev);
        assert_eq!(seeds.len(), 20);
        assert!(seeds[..10]
            .iter()
            .all(|&(i, f)| i < 10 && f == Faction::Fire));
        assert!(seeds[10..]
            .iter()
            .all(|&(i, f)| i >= 20 && f == Faction::Water));
        assert_eq!(natural_seeds(vec![], &elev[..6]).len(), 6);
    }

    #[test]
    fn sparks_are_due_only_when_idle_and_by_chance() {
        let sp = Sparks {
            share: 0.4,
            max: 2,
            per_min: 3.0,
            ..Default::default()
        };
        let busy = |b: bool| move || b;
        assert!(sp.due(50.0, 100.0, 0, busy(false), || 0.49));
        assert!(!sp.due(50.0, 100.0, 0, busy(false), || 0.5));
        assert!(!sp.due(30.0, 100.0, 0, busy(false), || 0.0));
        assert!(!sp.due(50.0, 100.0, 2, busy(false), || 0.0));
        assert!(!sp.due(50.0, 100.0, 0, busy(true), || 0.0));
        // the roll is not drawn when something earlier says no
        assert!(!sp.due(50.0, 100.0, 0, busy(true), || panic!("rolled")));
    }

    #[test]
    fn held_traces_owned_nodes_to_their_source() {
        let state = [(0.9, 0.0), (0.0, 0.9), (0.1, 0.1), (0.9, 0.0)];
        let origin = [(0, 9), (9, 1), (0, 1), (5, 9)];
        let by = |o: u32| match o {
            0 => Some("fire source"),
            1 => Some("water source"),
            _ => None,
        };
        let got: Vec<_> = held(&state, &origin, by).collect();
        assert_eq!(got, vec![(0, "fire source"), (1, "water source")]);
    }
}

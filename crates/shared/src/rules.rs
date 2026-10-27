//! Game rules as data (TOML).

use serde::{Deserialize, Serialize};

use crate::field::FieldParams;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub name: String,
    pub field: FieldParams,
    pub claim_radius_m: f32,
    pub claim_cooldown_s: f32,
    /// How long a claimed node stays a source.
    pub claim_ttl_s: f32,
    /// Pinned intensity of a claimed source (1.0 = as strong as a natural one).
    pub claim_strength: f32,
    /// A claim builds up instead of taking at once: it reaches full strength
    /// after `claim_ramp_s` times the enemy's strength at the node (neutral or
    /// own ground: at once; firmly held enemy ground: the full time). 0 = off.
    pub claim_ramp_s: f32,
    pub hotspot_ttl_factor: f32,
    /// Claims this close to an active hotspot get `hotspot_ttl_factor` and
    /// `[score] hotspot_factor`.
    pub hotspot_radius_m: f32,
    /// Not read by the server (`frontier`, `capture_*`, `energy_*`; it warns):
    /// only claim nodes in or next to your faction's territory.
    pub frontier: bool,
    /// Stay within `capture_radius_m` this long before a claim takes (0 = instant).
    pub capture_s: f32,
    pub capture_radius_m: f32,
    /// Claim budget; 0 = unlimited. Refills by `energy_per_min`, a claim costs 1.
    pub energy_max: f32,
    pub energy_per_min: f32,
    /// Pinned intensity of the natural sources per element (balance knob).
    pub natural_fire_strength: f32,
    pub natural_water_strength: f32,
    /// Small water sources (fountains, drinking water; `spring_source` from
    /// the import): water, but weaker than the shore.
    pub natural_spring_strength: f32,
    /// How a claimed source weakens over its lifetime.
    pub decay: Decay,
    /// Half-life (s) for `Decay::Halflife`.
    pub decay_halflife_s: f32,
    /// Ageing speed per element (1 = normal, > 1 = burns down faster).
    pub fire_decay: f32,
    pub water_decay: f32,
    /// Surroundings slow ageing: at a node fully leaning to the source's own
    /// element (affinity 1) it ages this fraction slower, at an opposite one faster.
    pub decay_affinity: f32,
    /// A claim on an enemy claimed source needs more strength than it has left.
    pub takeover_stronger: bool,
    /// With `takeover_stronger`: the enemy source must be below this fraction
    /// of a fresh claim (1 = just weaker, 0.5 = at most half left).
    pub takeover_below: f32,
    /// Levels, XP, abilities, challenges (off unless `[progression] enabled`).
    pub progression: crate::progression::Progression,
    /// Rare tunnels to an enemy-held hilltop or sink (server only).
    pub tunnel: Tunnel,
    /// Sparks of the other element where there is nothing to do.
    pub sparks: crate::hinterland::Sparks,
    /// A source rises where a player waits at a fitting place.
    pub rise: crate::hinterland::Rise,
    /// Anchors supply the sources at the front.
    pub supply: crate::hinterland::Supply,
    /// Permanent player sources.
    pub keep: crate::hinterland::Keep,
    /// Weather tilts the elements (the server stores the day's weather).
    pub weather: crate::weather::Weather,
    /// Points for claims, ratings and holding, and their caps.
    pub score: crate::scoring::Score,
    /// Named places to uncover (server only).
    pub finds: Finds,
    /// Blocks enclosed by streets (server `area.rs`).
    pub area: Area,
    /// Encirclement (`[siege]`): a source whose junction is surrounded by the
    /// other side ages faster.
    pub siege: Siege,
    /// Reinforcing own ground at the front (`[reinforce]`).
    pub reinforce: Reinforce,
    /// Tide at the waterfront (`[tide]`).
    pub tide: crate::tide::Tide,
    /// Weekly streaks of play days (`[streak]`, server).
    pub streak: crate::streak::StreakRules,
    /// Chance that a new player joins the weaker side (fewer players today,
    /// else less land); 0.5 = a coin, 1 = always the weaker one.
    pub join_weaker: f32,
}

/// Where nothing is left to take and no anchor fits, a claim may put an
/// ordinary source on an own junction without one; with `front_only` only
/// where a neighbour isn't ours. No points (own ground never brings any).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reinforce {
    pub enabled: bool,
    pub front_only: bool,
}

/// Blocks (`geo.blocks`): a side holds a block when it owns
/// at least `share` of its border junctions and the other side none. Held
/// blocks pay `points_per_ha_min * ha^exponent` per minute to the players whose
/// sources hold the border (by junctions), and their border sources age by
/// `age_factor` (1 = normal, 0 = not at all). Breaking a ring: when a block
/// that was held at least `break_held_s` with a player's source on its border
/// loses its owner, the other side's players on the border share
/// `break_minutes` of its area points, between `break_min` and `break_max`;
/// once per `break_every_s` and block (0 minutes = off).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Area {
    pub enabled: bool,
    pub share: f32,
    pub points_per_ha_min: f32,
    pub exponent: f32,
    pub age_factor: f32,
    pub break_minutes: f32,
    pub break_min: f32,
    pub break_max: f32,
    pub break_held_s: f32,
    pub break_every_s: f32,
}

/// Encirclement: a claimed or permanent
/// source whose junction has at least `min_neighbours` neighbours, all held by
/// the other side, for `delay_s`, ages `factor` times as fast (permanent
/// sources `keep_factor`), until the ring breaks. Natural sources and sparks
/// never; a dead end (one neighbour) can't be surrounded below 2. With
/// `by_players` a neighbour counts only if a player's source of the other
/// side holds it (a ring the players made, not just enemy ground). Closing a
/// ring pays `points` (permanent sources `keep_points`) to the players whose
/// sources hold it, split among them, once per surrounded source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Siege {
    pub enabled: bool,
    pub min_neighbours: u32,
    pub delay_s: f32,
    pub factor: f32,
    pub keep_factor: f32,
    pub by_players: bool,
    pub points: f32,
    pub keep_points: f32,
}

impl Default for Siege {
    fn default() -> Self {
        Self {
            enabled: false,
            min_neighbours: 2,
            delay_s: 120.0,
            factor: 4.0,
            keep_factor: 4.0,
            by_players: true,
            points: 0.0,
            keep_points: 0.0,
        }
    }
}

impl Siege {
    /// Is a source of side `own` (1 fire, 2 water) at a junction whose
    /// neighbours have these owners (0 none) surrounded?
    pub fn surrounded(&self, own: u8, neighbours: impl Iterator<Item = u8>) -> bool {
        let other = 3 - own;
        let mut n = 0;
        for o in neighbours {
            if o != other {
                return false;
            }
            n += 1;
        }
        n >= self.min_neighbours.max(1)
    }
}

impl Default for Area {
    fn default() -> Self {
        Self {
            enabled: false,
            share: 0.9,
            points_per_ha_min: 1.0,
            exponent: 1.0,
            age_factor: 1.0,
            break_minutes: 0.0,
            break_min: 10.0,
            break_max: 50.0,
            break_held_s: 300.0,
            break_every_s: 3600.0,
        }
    }
}

impl Area {
    /// Owner of a block from its border: 1 fire, 2 water, 0 none.
    pub fn owner(&self, fire: usize, water: usize, border: usize) -> u8 {
        let need = (self.share * border as f32).ceil().max(1.0) as usize;
        if water == 0 && fire >= need {
            1
        } else if fire == 0 && water >= need {
            2
        } else {
            0
        }
    }

    /// Points per minute for a held block of `area_m2`.
    pub fn points(&self, area_m2: f32) -> f32 {
        self.points_per_ha_min * (area_m2 / 10_000.0).powf(self.exponent)
    }

    /// Points for breaking the ring of a block of `area_m2` (0: off).
    pub fn break_points(&self, area_m2: f32) -> f32 {
        if self.break_minutes <= 0.0 {
            return 0.0;
        }
        (self.break_minutes * self.points(area_m2)).clamp(self.break_min, self.break_max)
    }

    /// Does losing a block pay a break? Held `held_s` with a player's source
    /// on the border (`player`), last break `since_break_s` ago.
    pub fn breaks(&self, held_s: f32, player: bool, since_break_s: f32) -> bool {
        self.enabled
            && self.break_minutes > 0.0
            && player
            && held_s >= self.break_held_s
            && since_break_s >= self.break_every_s
    }
}

/// A successful claim rarely opens a tunnel: for `window_s` the player may
/// claim one remote junction where the enemy's element gathers (water: a
/// sink, fire: a hilltop) and the enemy holds it. The player stays put.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tunnel {
    /// Chance per successful claim (0 = off).
    pub chance: f32,
    pub window_s: f32,
    /// At most one per player this often.
    pub cooldown_s: f32,
    /// Distance of the far end from the player.
    pub min_m: f32,
    pub max_m: f32,
    /// A hilltop (sink) is higher (lower) than every junction within `hops`
    /// by at least `margin_m`.
    pub hops: u32,
    pub margin_m: f32,
    /// Only where the enemy holds less than this share of the claim's peak
    /// strength (the claim must be able to take the junction).
    pub enemy_below: f32,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self {
            chance: 0.03,
            window_s: 60.0,
            cooldown_s: 900.0,
            min_m: 300.0,
            max_m: 2500.0,
            hops: 3,
            margin_m: 1.0,
            enemy_below: 0.9,
        }
    }
}

/// Finds: hidden places uncovered by a claim within `uncover_m`, or by
/// `dwell_s` within `uncover_m`; a hint from `hint_m` (from `first_hint_m`
/// for a player who has found nothing yet), again for the same find after
/// `hint_every_s`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Finds {
    pub uncover_m: f32,
    pub dwell_s: f32,
    pub hint_m: f32,
    pub first_hint_m: f32,
    pub hint_every_s: f32,
}

impl Default for Finds {
    fn default() -> Self {
        Self {
            uncover_m: 40.0,
            dwell_s: 10.0,
            hint_m: 150.0,
            first_hint_m: 400.0,
            hint_every_s: 300.0,
        }
    }
}

/// How a claimed source weakens: not at all until it expires, linearly to 0
/// at the end of its lifetime, or by half every `decay_halflife_s`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decay {
    #[default]
    None,
    Linear,
    Halflife,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            name: "baseline".into(),
            field: FieldParams::default(),
            claim_radius_m: 80.0,
            claim_cooldown_s: 1.0,
            claim_ttl_s: 180.0,
            claim_strength: 1.0,
            claim_ramp_s: 0.0,
            hotspot_ttl_factor: 3.0,
            hotspot_radius_m: 40.0,
            frontier: false,
            capture_s: 0.0,
            capture_radius_m: 30.0,
            energy_max: 0.0,
            energy_per_min: 1.0,
            natural_fire_strength: 1.0,
            natural_water_strength: 1.0,
            natural_spring_strength: 0.8,
            decay: Decay::None,
            decay_halflife_s: 180.0,
            fire_decay: 1.0,
            water_decay: 1.0,
            decay_affinity: 0.0,
            takeover_stronger: false,
            takeover_below: 1.0,
            progression: Default::default(),
            tunnel: Default::default(),
            sparks: Default::default(),
            rise: Default::default(),
            supply: Default::default(),
            keep: Default::default(),
            weather: Default::default(),
            score: Default::default(),
            finds: Default::default(),
            area: Default::default(),
            siege: Default::default(),
            reinforce: Default::default(),
            tide: Default::default(),
            streak: Default::default(),
            join_weaker: 0.67,
        }
    }
}

impl Rules {
    /// Effective age of a claimed source `age_s` after the claim at a node
    /// with `affinity` (-1 water .. +1 fire): element speed and surroundings.
    pub fn source_age(&self, f: crate::elements::Faction, age_s: f32, affinity: f32) -> f32 {
        use crate::elements::Faction;
        let (speed, lean) = match f {
            Faction::Fire => (self.fire_decay, affinity),
            Faction::Water => (self.water_decay, -affinity),
        };
        age_s * speed * (1.0 - self.decay_affinity * lean).max(0.1)
    }

    /// Strength of a claimed source at effective age `age` (see `source_age`)
    /// with lifetime `life` (normally `claim_ttl_s`, longer at a hotspot);
    /// `None` once it is over.
    pub fn source_strength(&self, age: f32, life: f32) -> Option<f32> {
        if age >= life {
            return None;
        }
        let s = self.claim_strength;
        Some(match self.decay {
            Decay::None => s,
            Decay::Linear => s * (1.0 - age / life),
            Decay::Halflife => s * 0.5f32.powf(age / self.decay_halflife_s.max(1.0)),
        })
    }

    /// Build-up time of a claim at a node where the enemy has `enemy` (0..1).
    pub fn claim_ramp(&self, enemy: f32) -> f32 {
        self.claim_ramp_s * enemy.clamp(0.0, 1.0)
    }

    /// Share of full strength `since` seconds after a claim with build-up `ramp`.
    pub fn ramp_factor(ramp: f32, since: f32) -> f32 {
        if ramp <= 0.0 {
            1.0
        } else {
            (since / ramp).clamp(0.0, 1.0)
        }
    }

    /// Set one value by its TOML key (`claim_ttl_s`, `field.reach_scale`).
    pub fn set(&self, key: &str, value: &str) -> Result<Rules, String> {
        let mut t = toml::Table::try_from(self).map_err(|e| e.to_string())?;
        let mut parts: Vec<&str> = key.trim().split('.').collect();
        let last = parts.pop().ok_or("empty key")?;
        let mut cur = &mut t;
        for p in parts {
            cur = cur
                .entry(p)
                .or_insert_with(|| toml::Value::Table(Default::default()))
                .as_table_mut()
                .ok_or_else(|| format!("{key}: not a table"))?;
        }
        let Some(was) = cur.get(last) else {
            return Err(format!("unknown rule {key}"));
        };
        // Whole numbers stay integers only where the rule is one.
        let v = value.trim();
        let parsed = match v.parse::<i64>() {
            Ok(i) if was.is_integer() => toml::Value::Integer(i),
            Ok(i) => toml::Value::Float(i as f64),
            Err(_) => v
                .parse::<f64>()
                .map(toml::Value::Float)
                .or_else(|_| v.parse::<bool>().map(toml::Value::Boolean))
                .unwrap_or_else(|_| toml::Value::String(v.into())),
        };
        cur.insert(last.into(), parsed);
        t.try_into()
            .map_err(|e: toml::de::Error| format!("{key}={value}: {e}"))
    }

    /// Apply `key=value; other.key=value` (e.g. from `ELEMENTS_RULES_OVERRIDE`).
    pub fn with_overrides(&self, s: &str) -> Result<Rules, String> {
        let mut r = self.clone();
        // Quotes around the whole value come through literally from an env
        // file (`--env-file .env`): drop them.
        let s = s.trim();
        let s = match (s.chars().next(), s.chars().last()) {
            (Some(a @ ('\'' | '"')), Some(b)) if a == b && s.len() >= 2 => &s[1..s.len() - 1],
            _ => s,
        };
        for part in s.split(';').filter(|p| !p.trim().is_empty()) {
            let (k, v) = part
                .split_once('=')
                .ok_or_else(|| format!("{part}: want key=value"))?;
            r = r.set(k, v)?;
        }
        Ok(r)
    }

    /// A natural source's strength: a spring (fountain) or its element's.
    pub fn natural_strength_at(&self, f: crate::elements::Faction, spring: bool) -> f32 {
        if spring {
            self.natural_spring_strength
        } else {
            self.natural_strength(f)
        }
    }

    pub fn natural_strength(&self, f: crate::elements::Faction) -> f32 {
        match f {
            crate::elements::Faction::Fire => self.natural_fire_strength,
            crate::elements::Faction::Water => self.natural_water_strength,
        }
    }

    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Load a rule file. `base = "other.toml"` (relative to the file) is
    /// read first and the file's values go on top, table by table, so a set
    /// lists only what it changes; bases can have bases.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let t = table_from_file(path.as_ref(), 0)?;
        toml::Value::Table(t)
            .try_into()
            .map_err(|e: toml::de::Error| format!("{}: {e}", path.as_ref().display()))
    }
}

fn table_from_file(path: &std::path::Path, depth: u32) -> Result<toml::Table, String> {
    let at = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    if depth > 16 {
        return Err(at(&"base chain too deep (a loop?)"));
    }
    let text = std::fs::read_to_string(path).map_err(|e| at(&e))?;
    let mut t: toml::Table = text.parse().map_err(|e| at(&e))?;
    let Some(base) = t.remove("base") else {
        return Ok(t);
    };
    let base = base
        .as_str()
        .ok_or_else(|| at(&"base must be a file name"))?;
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let mut merged = table_from_file(&dir.join(base), depth + 1)?;
    merge(&mut merged, t);
    Ok(merged)
}

/// `over` on top of `into`; nested tables merge key by key.
fn merge(into: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (into.get_mut(&k), v) {
            (Some(toml::Value::Table(a)), toml::Value::Table(b)) => merge(a, b),
            (_, v) => {
                into.insert(k, v);
            }
        }
    }
}

#[cfg(test)]
mod decay_tests {
    use super::*;
    use crate::elements::Faction;

    #[test]
    fn toml_overrides_defaults() {
        let r = Rules::from_toml("name = \"x\"\nfrontier = true\n[field]\nreach_scale = 80.0\n")
            .unwrap();
        assert!(r.frontier && r.field.reach_scale == 80.0 && r.claim_radius_m == 80.0);
    }

    #[test]
    fn base_files_merge_by_table() {
        let dir = std::env::temp_dir().join(format!("elements-rules-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(
            dir.join("a.toml"),
            "name = \"a\"\nclaim_ttl_s = 540\nclaim_strength = 0.8\n[field]\nreach_scale = 220.0\naffinity_weight = 0.15\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.toml"),
            "base = \"a.toml\"\nname = \"b\"\nclaim_ramp_s = 120\n[field]\nslope_k = 8.0\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("sub/c.toml"),
            "base = \"../b.toml\"\nname = \"c\"\nclaim_ttl_s = 300\n",
        )
        .unwrap();
        let c = Rules::from_file(dir.join("sub/c.toml")).unwrap();
        assert_eq!(c.name, "c");
        assert_eq!(
            (c.claim_ttl_s, c.claim_strength, c.claim_ramp_s),
            (300.0, 0.8, 120.0)
        );
        assert_eq!(
            (
                c.field.reach_scale,
                c.field.affinity_weight,
                c.field.slope_k
            ),
            (220.0, 0.15, 8.0)
        );
        std::fs::write(dir.join("loop.toml"), "base = \"loop.toml\"\n").unwrap();
        assert!(Rules::from_file(dir.join("loop.toml")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn claims_build_up_by_enemy_strength() {
        let r = Rules {
            claim_ramp_s: 120.0,
            ..Default::default()
        };
        // Neutral ground: at once; enemy at 0.5: 60 s, half way after 30 s.
        assert_eq!(r.claim_ramp(0.0), 0.0);
        assert_eq!(Rules::ramp_factor(r.claim_ramp(0.0), 0.0), 1.0);
        assert_eq!(r.claim_ramp(0.5), 60.0);
        assert_eq!(Rules::ramp_factor(60.0, 30.0), 0.5);
        assert_eq!(Rules::ramp_factor(60.0, 90.0), 1.0);
        // Off by default.
        assert_eq!(Rules::default().claim_ramp(1.0), 0.0);
    }

    fn rules(decay: Decay) -> Rules {
        Rules {
            decay,
            claim_ttl_s: 600.0,
            claim_strength: 0.8,
            decay_halflife_s: 200.0,
            ..Default::default()
        }
    }

    #[test]
    fn curves() {
        assert_eq!(rules(Decay::None).source_strength(599.0, 600.0), Some(0.8));
        assert_eq!(rules(Decay::None).source_strength(600.0, 600.0), None);
        assert!((rules(Decay::Linear).source_strength(300.0, 600.0).unwrap() - 0.4).abs() < 1e-6);
        assert!(
            (rules(Decay::Halflife)
                .source_strength(200.0, 600.0)
                .unwrap()
                - 0.4)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn overrides() {
        let r = Rules::default()
            .with_overrides(
                "claim_ttl_s=420; decay=linear; field.reach_scale=250; takeover_stronger=true; tunnel.hops=4; score.claim=12",
            )
            .unwrap();
        assert_eq!((r.tunnel.hops, r.score.claim), (4, 12));
        assert_eq!(r.claim_ttl_s, 420.0);
        assert_eq!(r.decay, Decay::Linear);
        assert_eq!(r.field.reach_scale, 250.0);
        assert!(r.takeover_stronger);
        assert!(Rules::default().with_overrides("claim_tll_s=1").is_err());
        // From an env file the quotes arrive too.
        let r = Rules::default()
            .with_overrides("'rise.enabled=true ; rise.dwell_s=20'")
            .unwrap();
        assert!(r.rise.enabled);
        assert_eq!(r.rise.dwell_s, 20.0);
        assert_eq!(
            Rules::default()
                .with_overrides("\"claim_ttl_s=7\"")
                .unwrap()
                .claim_ttl_s,
            7.0
        );
    }

    #[test]
    fn surroundings_slow_own_element() {
        let r = Rules {
            decay_affinity: 0.5,
            fire_decay: 1.2,
            ..rules(Decay::Linear)
        };
        // fire at a fire place ages slower, at a water place faster
        assert!((r.source_age(Faction::Fire, 100.0, 1.0) - 60.0).abs() < 1e-4);
        assert!((r.source_age(Faction::Fire, 100.0, -1.0) - 180.0).abs() < 1e-4);
        assert!((r.source_age(Faction::Water, 100.0, -1.0) - 50.0).abs() < 1e-4);
    }

    #[test]
    fn surrounded_needs_all_neighbours_and_enough_of_them() {
        let s = Siege::default();
        assert!(s.surrounded(1, [2, 2, 2].into_iter()));
        assert!(!s.surrounded(1, [2, 2, 0].into_iter()), "one free road");
        assert!(!s.surrounded(1, [2, 1].into_iter()), "one own neighbour");
        assert!(!s.surrounded(2, [2].into_iter()));
        assert!(
            !s.surrounded(1, [2].into_iter()),
            "a dead end is not a ring"
        );
        assert!(s.surrounded(2, [1, 1].into_iter()));
    }
}
#[cfg(test)]
mod shipped {
    /// The image's rule file loads and turns on what it says.
    #[test]
    fn default_file_loads() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules");
        let r = super::Rules::from_file(root.join("default.toml")).unwrap();
        assert_eq!(r.name, "default");
        assert!(r.keep.enabled && r.weather.enabled && r.streak.enabled);
    }
}

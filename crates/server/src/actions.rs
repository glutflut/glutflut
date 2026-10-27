//! Player actions: claiming a node.

use bevy::prelude::*;

use elements_shared::prelude::*;
use elements_shared::rules::Rules;

use crate::sim::{ElementSource, NodeById, SourceKind};

/// The file of a named rule set: next to `ELEMENTS_RULES`, else in `rules/`.
pub fn rules_path(name: &str) -> std::path::PathBuf {
    let default = std::env::var("ELEMENTS_RULES").ok();
    let dir = default
        .as_deref()
        .and_then(|p| std::path::Path::new(p).parent())
        .unwrap_or(std::path::Path::new("rules"));
    dir.join(format!("{name}.toml"))
}

/// Rules of a region: the named set (`geo.regions.rules`, a file next to
/// `ELEMENTS_RULES`), else `ELEMENTS_RULES` (TOML file), else defaults. A named
/// set that doesn't load falls back to `ELEMENTS_RULES`, not to the defaults.
pub fn load_rules(set: Option<&str>) -> Rules {
    let default = std::env::var("ELEMENTS_RULES").ok();
    let from_default = |path: &str| Rules::from_file(path).unwrap_or_else(|e| panic!("rules {e}"));
    let named = set.map(rules_path);
    let mut r = match (named, default.as_deref()) {
        (Some(path), d) => Rules::from_file(&path).unwrap_or_else(|e| {
            error!(
                set = set.unwrap_or_default(),
                "rules {e}; using ELEMENTS_RULES"
            );
            d.map_or_else(Rules::default, from_default)
        }),
        (None, Some(path)) => from_default(path),
        (None, None) => Rules::default(),
    };
    // Single values without a new image, e.g. "claim_ttl_s=420; claim_strength=0.8".
    // A mistake there must not keep the region from loading: play on without.
    if let Ok(o) = std::env::var("ELEMENTS_RULES_OVERRIDE") {
        match r.with_overrides(&o) {
            Ok(x) => {
                r = x;
                info!(overrides = %o, "rule overrides");
            }
            Err(e) => error!(overrides = %o, "ELEMENTS_RULES_OVERRIDE ignored, playing without it: {e}"),
        }
    }
    if r.frontier || r.capture_s > 0.0 || r.energy_max > 0.0 {
        warn!("rules: frontier/capture/energy are not read by the server; ignored");
    }
    info!(
        rules = %r.name,
        lifetime_s = r.claim_ttl_s,
        strength = r.claim_strength,
        decay = ?r.decay,
        cooldown_s = r.claim_cooldown_s,
        takeover_below = r.takeover_stronger.then_some(r.takeover_below),
        ramp_s = r.claim_ramp_s,
        slope_k = r.field.slope_k,
        water_climb_m = r.field.water_climb_m,
        fire_drop_m = r.field.fire_drop_m,
        "game rules"
    );
    r
}

/// What the claiming player brings (levels): the
/// strength factor of their level, Rückenwind on their first day, and the reach
/// (longer with Weitblick).
#[derive(Clone, Copy, Debug)]
pub struct Power {
    pub mult: f32,
    pub newcomer: bool,
    pub reach_m: f32,
}

impl Power {
    pub fn plain(rules: &Rules) -> Self {
        Self {
            mult: 1.0,
            newcomer: false,
            reach_m: rules.claim_radius_m,
        }
    }
}

/// Per-player attack cooldown (seconds remaining).
#[derive(Component, Default)]
pub struct AttackCooldown(pub f32);

pub fn tick_cooldowns(time: Res<Time>, mut cooldowns: Query<&mut AttackCooldown>) {
    let dt = time.delta_secs();
    for mut cd in &mut cooldowns {
        cd.0 = (cd.0 - dt).max(0.0);
    }
}

/// Why a claim was refused.
#[derive(Clone, Copy, Debug)]
pub enum Reject {
    Cooldown,
    UnknownNode,
    OutOfRange,
    /// The node already is a source of the claimer's side.
    Own,
    /// An enemy source that is still too strong to take over.
    Strong,
    /// On own ground with an anchor close by (`Rules::supply`).
    Supplied,
    /// A permanent source too strong to take, worn down a bit (`Rules::keep`).
    Worn,
}

impl Reject {
    pub fn as_str(self) -> &'static str {
        match self {
            Reject::Cooldown => "cooldown",
            Reject::UnknownNode => "unknown",
            Reject::OutOfRange => "range",
            Reject::Own => "own",
            Reject::Strong => "strong",
            Reject::Supplied => "supplied",
            Reject::Worn => "worn",
        }
    }
}

/// Does `ex` hold against a claim with `power` (`Source::holds`)?
pub fn holds(ex: &ElementSource, rules: &Rules, power: Power) -> bool {
    ex.holds(rules, ex.strength, rules.claim_strength * power.mult)
}

/// A successful claim: build-up time in s (0 = full strength at once), and
/// whether it put out a spark or placed an anchor.
#[derive(Clone, Copy, Debug, Default)]
pub struct Claimed {
    pub ramp: f32,
    pub doused: bool,
    pub anchor: bool,
}

/// Validate range + cooldown, then make the node a source of `faction`.
/// `supply_m`: street distance from the node to the side's nearest anchor
/// (`INFINITY` if none or unknown); a claim on own ground places an anchor.
/// `front`: a street neighbour isn't the side's (`[reinforce]`: an ordinary
/// source on own ground where no anchor fits).
#[allow(clippy::too_many_arguments)]
pub fn claim(
    commands: &mut Commands,
    node_by_id: &NodeById,
    nodes: &mut Query<(&GeoNode, &mut ElementState), Without<Player>>,
    region: &RegionInfo,
    player_pos: Vec3,
    faction: Faction,
    cooldown: &mut AttackCooldown,
    node_id: i64,
    rules: &Rules,
    life: f32,
    by: Option<u32>,
    existing: Option<ElementSource>,
    power: Power,
    supply_m: f32,
    front: bool,
) -> Result<Claimed, Reject> {
    if cooldown.0 > 0.0 {
        return Err(Reject::Cooldown);
    }
    if let Some(ex) = existing {
        // Re-claiming your side's source would reset its timer and take a
        // teammate's holding points.
        if ex.faction == faction {
            return Err(Reject::Own);
        }
        if holds(&ex, rules, power) {
            return Err(Reject::Strong);
        }
    }
    let target = *node_by_id.get(&node_id).ok_or(Reject::UnknownNode)?;
    let (geo, mut state) = nodes.get_mut(target).map_err(|_| Reject::UnknownNode)?;
    let world = project_enu(geo.lat, geo.lon, region.centroid_lat, region.centroid_lon);
    let d = Vec2::new(player_pos.x, player_pos.z).distance(Vec2::new(world.x, world.z));
    if d > power.reach_m {
        return Err(Reject::OutOfRange);
    }
    let doused = existing.is_some_and(|ex| ex.kind == SourceKind::Spark);
    let mut anchor = rules.supply.enabled && !doused && state.owner() == Some(faction);
    if anchor && supply_m <= rules.supply.anchor_gap_m {
        let r = &rules.reinforce;
        if !(r.enabled && (front || !r.front_only)) {
            return Err(Reject::Supplied);
        }
        anchor = false;
    }
    let life = if anchor {
        rules.supply.anchor_ttl_s
    } else {
        life
    };
    // On enemy ground the claim builds up (see `Rules::claim_ramp`); at once elsewhere.
    let enemy = match faction {
        Faction::Fire => state.water,
        Faction::Water => state.fire,
    };
    let ramp = rules.claim_ramp(enemy);
    let kind = if anchor {
        SourceKind::Anchor
    } else {
        SourceKind::Claim
    };
    let src = ElementSource::claim(
        rules,
        faction,
        kind,
        life,
        by,
        power.mult,
        power.newcomer,
        ramp,
    );
    if ramp <= 0.0 {
        match faction {
            Faction::Fire => state.fire = state.fire.max(src.strength),
            Faction::Water => state.water = state.water.max(src.strength),
        }
    }
    commands.entity(target).insert(src);
    cooldown.0 = rules.claim_cooldown_s;
    // Debug only: a claim is a place and a time (log privacy).
    debug!(node_id, ?faction, ramp, doused, anchor, "node claimed");
    Ok(Claimed {
        ramp,
        doused,
        anchor,
    })
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::testkit::*;

    fn rules() -> Rules {
        Rules {
            claim_cooldown_s: 5.0,
            takeover_stronger: true,
            takeover_below: 0.5,
            ..Default::default()
        }
    }

    /// Nodes 100 m apart, natural fire at 1 and water at 5, no steps yet.
    fn setup(r: &Rules) -> World {
        let (n, e) = line(5, 100.0, &[(1, "fire_source"), (5, "water_source")]);
        world(r.clone(), n, e)
    }

    struct Try {
        x: f32,
        f: Faction,
        id: i64,
        cooldown: f32,
        power: Option<Power>,
        supply_m: f32,
        front: bool,
    }

    fn at_node(id: i64, f: Faction) -> Try {
        Try {
            x: (id - 1) as f32 * 100.0,
            f,
            id,
            cooldown: 0.0,
            power: None,
            supply_m: f32::INFINITY,
            front: false,
        }
    }

    /// Claim as `t` says, with the node's current source; the result and the
    /// cooldown afterwards.
    fn go(w: &mut World, r: &Rules, t: Try) -> (Result<Claimed, Reject>, f32) {
        let existing = w
            .resource::<NodeById>()
            .get(&t.id)
            .and_then(|&e| w.get::<ElementSource>(e).copied());
        let r = r.clone();
        w.run_system_once(
            move |mut c: Commands,
                  by_id: Res<NodeById>,
                  mut nodes: Query<(&GeoNode, &mut ElementState), Without<Player>>,
                  region: Res<RegionInfo>| {
                let mut cd = AttackCooldown(t.cooldown);
                let power = t.power.unwrap_or(Power::plain(&r));
                let res = claim(
                    &mut c,
                    &by_id,
                    &mut nodes,
                    &region,
                    at(t.x),
                    t.f,
                    &mut cd,
                    t.id,
                    &r,
                    r.claim_ttl_s,
                    Some(3),
                    existing,
                    power,
                    t.supply_m,
                    t.front,
                );
                (res, cd.0)
            },
        )
        .unwrap()
    }

    fn src(w: &World, id: i64) -> Option<ElementSource> {
        w.get::<ElementSource>(ent(w, id)).copied()
    }

    #[test]
    fn claim_pins_at_once_and_starts_the_cooldown() {
        let r = rules();
        let mut w = setup(&r);
        let (res, cd) = go(&mut w, &r, at_node(3, Faction::Water));
        let c = res.unwrap();
        assert_eq!((c.ramp, c.doused, c.anchor), (0.0, false, false));
        assert_eq!(cd, 5.0);
        let s = src(&w, 3).unwrap();
        assert_eq!(
            (s.faction, s.kind, s.by),
            (Faction::Water, SourceKind::Claim, Some(3))
        );
        assert_eq!(s.life, Some(r.claim_ttl_s));
        assert_eq!(state(&w, 3).water, r.claim_strength);
    }

    #[test]
    fn refusals_keep_the_cooldown_and_the_node() {
        let r = rules();
        let mut w = setup(&r);
        let mut t = at_node(3, Faction::Fire);
        t.cooldown = 0.5;
        assert!(matches!(go(&mut w, &r, t), (Err(Reject::Cooldown), 0.5)));
        assert!(matches!(
            go(&mut w, &r, at_node(42, Faction::Fire)),
            (Err(Reject::UnknownNode), 0.0)
        ));
        assert!(src(&w, 3).is_none());
    }

    #[test]
    fn range_is_the_claim_radius_or_the_players_reach() {
        let r = rules(); // 80 m
        let mut w = setup(&r);
        let mut t = at_node(3, Faction::Fire);
        t.x += 79.0;
        assert!(go(&mut w, &r, t).0.is_ok());
        let mut t = at_node(4, Faction::Fire);
        t.x -= 81.0;
        assert!(matches!(go(&mut w, &r, t).0, Err(Reject::OutOfRange)));
        let mut t = at_node(4, Faction::Fire);
        t.x -= 81.0;
        t.power = Some(Power {
            reach_m: 120.0,
            ..Power::plain(&r)
        });
        assert!(go(&mut w, &r, t).0.is_ok());
    }

    #[test]
    fn own_sources_are_refused_and_strong_ones_hold() {
        let r = rules();
        let mut w = setup(&r);
        go(&mut w, &r, at_node(3, Faction::Fire)).0.unwrap();
        assert!(matches!(
            go(&mut w, &r, at_node(3, Faction::Fire)).0,
            Err(Reject::Own)
        ));
        // fresh at 1.0: water needs it below 0.5
        assert!(matches!(
            go(&mut w, &r, at_node(3, Faction::Water)).0,
            Err(Reject::Strong)
        ));
        let e = ent(&w, 3);
        w.get_mut::<ElementSource>(e).unwrap().strength = 0.49;
        go(&mut w, &r, at_node(3, Faction::Water)).0.unwrap();
        assert_eq!(src(&w, 3).unwrap().faction, Faction::Water);
        // a stronger player takes it sooner (needs below 0.5 * 1.5)
        w.get_mut::<ElementSource>(e).unwrap().strength = 0.7;
        let mut t = at_node(3, Faction::Fire);
        t.power = Some(Power {
            mult: 1.5,
            ..Power::plain(&r)
        });
        go(&mut w, &r, t).0.unwrap();
        assert_eq!(src(&w, 3).unwrap().strength, 1.5);
        // natural sources never hold
        assert!(go(&mut w, &r, at_node(5, Faction::Fire)).0.is_ok());
    }

    #[test]
    fn claims_on_enemy_ground_build_up() {
        let r = Rules {
            claim_ramp_s: 20.0,
            ..rules()
        };
        let mut w = setup(&r);
        let e = ent(&w, 3);
        *w.get_mut::<ElementState>(e).unwrap() = ElementState {
            fire: 0.5,
            water: 0.0,
        };
        let c = go(&mut w, &r, at_node(3, Faction::Water)).0.unwrap();
        assert_eq!(c.ramp, 10.0);
        let s = src(&w, 3).unwrap();
        assert_eq!((s.strength, s.ramp), (0.0, 10.0));
        // not pinned yet
        assert_eq!(state(&w, 3).water, 0.0);
    }

    #[test]
    fn putting_out_a_spark() {
        let r = rules();
        let mut w = setup(&r);
        let e = ent(&w, 3);
        w.entity_mut(e)
            .insert(ElementSource::spark(Faction::Water, &r.sparks));
        let c = go(&mut w, &r, at_node(3, Faction::Fire)).0.unwrap();
        assert!(c.doused && !c.anchor);
        assert_eq!(src(&w, 3).unwrap().kind, SourceKind::Claim);
    }

    #[test]
    fn reinforce_puts_a_plain_source_at_the_front_where_no_anchor_fits() {
        let mut r = rules();
        r.supply.enabled = true;
        r.reinforce.enabled = true;
        r.reinforce.front_only = true;
        let mut w = setup(&r);
        let e = ent(&w, 3);
        *w.get_mut::<ElementState>(e).unwrap() = ElementState {
            fire: 0.6,
            water: 0.0,
        };
        let mut t = at_node(3, Faction::Fire);
        t.supply_m = r.supply.anchor_gap_m;
        assert!(
            matches!(go(&mut w, &r, t), (Err(Reject::Supplied), 0.0)),
            "not at the front"
        );
        let mut t = at_node(3, Faction::Fire);
        t.supply_m = r.supply.anchor_gap_m;
        t.front = true;
        assert!(!go(&mut w, &r, t).0.unwrap().anchor);
        let s = src(&w, 3).unwrap();
        assert_eq!((s.kind, s.life), (SourceKind::Claim, Some(r.claim_ttl_s)));
    }

    #[test]
    fn claims_on_own_ground_place_anchors_apart() {
        let mut r = rules();
        r.supply.enabled = true;
        let mut w = setup(&r);
        let e = ent(&w, 3);
        *w.get_mut::<ElementState>(e).unwrap() = ElementState {
            fire: 0.6,
            water: 0.0,
        };
        let mut t = at_node(3, Faction::Fire);
        t.supply_m = r.supply.anchor_gap_m;
        assert!(matches!(go(&mut w, &r, t), (Err(Reject::Supplied), 0.0)));
        let mut t = at_node(3, Faction::Fire);
        t.supply_m = r.supply.anchor_gap_m + 1.0;
        assert!(go(&mut w, &r, t).0.unwrap().anchor);
        let s = src(&w, 3).unwrap();
        assert_eq!(
            (s.kind, s.life),
            (SourceKind::Anchor, Some(r.supply.anchor_ttl_s))
        );
        // without supply rules a claim there is a plain one
        let mut w = setup(&rules());
        *w.get_mut::<ElementState>(ent(&w, 3)).unwrap() = ElementState {
            fire: 0.6,
            water: 0.0,
        };
        assert!(
            !go(&mut w, &rules(), at_node(3, Faction::Fire))
                .0
                .unwrap()
                .anchor
        );
    }
}

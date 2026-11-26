//! The shared aether (`[aether]`): every few seconds, each permanent source's
//! share by the lasting sources around it (natural, built, permanent; both
//! sides). Natural sources and buildings draw first and keep theirs.

use bevy::prelude::*;

use elements_shared::geo::GeoNode;

use crate::sim::{ElementSource, NaturalSource, SourceKind};

/// How often the shares are worked out again (s).
const EVERY_S: f64 = 5.0;

/// Metres east and north of `o` (small regions: flat is fine).
fn flat(o: (f64, f64), k: f64, lat: f64, lon: f64) -> Vec2 {
    Vec2::new(
        ((lon - o.1) * 111_320.0 * k) as f32,
        ((lat - o.0) * 110_540.0) as f32,
    )
}

pub fn tick(
    rules: Res<crate::sim::GameRules>,
    naturals: Query<&GeoNode, With<NaturalSource>>,
    mut keeps: Query<(Entity, &GeoNode, &mut ElementSource), Without<NaturalSource>>,
    mut next: Local<f64>,
) {
    let a = &rules.0.aether;
    let now = crate::analytics::now_s();
    if now < *next {
        return;
    }
    *next = now + EVERY_S;
    let Some(o) = keeps.iter().next().map(|(_, g, _)| (g.lat, g.lon)) else {
        return;
    };
    let k = o.0.to_radians().cos();
    let at = |g: &GeoNode| flat(o, k, g.lat, g.lon);
    let others: Vec<Vec2> = naturals.iter().map(at).collect();
    let mine: Vec<(Entity, Vec2)> = keeps
        .iter()
        .filter(|(_, _, s)| s.kind == SourceKind::Keep)
        .map(|(e, g, _)| (e, at(g)))
        .collect();
    let reach = a.radius_m;
    for &(e, p) in &mine {
        let near = others
            .iter()
            .copied()
            .chain(mine.iter().filter(|(o, _)| *o != e).map(|(_, q)| *q))
            .map(|q| q.distance(p))
            .filter(|&d| d < reach);
        let share = a.share(near);
        if let Ok((_, _, mut s)) = keeps.get_mut(e) {
            if (s.aether - share).abs() > 1e-3 {
                s.aether = share;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ent, line, world};
    use bevy::ecs::system::RunSystemOnce;
    use elements_shared::prelude::Faction;

    fn keep(r: &elements_shared::rules::Rules, f: Faction) -> ElementSource {
        let mut s = ElementSource::claim(r, f, SourceKind::Claim, 10.0, Some(1), 1.0, false, 0.0);
        s.kind = SourceKind::Keep;
        s.life = Some(f32::INFINITY);
        s
    }

    #[test]
    fn crowded_permanent_sources_share_the_aether() {
        let mut r = elements_shared::rules::Rules::default();
        r.aether.enabled = true;
        // Junctions 1..6, 50 m apart.
        let (n, e) = line(6, 50.0, &[]);
        let mut w = world(r.clone(), n, e);
        // A natural fire source at 1, a water keep at 2 (50 m), a fire keep
        // at 3 (100 m), one alone at 6 (150 m from 3).
        // The test world seeds natural sources itself: none but ours.
        for id in 1..=6 {
            let en = ent(&w, id);
            w.entity_mut(en).remove::<(NaturalSource, ElementSource)>();
        }
        let nat = ElementSource::natural(Faction::Fire, &r);
        let e0 = ent(&w, 1);
        w.entity_mut(e0).insert((nat, NaturalSource(nat)));
        for (id, f) in [(2, Faction::Water), (3, Faction::Fire), (6, Faction::Fire)] {
            let en = ent(&w, id);
            w.entity_mut(en).insert(keep(&r, f));
        }
        w.run_system_once(tick).unwrap();
        let share = |w: &World, id| w.get::<ElementSource>(ent(w, id)).unwrap().aether;
        let near = |d: f32| 1.0 - d / 150.0;
        // 2: the natural source at 50 m and the keep at 50 m, either side.
        let s2 = 1.0 / (1.0 + near(50.0) + near(50.0));
        assert!((share(&w, 2) - s2).abs() < 0.01, "{}", share(&w, 2));
        // 3: the natural one at 100 m, the keep at 50 m.
        let s3 = 1.0 / (1.0 + near(100.0) + near(50.0));
        assert!((share(&w, 3) - s3).abs() < 0.01, "{}", share(&w, 3));
        assert!(share(&w, 6) > 0.99, "alone (150 m: nothing)");
        assert_eq!(share(&w, 1), 1.0, "natural sources keep theirs");
        // Its strength is the share of what it would have.
        let s1 = s2;
        let s = *w.get::<ElementSource>(ent(&w, 2)).unwrap();
        let full = ElementSource { aether: 1.0, ..s }.current(&r, 0.0).unwrap();
        assert!((s.current(&r, 0.0).unwrap() - full * s1).abs() < 0.01);
    }

    #[test]
    fn floor_and_off() {
        let mut a = elements_shared::rules::Aether::default();
        assert_eq!(a.share([0.0, 0.0, 0.0]), 1.0, "off");
        a.enabled = true;
        assert_eq!(a.share([0.0; 10]), a.floor);
        assert_eq!(a.share([150.0, 400.0]), 1.0);
    }
}

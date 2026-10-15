//! Abilities by level.

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::web::{send, send_challenge, ServerMsg};

use super::{Ctx, Game};

/// Wire name and kind.
pub(super) const ABILITIES: [(&str, elements_shared::progression::Ability); 4] = [
    ("reach", elements_shared::progression::Ability::Reach),
    ("nourish", elements_shared::progression::Ability::Nourish),
    ("boost", elements_shared::progression::Ability::Boost),
    ("double", elements_shared::progression::Ability::Double),
];
pub(super) const REACH: usize = 0;
const NOURISH: usize = 1;
pub(super) const DOUBLE: usize = 3;

/// Per player: when each ability is ready again, and which are armed for the
/// next claim (Weitblick, Funkenflug/Welle). Memory only.
#[derive(Component, Default)]
pub struct Abilities {
    pub(super) ready: [f64; 4],
    pub(super) armed: [bool; 4],
}

impl Abilities {
    pub(super) fn use_up(
        &mut self,
        i: usize,
        now: f64,
        p: &elements_shared::progression::Progression,
    ) {
        self.armed[i] = false;
        self.ready[i] = now + p.cooldown_s(ABILITIES[i].1) as f64;
    }
}

pub(super) fn ability(game: &mut Game, c: &Ctx, conn: u64, name: &str) {
    let (now, region, rules) = (c.now, c.region, c.rules);
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let Some(i) = ABILITIES.iter().position(|a| a.0 == name) else {
        return;
    };
    let Ok((&faction, pos, _, _, slot, _)) = game.players.get(*e) else {
        return;
    };
    let Some(slot) = slot.map(|s| s.0) else {
        return;
    };
    let Ok(mut ab) = game.abilities.get_mut(*e) else {
        return;
    };
    let p = &rules.progression;
    let level = game.scores.xp(slot).1;
    let reply = |ab: &Abilities, ok: bool, reason: Option<&'static str>| ServerMsg::Ability {
        name: ABILITIES[i].0,
        ok,
        reason,
        armed: ab.armed[i],
        ready_in: (ab.ready[i] - now).max(0.0) as u32,
    };
    if !p.has(level, ABILITIES[i].1) {
        send(out, &reply(&ab, false, Some("level")));
        return;
    }
    if now < ab.ready[i] {
        send(out, &reply(&ab, false, Some("cooldown")));
        return;
    }
    match i {
        // Armed: acts with the next successful claim.
        REACH | DOUBLE => {
            ab.armed[i] = !ab.armed[i];
            send(out, &reply(&ab, true, None));
        }
        NOURISH => {
            // Your weakest own source back to full strength.
            let weakest = game
                .sources
                .iter_mut()
                .filter(|(_, s)| s.by == Some(slot) && s.life.is_some())
                .min_by(|a, b| a.1.strength.total_cmp(&b.1.strength));
            match weakest {
                Some((_, mut s)) => {
                    s.age = 0.0;
                    ab.use_up(i, now, p);
                    send(out, &reply(&ab, true, None));
                }
                None => send(out, &reply(&ab, false, Some("nothing"))),
            }
        }
        _ => {
            // Verstärken: the nearest teammate source within reach.
            let here = Vec2::new(pos.0.x, pos.0.z);
            let best = game
                .sources
                .iter_mut()
                .filter(|(_, s)| {
                    s.faction == faction && s.life.is_some() && s.by.is_some_and(|b| b != slot)
                })
                .map(|(g, s)| {
                    let w = project_enu(g.lat, g.lon, region.centroid_lat, region.centroid_lon);
                    (Vec2::new(w.x, w.z).distance(here), s)
                })
                .filter(|(d, _)| *d <= rules.claim_radius_m)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match best {
                Some((_, mut s)) => {
                    s.age = 0.0;
                    let owner = s.by;
                    ab.use_up(i, now, p);
                    game.scores
                        .add_xp(slot, elements_shared::progression::Goal::Boost);
                    if let Some(o) = owner {
                        game.scores
                            .add_xp(o, elements_shared::progression::Goal::Boost);
                    }
                    send(out, &reply(&ab, true, None));
                    if let Some(ch) = game.challenges.as_deref_mut() {
                        for ev in ch.on_boost(conn, now, p, &mut game.scores) {
                            send_challenge(out, ev);
                        }
                    }
                }
                None => send(out, &reply(&ab, false, Some("nothing"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ABILITIES;
    use ts_rs::TS;

    #[test]
    fn ts_names_match() {
        let names: Vec<_> = ABILITIES.iter().map(|(n, _)| format!("\"{n}\"")).collect();
        let ts = crate::protocol::AbilityName::inline(&ts_rs::Config::new());
        assert_eq!(ts, names.join(" | "));
    }
}

//! Personal challenges: the game proposes one per
//! round, the player doesn't choose. The first starts with the first claim;
//! the next when the round ends (done or not). A round without any claim ends
//! the series until the next claim. With `challenge_gap_s` a pause follows
//! each round (the card counts down), and `challenges_per_day` caps the day. XP goes to the score row; nothing about
//! places or times is stored, the state here lives in memory only.

use std::collections::HashMap;

use bevy::prelude::*;
use rand::seq::IndexedRandom;
use serde::Serialize;

use elements_shared::progression::{Challenge, Goal, Progression};

use crate::score::Scores;

pub struct Run {
    slot: u32,
    kind: Challenge,
    until: f64,
    progress: u32,
    done: bool,
    /// Claims during this round (none: the series pauses).
    claims: u32,
    zones_at_start: u64,
    hint: Option<String>,
    /// Round over, waiting for the next one (`challenge_gap_s`).
    wait: bool,
}

/// What the player sees.
#[derive(Serialize, ts_rs::TS)]
#[ts(rename = "ChallengeView")]
pub struct View {
    pub kind: Challenge,
    pub goal: u32,
    pub progress: u32,
    pub ends_in: u32,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hint: Option<String>,
    /// Pause before the next challenge; `ends_in` counts it down.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wait: Option<bool>,
}

/// What a claim meant for the challenge.
pub struct ClaimInfo {
    pub pos: Vec3,
    /// The junction was contested (front) before the claim.
    pub front: bool,
    /// It held an enemy source.
    pub retake: bool,
}

pub enum Event {
    /// New or changed challenge.
    Update(View),
    /// Just finished: XP for it.
    Done { kind: Challenge, xp: u32 },
    /// The series paused (a round without claims).
    Paused,
}

#[derive(Resource, Default)]
pub struct Challenges {
    runs: HashMap<u64, Run>,
    /// Zones (name, ENU position, own radius) for Entdecken.
    zones: Vec<(String, Vec2, Option<f32>)>,
    /// Per score slot: day and the zones claimed in today (bits).
    visited: HashMap<u32, (i64, u64)>,
    held_seen: f64,
    /// Per score slot: day and challenges given; no new one before this time.
    given: HashMap<u32, (i64, u32)>,
    next_at: HashMap<u32, f64>,
    /// Local days of the region (the daily limit).
    clock: crate::clock::Clock,
}

impl Challenges {
    pub fn new(zones: Vec<(String, Vec2, Option<f32>)>, clock: crate::clock::Clock) -> Self {
        Self {
            zones: zones.into_iter().take(64).collect(),
            clock,
            ..Default::default()
        }
    }

    /// New zones (editor): today's visits start again (bits were by index).
    pub fn set_zones(&mut self, zones: Vec<(String, Vec2, Option<f32>)>) {
        self.zones = zones.into_iter().take(64).collect();
        self.visited.clear();
    }

    fn zone_of(&self, p: Vec3, r: &Progression) -> Option<usize> {
        let p = Vec2::new(p.x, p.z);
        self.zones
            .iter()
            .enumerate()
            .map(|(i, (_, c, zr))| (i, c.distance(p), zr.unwrap_or(r.zone_r_m)))
            .filter(|&(_, d, zr)| d <= zr)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _, _)| i)
    }

    fn visited(&mut self, slot: u32, now: f64) -> &mut u64 {
        let day = self.clock.day(now);
        let v = self.visited.entry(slot).or_insert((day, 0));
        if v.0 != day {
            *v = (day, 0);
        }
        &mut v.1
    }

    fn view(run: &Run, now: f64) -> View {
        View {
            kind: run.kind,
            goal: run.kind.goal(),
            progress: run.progress.min(run.kind.goal()),
            ends_in: (run.until - now).max(0.0) as u32,
            done: run.done,
            hint: run.hint.clone(),
            wait: run.wait.then_some(true),
        }
    }

    /// May `slot` get a new challenge now (pause over, day not used up)?
    fn may_start(&self, slot: u32, now: f64, p: &Progression) -> bool {
        let day = self.clock.day(now);
        let given = self
            .given
            .get(&slot)
            .filter(|g| g.0 == day)
            .map_or(0, |g| g.1);
        now >= self.next_at.get(&slot).copied().unwrap_or(0.0)
            && (p.challenges_per_day == 0 || given < p.challenges_per_day)
    }

    /// Propose the next challenge for `slot` at `level`.
    fn start(
        &mut self,
        conn: u64,
        slot: u32,
        level: u8,
        pos: Vec3,
        now: f64,
        p: &Progression,
    ) -> View {
        let visited = *self.visited(slot, now);
        let open: Vec<Challenge> = Challenge::ALL
            .into_iter()
            .filter(|c| c.level() <= level)
            // Not on the server yet: quiet cells. Team needs Verstärken.
            .filter(|c| *c != Challenge::Quiet)
            .filter(|c| {
                *c != Challenge::Team || p.has(level, elements_shared::progression::Ability::Boost)
            })
            .filter(|c| *c != Challenge::Discover || self.zones.len() as u32 > visited.count_ones())
            .collect();
        let kind = *open.choose(&mut rand::rng()).unwrap_or(&Challenge::Claim3);
        // Entdecken: name the nearest zone not claimed in today.
        let hint = (kind == Challenge::Discover)
            .then(|| {
                let here = Vec2::new(pos.x, pos.z);
                self.zones
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| visited & (1 << i) == 0)
                    .min_by(|a, b| a.1 .1.distance(here).total_cmp(&b.1 .1.distance(here)))
                    .map(|(_, (name, _, _))| name.clone())
            })
            .flatten();
        let day = self.clock.day(now);
        let g = self.given.entry(slot).or_insert((day, 0));
        if g.0 != day {
            *g = (day, 0);
        }
        g.1 += 1;
        let run = Run {
            slot,
            kind,
            until: now + kind.round_s(p) as f64,
            progress: 0,
            done: false,
            claims: 0,
            zones_at_start: visited,
            hint,
            wait: false,
        };
        let v = Self::view(&run, now);
        self.runs.insert(conn, run);
        v
    }

    fn finish(run: &mut Run, scores: &mut Scores, p: &Progression) -> Event {
        run.done = true;
        scores.add_xp(run.slot, Goal::Challenge);
        scores.streak_day(run.slot);
        Event::Done {
            kind: run.kind,
            xp: p.xp_challenge as u32,
        }
    }

    /// A successful claim: XP for it (and a new zone), progress, and the
    /// first challenge if none runs yet.
    pub fn on_claim(
        &mut self,
        conn: u64,
        slot: u32,
        info: ClaimInfo,
        now: f64,
        p: &Progression,
        scores: &mut Scores,
    ) -> Vec<Event> {
        let mut ev = Vec::new();
        scores.add_xp(slot, if info.front { Goal::Front } else { Goal::Claim });
        let zone = self.zone_of(info.pos, p);
        if let Some(z) = zone {
            let v = self.visited(slot, now);
            if *v & (1 << z) == 0 {
                *v |= 1 << z;
                scores.add_xp(slot, Goal::Zone);
            }
        }
        if !p.challenges {
            return ev;
        }
        if !self.runs.contains_key(&conn) {
            if !self.may_start(slot, now, p) {
                return ev;
            }
            let level = scores.xp(slot).1;
            ev.push(Event::Update(
                self.start(conn, slot, level, info.pos, now, p),
            ));
        }
        let run = self.runs.get_mut(&conn).expect("run");
        run.claims += 1;
        if run.done || run.wait {
            return ev;
        }
        let hit = match run.kind {
            Challenge::Claim3 => {
                run.progress += 1;
                run.progress >= run.kind.goal()
            }
            Challenge::Discover => zone.is_some_and(|z| run.zones_at_start & (1 << z) == 0),
            Challenge::Front => info.front,
            Challenge::Retake => info.retake,
            Challenge::Hold | Challenge::Quiet | Challenge::Team => false,
        };
        if hit {
            run.progress = run.kind.goal();
            ev.push(Self::finish(run, scores, p));
        }
        ev.push(Event::Update(Self::view(run, now)));
        ev
    }

    /// Round ends and "Halten" (junctions held per minute from the score
    /// tick). Returns events per connection.
    pub fn tick(
        &mut self,
        now: f64,
        p: &Progression,
        scores: &mut Scores,
        pos: impl Fn(u64) -> Option<Vec3>,
    ) -> Vec<(u64, Event)> {
        let mut out = Vec::new();
        let held_new = scores.held_at > self.held_seen;
        if held_new {
            self.held_seen = scores.held_at;
        }
        let conns: Vec<u64> = self.runs.keys().copied().collect();
        for conn in conns {
            let run = self.runs.get_mut(&conn).expect("run");
            if held_new && run.kind == Challenge::Hold && !run.done && !run.wait {
                let n = scores.held.get(&run.slot).copied().unwrap_or(0);
                // Progress = minutes in a row with enough junctions.
                run.progress = if n >= p.hold_junctions {
                    run.progress + 1
                } else {
                    0
                };
                if run.progress >= run.kind.goal() {
                    out.push((conn, Self::finish(run, scores, p)));
                }
                out.push((conn, Event::Update(Self::view(run, now))));
            }
            if now < run.until {
                continue;
            }
            let (slot, claims, waited) = (run.slot, run.claims, run.wait);
            let gap = f64::from(p.challenge_gap_s);
            if claims == 0 && !waited {
                self.runs.remove(&conn);
                if gap > 0.0 {
                    self.next_at.insert(slot, now + gap);
                }
                out.push((conn, Event::Paused));
                continue;
            }
            // Round over: a pause first (the card counts it down).
            if gap > 0.0 && !waited {
                self.next_at.insert(slot, now + gap);
                let day_left = p.challenges_per_day == 0
                    || self
                        .given
                        .get(&slot)
                        .filter(|g| g.0 == self.clock.day(now))
                        .map_or(0, |g| g.1)
                        < p.challenges_per_day;
                if !day_left {
                    self.runs.remove(&conn);
                    out.push((conn, Event::Paused));
                    continue;
                }
                run.wait = true;
                run.until = now + gap;
                out.push((conn, Event::Update(Self::view(run, now))));
                continue;
            }
            if !self.may_start(slot, now, p) {
                self.runs.remove(&conn);
                out.push((conn, Event::Paused));
                continue;
            }
            let level = scores.xp(slot).1;
            let here = pos(conn).unwrap_or(Vec3::ZERO);
            let v = self.start(conn, slot, level, here, now, p);
            out.push((conn, Event::Update(v)));
        }
        out
    }

    /// Verstärken used: completes the Team challenge.
    pub fn on_boost(
        &mut self,
        conn: u64,
        now: f64,
        p: &Progression,
        scores: &mut Scores,
    ) -> Vec<Event> {
        let Some(run) = self.runs.get_mut(&conn) else {
            return Vec::new();
        };
        run.claims += 1;
        if run.kind != Challenge::Team || run.done {
            return Vec::new();
        }
        run.progress = 1;
        let done = Self::finish(run, scores, p);
        vec![done, Event::Update(Self::view(run, now))]
    }

    pub fn forget(&mut self, conn: u64) {
        self.runs.remove(&conn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elements_shared::prelude::Faction;

    fn setup() -> (Challenges, Scores, u32, Progression) {
        let mut scores = Scores::new(None, "test".into(), Default::default(), Default::default());
        let p = Progression {
            enabled: true,
            ..Default::default()
        };
        scores.prog = p.clone();
        let slot = scores.create("tok", Faction::Fire);
        let ch = Challenges::new(
            vec![
                ("Markt".into(), Vec2::ZERO, None),
                ("Hafen".into(), Vec2::new(1000.0, 0.0), None),
            ],
            Default::default(),
        );
        (ch, scores, slot, p)
    }

    fn claim(front: bool) -> ClaimInfo {
        ClaimInfo {
            pos: Vec3::ZERO,
            front,
            retake: false,
        }
    }

    #[test]
    fn first_claim_starts_a_round_and_xp_counts() {
        let (mut ch, mut scores, slot, p) = setup();
        let t0 = 1_000_000.0;
        let ev = ch.on_claim(1, slot, claim(false), t0, &p, &mut scores);
        assert!(
            matches!(ev.first(), Some(Event::Update(_))),
            "a challenge starts"
        );
        // Claim (2) + new zone (15), two kinds of goal? Zone only: no bonus.
        assert_eq!(scores.xp(slot).0, 17.0);
    }

    #[test]
    fn round_ends_with_a_new_challenge_or_a_pause() {
        let (mut ch, mut scores, slot, p) = setup();
        let t0 = 1_000_000.0;
        ch.on_claim(1, slot, claim(false), t0, &p, &mut scores);
        // Round over after claims: the next challenge comes.
        let ev = ch.tick(t0 + 600.0, &p, &mut scores, |_| Some(Vec3::ZERO));
        assert!(matches!(ev.as_slice(), [(1, Event::Update(_))]));
        // A whole round without claims: the series pauses.
        let ev = ch.tick(t0 + 1200.0, &p, &mut scores, |_| Some(Vec3::ZERO));
        assert!(matches!(ev.as_slice(), [(1, Event::Paused)]));
        assert!(ch.tick(t0 + 1800.0, &p, &mut scores, |_| None).is_empty());
    }

    #[test]
    fn a_pause_follows_each_round_and_the_day_has_a_limit() {
        let (mut ch, mut scores, slot, mut p) = setup();
        p.challenge_gap_s = 1800.0;
        p.challenges_per_day = 2;
        // Midday, so the test stays within one day.
        let t0 = (crate::score::day_of(1_000_000.0) * 86_400 + 43_200) as f64;
        ch.on_claim(1, slot, claim(false), t0, &p, &mut scores);
        let pos = |_| Some(Vec3::ZERO);
        // Round over: the card waits 30 minutes.
        let ev = ch.tick(t0 + 600.0, &p, &mut scores, pos);
        assert!(
            matches!(ev.as_slice(), [(1, Event::Update(v))] if v.wait == Some(true) && v.ends_in == 1800)
        );
        // A claim during the pause starts nothing.
        ch.on_claim(1, slot, claim(false), t0 + 700.0, &p, &mut scores);
        assert!(ch.runs[&1].wait);
        // Pause over: the second (and last) challenge of the day.
        let ev = ch.tick(t0 + 2400.0, &p, &mut scores, pos);
        assert!(matches!(ev.as_slice(), [(1, Event::Update(v))] if v.wait.is_none()));
        ch.on_claim(1, slot, claim(false), t0 + 2500.0, &p, &mut scores);
        // Its round ends: the day is used up, the card goes.
        let ev = ch.tick(t0 + 3100.0, &p, &mut scores, pos);
        assert!(matches!(ev.as_slice(), [(1, Event::Paused)]));
        assert!(ch
            .on_claim(1, slot, claim(false), t0 + 9000.0, &p, &mut scores)
            .iter()
            .all(|e| !matches!(e, Event::Update(_))));
    }

    #[test]
    fn hold_needs_minutes_in_a_row() {
        let (mut ch, mut scores, slot, p) = setup();
        let t0 = 1_000_000.0;
        ch.runs.insert(
            1,
            Run {
                slot,
                kind: Challenge::Hold,
                until: t0 + 300.0,
                progress: 0,
                done: false,
                claims: 1,
                zones_at_start: 0,
                hint: None,
                wait: false,
            },
        );
        for (i, n) in [25, 25, 5, 25, 25, 25].into_iter().enumerate() {
            scores.held = [(slot, n)].into();
            scores.held_at = t0 + i as f64;
            ch.tick(t0 + i as f64, &p, &mut scores, |_| None);
        }
        assert!(ch.runs[&1].done, "three minutes in a row after the dip");
    }
}

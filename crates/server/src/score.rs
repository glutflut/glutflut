//! Accountless scores. A browser's random token
//! is the only identity; the row is keyed by its SHA-256. Stored: faction,
//! a generated name and point totals. Positions only decide points in memory;
//! places, routes and times of single actions are never stored.
//!
//! A player is the same in every region (`game.players`: side, name, XP,
//! recovery code, hidden); points are per region (`game.player_region`). The
//! gateway reads the player row on connect ([`lookup`]), so a region that has
//! been loaded for days doesn't write back an old XP.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use serde::Serialize;
use sha2::{Digest, Sha256};

use elements_shared::prelude::*;
use elements_shared::progression::{DayXp, Goal, Progression};
use elements_shared::streak::{Step, Streak, StreakRules};

use crate::analytics::now_s;
use crate::sim::{ElementSim, ElementSource};

use elements_shared::scoring::Score;

/// Holding is counted per minute (`[score] hold_m_per_point`, "Halten" counts
/// minutes in a row).
const HOLD_EVERY_S: f64 = 60.0;
const FLUSH_EVERY_S: f64 = 30.0;
const BOARD_EVERY_S: f64 = 10.0;
/// Rows unseen this long are deleted.
const RETAIN_DAYS: i64 = 30;
/// Leaderboard length (presentation, not balance).
const TOP_N: usize = 10;

/// Score slot of a (non-bot) web player.
#[derive(Component, Clone, Copy)]
pub struct ScoreSlot(pub u32);

/// Leaderboard JSON for `GET /api/leaderboard`.
pub type SharedBoard = Arc<RwLock<Option<Arc<str>>>>;

/// The UTC day (tests; the game counts the region's local days, `clock.rs`).
#[cfg(test)]
pub fn day_of(now: f64) -> i64 {
    (now as i64).div_euclid(86_400)
}

pub use elements_shared::streak::week_of;

/// Claims that earn a badge (`claims-N`).
const CLAIM_BADGES: [i32; 5] = [1, 10, 100, 500, 1000];
/// Cities played in that earn a badge (`cities-N`).
const CITY_BADGES: [usize; 2] = [3, 5];

pub fn token_id(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Crockford base32 without I, L, O, U: easy to read and type.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A new recovery code: 12 characters (60 random bits) as XXXX-XXXX-XXXX.
pub fn recovery_code() -> String {
    let bits: u64 = rand::random();
    let chars: Vec<char> = (0..12)
        .map(|i| CODE_ALPHABET[((bits >> (i * 5)) & 31) as usize] as char)
        .collect();
    format!(
        "{}-{}-{}",
        chars[..4].iter().collect::<String>(),
        chars[4..8].iter().collect::<String>(),
        chars[8..].iter().collect::<String>()
    )
}

/// Hash of a code as typed: case, dashes and spaces don't matter, O/I/L are
/// read as 0/1/1. `None` if it can't be a code.
pub fn code_hash(input: &str) -> Option<[u8; 32]> {
    let norm: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    (norm.len() == 12 && norm.bytes().all(|b| CODE_ALPHABET.contains(&b)))
        .then(|| Sha256::digest(format!("elements-recovery:{norm}").as_bytes()).into())
}

// Names: adjective + animal of the lake region + number. German strong
// adjective endings by gender (m -er, f -e, n -es).
pub struct Rec {
    pub id: [u8; 32],
    /// XP of the campaign before `xp_day`, and that day's (variety bonus per day).
    xp_prev: f32,
    day_xp: DayXp,
    xp_day: i64,
    /// XP counts from this time (unix s): a campaign start or end after it
    /// starts the levels again (`Scores::campaign_reset`).
    xp_since: f64,
    /// SHA-256 of the current recovery code, if the player made one.
    recovery: Option<[u8; 32]>,
    pub faction: Faction,
    pub name: String,
    /// Points in this region, and the day of `today`.
    pub total: i64,
    pub today: i64,
    /// Points since the campaign start (`campaign_reset`).
    pub campaign: i64,
    pub day: i64,
    pub hidden: bool,
    rated: u8,
    carry: f32,
    /// Last day seen in this region.
    last_seen: i64,
    dirty: bool,
    /// Opted in to collecting finds; `found` is stored, `seen` only uncovered
    /// this session (not collecting).
    pub collect: bool,
    pub found: HashSet<i64>,
    /// Presence codes confirmed on site (stored, no times).
    presence: HashSet<String>,
    pub seen: HashSet<i64>,
    /// Claims in all regions, badges earned (no times).
    claims: i32,
    pub badges: HashSet<String>,
    /// Weekly streak (counters only, the same in every region).
    streak: Streak,
    /// Monday of the week of `points_week` (this region).
    week: i64,
    points_week: i64,
    /// Points in the current hour (cap), memory only.
    hour: i64,
    points_hour: i64,
    /// Flagged by the plausibility check until then (unix s): plays on and
    /// sees their points, but no leaderboard and no points for the side.
    shadow_until: f64,
}

/// The player as stored in `game.players` (the same in every region).
pub struct Global {
    pub faction: Faction,
    pub name: String,
    pub hidden: bool,
    pub recovery: Option<[u8; 32]>,
    pub xp_prev: f32,
    pub day_xp: DayXp,
    pub xp_day: i64,
    pub xp_since: f64,
    pub collect: bool,
    pub finds: Vec<i64>,
    pub claims: i32,
    pub badges: Vec<String>,
    /// Regions the player has points in.
    pub regions: Vec<String>,
    /// Presence codes confirmed (no times).
    pub presence: Vec<String>,
    pub streak: Streak,
}

impl Rec {
    fn roll(&mut self, day: i64, prog: &Progression) {
        if self.xp_day != day {
            self.xp_prev += self.day_xp.total(prog);
            self.day_xp = DayXp::default();
            self.xp_day = day;
            self.dirty = true;
        }
        if self.day != day {
            self.day = day;
            self.today = 0;
            self.rated = 0;
            self.dirty = true;
        }
        if self.last_seen != day {
            self.last_seen = day;
            self.dirty = true;
        }
    }
}

enum Write {
    Upsert(Vec<RowOut>),
    Delete([u8; 32]),
    /// A find for a collecting player (no time).
    Find {
        id: [u8; 32],
        find: i64,
    },
    /// A presence code confirmed on site (no time).
    Presence {
        id: [u8; 32],
        code: String,
    },
    Badge {
        id: [u8; 32],
        badge: String,
    },
    /// Collecting switched off: forget the finds.
    ClearFinds([u8; 32]),
    /// Rows unseen since this day go (daily; a pinned region never reloads).
    Sweep(i32),
    FactionDay {
        day: i64,
        fac: [(i64, i64); 2],
        claims: [i64; 2],
        /// Players who earned points that day (city streak).
        players: [i64; 2],
    },
    Barrier(Sender<()>),
}

struct RowOut {
    id: [u8; 32],
    collect: bool,
    claims: i32,
    week: i64,
    points_week: i64,
    xp: (f32, f32, f32, u8),
    xp_day: i64,
    xp_since: f64,
    recovery: Option<[u8; 32]>,
    faction: &'static str,
    name: String,
    total: i64,
    today: i64,
    campaign: i64,
    day: i64,
    hidden: bool,
    last_seen: i64,
    streak: Streak,
}

/// What a player sees about their own standing.
#[derive(Serialize, ts_rs::TS)]
pub struct Mine {
    pub name: String,
    /// Campaign XP, level (1-5) and the XP the next level needs (none at 5).
    pub xp: u32,
    pub level: u8,
    pub next: Option<u32>,
    pub today: i64,
    pub total: i64,
    /// Points since the campaign start.
    pub campaign: i64,
    pub rank_today: Option<usize>,
    pub rank_total: Option<usize>,
    pub rank_campaign: Option<usize>,
    pub hidden: bool,
    pub badges: Vec<String>,
    /// Most points this week on their side in this city.
    pub title: bool,
    /// Weekly streak, if the rule set has one.
    pub streak: Option<StreakView>,
}

/// The streak as the app shows it.
#[derive(Serialize, ts_rs::TS)]
pub struct StreakView {
    /// Weeks done in a row (paused weeks between campaigns don't break it).
    pub weeks: u16,
    /// Play days this week, and how many make the week.
    pub days: u8,
    pub need: u8,
    /// Missed weeks still spared in this campaign.
    pub spare: u8,
    /// This week counts (a campaign runs, or the city has none).
    pub counts: bool,
}

#[derive(Resource)]
pub struct Scores {
    recs: Vec<Option<Rec>>,
    by_id: HashMap<[u8; 32], u32>,
    today: i64,
    /// Today per faction: (node-minutes shaped by players, points).
    fac: [(i64, i64); 2],
    /// Today's claims per faction (weather-day targets).
    fac_claims: [i64; 2],
    /// Days won this season before today (fire, water).
    wins: [u32; 2],
    /// City streak: days in a row before today on which at least `k` people
    /// played here (`game.faction_day.players`).
    city_days: u32,
    /// Local days of the region (`clock.rs`).
    pub clock: crate::clock::Clock,
    fac_dirty: bool,
    tx: Option<crate::writers::LaneTx<Write>>,
    board: SharedBoard,
    /// Sorted slots (today, total, campaign), refreshed with the board.
    order: (Vec<u32>, Vec<u32>, Vec<u32>),
    pub changed: Vec<u32>,
    /// XP and level rules (from the rule set).
    pub prog: Progression,
    /// Points and caps (from the rule set).
    pub points: Score,
    /// Junctions held per slot at the last holding tick, and when.
    pub held: HashMap<u32, u32>,
    pub held_at: f64,
    /// Region language for new player names (`names.rs`).
    pub lang: String,
    /// This region (badges for collections and cities).
    region: String,
    /// Badges earned since the last send: (slot, badge).
    pub news: Vec<(u32, String)>,
    /// Rings broken since the last send: (slot, points paid).
    pub rings: Vec<(u32, i64)>,
    /// Rings closed around a source since the last send: (slot, points, the
    /// surrounded source's side).
    pub sieges: Vec<(u32, i64, elements_shared::prelude::Faction)>,
    /// Blocks held with players' sources (id, their slots), new each minute
    /// until sent.
    pub areas: Option<Vec<(i32, Vec<u32>)>>,
    /// Weekly title holders (fire, water), refreshed with the board.
    titles: [Option<u32>; 2],
    /// Points earned per side since the campaign last took them.
    pub earned: [i64; 2],
    /// Streak rules and the city's campaign periods (unix s; none: every week counts).
    pub streak_rules: StreakRules,
    pub periods: Vec<(f64, f64)>,
}

impl Scores {
    /// Load the region's rows (deleting stale ones) and start the writer.
    /// Days are the region's local days from the start: loading by the UTC
    /// day carried yesterday's side totals into a day that began before UTC
    /// midnight (Konstanz 00:00-02:00), or started them at zero under
    /// tomorrow's date (the Americas in the evening).
    pub fn new(
        db: Option<String>,
        region: String,
        board: SharedBoard,
        clock: crate::clock::Clock,
    ) -> Self {
        let today = clock.day(now_s());
        let mut s = Self {
            recs: Vec::new(),
            by_id: HashMap::new(),
            today,
            fac: [(0, 0); 2],
            fac_claims: [0; 2],
            wins: [0; 2],
            city_days: 0,
            clock,
            fac_dirty: false,
            tx: None,
            board,
            order: Default::default(),
            changed: Vec::new(),
            lang: "de".into(),
            prog: Progression::default(),
            points: Score::default(),
            held: HashMap::new(),
            held_at: 0.0,
            region: region.clone(),
            news: Vec::new(),
            rings: Vec::new(),
            sieges: Vec::new(),
            areas: None,
            titles: [None; 2],
            earned: [0; 2],
            streak_rules: StreakRules::default(),
            periods: Vec::new(),
        };
        if let Some(conn) = db.as_deref() {
            match load(conn, &region, today) {
                Ok((recs, fac, fac_claims, wins, city_days)) => {
                    s.city_days = city_days;
                    for r in recs {
                        s.insert(r);
                    }
                    s.fac = fac;
                    s.fac_claims = fac_claims;
                    s.wins = wins;
                }
                Err(e) => error!(error = %e, "scores: load failed"),
            }
            let key = format!("scores {region}");
            s.tx = Some(crate::writers::lane(&key, |rx| Writer { db, region, rx }));
        }
        info!(players = s.by_id.len(), wins = ?s.wins, "scores loaded");
        s.refresh_board();
        s
    }

    fn insert(&mut self, r: Rec) -> u32 {
        let slot = self.recs.len() as u32;
        self.by_id.insert(r.id, slot);
        self.recs.push(Some(r));
        slot
    }

    pub fn get(&self, slot: u32) -> Option<&Rec> {
        self.recs.get(slot as usize)?.as_ref()
    }

    fn get_mut(&mut self, slot: u32) -> Option<&mut Rec> {
        self.recs.get_mut(slot as usize)?.as_mut()
    }

    /// Players seen today per faction (fire, water).
    /// Players here who earned points today, per side (flagged ones not).
    pub fn played_today(&self) -> [u32; 2] {
        let now = now_s();
        let mut n = [0; 2];
        for r in self.recs.iter().flatten() {
            if r.day == self.today && r.today > 0 && r.shadow_until <= now {
                n[r.faction.index()] += 1;
            }
        }
        n
    }

    pub fn active_today(&self) -> [u32; 2] {
        let mut n = [0; 2];
        for r in self
            .recs
            .iter()
            .flatten()
            .filter(|r| r.last_seen == self.today)
        {
            n[r.faction.index()] += 1;
        }
        n
    }

    /// Slot of a known token.
    pub fn find(&self, token: &str) -> Option<u32> {
        self.by_id.get(&token_id(token)).copied()
    }

    pub fn create(&mut self, token: &str, faction: Faction) -> u32 {
        let g = Global {
            faction,
            name: crate::names::random_name(faction, &self.lang),
            hidden: false,
            recovery: None,
            xp_prev: 0.0,
            day_xp: DayXp::default(),
            xp_day: self.today,
            xp_since: now_s(),
            collect: false,
            finds: Vec::new(),
            claims: 0,
            badges: Vec::new(),
            regions: Vec::new(),
            presence: Vec::new(),
            streak: Streak::default(),
        };
        self.adopt(token, g)
    }

    /// The player as stored (read by the gateway on connect): refresh the
    /// global part, or start their points in this region.
    pub fn adopt(&mut self, token: &str, g: Global) -> u32 {
        let id = token_id(token);
        let known = self.by_id.get(&id).copied();
        let slot = match known {
            Some(slot) => slot,
            None => self.insert(Rec {
                id,
                xp_prev: 0.0,
                day_xp: DayXp::default(),
                xp_day: g.xp_day,
                xp_since: g.xp_since,
                recovery: None,
                faction: g.faction,
                name: String::new(),
                total: 0,
                today: 0,
                campaign: 0,
                day: self.today,
                hidden: false,
                rated: 0,
                carry: 0.0,
                last_seen: self.today,
                dirty: true,
                collect: false,
                found: HashSet::new(),
                presence: HashSet::new(),
                seen: HashSet::new(),
                claims: 0,
                badges: HashSet::new(),
                streak: Streak::default(),
                hour: 0,
                points_hour: 0,
                shadow_until: 0.0,
                week: week_of(self.today),
                points_week: 0,
            }),
        };
        // Unwritten changes here are newer than the stored row (a rename
        // before the next flush); otherwise the stored row wins (played elsewhere).
        if let Some(r) = self.get_mut(slot).filter(|r| known.is_none() || !r.dirty) {
            r.faction = g.faction;
            r.name = g.name;
            r.hidden = g.hidden;
            r.recovery = g.recovery;
            r.xp_prev = g.xp_prev;
            r.day_xp = g.day_xp;
            r.xp_day = g.xp_day;
            r.xp_since = g.xp_since;
            r.collect = g.collect;
            r.found.extend(g.finds);
            r.presence.extend(g.presence);
            r.claims = r.claims.max(g.claims);
            r.badges.extend(g.badges);
            r.streak = g.streak;
        }
        self.changed.push(slot);
        // Weltenbummler: points in several cities (this one counts from now).
        let mut cities: HashSet<String> = g.regions.into_iter().collect();
        cities.insert(self.region.clone());
        for n in CITY_BADGES {
            if cities.len() >= n {
                self.grant(slot, &format!("cities-{n}"));
            }
        }
        slot
    }

    /// Earn `badge` (once). Queued in `news` for the app.
    pub fn grant(&mut self, slot: u32, badge: &str) -> bool {
        let Some(r) = self.get_mut(slot) else {
            return false;
        };
        if !r.badges.insert(badge.to_string()) {
            return false;
        }
        let id = r.id;
        // The player row first (a new player isn't written yet).
        self.flush();
        if let Some(tx) = &self.tx {
            let _ = tx.send(Write::Badge {
                id,
                badge: badge.to_string(),
            });
        }
        self.news.push((slot, badge.to_string()));
        true
    }

    /// A successful claim: count it (player and side), milestones.
    pub fn claimed(&mut self, slot: u32) {
        let Some(r) = self.get_mut(slot) else { return };
        r.claims += 1;
        r.dirty = true;
        let (n, f, shadow) = (r.claims, r.faction.index(), r.shadow_until > now_s());
        if !shadow {
            self.fac_claims[f] += 1;
            self.fac_dirty = true;
        }
        if CLAIM_BADGES.contains(&n) {
            self.grant(slot, &format!("claims-{n}"));
        }
        self.streak_act(slot, 1);
    }

    /// Does the week of `monday` count for streaks (a campaign runs in it)?
    fn week_counts(periods: &[(f64, f64)], monday: i64) -> bool {
        let (a, b) = (monday as f64 * 86_400.0, (monday + 7) as f64 * 86_400.0);
        periods.is_empty() || periods.iter().any(|&(s, e)| s < b && e > a)
    }

    /// A small action for the streak: `weight` 1 for a claim, a whole play
    /// day for a challenge, a find or a rating (`streak_day`).
    pub fn streak_act(&mut self, slot: u32, weight: u8) {
        if !self.streak_rules.enabled {
            return;
        }
        let (today, rules, periods) = (self.today, self.streak_rules.clone(), self.periods.clone());
        let Some(r) = self.get_mut(slot) else { return };
        let Some(step) = r
            .streak
            .act(&rules, today, weight, |w| Self::week_counts(&periods, w))
        else {
            r.dirty = true;
            return;
        };
        r.dirty = true;
        self.changed.push(slot);
        if let Step::Week(n) = step {
            if rules.badges.contains(&n) {
                self.grant(slot, &format!("streak-{n}"));
            }
        }
    }

    /// A whole play day's worth (challenge done, find, rating).
    pub fn streak_day(&mut self, slot: u32) {
        let w = self.streak_rules.acts_per_day;
        self.streak_act(slot, w);
    }

    /// The streak for the app.
    pub fn streak_view(&self, slot: u32) -> Option<StreakView> {
        let rules = &self.streak_rules;
        if !rules.enabled {
            return None;
        }
        let r = self.get(slot)?;
        let counts = |w| Self::week_counts(&self.periods, w);
        let (weeks, days, spare) = r.streak.view(rules, self.today, counts);
        Some(StreakView {
            weeks,
            days,
            need: rules.days_per_week,
            spare,
            counts: counts(week_of(self.today)),
        })
    }

    /// Players here who took part in chapter `key` on the winning side.
    pub fn grant_winners(&mut self, key: &str, winner: Faction) {
        let took_part = format!("chapter:{key}");
        let slots: Vec<u32> = self
            .recs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|r| (i as u32, r)))
            .filter(|(_, r)| r.faction == winner && r.badges.contains(&took_part))
            .map(|(i, _)| i)
            .collect();
        for s in slots {
            self.grant(s, &format!("chapter-win:{key}"));
        }
    }

    /// Winner badges of finished chapters a returning player took part in.
    pub fn catch_up(&mut self, slot: u32, past: &[(String, Faction)]) {
        let Some(r) = self.get(slot) else { return };
        let due: Vec<String> = past
            .iter()
            .filter(|(key, w)| *w == r.faction && r.badges.contains(&format!("chapter:{key}")))
            .map(|(key, _)| format!("chapter-win:{key}"))
            .collect();
        for b in due {
            self.grant(slot, &b);
        }
    }

    /// A complete collection (only when collecting).
    pub fn completed(&mut self, slot: u32, collection: &str) {
        let badge = format!(
            "coll:{}:{collection}",
            self.region.trim_start_matches("region-")
        );
        self.grant(slot, &badge);
    }

    /// A presence code confirmed on site: the number of distinct codes, or
    /// `None` if the player had it already. Badges at 1 and 3.
    pub fn confirm_presence(&mut self, slot: u32, code: &str) -> Option<usize> {
        let r = self.get_mut(slot)?;
        if !r.presence.insert(code.to_string()) {
            return None;
        }
        let (id, n) = (r.id, r.presence.len());
        self.flush();
        if let Some(tx) = &self.tx {
            let _ = tx.send(Write::Presence {
                id,
                code: code.to_string(),
            });
        }
        if n == 1 || n == 3 {
            self.grant(slot, &format!("presence-{n}"));
        }
        // The client's score (badges) is sent again.
        self.changed.push(slot);
        Some(n)
    }

    pub fn presence(&self, slot: u32) -> usize {
        self.get(slot).map_or(0, |r| r.presence.len())
    }

    /// A claim uncovered `find`: `Some(stored)` if it is new for the player.
    pub fn uncover(&mut self, slot: u32, find: i64) -> Option<bool> {
        let r = self.get_mut(slot)?;
        if r.found.contains(&find) || r.seen.contains(&find) {
            return None;
        }
        let collect = r.collect;
        self.streak_day(slot);
        let r = self.get_mut(slot)?;
        if !collect {
            r.seen.insert(find);
            return Some(false);
        }
        r.found.insert(find);
        let id = r.id;
        // The player row first (a new player isn't written yet).
        self.flush();
        if let Some(tx) = &self.tx {
            let _ = tx.send(Write::Find { id, find });
        }
        Some(true)
    }

    /// Opt in to collecting (this session's finds count too) or out (stored
    /// finds are deleted).
    pub fn set_collect(&mut self, slot: u32, on: bool) {
        let Some(r) = self.get_mut(slot) else { return };
        if r.collect == on {
            return;
        }
        r.collect = on;
        r.dirty = true;
        let id = r.id;
        let writes: Vec<Write> = if on {
            let seen: Vec<i64> = r.seen.drain().collect();
            r.found.extend(&seen);
            seen.into_iter()
                .map(|find| Write::Find { id, find })
                .collect()
        } else {
            r.seen.extend(r.found.drain());
            vec![Write::ClearFinds(id)]
        };
        self.flush();
        if let Some(tx) = &self.tx {
            for w in writes {
                let _ = tx.send(w);
            }
        }
    }

    /// Finds the player knows (stored or this session), and whether collecting.
    pub fn finds(&self, slot: u32) -> Option<(bool, HashSet<i64>)> {
        let r = self.get(slot)?;
        Some((r.collect, r.found.union(&r.seen).copied().collect()))
    }

    /// A recovery code moved the player to a new token (done in the database).
    pub fn rekey(&mut self, old: &[u8; 32], new: [u8; 32]) {
        if let Some(slot) = self.by_id.remove(old) {
            if let Some(r) = self.get_mut(slot) {
                r.id = new;
                r.recovery = None;
            }
            self.by_id.insert(new, slot);
        }
    }

    pub fn sweep(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Write::Sweep((self.today - RETAIN_DAYS) as i32));
        }
    }

    /// The player was deleted elsewhere: drop them here too (no write).
    pub fn evict(&mut self, id: &[u8; 32]) {
        if let Some(slot) = self.by_id.remove(id) {
            self.recs[slot as usize] = None;
        }
    }

    /// Mark as seen today.
    pub fn touch(&mut self, slot: u32) {
        let day = self.today;
        let prog = self.prog.clone();
        if let Some(r) = self.get_mut(slot) {
            r.roll(day, &prog);
        }
    }

    /// Add points, within the caps per hour and day; the points granted.
    pub fn award(&mut self, slot: u32, points: i64) -> i64 {
        let day = self.today;
        let prog = self.prog.clone();
        let now = crate::analytics::now_s();
        let cap = self.points.clone();
        let level = if prog.enabled { self.xp(slot).1 } else { 1 };
        let Some(r) = self.get_mut(slot) else {
            return 0;
        };
        r.roll(day, &prog);
        let hour = (now as i64).div_euclid(3600);
        if r.hour != hour {
            r.hour = hour;
            r.points_hour = 0;
        }
        let points = points
            .min(cap.points_hour_max - r.points_hour)
            .min(cap.day_cap(level) - r.today)
            .max(0);
        if points == 0 {
            return 0;
        }
        r.points_hour += points;
        let shadow = r.shadow_until > now;
        r.today += points;
        r.total += points;
        r.campaign += points;
        let week = week_of(day);
        if r.week != week {
            r.week = week;
            r.points_week = 0;
        }
        r.points_week += points;
        r.dirty = true;
        let f = r.faction.index();
        if !shadow {
            self.fac[f].1 += points;
            self.earned[f] += points;
            self.fac_dirty = true;
        }
        self.changed.push(slot);
        points
    }

    pub fn shadowed(&self, slot: u32, now: f64) -> bool {
        self.get(slot).is_some_and(|r| r.shadow_until > now)
    }

    /// Shadow ban until `until` (unix s); see [`Rec::shadow_until`].
    pub fn shadow(&mut self, slot: u32, until: f64) {
        if let Some(r) = self.get_mut(slot) {
            r.shadow_until = until;
        }
    }

    /// Points for a rating, while under the daily cap.
    pub fn rated(&mut self, slot: u32) -> i64 {
        let day = self.today;
        let prog = self.prog.clone();
        let (max, pts) = (self.points.rate_max_per_day, self.points.rate);
        let Some(r) = self.get_mut(slot) else {
            return 0;
        };
        r.roll(day, &prog);
        if r.rated >= max {
            return 0;
        }
        r.rated += 1;
        self.award(slot, pts);
        self.streak_day(slot);
        pts
    }

    pub fn rename(&mut self, slot: u32) {
        let lang = self.lang.clone();
        if let Some(r) = self.get_mut(slot) {
            r.name = crate::names::random_name(r.faction, &lang);
            r.dirty = true;
            self.changed.push(slot);
        }
    }

    /// Switch sides (test servers), with a name that fits the new side.
    pub fn set_faction(&mut self, slot: u32, faction: Faction) {
        let lang = self.lang.clone();
        if let Some(r) = self.get_mut(slot) {
            r.faction = faction;
            r.name = crate::names::random_name(faction, &lang);
            r.dirty = true;
            self.changed.push(slot);
        }
    }

    pub fn set_hidden(&mut self, slot: u32, hidden: bool) {
        if let Some(r) = self.get_mut(slot) {
            r.hidden = hidden;
            r.dirty = true;
            self.changed.push(slot);
        }
    }

    /// Delete the player now (memory and DB, every region). Faction totals
    /// keep the points. Returns the id, for the other regions.
    pub fn forget(&mut self, slot: u32) -> Option<[u8; 32]> {
        let r = self.recs.get_mut(slot as usize).and_then(Option::take)?;
        self.by_id.remove(&r.id);
        if let Some(tx) = &self.tx {
            let _ = tx.send(Write::Delete(r.id));
        }
        Some(r.id)
    }

    /// A new recovery code for the player (the previous one stops working).
    pub fn new_code(&mut self, slot: u32) -> Option<String> {
        let code = recovery_code();
        let hash = code_hash(&code)?;
        let r = self.recs.get_mut(slot as usize)?.as_mut()?;
        r.recovery = Some(hash);
        r.dirty = true;
        self.flush();
        Some(code)
    }

    /// Book a goal for the player's XP of today.
    pub fn add_xp(&mut self, slot: u32, goal: Goal) {
        let today = self.today;
        let prog = self.prog.clone();
        let before = self.xp(slot).1;
        if let Some(r) = self.get_mut(slot) {
            r.roll(today, &prog);
            r.day_xp.add(&prog, goal);
            r.dirty = true;
            self.changed.push(slot);
        }
        let after = self.xp(slot).1;
        if after > before {
            self.grant(slot, &format!("level-{after}"));
        }
    }

    /// Test servers: put the player at the start of `level` (1-5).
    pub fn set_level(&mut self, slot: u32, level: u8) {
        let today = self.today;
        let xp = match level.clamp(1, 5) {
            1 => 0.0,
            n => self
                .prog
                .thresholds
                .get(n as usize - 2)
                .copied()
                .unwrap_or(0.0),
        };
        if let Some(r) = self.get_mut(slot) {
            r.xp_prev = xp;
            r.day_xp = DayXp::default();
            r.xp_day = today;
            r.dirty = true;
            self.changed.push(slot);
        }
    }

    /// A campaign started at `boundary` (unix s): players whose XP is older
    /// start their levels again (their level badges stay), from
    /// `carry_to_level` if they had reached `carry_from_level`; their
    /// campaign points start at 0.
    pub fn campaign_reset(&mut self, boundary: f64, now: f64) {
        let today = self.today;
        let prog = &self.prog;
        let carry = (prog.carry_from_level > 0 && prog.carry_to_level > 1)
            .then(|| {
                prog.thresholds
                    .get(prog.carry_to_level as usize - 2)
                    .copied()
            })
            .flatten()
            .unwrap_or(0.0);
        for (slot, r) in self.recs.iter_mut().enumerate() {
            let Some(r) = r.as_mut().filter(|r| r.xp_since < boundary) else {
                continue;
            };
            let xp = r.xp_prev
                + if r.xp_day == today {
                    r.day_xp.total(prog)
                } else {
                    0.0
                };
            let keep = prog.carry_from_level > 0 && prog.level(xp) >= prog.carry_from_level;
            r.xp_prev = if keep { carry } else { 0.0 };
            r.day_xp = DayXp::default();
            r.xp_day = today;
            r.xp_since = now;
            r.campaign = 0;
            r.dirty = true;
            self.changed.push(slot as u32);
        }
    }

    /// First day in this campaign (no XP before today): Rückenwind.
    pub fn newcomer(&self, slot: u32) -> bool {
        self.get(slot).is_some_and(|r| r.xp_prev == 0.0)
    }

    /// Campaign XP (today's with the variety bonus) and the level.
    pub fn xp(&self, slot: u32) -> (f32, u8) {
        self.get(slot).map_or((0.0, 1), |r| {
            let today = if r.xp_day == self.today {
                r.day_xp.total(&self.prog)
            } else {
                0.0
            };
            let xp = r.xp_prev + today;
            (xp, self.prog.level(xp))
        })
    }

    pub fn mine(&self, slot: u32) -> Option<Mine> {
        let r = self.get(slot)?;
        let (xp, level) = self.xp(slot);
        let rank = |order: &[u32]| order.iter().position(|&s| s == slot).map(|i| i + 1);
        Some(Mine {
            name: r.name.clone(),
            xp: xp as u32,
            level,
            next: self
                .prog
                .thresholds
                .get(level as usize - 1)
                .map(|&t| t as u32),
            today: if r.day == self.today { r.today } else { 0 },
            total: r.total,
            campaign: r.campaign,
            rank_today: rank(&self.order.0),
            rank_total: rank(&self.order.1),
            rank_campaign: rank(&self.order.2),
            hidden: r.hidden,
            badges: {
                let mut b: Vec<String> = r.badges.iter().cloned().collect();
                b.sort();
                b
            },
            title: self.titles.contains(&Some(slot)),
            streak: self.streak_view(slot),
        })
    }

    /// New local day: book yesterday's winner, reset daily totals. Never back.
    fn roll_day(&mut self, day: i64) {
        if day <= self.today {
            return;
        }
        self.flush();
        let [(f, _), (w, _)] = self.fac;
        if f != w {
            self.wins[if f > w { 0 } else { 1 }] += 1;
        }
        // City streak: yesterday counts with k players; a day without the
        // server running (no row) breaks it.
        let played: u32 = self.played_today().iter().sum();
        self.city_days = if played as usize >= crate::analytics::k_min() && day == self.today + 1 {
            self.city_days + 1
        } else {
            0
        };
        self.today = day;
        self.fac = [(0, 0); 2];
        self.fac_claims = [0; 2];
    }

    fn flush(&mut self) {
        let Some(tx) = &self.tx else {
            // No database: nothing to write, rows are as good as written.
            self.recs.iter_mut().flatten().for_each(|r| r.dirty = false);
            return;
        };
        let rows: Vec<RowOut> = self
            .recs
            .iter_mut()
            .flatten()
            .filter(|r| r.dirty)
            .map(|r| {
                r.dirty = false;
                RowOut {
                    xp: {
                        let (b, c, k) = r.day_xp.parts();
                        (r.xp_prev, b, c, k)
                    },
                    xp_day: r.xp_day,
                    xp_since: r.xp_since,
                    collect: r.collect,
                    claims: r.claims,
                    week: r.week,
                    points_week: r.points_week,
                    id: r.id,
                    recovery: r.recovery,
                    faction: r.faction.as_str(),
                    name: r.name.clone(),
                    total: r.total,
                    today: r.today,
                    campaign: r.campaign,
                    day: r.day,
                    hidden: r.hidden,
                    last_seen: r.last_seen,
                    streak: r.streak,
                }
            })
            .collect();
        if !rows.is_empty() {
            let _ = tx.send(Write::Upsert(rows));
        }
        if self.fac_dirty {
            self.fac_dirty = false;
            let players = self.played_today().map(i64::from);
            let _ = tx.send(Write::FactionDay {
                day: self.today,
                fac: self.fac,
                claims: self.fac_claims,
                players,
            });
        }
    }

    /// The city streak for the board: days in a row before today, people who
    /// played today (only from `k` on, else 0) and how many make a day.
    fn city_view(&self) -> serde_json::Value {
        let k = crate::analytics::k_min();
        let played: u32 = self.played_today().iter().sum();
        let enough = played as usize >= k;
        serde_json::json!({
            "days": self.city_days + u32::from(enough),
            "today": enough,
            "missing": if enough { 0 } else { k - played as usize },
            "need": k,
        })
    }

    /// Rank order and the public leaderboard JSON.
    fn refresh_board(&mut self) {
        let today = self.today;
        let now = crate::analytics::now_s();
        let mut visible: Vec<(u32, &Rec)> = self
            .recs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| Some((i as u32, r.as_ref()?)))
            .filter(|(_, r)| !r.hidden && r.shadow_until <= now)
            .collect();
        visible.sort_by_key(|(_, r)| std::cmp::Reverse(r.total));
        let by_total: Vec<u32> = visible
            .iter()
            .filter(|(_, r)| r.total > 0)
            .map(|(s, _)| *s)
            .collect();
        visible.sort_by_key(|(_, r)| std::cmp::Reverse(r.campaign));
        let by_campaign: Vec<u32> = visible
            .iter()
            .filter(|(_, r)| r.campaign > 0)
            .map(|(s, _)| *s)
            .collect();
        visible.retain(|(_, r)| r.day == today && r.today > 0);
        visible.sort_by_key(|(_, r)| std::cmp::Reverse(r.today));
        let by_today: Vec<u32> = visible.iter().map(|(s, _)| *s).collect();

        // Weekly titles: most points this week per side (visible players).
        let week = week_of(today);
        let mut titles = [None::<(u32, i64)>; 2];
        for (i, r) in self.recs.iter().enumerate() {
            let Some(r) = r else { continue };
            if r.hidden || r.shadow_until > now || r.week != week || r.points_week <= 0 {
                continue;
            }
            let t = &mut titles[r.faction.index()];
            if t.is_none_or(|(_, p)| r.points_week > p) {
                *t = Some((i as u32, r.points_week));
            }
        }
        self.titles = titles.map(|t| t.map(|(s, _)| s));
        let titles = self.titles;

        #[derive(Serialize)]
        struct Entry<'a> {
            name: &'a str,
            faction: &'static str,
            points: i64,
            #[serde(skip_serializing_if = "std::ops::Not::not")]
            title: bool,
            /// Weeks in a row (streak), if any.
            #[serde(skip_serializing_if = "Option::is_none")]
            streak: Option<u16>,
        }
        #[derive(Serialize)]
        struct Side {
            node_minutes: i64,
            points: i64,
            wins: u32,
        }
        let entries = |order: &[u32], pts: fn(&Rec) -> i64| -> Vec<Entry> {
            order
                .iter()
                .take(TOP_N)
                .filter_map(|&s| Some((s, self.get(s)?)))
                .map(|(s, r)| Entry {
                    name: &r.name,
                    faction: r.faction.as_str(),
                    points: pts(r),
                    title: titles.contains(&Some(s)),
                    streak: self.streak_view(s).map(|v| v.weeks).filter(|&w| w > 0),
                })
                .collect()
        };
        let side = |i: usize| Side {
            node_minutes: self.fac[i].0,
            points: self.fac[i].1,
            wins: self.wins[i],
        };
        let json = serde_json::json!({
            "today": entries(&by_today, |r| r.today),
            "season": entries(&by_total, |r| r.total),
            "campaign": entries(&by_campaign, |r| r.campaign),
            "fire": side(0),
            "water": side(1),
            "titles": titles.map(|t| t.and_then(|s| self.get(s)).map(|r| r.name.clone())),
            "city": self.city_view(),
        });
        *self.board.write().unwrap() = Some(json.to_string().into());
        self.order = (by_today, by_total, by_campaign);
    }
}

/// Rows unseen since day `stale`: gone, as the privacy page promises.
fn delete_stale(c: &mut postgres::Client, region: &str, stale: i32) -> Result<(), postgres::Error> {
    let n = c.execute(
        "DELETE FROM game.players WHERE last_seen < DATE '1970-01-01' + $1::int",
        &[&stale],
    )?;
    let m = c.execute(
        "DELETE FROM game.player_region WHERE region = $1 \
         AND last_seen < DATE '1970-01-01' + $2::int",
        &[&region, &stale],
    )?;
    if n + m > 0 {
        info!(players = n, here = m, "scores: deleted stale rows");
    }
    Ok(())
}

type Loaded = (Vec<Rec>, [(i64, i64); 2], [i64; 2], [u32; 2], u32);

fn load(conn: &str, region: &str, today: i64) -> Result<Loaded, Box<dyn std::error::Error>> {
    let mut c = crate::db::connect(conn)?;
    delete_stale(&mut c, region, (today - RETAIN_DAYS) as i32)?;
    let mut recs = Vec::new();
    for r in c.query(
        "SELECT p.id, p.faction, p.name, r.points_total, r.points_day, \
         (r.day - DATE '1970-01-01')::int, p.hidden, (r.last_seen - DATE '1970-01-01')::int, \
         p.recovery_hash, p.xp_prev, p.xp_day_base, p.xp_day_claims, p.xp_day_kinds, \
         (p.day - DATE '1970-01-01')::int, p.collect, p.claims, \
         (r.week - DATE '1970-01-01')::int, r.points_week, extract(epoch FROM p.xp_since)::float8, \
         p.streak_weeks, (p.streak_week - DATE '1970-01-01')::int, p.streak_days, \
         (p.streak_day - DATE '1970-01-01')::int, p.streak_acts, p.streak_spared, r.points_campaign \
         FROM game.player_region r JOIN game.players p USING (id) WHERE r.region = $1",
        &[&region],
    )? {
        let id: Vec<u8> = r.get(0);
        let Ok(id) = <[u8; 32]>::try_from(id) else {
            continue;
        };
        let faction = Faction::parse(r.get(1)).unwrap_or(Faction::Fire);
        recs.push(Rec {
            id,
            xp_prev: r.get(9),
            day_xp: DayXp::from_parts(r.get(10), r.get(11), r.get::<_, i16>(12) as u8),
            xp_day: r.get::<_, i32>(13) as i64,
            xp_since: r.get(18),
            recovery: r
                .get::<_, Option<Vec<u8>>>(8)
                .and_then(|h| <[u8; 32]>::try_from(h).ok()),
            faction,
            name: r.get(2),
            total: r.get(3),
            today: r.get(4),
            campaign: r.get(25),
            day: r.get::<_, i32>(5) as i64,
            hidden: r.get(6),
            rated: 0,
            carry: 0.0,
            last_seen: r.get::<_, i32>(7) as i64,
            dirty: false,
            collect: r.get(14),
            found: HashSet::new(),
            presence: HashSet::new(),
            seen: HashSet::new(),
            claims: r.get(15),
            badges: HashSet::new(),
            streak: streak_of(&r, 19),
            week: r
                .get::<_, Option<i32>>(16)
                .map_or(week_of(today), i64::from),
            points_week: r.get(17),
            hour: 0,
            points_hour: 0,
            shadow_until: 0.0,
        });
    }
    // Season = all stored days (clear game.faction_day to start a new one).
    let mut fac = [(0, 0); 2];
    let mut fac_claims = [0; 2];
    let mut days: HashMap<i32, [i64; 2]> = HashMap::new();
    let mut played: HashMap<i32, i32> = HashMap::new();
    for r in c.query(
        "SELECT (day - DATE '1970-01-01')::int, faction, node_minutes, points, claims, players \
         FROM game.faction_day WHERE region = $1",
        &[&region],
    )? {
        let day: i32 = r.get(0);
        *played.entry(day).or_default() += r.get::<_, i32>(5);
        let i = Faction::parse(r.get(1)).unwrap_or(Faction::Fire).index();
        if day as i64 == today {
            fac[i] = (r.get(2), r.get(3));
            fac_claims[i] = r.get(4);
        } else {
            days.entry(day).or_default()[i] = r.get(2);
        }
    }
    let mut wins = [0; 2];
    for [f, w] in days.into_values() {
        if f != w {
            wins[if f > w { 0 } else { 1 }] += 1;
        }
    }
    let k = crate::analytics::k_min() as i32;
    let mut city = 0;
    while played
        .get(&(today as i32 - 1 - city))
        .is_some_and(|&n| n >= k)
    {
        city += 1;
    }
    Ok((recs, fac, fac_claims, wins, city as u32))
}

/// The stored player of a token, if any (gateway, on connect).
pub fn lookup(c: &mut postgres::Client, token: &str) -> Result<Option<Global>, postgres::Error> {
    let row = c.query_opt(
        "SELECT faction, name, hidden, recovery_hash, xp_prev, xp_day_base, xp_day_claims, \
         xp_day_kinds, (day - DATE '1970-01-01')::int, collect, \
         ARRAY(SELECT find FROM game.player_finds f WHERE f.id = p.id), claims, \
         ARRAY(SELECT badge FROM game.player_badges b WHERE b.id = p.id), \
         ARRAY(SELECT region FROM game.player_region g WHERE g.id = p.id), \
         ARRAY(SELECT code FROM game.player_presence pp WHERE pp.id = p.id), \
         extract(epoch FROM xp_since)::float8, \
         streak_weeks, (streak_week - DATE '1970-01-01')::int, streak_days, \
         (streak_day - DATE '1970-01-01')::int, streak_acts, streak_spared \
         FROM game.players p WHERE id = $1",
        &[&&token_id(token)[..]],
    )?;
    Ok(row.map(|r| Global {
        collect: r.get(9),
        finds: r.get(10),
        claims: r.get(11),
        badges: r.get(12),
        regions: r.get(13),
        presence: r.get(14),
        faction: Faction::parse(r.get(0)).unwrap_or(Faction::Fire),
        name: r.get(1),
        hidden: r.get(2),
        recovery: r
            .get::<_, Option<Vec<u8>>>(3)
            .and_then(|h| <[u8; 32]>::try_from(h).ok()),
        xp_prev: r.get(4),
        day_xp: DayXp::from_parts(r.get(5), r.get(6), r.get::<_, i16>(7) as u8),
        xp_day: r.get::<_, i32>(8) as i64,
        xp_since: r.get(15),
        streak: streak_of(&r, 16),
    }))
}

/// The six streak columns from `at` on (`streak_weeks` … `streak_spared`).
pub fn streak_of(r: &postgres::Row, at: usize) -> Streak {
    let n = |i: usize| r.get::<_, i16>(at + i).max(0);
    let d = |i: usize| r.get::<_, Option<i32>>(at + i).map_or(0, i64::from);
    Streak {
        weeks: n(0) as u16,
        week: d(1),
        days: n(2) as u8,
        day: d(3),
        acts: n(4) as u8,
        spared: n(5) as u8,
    }
}

/// Redeem a recovery code (gateway): the player moves to `new_token` in every
/// region (the old device loses them) and the code is used up. Returns the
/// old id, or `None` for an unknown code.
pub fn recover(
    c: &mut postgres::Client,
    code: &str,
    new_token: &str,
) -> Result<Option<[u8; 32]>, postgres::Error> {
    let Some(hash) = code_hash(code) else {
        return Ok(None);
    };
    let row = c.query_opt(
        "WITH old AS (SELECT id FROM game.players WHERE recovery_hash = $1 FOR UPDATE) \
         UPDATE game.players p SET id = $2, recovery_hash = NULL FROM old \
         WHERE p.id = old.id RETURNING old.id",
        &[&&hash[..], &&token_id(new_token)[..]],
    )?;
    Ok(row.and_then(|r| <[u8; 32]>::try_from(r.get::<_, Vec<u8>>(0)).ok()))
}

/// The region's score writer (a lane on the shared writer threads).
struct Writer {
    db: Option<String>,
    region: String,
    rx: Receiver<Write>,
}

impl crate::writers::Lane for Writer {
    fn run(&mut self, pg: &mut crate::writers::Db) -> bool {
        loop {
            let msg = match self.rx.try_recv() {
                Ok(Write::Barrier(ack)) => {
                    let _ = ack.send(());
                    continue;
                }
                Ok(m) => m,
                Err(crossbeam_channel::TryRecvError::Empty) => return true,
                Err(crossbeam_channel::TryRecvError::Disconnected) => return false,
            };
            let db = self.db.as_deref();
            let Some(c) = pg.get(db) else { continue };
            let t = std::time::Instant::now();
            let n = match &msg {
                Write::Upsert(rows) => rows.len(),
                _ => 1,
            };
            if let Err(e) = write(c, &self.region, msg) {
                pg.failed(db, "scores write", &e);
            } else {
                debug!(
                    rows = n,
                    ms = t.elapsed().as_millis() as u64,
                    region = %self.region,
                    "scores written"
                );
            }
        }
    }
}

fn write(c: &mut postgres::Client, region: &str, msg: Write) -> Result<(), postgres::Error> {
    match msg {
        Write::Upsert(rows) => {
            // One statement for all rows (arrays + unnest), not one round trip each.
            let ids: Vec<&[u8]> = rows.iter().map(|r| &r.id[..]).collect();
            let factions: Vec<&str> = rows.iter().map(|r| r.faction).collect();
            let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
            let totals: Vec<i64> = rows.iter().map(|r| r.total).collect();
            let todays: Vec<i64> = rows.iter().map(|r| r.today).collect();
            let days: Vec<i32> = rows.iter().map(|r| r.day as i32).collect();
            let xp_days: Vec<i32> = rows.iter().map(|r| r.xp_day as i32).collect();
            let hidden: Vec<bool> = rows.iter().map(|r| r.hidden).collect();
            let seen: Vec<i32> = rows.iter().map(|r| r.last_seen as i32).collect();
            let rec: Vec<Option<&[u8]>> = rows
                .iter()
                .map(|r| r.recovery.as_ref().map(|h| &h[..]))
                .collect();
            let xp_prev: Vec<f32> = rows.iter().map(|r| r.xp.0).collect();
            let xp_base: Vec<f32> = rows.iter().map(|r| r.xp.1).collect();
            let xp_claims: Vec<f32> = rows.iter().map(|r| r.xp.2).collect();
            let xp_kinds: Vec<i16> = rows.iter().map(|r| r.xp.3 as i16).collect();
            let xp_since: Vec<f64> = rows.iter().map(|r| r.xp_since).collect();
            let collect: Vec<bool> = rows.iter().map(|r| r.collect).collect();
            let claims: Vec<i32> = rows.iter().map(|r| r.claims).collect();
            let weeks: Vec<i32> = rows.iter().map(|r| r.week as i32).collect();
            let week_pts: Vec<i64> = rows.iter().map(|r| r.points_week).collect();
            let camps: Vec<i64> = rows.iter().map(|r| r.campaign).collect();
            let st = |f: fn(&Streak) -> i64| -> Vec<i32> { rows.iter().map(|r| f(&r.streak) as i32).collect() };
            let (sw, swk, sd, sdy, sa, ss) = (
                st(|s| s.weeks as i64),
                st(|s| s.week),
                st(|s| s.days as i64),
                st(|s| s.day),
                st(|s| s.acts as i64),
                st(|s| s.spared as i64),
            );
            // One statement per table for all rows (arrays + unnest). `region`
            // of a new player is where they started.
            let mut t = c.transaction()?;
            t.execute(
                "INSERT INTO game.players \
                 (region, id, faction, name, day, hidden, last_seen, recovery_hash, \
                  xp_prev, xp_day_base, xp_day_claims, xp_day_kinds, collect, claims, xp_since, \
                  streak_weeks, streak_week, streak_days, streak_day, streak_acts, streak_spared) \
                 SELECT $1, i, f, n, DATE '1970-01-01' + dy, h, DATE '1970-01-01' + ls, rc, xp, xb, xc, xk, co, cl, \
                        to_timestamp(xs), sw, NULLIF(swk, 0) + DATE '1970-01-01', sd, \
                        NULLIF(sdy, 0) + DATE '1970-01-01', sa, ss \
                 FROM unnest($2::bytea[], $3::text[], $4::text[], $5::int[], $6::bool[], $7::int[], \
                             $8::bytea[], $9::real[], $10::real[], $11::real[], $12::int2[], $13::bool[], \
                             $14::int4[], $15::float8[], $16::int4[], $17::int4[], $18::int4[], $19::int4[], \
                             $20::int4[], $21::int4[]) \
                      AS x(i, f, n, dy, h, ls, rc, xp, xb, xc, xk, co, cl, xs, sw, swk, sd, sdy, sa, ss) \
                 ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, faction = EXCLUDED.faction, \
                 day = EXCLUDED.day, hidden = EXCLUDED.hidden, \
                 last_seen = GREATEST(game.players.last_seen, EXCLUDED.last_seen), \
                 recovery_hash = EXCLUDED.recovery_hash, xp_prev = EXCLUDED.xp_prev, \
                 xp_day_base = EXCLUDED.xp_day_base, xp_day_claims = EXCLUDED.xp_day_claims, \
                 xp_day_kinds = EXCLUDED.xp_day_kinds, collect = EXCLUDED.collect, \
                 xp_since = EXCLUDED.xp_since, streak_weeks = EXCLUDED.streak_weeks, \
                 streak_week = EXCLUDED.streak_week, streak_days = EXCLUDED.streak_days, \
                 streak_day = EXCLUDED.streak_day, streak_acts = EXCLUDED.streak_acts, \
                 streak_spared = EXCLUDED.streak_spared, \
                 claims = GREATEST(game.players.claims, EXCLUDED.claims)",
                &[
                    &region, &ids, &factions, &names, &xp_days, &hidden, &seen, &rec, &xp_prev,
                    &xp_base, &xp_claims, &xp_kinds, &collect, &claims, &xp_since, &sw, &swk,
                    &sd, &sdy, &sa, &ss,
                ],
            )?;
            t.execute(
                "INSERT INTO game.player_region AS r (region, id, points_total, points_day, day, last_seen, \
                  week, points_week, since, points_campaign) \
                 SELECT $1, i, t, d, DATE '1970-01-01' + dy, DATE '1970-01-01' + ls, DATE '1970-01-01' + wk, pw, \
                        DATE '1970-01-01' + ls, pc \
                 FROM unnest($2::bytea[], $3::int8[], $4::int8[], $5::int[], $6::int[], $7::int[], $8::int8[], $9::int8[]) \
                      AS x(i, t, d, dy, ls, wk, pw, pc) \
                 ON CONFLICT (region, id) DO UPDATE SET points_total = EXCLUDED.points_total, \
                 points_day = EXCLUDED.points_day, day = EXCLUDED.day, last_seen = EXCLUDED.last_seen, \
                 days = LEAST(r.days + (EXCLUDED.last_seen > r.last_seen)::int, 32767), \
                 week = EXCLUDED.week, points_week = EXCLUDED.points_week, points_campaign = EXCLUDED.points_campaign",
                &[&region, &ids, &totals, &todays, &days, &seen, &weeks, &week_pts, &camps],
            )?;
            // Growth numbers of the day (only counts; the read role sees no players).
            if let Some(&today) = seen.iter().max() {
                t.execute(
                    "INSERT INTO analytics.growth_day (region, day, players, new, multi, total) \
                     SELECT $1, d, count(*) FILTER (WHERE last_seen = d), count(*) FILTER (WHERE since = d), \
                            count(*) FILTER (WHERE days >= 2), count(*) \
                     FROM game.player_region, (SELECT DATE '1970-01-01' + $2::int AS d) x WHERE region = $1 \
                     GROUP BY d \
                     ON CONFLICT (region, day) DO UPDATE SET players = EXCLUDED.players, new = EXCLUDED.new, \
                     multi = EXCLUDED.multi, total = EXCLUDED.total",
                    &[&region, &today],
                )?;
            }
            t.commit()
        }
        Write::Presence { id, code } => c
            .execute(
                "INSERT INTO game.player_presence (id, code) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                &[&&id[..], &code],
            )
            .map(|_| ()),
        Write::Find { id, find } => c
            .execute(
                "INSERT INTO game.player_finds (id, find) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                &[&&id[..], &find],
            )
            .map(|_| ()),
        Write::Badge { id, badge } => {
            c.execute(
                "INSERT INTO game.player_badges (id, badge) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                &[&&id[..], &badge],
            )?;
            // A visitor's home gets it too.
            crate::shards::queue_badge(c, &id, &badge)
        }
        Write::ClearFinds(id) => c
            .execute("DELETE FROM game.player_finds WHERE id = $1", &[&&id[..]])
            .map(|_| ()),
        Write::Delete(id) => {
            // The other shards forget them too (visitor rows, or their home).
            let mut t = c.transaction()?;
            crate::shards::queue_forget(&mut t, &id)?;
            t.execute("DELETE FROM game.players WHERE id = $1", &[&&id[..]])?;
            t.commit()
        }
        Write::FactionDay {
            day,
            fac,
            claims,
            players,
        } => {
            for (i, (nm, pts)) in fac.iter().enumerate() {
                c.execute(
                    "INSERT INTO game.faction_day (region, day, faction, node_minutes, points, claims, players) \
                     VALUES ($1, DATE '1970-01-01' + $2::int, $3, $4, $5, $6, $7) \
                     ON CONFLICT (region, day, faction) DO UPDATE SET \
                     node_minutes = EXCLUDED.node_minutes, points = EXCLUDED.points, claims = EXCLUDED.claims, \
                     players = GREATEST(game.faction_day.players, EXCLUDED.players)",
                    &[&region, &(day as i32), &Faction::ALL[i].as_str(), nm, pts, &claims[i], &(players[i] as i32)],
                )?;
            }
            Ok(())
        }
        Write::Sweep(stale) => delete_stale(c, region, stale),
        Write::Barrier(_) => Ok(()),
    }
}

/// Every minute: holding points for players' sources, player-shaped territory
/// per faction. Also rolls the day, flushes rows and refreshes the board.
pub fn tick(
    mut scores: ResMut<Scores>,
    sim: Res<ElementSim>,
    sources: Query<&ElementSource>,
    rules: Res<crate::sim::GameRules>,
    mut campaign: Option<ResMut<crate::campaign::Campaign>>,
    mut blocks: ResMut<crate::area::Blocks>,
    by_id: Res<crate::sim::NodeById>,
    analytics: Option<Res<crate::analytics::AnalyticsTx>>,
    mut area_log: Local<crate::area::AreaLog>,
    mut next: Local<(f64, f64, f64, f64)>,
) {
    let now = now_s();
    let day = scores.clock.day(now);
    scores.roll_day(day);
    let (hold_at, flush_at, board_at, sweep_at) = &mut *next;
    // Stale rows went at load; again once a day (stale players in memory are
    // clean, so nothing writes them back unless they return).
    if *sweep_at == 0.0 {
        *sweep_at = now + 86_400.0;
    } else if now >= *sweep_at {
        *sweep_at = now + 86_400.0;
        scores.sweep();
    }
    if now >= *hold_at {
        *hold_at = now + HOLD_EVERY_S;
        // A campaign started: levels start again (not at an end).
        if let Some(b) = campaign.as_ref().and_then(|c| c.start) {
            scores.campaign_reset(b, now);
        }
        if let Some(c) = campaign.as_ref() {
            if scores.periods != c.periods {
                scores.periods.clone_from(&c.periods);
            }
        }
        let src: HashMap<u32, (Faction, u32)> = sim
            .nodes()
            .iter()
            .enumerate()
            .filter_map(|(i, &e)| {
                let s = sources.get(e).ok()?;
                Some((i as u32, (s.faction, s.by?)))
            })
            .collect();
        let mut held: HashMap<u32, u32> = HashMap::new();
        let mut held_m: HashMap<u32, f32> = HashMap::new();
        let mut fac = [0i64; 2];
        let owned = elements_shared::source::held(sim.state(), sim.origin(), |o| src.get(&o));
        let mut holder: Vec<Option<u32>> = vec![None; sim.state().len()];
        for (i, &(faction, slot)) in owned {
            *held.entry(slot).or_default() += 1;
            *held_m.entry(slot).or_default() += sim.metres()[i];
            fac[faction.index()] += 1;
            holder[i] = Some(slot);
        }
        // Blocks: area points (in the same carry as holding), slower ageing.
        let mut area = blocks.minute(&rules.0.area, sim.state(), &holder, now, |id| {
            by_id.index(id)
        });
        // Held area for the dashboard, written as the hour turns.
        if rules.0.area.enabled {
            let day = scores.clock.day(now);
            let rows = area_log.add(now, day, &area);
            if let Some(tx) = &analytics {
                tx.send(rows);
            }
        }
        for (i, n) in fac.into_iter().enumerate() {
            scores.fac[i].0 += n;
        }
        scores.fac_dirty |= fac.iter().any(|&n| n > 0);
        scores.held = held;
        scores.held_at = now;
        if rules.0.area.enabled {
            scores.areas = Some(std::mem::take(&mut area.players));
        }
        let per_point = scores.points.hold_m_per_point;
        let mut earned: HashMap<u32, f32> = held_m
            .into_iter()
            .map(|(s, m)| (s, m / per_point))
            .collect();
        // Chapter goal "hold area": the side's area, badges for its holders.
        if let Some(c) = campaign.as_deref_mut() {
            for (i, f) in [Faction::Fire, Faction::Water].into_iter().enumerate() {
                if let Some(key) = c.hold(f, area.player_ha[i], now) {
                    let slots: Vec<u32> = area
                        .area
                        .keys()
                        .copied()
                        .filter(|&s| scores.get(s).is_some_and(|r| r.faction == f))
                        .collect();
                    for s in slots {
                        scores.grant(s, &format!("chapter:{key}"));
                    }
                }
            }
        }
        for (slot, p) in area.area {
            *earned.entry(slot).or_default() += p;
        }
        // Broken rings: paid at once, and the app hears of it.
        for (slot, p) in area.broken {
            let got = scores.award(slot, p.round() as i64);
            if got > 0 {
                scores.rings.push((slot, got));
            }
        }
        for (slot, p) in earned {
            let Some(r) = scores.get_mut(slot) else {
                continue;
            };
            r.carry += p;
            let pts = r.carry.floor();
            r.carry -= pts;
            if pts > 0.0 {
                scores.award(slot, pts as i64);
            }
        }
    }
    if now >= *flush_at {
        *flush_at = now + FLUSH_EVERY_S;
        scores.flush();
    }
    if now >= *board_at {
        *board_at = now + BOARD_EVERY_S;
        scores.refresh_board();
    }
}

impl Scores {
    /// Write everything and wait for it (shutdown).
    /// Write now; `ack` once the writer got that far.
    pub fn flush_then(&mut self, ack: Sender<()>) {
        self.flush();
        match &self.tx {
            Some(tx) => {
                let _ = tx.send(Write::Barrier(ack));
            }
            None => {
                let _ = ack.send(());
            }
        }
    }

    pub fn flush_wait(&mut self, timeout: std::time::Duration) -> bool {
        self.flush();
        let Some(tx) = &self.tx else { return true };
        let (ack, done) = crossbeam_channel::bounded(1);
        tx.send(Write::Barrier(ack)).is_ok() && done.recv_timeout(timeout).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores() -> Scores {
        Scores::new(
            None,
            "test".into(),
            SharedBoard::default(),
            Default::default(),
        )
    }

    /// A region starts on its local day, whatever the UTC day: at any time
    /// at least one of UTC+14 and UTC-11 is on another day than UTC.
    #[test]
    fn starts_on_the_local_day() {
        for tz in ["Pacific/Kiritimati", "Pacific/Pago_Pago", "Europe/Berlin"] {
            let clock = crate::clock::Clock::new(Some(tz));
            let s = Scores::new(None, "t".into(), SharedBoard::default(), clock.clone());
            assert_eq!(s.today, clock.day(now_s()), "{tz}");
        }
    }

    fn global(name: &str, faction: Faction, xp_prev: f32) -> Global {
        Global {
            faction,
            name: name.into(),
            hidden: false,
            recovery: None,
            xp_prev,
            day_xp: DayXp::default(),
            xp_day: day_of(now_s()),
            xp_since: now_s(),
            collect: false,
            finds: Vec::new(),
            claims: 0,
            badges: Vec::new(),
            regions: Vec::new(),
            presence: Vec::new(),
            streak: Default::default(),
        }
    }

    #[test]
    fn a_campaign_boundary_starts_levels_again_and_keeps_badges() {
        let mut s = scores();
        s.prog = Progression {
            enabled: true,
            ..Default::default()
        };
        let old = s.adopt("old", global("Alt", Faction::Fire, 590.0));
        let t0 = now_s();
        // Reaches level 3 (600 XP) with a challenge: its badge.
        s.add_xp(old, Goal::Challenge);
        assert_eq!(s.xp(old).1, 3);
        // A campaign started an hour ago; this player's XP is from before.
        s.get_mut(old).unwrap().xp_since = t0 - 7200.0;
        let fresh = s.create("fresh", Faction::Water);
        s.add_xp(fresh, Goal::Claim);
        let fresh_xp = s.xp(fresh).0;
        s.campaign_reset(t0 - 3600.0, t0);
        assert_eq!(s.xp(old), (0.0, 1), "levels again from 1");
        assert!(
            s.get(old).unwrap().badges.contains("level-3"),
            "the badge stays"
        );
        assert_eq!(s.xp(fresh).0, fresh_xp, "XP after the boundary stays");
        // Once reset, not again for the same boundary.
        s.add_xp(old, Goal::Claim);
        let x = s.xp(old).0;
        s.campaign_reset(t0 - 3600.0, t0 + 60.0);
        assert_eq!(s.xp(old).0, x);
    }

    #[test]
    fn a_campaign_start_carries_high_levels_to_level_2() {
        let mut s = scores();
        s.prog = Progression {
            enabled: true,
            carry_from_level: 3,
            carry_to_level: 2,
            ..Default::default()
        };
        let t0 = now_s();
        let high = s.adopt("high", global("Hoch", Faction::Fire, 700.0));
        let low = s.adopt("low", global("Tief", Faction::Water, 300.0));
        for slot in [high, low] {
            s.get_mut(slot).unwrap().xp_since = t0 - 7200.0;
            s.get_mut(slot).unwrap().campaign = 50;
        }
        assert_eq!((s.xp(high).1, s.xp(low).1), (3, 2));
        s.campaign_reset(t0 - 3600.0, t0);
        assert_eq!(s.xp(high), (100.0, 2), "level 3 and up start at level 2");
        assert_eq!(s.xp(low), (0.0, 1), "below that from 1");
        assert_eq!(
            s.get(high).unwrap().campaign,
            0,
            "campaign points start again"
        );
    }

    #[test]
    fn weeks_start_on_monday() {
        // 2026-09-28 is a Monday (day 20724), 2026-10-04 a Sunday.
        assert_eq!(week_of(20724), 20724);
        assert_eq!(week_of(20730), 20724);
        assert_eq!(week_of(20731), 20731);
    }

    #[test]
    fn streak_play_days_weeks_and_campaign_pause() {
        let mut s = scores();
        s.streak_rules = StreakRules {
            enabled: true,
            ..Default::default()
        };
        let p = s.create("p", Faction::Fire);
        let monday = 20731; // 2026-10-05

        // Campaign in the first two weeks only.
        s.periods = vec![(monday as f64 * 86_400.0, (monday + 14) as f64 * 86_400.0)];
        for d in [0, 2, 4, 7, 8, 9] {
            s.today = monday + d;
            s.claimed(p);
            s.claimed(p);
            let before = s.streak_view(p).unwrap().days;
            s.claimed(p);
            assert_eq!(
                s.streak_view(p).unwrap().days,
                before + 1,
                "the third claim"
            );
        }
        let v = s.streak_view(p).unwrap();
        assert_eq!((v.weeks, v.days, v.need), (2, 3, 3));
        assert!(s.news.iter().any(|(_, b)| b == "streak-2"));
        // A rating is a whole play day; outside the campaign the week pauses.
        s.today = monday + 21;
        s.rated(p);
        let v = s.streak_view(p).unwrap();
        assert_eq!((v.weeks, v.days, v.counts), (2, 1, false));
    }

    #[test]
    fn local_days_never_run_back() {
        let mut s = scores();
        let p = s.create("p", Faction::Fire);
        s.award(p, 10);
        let t0 = s.today;
        // Switching from UTC to Toronto in the evening: the local day is behind.
        s.roll_day(t0 - 1);
        assert_eq!(
            (s.today, s.mine(p).unwrap().today),
            (t0, 10),
            "today's points stay"
        );
        s.roll_day(t0 + 1);
        assert_eq!((s.today, s.mine(p).unwrap().today), (t0 + 1, 0));
    }

    #[test]
    fn city_streak_counts_days_with_k_players() {
        let mut s = scores();
        let t0 = s.today;
        let slots: Vec<u32> = (0..5)
            .map(|i| s.create(&format!("p{i}"), Faction::Fire))
            .collect();
        for &p in &slots[..4] {
            s.award(p, 10);
        }
        assert_eq!(s.city_view()["missing"], 1);
        s.award(slots[4], 10);
        assert_eq!(
            (
                s.city_view()["days"].as_u64(),
                s.city_view()["today"].as_bool()
            ),
            (Some(1), Some(true))
        );
        s.roll_day(t0 + 1);
        assert_eq!(s.city_days, 1);
        assert_eq!(s.city_view()["days"], 1, "yesterday counts, today not yet");
        // Nobody played on day 1, and day 2 is skipped altogether.
        s.roll_day(t0 + 3);
        assert_eq!(s.city_days, 0);
    }

    #[test]
    fn claim_milestones_and_weekly_titles() {
        let mut s = scores();
        let a = s.create("a", Faction::Fire);
        let b = s.create("b", Faction::Fire);
        let w = s.create("w", Faction::Water);
        for _ in 0..10 {
            s.claimed(a);
        }
        let got: Vec<&str> = s.news.iter().map(|(_, b)| b.as_str()).collect();
        assert_eq!(got, ["claims-1", "claims-10"]);
        s.award(a, 30);
        s.award(b, 50);
        s.award(w, 5);
        s.refresh_board();
        assert!(
            s.mine(b).unwrap().title && !s.mine(a).unwrap().title,
            "most this week on fire"
        );
        assert!(s.mine(w).unwrap().title, "and on water");
        s.set_hidden(b, true);
        s.refresh_board();
        assert!(s.mine(a).unwrap().title, "hidden players hold no title");
    }

    #[test]
    fn finds_count_once_and_opt_in_keeps_the_session() {
        let mut s = scores();
        let slot = s.create("t", Faction::Water);
        assert_eq!(
            s.uncover(slot, 7),
            Some(false),
            "not collecting: session only"
        );
        assert_eq!(s.uncover(slot, 7), None, "once");
        s.set_collect(slot, true);
        assert!(
            s.get(slot).unwrap().found.contains(&7),
            "opt-in keeps this session's"
        );
        assert_eq!(s.uncover(slot, 8), Some(true));
        s.set_collect(slot, false);
        let r = s.get(slot).unwrap();
        assert!(
            r.found.is_empty() && r.seen.len() == 2,
            "opt-out forgets the stored ones"
        );
        assert_eq!(s.finds(slot).unwrap().1.len(), 2);
    }

    #[test]
    fn codes_read_sloppily() {
        let code = recovery_code();
        assert_eq!(code.len(), 14, "XXXX-XXXX-XXXX");
        // Typed sloppily: lower case, no dashes, O for 0.
        let typed = code.replace('-', " ").to_lowercase().replace('0', "o");
        assert_eq!(code_hash(&typed), code_hash(&code));
        assert!(code_hash("nonsense").is_none());
        let mut s = scores();
        let slot = s.create("t", Faction::Fire);
        let first = s.new_code(slot).unwrap();
        assert_eq!(s.get(slot).unwrap().recovery, code_hash(&first));
    }

    #[test]
    fn adopt_takes_the_stored_player_and_keeps_local_points() {
        let mut s = scores();
        let slot = s.create("t", Faction::Water);
        s.award(slot, 20);
        s.flush();
        // Played elsewhere meanwhile: new name, more XP.
        assert_eq!(
            s.adopt("t", global("Elsewhere", Faction::Water, 300.0)),
            slot
        );
        let r = s.get(slot).unwrap();
        assert_eq!((r.name.as_str(), r.total), ("Elsewhere", 20));
        assert_eq!(s.xp(slot).0, 300.0);
        assert!(!s.newcomer(slot));
        // First time here: points start at zero, side from the player.
        let other = s.adopt("u", global("Visitor", Faction::Fire, 50.0));
        let r = s.get(other).unwrap();
        assert_eq!((r.faction, r.total), (Faction::Fire, 0));
    }

    #[test]
    fn adopt_keeps_unwritten_changes() {
        let mut s = scores();
        let slot = s.create("t", Faction::Fire);
        s.flush();
        s.rename(slot);
        let name = s.get(slot).unwrap().name.clone();
        // Reconnect before the next flush: the stored row still has the old name.
        s.adopt("t", global("Old", Faction::Fire, 0.0));
        assert_eq!(s.get(slot).unwrap().name, name);
    }

    #[test]
    fn rekey_and_evict() {
        let mut s = scores();
        let slot = s.create("old", Faction::Fire);
        s.award(slot, 30);
        s.rekey(&token_id("old"), token_id("new"));
        assert_eq!(s.find("new"), Some(slot));
        assert_eq!(s.find("old"), None, "old device loses it");
        assert_eq!(s.get(slot).unwrap().total, 30);
        s.evict(&token_id("new"));
        assert!(s.find("new").is_none() && s.get(slot).is_none());
    }

    #[test]
    fn ratings_capped_per_day() {
        let mut s = scores();
        let slot = s.create("tok", Faction::Fire);
        let got: i64 = (0..8).map(|_| s.rated(slot)).sum();
        let p = Score::default();
        assert_eq!(got, p.rate * p.rate_max_per_day as i64);
        assert_eq!(s.fac[0].1, got);
    }

    #[test]
    fn presence_counts_distinct_codes_with_badges() {
        let mut s = scores();
        let a = s.create("a", Faction::Water);
        assert_eq!(s.confirm_presence(a, "abc"), Some(1));
        assert_eq!(s.confirm_presence(a, "abc"), None, "once per code");
        assert_eq!(s.confirm_presence(a, "def"), Some(2));
        assert_eq!(s.confirm_presence(a, "ghi"), Some(3));
        let r = s.get(a).unwrap();
        assert!(r.badges.contains("presence-1") && r.badges.contains("presence-3"));
        assert_eq!(s.presence(a), 3);
    }

    #[test]
    fn points_are_capped_per_hour_and_shadow_keeps_side_out() {
        let mut s = scores();
        let a = s.create("a", Faction::Fire);
        assert_eq!(s.award(a, 1000), 1000);
        assert_eq!(s.award(a, 1000), 500, "hour cap");
        assert_eq!(s.award(a, 10), 0);
        let side = s.fac[Faction::Fire.index()].1;
        let b = s.create("b", Faction::Fire);
        s.shadow(b, crate::analytics::now_s() + 60.0);
        assert_eq!(s.award(b, 30), 30, "a flagged player still sees points");
        assert_eq!(
            s.fac[Faction::Fire.index()].1,
            side,
            "but the side gets none"
        );
    }

    #[test]
    fn day_cap_grows_with_level() {
        let mut s = scores();
        s.points.points_hour_max = i64::MAX / 2;
        let a = s.create("a", Faction::Fire);
        assert_eq!(s.award(a, 10_000), 6000, "levels off: the base cap");
        s.prog = Progression {
            enabled: true,
            ..Default::default()
        };
        let b = s.create("b", Faction::Water);
        s.set_level(b, 3);
        assert_eq!(s.award(b, 20_000), 12_000, "level 3: two steps more");
    }

    #[test]
    fn board_ranks_and_hides() {
        let mut s = scores();
        let a = s.create("a", Faction::Fire);
        let b = s.create("b", Faction::Water);
        s.award(a, 10);
        s.award(b, 30);
        s.refresh_board();
        assert_eq!(s.mine(b).unwrap().rank_today, Some(1));
        assert_eq!(s.mine(a).unwrap().rank_today, Some(2));
        s.set_hidden(b, true);
        s.refresh_board();
        assert_eq!(s.mine(a).unwrap().rank_today, Some(1));
        assert_eq!(s.mine(b).unwrap().rank_today, None);
        s.forget(a);
        assert!(s.find("a").is_none() && s.get(a).is_none());
    }

    #[test]
    fn day_roll_books_winner() {
        let mut s = scores();
        s.fac = [(50, 0), (20, 0)];
        let d = s.today + 1;
        s.roll_day(d);
        assert_eq!(s.wins, [1, 0]);
        assert_eq!(s.fac, [(0, 0); 2]);
    }
}

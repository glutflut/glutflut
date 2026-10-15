//! Weekly streaks: a week counts with
//! `days_per_week` play days, a play day needs `acts_per_day` small actions
//! (a claim is one, a challenge, find or rating a whole day). Weeks without a
//! running campaign neither count nor break the streak; a few missed weeks per
//! campaign are spared. Days are the region's local days (`clock.rs`), numbered
//! from 1970-01-01; weeks start on their Monday.

use serde::{Deserialize, Serialize};

/// `[streak]` in the rule set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreakRules {
    pub enabled: bool,
    pub days_per_week: u8,
    pub acts_per_day: u8,
    /// Missed weeks spared per campaign.
    pub spare_weeks: u8,
    /// Streak lengths (weeks) that earn a badge `streak-N`.
    pub badges: Vec<u16>,
}

impl Default for StreakRules {
    fn default() -> Self {
        Self {
            enabled: false,
            days_per_week: 3,
            acts_per_day: 3,
            spare_weeks: 1,
            badges: vec![2, 4, 8],
        }
    }
}

/// The Monday of `day`'s week (1970-01-01 was a Thursday).
pub fn week_of(day: i64) -> i64 {
    day - (day + 3).rem_euclid(7)
}

/// One player's streak, as stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Streak {
    /// Weeks done in a row.
    pub weeks: u16,
    /// Monday of the week `days` belong to (0: never played).
    pub week: i64,
    /// Play days in `week`.
    pub days: u8,
    /// The day `acts` belong to.
    pub day: i64,
    pub acts: u8,
    /// Missed weeks spared in this campaign.
    pub spared: u8,
}

/// What an action changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// A new play day (this week's count).
    Day(u8),
    /// The week is done: the streak is now this long.
    Week(u16),
}

impl Streak {
    /// An action on `day` worth `weight` (1 = a claim; `acts_per_day` = a
    /// whole play day). `counts(monday)`: does that week count (a campaign runs)?
    pub fn act(
        &mut self,
        r: &StreakRules,
        day: i64,
        weight: u8,
        counts: impl Fn(i64) -> bool,
    ) -> Option<Step> {
        if day != self.day {
            self.day = day;
            self.acts = 0;
        }
        let need = r.acts_per_day.max(1);
        if self.acts >= need {
            return None;
        }
        self.acts = self.acts.saturating_add(weight).min(need);
        if self.acts < need {
            return None;
        }
        let w = week_of(day);
        if w != self.week {
            self.settle(r, w, &counts);
            self.week = w;
            self.days = 0;
        }
        self.days += 1;
        if self.days == r.days_per_week.max(1) && counts(w) {
            self.weeks += 1;
            Some(Step::Week(self.weeks))
        } else {
            Some(Step::Day(self.days))
        }
    }

    /// Close the weeks from `self.week` up to (not including) `w`: missed
    /// weeks that counted use the spare ones, else the streak starts again.
    fn settle(&mut self, r: &StreakRules, w: i64, counts: &impl Fn(i64) -> bool) {
        if self.week == 0 {
            return;
        }
        let mut missed = 0u32;
        if self.days < r.days_per_week.max(1) && counts(self.week) {
            missed += 1;
        }
        let mut wk = self.week + 7;
        while wk < w {
            if counts(wk) {
                missed += 1;
            } else {
                // A pause between campaigns: the next one spares anew.
                self.spared = 0;
            }
            wk += 7;
        }
        if missed == 0 {
            return;
        }
        if self.spared as u32 + missed <= r.spare_weeks as u32 {
            self.spared += missed as u8;
        } else {
            self.weeks = 0;
            self.spared = 0;
        }
    }

    /// For the app on `today`: (streak, play days this week, spare weeks left).
    pub fn view(&self, r: &StreakRules, today: i64, counts: impl Fn(i64) -> bool) -> (u16, u8, u8) {
        let mut s = *self;
        let w = week_of(today);
        if w != s.week {
            s.settle(r, w, &counts);
            s.days = 0;
        }
        (s.weeks, s.days, r.spare_weeks.saturating_sub(s.spared))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: i64 = 20_731; // Monday 2026-10-05
    fn always(_: i64) -> bool {
        true
    }
    fn rules() -> StreakRules {
        StreakRules::default()
    }
    /// Play days on these days (each a whole day's worth).
    fn play(s: &mut Streak, days: &[i64], counts: impl Fn(i64) -> bool + Copy) -> Vec<Step> {
        days.iter()
            .filter_map(|&d| s.act(&rules(), d, 3, counts))
            .collect()
    }

    #[test]
    fn monday_is_monday() {
        assert_eq!(week_of(MON), MON);
        assert_eq!(week_of(MON + 6), MON);
        assert_eq!(week_of(MON + 7), MON + 7);
    }

    #[test]
    fn three_claims_make_a_play_day_once() {
        let r = rules();
        let mut s = Streak::default();
        assert_eq!(s.act(&r, MON, 1, always), None);
        assert_eq!(s.act(&r, MON, 1, always), None);
        assert_eq!(s.act(&r, MON, 1, always), Some(Step::Day(1)));
        assert_eq!(s.act(&r, MON, 3, always), None);
        assert_eq!(s.act(&r, MON + 1, 3, always), Some(Step::Day(2)));
    }

    #[test]
    fn three_days_make_a_week_and_weeks_chain() {
        let mut s = Streak::default();
        let steps = play(
            &mut s,
            &[MON, MON + 2, MON + 4, MON + 7, MON + 8, MON + 13],
            always,
        );
        assert_eq!(steps.last(), Some(&Step::Week(2)));
        assert_eq!(s.view(&rules(), MON + 14, always), (2, 0, 1));
    }

    #[test]
    fn one_missed_week_is_spared_two_break() {
        let mut s = Streak::default();
        play(&mut s, &[MON, MON + 1, MON + 2], always);
        // Week 2 missed, week 3 played: spared.
        play(&mut s, &[MON + 14, MON + 15, MON + 16], always);
        assert_eq!(s.weeks, 2);
        assert_eq!(s.spared, 1);
        // Week 4 missed again: no spare left.
        assert_eq!(s.view(&rules(), MON + 28, always).0, 0);
        play(&mut s, &[MON + 28], always);
        assert_eq!(s.weeks, 0);
    }

    #[test]
    fn a_half_week_counts_as_missed() {
        let mut s = Streak::default();
        play(&mut s, &[MON, MON + 1, MON + 2, MON + 7], always);
        assert_eq!(s.view(&rules(), MON + 14, always), (1, 0, 0));
    }

    #[test]
    fn weeks_without_a_campaign_pause() {
        // Campaign in weeks 0-1 and from week 8 on.
        let counts = |w: i64| !(MON + 14..MON + 56).contains(&w);
        let mut s = Streak::default();
        play(
            &mut s,
            &[MON, MON + 1, MON + 2, MON + 7, MON + 8, MON + 9],
            counts,
        );
        assert_eq!(s.weeks, 2);
        // A week done outside a campaign adds nothing, breaks nothing.
        assert_eq!(
            play(&mut s, &[MON + 21, MON + 22, MON + 23], counts).last(),
            Some(&Step::Day(3))
        );
        assert_eq!(s.weeks, 2);
        assert_eq!(
            play(&mut s, &[MON + 56, MON + 57, MON + 58], counts).last(),
            Some(&Step::Week(3))
        );
    }
}

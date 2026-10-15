//! Cheering: watchers tap 🔥 or 💧 for a side, players for their own. Purely for show (no score, no
//! effect on the game, nothing stored). The apps get how many connections
//! cheered for each side in the last `WINDOW_S`, and the new cheers since the
//! last message (floating icons on the map).

use std::collections::HashMap;

use bevy::prelude::*;

use elements_shared::prelude::Faction;

use crate::analytics::now_s;
use crate::protocol::ServerMsg;
use crate::web::{send, to_all, WebClients};

/// Who counts as cheering: a cheer within this time.
pub const WINDOW_S: f64 = 300.0;
/// One cheer per connection in this time; more are dropped.
pub const GAP_S: f64 = 3.0;
/// At most one message to the apps in this time.
const EVERY_S: f64 = 2.0;

#[derive(Resource, Default)]
pub struct Cheers {
    /// Connection -> side and time of its last cheer.
    by: HashMap<u64, (Faction, f64)>,
    /// New cheers since the last message [fire, water].
    burst: [u32; 2],
    /// Counts as last sent.
    sent: [u32; 2],
}

impl Cheers {
    /// A cheer from `conn`; false when it came too soon after its last one.
    pub fn add(&mut self, conn: u64, side: Faction, now: f64) -> bool {
        if self.by.get(&conn).is_some_and(|&(_, at)| now - at < GAP_S) {
            return false;
        }
        self.by.insert(conn, (side, now));
        self.burst[side.index()] += 1;
        true
    }

    /// Connections cheering for [fire, water] (their last side counts).
    pub fn counts(&mut self, now: f64) -> [u32; 2] {
        self.by.retain(|_, (_, at)| now - *at < WINDOW_S);
        let mut c = [0; 2];
        for (f, _) in self.by.values() {
            c[f.index()] += 1;
        }
        c
    }

    /// The message for all apps, when anything changed.
    fn take(&mut self, now: f64) -> Option<ServerMsg<'static>> {
        let c = self.counts(now);
        if c == self.sent && self.burst == [0, 0] {
            return None;
        }
        self.sent = c;
        let burst = std::mem::take(&mut self.burst);
        Some(ServerMsg::Cheers {
            fire: c[0],
            water: c[1],
            burst,
        })
    }

    /// The current counts for a new connection (none when nobody cheers).
    pub fn now_msg(&self) -> Option<ServerMsg<'static>> {
        (self.sent != [0, 0]).then_some(ServerMsg::Cheers {
            fire: self.sent[0],
            water: self.sent[1],
            burst: [0, 0],
        })
    }
}

/// From the inbox: a cheer, unless too soon.
pub fn cheer(cheers: &mut Cheers, conn: u64, side: &str, now: f64) {
    if let Some(f) = Faction::parse(side) {
        cheers.add(conn, f, now);
    }
}

/// Counts and new cheers to every app, at most every `EVERY_S`.
pub fn tick(mut cheers: ResMut<Cheers>, clients: Res<WebClients>, mut next: Local<f64>) {
    let now = now_s();
    if now < *next {
        return;
    }
    *next = now + EVERY_S;
    if let Some(m) = cheers.take(now) {
        to_all(&clients, &m);
    }
}

/// Send the current counts to one new connection.
pub fn greet(cheers: &Cheers, out: &crate::gateway::Outbox) {
    if let Some(m) = cheers.now_msg() {
        send(out, &m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_cheer_per_gap() {
        let mut c = Cheers::default();
        assert!(c.add(1, Faction::Fire, 100.0));
        assert!(!c.add(1, Faction::Fire, 101.0));
        assert!(!c.add(1, Faction::Water, 102.9));
        assert!(c.add(1, Faction::Water, 103.0));
        assert!(c.add(2, Faction::Fire, 101.0));
        assert_eq!(c.burst, [2, 1]);
        // the last side counts, once per connection
        assert_eq!(c.counts(104.0), [1, 1]);
    }

    #[test]
    fn window_and_messages() {
        let mut c = Cheers::default();
        assert!(c.take(0.0).is_none());
        c.add(1, Faction::Fire, 10.0);
        c.add(2, Faction::Fire, 20.0);
        c.add(3, Faction::Water, 30.0);
        match c.take(31.0) {
            Some(ServerMsg::Cheers { fire, water, burst }) => {
                assert_eq!((fire, water, burst), (2, 1, [2, 1]));
            }
            _ => panic!("expected cheers"),
        }
        assert!(c.take(32.0).is_none());
        assert!(c.now_msg().is_some());
        // the first leaves the window
        match c.take(10.0 + WINDOW_S) {
            Some(ServerMsg::Cheers { fire, water, burst }) => {
                assert_eq!((fire, water, burst), (1, 1, [0, 0]));
            }
            _ => panic!("expected cheers"),
        }
        // all gone: one last message with zeros, then quiet
        assert!(matches!(
            c.take(31.0 + WINDOW_S),
            Some(ServerMsg::Cheers {
                fire: 0,
                water: 0,
                ..
            })
        ));
        assert!(c.take(40.0 + WINDOW_S).is_none());
        assert!(c.now_msg().is_none());
        assert!(c.by.is_empty());
    }
}

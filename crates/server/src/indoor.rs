//! Indoors (`[indoor]`): in halls GPS is useless, so a scanned code places the
//! player at a junction for `hold_s`. Each junction has its own secret,
//! derived from `ELEMENTS_INDOOR_KEY` (else from `ELEMENTS_SHARD_KEY`):
//!
//! - sheet on a wall: `<node>-<mac("s")>`, fixed;
//! - screen (`node_type = 'screen'`): `<node>-<w>-<mac("w<w>")>`, `w` the
//!   `window_s` window of unix time, so a photo is soon worthless.
//!
//! Node and window in base 36, mac = the first 6 bytes of HMAC-SHA256 with
//! the junction's secret, base 32 (lower case). A screen computes its own
//! code from the secret (`web/knoten.html`, or a microcontroller).
//! The server sees "this token scanned junction X", never a position.

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::shards::{hex, hmac, same};

/// Windows a screen code is good for after its own: e-ink refreshes slowly.
const LATE_WINDOWS: u64 = 2;
/// Codes are exact (no GPS noise): from one junction to another at most
/// this fast, plus `SCAN_SLACK_M`.
const WALK_MS: f32 = 3.0;
const SCAN_SLACK_M: f32 = 10.0;
/// While held, a fix at the junction this often (time played, "on site").
const STAY_EVERY_S: f64 = 5.0;

fn key() -> Option<&'static [u8]> {
    static K: std::sync::LazyLock<Option<Vec<u8>>> = std::sync::LazyLock::new(|| {
        if let Some(k) = std::env::var("ELEMENTS_INDOOR_KEY")
            .ok()
            .filter(|k| k.len() >= 32)
        {
            return Some(k.into_bytes());
        }
        let shard = std::env::var("ELEMENTS_SHARD_KEY").ok()?;
        Some(hmac(shard.as_bytes(), b"indoor").to_vec())
    });
    K.as_deref()
}

/// The junction's secret (screens get it, as hex).
pub fn node_secret(key: &[u8], region: &str, node: i64) -> [u8; 32] {
    hmac(key, format!("node:{region}:{node}").as_bytes())
}

const B32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

fn mac(secret: &[u8], msg: &str) -> String {
    let h = hmac(secret, msg.as_bytes());
    // 48 bits, 10 characters.
    let mut v = h[..6].iter().fold(0u64, |a, &b| (a << 8) | u64::from(b));
    let mut out = [0u8; 10];
    for c in out.iter_mut().rev() {
        *c = B32[(v & 31) as usize];
        v >>= 5;
    }
    String::from_utf8_lossy(&out).into()
}

fn b36(mut v: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = Vec::new();
    loop {
        s.push(D[(v % 36) as usize]);
        v /= 36;
        if v == 0 {
            break;
        }
    }
    s.reverse();
    String::from_utf8_lossy(&s).into()
}

fn un36(s: &str) -> Option<u64> {
    (!s.is_empty() && s.len() <= 13)
        .then(|| u64::from_str_radix(s, 36).ok())
        .flatten()
}

pub fn static_code(secret: &[u8], node: i64) -> String {
    format!("{}-{}", b36(node as u64), mac(secret, "s"))
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn screen_code(secret: &[u8], node: i64, window: u64) -> String {
    format!(
        "{}-{}-{}",
        b36(node as u64),
        b36(window),
        mac(secret, &format!("w{window}"))
    )
}

/// What a scanned code says.
#[derive(Debug, PartialEq)]
pub enum Code {
    Sheet { node: i64 },
    Screen { node: i64, window: u64 },
}

/// The junction named in a code, before checking it.
pub fn code_node(code: &str) -> Option<i64> {
    let n = un36(code.split('-').next()?)?;
    i64::try_from(n).ok()
}

/// Check `code` against the junction's secret; `window` = now's window.
pub fn verify(secret: &[u8], code: &str, window: u64) -> Result<Code, &'static str> {
    let parts: Vec<&str> = code.split('-').collect();
    let node = code_node(code).ok_or("unknown")?;
    match parts[..] {
        [_, m] if same(m.as_bytes(), mac(secret, "s").as_bytes()) => Ok(Code::Sheet { node }),
        [_, w, m] => {
            let w = un36(w).ok_or("unknown")?;
            if !same(m.as_bytes(), mac(secret, &format!("w{w}")).as_bytes()) {
                return Err("unknown");
            }
            // One window ahead: a screen's clock may run a little fast.
            if w > window + 1 || w + LATE_WINDOWS < window {
                return Err("old");
            }
            Ok(Code::Screen { node, window: w })
        }
        _ => Err("unknown"),
    }
}

/// The code from a scanned text: a game link (`…?q=<code>`) or the bare code.
pub fn from_scan(text: &str) -> Option<String> {
    let t = text.trim();
    let code = match t.split_once("q=") {
        Some((_, rest)) => rest.split(['&', '#']).next()?,
        None => t,
    };
    let ok = (5..=40).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| code.to_string())
}

/// Window of unix time `now` (s).
pub fn window(now: f64, window_s: u32) -> u64 {
    (now.max(0.0) / f64::from(window_s.max(1))) as u64
}

/// For the ops sheet list: (static code, screen secret hex) of a junction.
pub fn codes_for(region: &str, node: i64) -> Option<(String, String)> {
    let s = node_secret(key()?, region, node);
    Some((static_code(&s, node), hex(&s)))
}

/// Junction names by node id (`geo.nodes.story_tag`).
#[derive(Resource, Default)]
pub struct Labels(pub std::collections::HashMap<i64, String>);

/// Junction held after a scan, until when (on the player's `Motion`).
#[derive(Clone, Copy, Debug)]
pub struct Held {
    pub node: i64,
    pub at: Vec3,
    pub until: f64,
    pub next_stay: f64,
}

/// A scan: the junction it names, if the code holds for this region now.
pub fn check(
    region: &str,
    code: &str,
    now: f64,
    rules: &elements_shared::rules::Indoor,
    is_screen: impl Fn(i64) -> Option<bool>,
) -> Result<i64, &'static str> {
    if !rules.enabled {
        return Err("off");
    }
    let key = key().ok_or("off")?;
    let node = code_node(code).ok_or("unknown")?;
    let screen = is_screen(node).ok_or("unknown")?;
    let secret = node_secret(key, region, node);
    match verify(&secret, code, window(now, rules.window_s))? {
        Code::Sheet { .. } if screen => Err("screen"),
        _ => Ok(node),
    }
}

/// From the last scan to `at` (junction `node`) on foot in time.
pub fn walkable(last: Option<(i64, Vec3, f64)>, node: i64, at: Vec3, now: f64) -> bool {
    last.is_none_or(|(n, p, t)| {
        n == node || p.distance(at) <= WALK_MS * (now - t).max(0.0) as f32 + SCAN_SLACK_M
    })
}

/// Held junctions run out; while held, a fix at the junction now and then.
pub fn hold(
    rules: Res<crate::sim::GameRules>,
    mut players: Query<(&mut Position, &mut crate::motion::Motion)>,
) {
    if !rules.0.indoor.enabled {
        return;
    }
    let now = crate::analytics::now_s();
    for (mut pos, mut m) in &mut players {
        let Some(h) = m.held else { continue };
        if now >= h.until {
            m.held = None;
            pos.0 = crate::motion::NOWHERE;
            continue;
        }
        // Back after a reconnect (the hold parks with the motion).
        if pos.0 != h.at {
            pos.0 = h.at;
        }
        if now >= h.next_stay {
            m.held = Some(Held {
                next_stay: now + STAY_EVERY_S,
                ..h
            });
            if m.fix(h.at, now, None) {
                m.last_fix = Some((h.at, now));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_check() {
        let s = node_secret(b"0123456789abcdef0123456789abcdef", "region-x", 1_000_123);
        let sheet = static_code(&s, 1_000_123);
        assert_eq!(code_node(&sheet), Some(1_000_123));
        assert_eq!(verify(&s, &sheet, 5), Ok(Code::Sheet { node: 1_000_123 }));
        let w = 58_000_000;
        let screen = screen_code(&s, 1_000_123, w);
        assert!(verify(&s, &screen, w).is_ok());
        assert!(verify(&s, &screen, w + 2).is_ok());
        assert_eq!(verify(&s, &screen, w + 3), Err("old"));
        assert_eq!(verify(&s, &screen, w - 2), Err("old"));
        // Another junction's secret, a changed character.
        let other = node_secret(b"0123456789abcdef0123456789abcdef", "region-x", 1_000_124);
        assert_eq!(verify(&other, &sheet, 5), Err("unknown"));
        let mut bad = sheet.clone();
        bad.pop();
        bad.push(if sheet.ends_with('a') { 'b' } else { 'a' });
        assert_eq!(verify(&s, &bad, 5), Err("unknown"));
    }

    #[test]
    fn walking_between_codes() {
        let a = Vec3::ZERO;
        let b = Vec3::new(90.0, 0.0, 0.0);
        assert!(walkable(None, 2, b, 10.0));
        assert!(!walkable(Some((1, a, 0.0)), 2, b, 10.0));
        assert!(walkable(Some((1, a, 0.0)), 2, b, 30.0));
        assert!(walkable(Some((2, a, 0.0)), 2, b, 1.0));
    }

    #[test]
    fn scanned_text() {
        assert_eq!(
            from_scan("https://glutflut.com/?region=r&q=lfls-abcdefghij").as_deref(),
            Some("lfls-abcdefghij")
        );
        assert_eq!(
            from_scan("lfls-abcdefghij").as_deref(),
            Some("lfls-abcdefghij")
        );
        assert_eq!(from_scan("https://example.com/"), None);
        assert_eq!(from_scan("q=<script>"), None);
    }

    #[test]
    fn known_vector() {
        // The same in web/src/indoor-code.ts (test there): keep both in step.
        let s = node_secret(b"0123456789abcdef0123456789abcdef", "region-x", 42);
        assert_eq!(
            hex(&s),
            "1b9f1369bbdfa34c2352de083f6fca462180c34dd4a87013e4d48fc3f0f705f8"
        );
        assert_eq!(static_code(&s, 42), "16-gbxyoghmce");
        assert_eq!(screen_code(&s, 42, 1000), "16-rs-cb2cntiewg");
    }
}

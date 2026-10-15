//! Proof of work for new identities: the server hands out a signed
//! challenge, the browser finds `n`
//! with SHA-256("<challenge>:<n>") starting with `bits` zero bits (about a
//! second on a phone). Invisible to people, expensive for a thousand fake
//! players. Stateless apart from the used challenges (single use, 10 min).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use sha2::{Digest, Sha256};

use crate::shards::{hex, hmac};

/// Challenges older than this are refused.
const TTL_S: u64 = 600;

/// Zero bits required (`ELEMENTS_POW_BITS`, default 18; 0 = off).
pub fn bits() -> u32 {
    static B: LazyLock<u32> = LazyLock::new(|| {
        std::env::var("ELEMENTS_POW_BITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(18)
            .min(30)
    });
    *B
}

/// Signing key: from the shard secret, so every cell of the shard accepts a
/// challenge another one issued; without it new per start (open challenges
/// then fail and are reissued).
static KEY: LazyLock<[u8; 32]> = LazyLock::new(|| {
    std::env::var("ELEMENTS_SHARD_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .map(|k| Sha256::digest(format!("elements-pow:{k}").as_bytes()).into())
        .unwrap_or_else(rand::random)
});
static USED: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Default::default);

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sign(body: &str) -> String {
    hex(&hmac(&*KEY, body.as_bytes())[..16])
}

/// A fresh challenge: `<time>.<random>.<bits>.<signature>`.
pub fn challenge() -> String {
    let body = format!("{}.{:016x}.{}", now(), rand::random::<u64>(), bits());
    format!("{body}.{}", sign(&body))
}

fn zero_bits(h: &[u8]) -> u32 {
    let mut n = 0;
    for &b in h {
        if b == 0 {
            n += 8;
        } else {
            return n + b.leading_zeros();
        }
    }
    n
}

/// A solved challenge of ours, fresh, unused, with enough zero bits.
pub fn check(c: &str, n: &str) -> bool {
    let Some((body, sig)) = c.rsplit_once('.') else {
        return false;
    };
    let mut parts = body.split('.');
    let (Some(ts), Some(_), Some(b), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let (Ok(ts), Ok(b)) = (ts.parse::<u64>(), b.parse::<u32>()) else {
        return false;
    };
    let t = now();
    if !crate::shards::same(sign(body).as_bytes(), sig.as_bytes())
        || t.saturating_sub(ts) > TTL_S
        || b < bits()
        || n.len() > 20
        || zero_bits(&Sha256::digest(format!("{c}:{n}").as_bytes())) < b
    {
        return false;
    }
    let mut used = USED.lock().unwrap();
    used.retain(|_, &mut at| t.saturating_sub(at) <= TTL_S);
    used.insert(c.to_string(), ts).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solve(c: &str, b: u32) -> String {
        (0u64..)
            .map(|n| n.to_string())
            .find(|n| zero_bits(&Sha256::digest(format!("{c}:{n}").as_bytes())) >= b)
            .unwrap()
    }

    #[test]
    fn solved_once_then_used() {
        let c = challenge();
        let n = solve(&c, bits());
        assert!(check(&c, &n));
        assert!(!check(&c, &n), "single use");
    }

    #[test]
    fn forged_or_unsolved_fails() {
        let c = challenge();
        let forged = c.replacen(&format!(".{}.", bits()), ".1.", 1);
        assert!(
            !check(&forged, &solve(&forged, 1)),
            "bits changed: signature breaks"
        );
        let wrong = (0u64..)
            .map(|n| n.to_string())
            .find(|n| zero_bits(&Sha256::digest(format!("{c}:{n}").as_bytes())) == 0)
            .unwrap();
        assert!(!check(&c, &wrong));
    }

    #[test]
    fn counts_leading_zero_bits() {
        assert_eq!(zero_bits(&[0, 0, 0b0001_0000]), 19);
        assert_eq!(zero_bits(&[0x80]), 0);
    }
}

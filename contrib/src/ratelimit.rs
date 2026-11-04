//! A per-client request limit kept in memory, and only there: client addresses are hashed with a
//! per-process random salt before they become keys, old entries are dropped as they expire, and
//! nothing is ever written out. The backstop for a raw flood; the quotas on pseudonyms and
//! clusters (`api`) are the real limits.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimit {
    per_minute: u32,
    salt: [u8; 16],
    windows: Mutex<HashMap<[u8; 16], (Instant, u32)>>,
}

impl RateLimit {
    pub fn new(per_minute: u32) -> Self {
        let salt: [u8; 16] = uuid::Uuid::new_v4().into_bytes();
        RateLimit { per_minute, salt, windows: Mutex::new(HashMap::new()) }
    }

    /// Whether one more request from `client` is within the limit, counting it if so.
    pub fn allow(&self, client: &str) -> bool {
        let mut h = Sha256::new();
        h.update(self.salt);
        h.update(client.as_bytes());
        let key: [u8; 16] = h.finalize()[..16].try_into().unwrap();
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap();
        if windows.len() > 10_000 {
            windows.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
        }
        let entry = windows.entry(key).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(60) {
            *entry = (now, 0);
        }
        if entry.1 >= self.per_minute {
            return false;
        }
        entry.1 += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_gets_its_minute_and_others_are_not_affected() {
        let r = RateLimit::new(3);
        assert!(r.allow("a") && r.allow("a") && r.allow("a"));
        assert!(!r.allow("a"));
        assert!(r.allow("b"));
    }
}

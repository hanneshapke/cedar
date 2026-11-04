//! The pattern scan: secrets and personal data that have a shape (keys, tokens, emails, phone
//! numbers, bank and card numbers, IP addresses, URL tokens, home directories), and the
//! [`Redactor`] that replaces them with typed placeholders, the same for the same value.
//!
//! What has no shape (a name, an employer, "the oat-milk startup in Portland") is for the
//! model pass and the person's own eyes; this scan does not promise to catch it.

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// What a finding is, and the placeholder kind it becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// An API key, token, password or private key: blocks sending until the person looks.
    Key,
    /// A token in a URL's query string.
    Token,
    Email,
    Phone,
    Iban,
    Card,
    Ip,
    /// A user name in a home directory path.
    User,
    /// The name of a file attached to a message (no pattern: given to the redactor by name).
    File,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Key => "KEY",
            Kind::Token => "TOKEN",
            Kind::Email => "EMAIL",
            Kind::Phone => "PHONE",
            Kind::Iban => "IBAN",
            Kind::Card => "CARD",
            Kind::Ip => "IP",
            Kind::User => "USER",
            Kind::File => "FILE",
        }
    }

    /// A secret rather than personal data: the one kind a client must not send on the scan's
    /// say-so alone, and the ingest refuses.
    pub fn is_secret(self) -> bool {
        matches!(self, Kind::Key | Kind::Token)
    }
}

/// A span of text the scan flagged. `start..end` are byte offsets into the scanned string;
/// `value` is what gets replaced (for a path, the user name only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    pub value: String,
}

struct Pattern {
    kind: Kind,
    re: Regex,
    /// Which capture group is the value (0: the whole match).
    group: usize,
}

fn patterns() -> &'static [Pattern] {
    static P: OnceLock<Vec<Pattern>> = OnceLock::new();
    P.get_or_init(|| {
        let p = |kind, re: &str, group| Pattern { kind, re: Regex::new(re).unwrap(), group };
        vec![
            // Secrets first, so a key that looks like something else is a KEY.
            p(Kind::Key, r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----", 0),
            p(Kind::Key, r"\b(?:sk|rk|pk)-(?:proj-|ant-|live-|test-)?[A-Za-z0-9_\-]{16,}", 0),
            p(Kind::Key, r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}", 0),
            p(Kind::Key, r"\bgithub_pat_[A-Za-z0-9_]{40,}", 0),
            p(Kind::Key, r"\bxox[abprs]-[A-Za-z0-9\-]{10,}", 0),
            p(Kind::Key, r"\bAKIA[0-9A-Z]{16}\b", 0),
            p(Kind::Key, r"\bAIza[0-9A-Za-z_\-]{35}", 0),
            p(Kind::Key, r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}", 0),
            p(Kind::Key, r"(?i)\bbearer\s+([A-Za-z0-9_\-\.=+/]{16,})", 1),
            // `OPENAI_API_KEY=...`, `password: ...`, `"token": "..."`: a name that contains one of
            // the words, then = or :, then a value of some length.
            p(Kind::Key, r#"(?i)\b[A-Za-z0-9_\-]*(?:api[_\-]?key|secret|token|password|passwd)[A-Za-z0-9_\-]*"?\s*[:=]\s*["']?([A-Za-z0-9_\-\.=+/]{12,})"#, 1),
            p(Kind::Token, r#"(?i)[?&](?:token|key|api_key|apikey|access_token|auth|sig|signature|secret|password)=([^&\s"'<>]{8,})"#, 1),
            p(Kind::Email, r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b", 0),
            p(Kind::Iban, r"\b[A-Z]{2}\d{2}(?:[ ]?[A-Z0-9]{4}){2,7}(?:[ ]?[A-Z0-9]{1,4})?\b", 0),
            p(Kind::Card, r"\b(?:\d[ \-]?){13,19}\b", 0),
            // No lookbehind in Rust's regex: the character before a + number is consumed and
            // left alone (group 1 is the number).
            p(Kind::Phone, r"(?:^|[^\w.])(\+\d{1,3}[ \-.]?(?:\(\d{1,4}\)[ \-.]?)?\d{2,4}[ \-.]?\d{2,4}(?:[ \-.]?\d{2,4}){0,2}\b|\(\d{3}\) ?\d{3}[\-. ]\d{4}\b|\b\d{3}[\-.]\d{3}[\-.]\d{4}\b)", 1),
            p(Kind::Ip, r"\b(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(?:\.(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}\b", 0),
            p(Kind::User, r#"(?:/Users/|/home/|[A-Za-z]:\\Users\\)([^/\\\s"'`:;,)]+)"#, 1),
        ]
    })
}

/// Addresses that identify nobody.
fn ip_is_public(ip: &str) -> bool {
    let octets: Vec<u8> = ip.split('.').filter_map(|o| o.parse().ok()).collect();
    if octets.len() != 4 {
        return false;
    }
    let documentation = (octets[0] == 192 && octets[1] == 0 && octets[2] == 2) || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100) || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113);
    !(documentation || octets[0] == 127 || octets[0] == 0 || octets[0] == 10 || (octets[0] == 192 && octets[1] == 168) || (octets[0] == 172 && (16..=31).contains(&octets[1])) || (octets[0] == 169 && octets[1] == 254) || octets == [255, 255, 255, 255])
}

/// Luhn, so a run of digits is a card number only when it checks out.
fn luhn_ok(digits: &str) -> bool {
    let ds: Vec<u32> = digits.chars().filter(|c| c.is_ascii_digit()).filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&ds.len()) {
        return false;
    }
    let sum: u32 = ds.iter().rev().enumerate().map(|(i, &d)| if i % 2 == 1 { let x = d * 2; if x > 9 { x - 9 } else { x } } else { d }).sum();
    sum.is_multiple_of(10)
}

/// Whether `value` is one of the redactor's own stand-ins (or lives in the same reserved range),
/// which the scan leaves alone: that keeps a redacted text stable, and such values identify
/// nobody anyway.
pub fn is_surrogate(kind: Kind, value: &str) -> bool {
    match kind {
        Kind::Email => value.rsplit_once('@').is_some_and(|(_, d)| DOMAINS.contains(&d.to_ascii_lowercase().as_str())),
        Kind::Phone => value.contains("555 01") || value.contains("555-01") || value.contains("555.01"),
        Kind::Iban => value.len() > 4 && &value[2..4] == "00",
        Kind::Card => {
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            CARDS.iter().any(|c| c.chars().filter(|c| c.is_ascii_digit()).collect::<String>() == digits)
        }
        Kind::Ip => !ip_is_public(value),
        Kind::User => FIRST_NAMES.contains(&value.trim_end_matches(|c: char| c.is_ascii_digit())),
        Kind::Key | Kind::Token => value.contains("stand1n"),
        Kind::File => value.starts_with("attachment-"),
    }
}

/// Home-directory names that are nobody.
const COMMON_USERS: [&str; 8] = ["root", "ubuntu", "user", "runner", "agent", "sandbox", "cedar", "Public"];

/// Everything the patterns find in `text`, earliest first, without overlaps (the first pattern
/// to claim a span keeps it; patterns are ordered secrets first).
pub fn scan(text: &str) -> Vec<Finding> {
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut out = Vec::new();
    for p in patterns() {
        for caps in p.re.captures_iter(text) {
            let whole = caps.get(0).unwrap();
            let Some(m) = caps.get(p.group) else { continue };
            let (start, end) = (m.start(), m.end());
            if taken.iter().any(|&(s, e)| start < e && s < end) {
                continue;
            }
            let value = m.as_str();
            // What the redactor itself writes is never found again (so a redacted text stays as
            // it is), and what identifies nobody is left alone.
            let keep = !is_surrogate(p.kind, value)
                && match p.kind {
                    Kind::Ip => ip_is_public(value),
                    Kind::Card => luhn_ok(value),
                    Kind::User => !COMMON_USERS.contains(&value),
                    Kind::Key | Kind::Token => !value.chars().all(|c| c == '*' || c == 'x' || c == 'X' || c == '.'),
                    Kind::Phone => value.chars().filter(|c| c.is_ascii_digit()).count() >= 7,
                    _ => true,
                };
            if !keep {
                continue;
            }
            taken.push((whole.start(), whole.end()));
            out.push(Finding { kind: p.kind, start, end, value: value.to_string() });
        }
    }
    out.sort_by_key(|f| f.start);
    out
}

/// Replaces findings with **surrogates**: values of the same kind and shape, drawn from ranges
/// reserved for examples and fiction, the same surrogate for the same value throughout a bundle
/// so a trajectory stays coherent (the agent still lists `/Users/maya`, mails the same address
/// twice, and a command stays a command). A model trained on them learns the shape of real data,
/// not a bracket token.
///
/// Surrogates come from a per-bundle random seed, never from the value: a surrogate derived from
/// the original would be a fingerprint (emails and names are guessable, so a hash of one can be
/// matched), and would link one person's contributions to each other. Two bundles never share a
/// seed; within one, the mapping is consistent.
///
/// The ranges: addresses at `example.com`, `example.net` and `example.org` (RFC 2606), North
/// American numbers in 555-0100 to 555-0199 (reserved for fiction), IP addresses in the
/// documentation blocks (RFC 5737), an IBAN whose check digits are invalid, the card industry's
/// published test numbers, and invented first and last names. Secrets become a value of the same
/// prefix whose body says it is a stand-in (`sk-stand1n...`), since a trajectory only needs a
/// key-shaped string there, never one that could be real.
#[derive(Debug, Clone)]
pub struct Redactor {
    seed: u64,
    /// (kind, original value) → surrogate.
    map: HashMap<(Kind, String), String>,
    /// Per kind, the surrogates handed out, in order.
    used: HashMap<Kind, Vec<String>>,
    /// Values given by name (`redact_exact`): (label, value, surrogate).
    exact: Vec<(String, String, String)>,
}

const FIRST_NAMES: [&str; 24] = [
    "maya", "theo", "rosa", "elias", "nora", "jonah", "lena", "milo", "ida", "oskar", "vera", "ruben",
    "alma", "felix", "june", "arlo", "mira", "levi", "sana", "noel", "ines", "tobias", "lia", "emil",
];
const LAST_NAMES: [&str; 24] = [
    "lindqvist", "okafor", "marchetti", "haddad", "novak", "fernandes", "ekström", "tanaka", "varga", "delacroix",
    "mbeki", "ivers", "castellano", "brandt", "quintero", "halvorsen", "adeyemi", "kowalczyk", "sandoval", "petrov",
    "nakamura", "oyelaran", "bergström", "almeida",
];
const DOMAINS: [&str; 3] = ["example.com", "example.net", "example.org"];
const CARDS: [&str; 4] = ["4111 1111 1111 1111", "5555 5555 5555 4444", "4012 8888 8888 1881", "3782 822463 10005"];

/// What one replacement did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub kind: Kind,
    pub surrogate: String,
}

/// A small deterministic mixer, so the n-th surrogate of a kind depends on the seed and n only.
fn mix(seed: u64, kind: Kind, n: usize) -> u64 {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(seed.to_le_bytes());
    h.update(kind.as_str().as_bytes());
    h.update((n as u64).to_le_bytes());
    u64::from_le_bytes(h.finalize()[..8].try_into().unwrap())
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

impl Redactor {
    /// A redactor with a fresh random seed.
    pub fn new() -> Self {
        let bytes = uuid_like_random();
        Self::with_seed(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
    }

    /// A redactor whose surrogates follow `seed` (tests, and a preview that must stay stable).
    pub fn with_seed(seed: u64) -> Self {
        Redactor { seed, map: HashMap::new(), used: HashMap::new(), exact: Vec::new() }
    }

    /// The n-th (0-based) surrogate of a kind for this seed.
    fn make(&self, kind: Kind, n: usize, original: &str) -> String {
        let r = mix(self.seed, kind, n);
        let first = FIRST_NAMES[(r as usize) % FIRST_NAMES.len()];
        let last = LAST_NAMES[((r >> 8) as usize) % LAST_NAMES.len()];
        let hex = format!("{:06x}", r & 0xff_ffff);
        match kind {
            Kind::Email => format!("{first}.{last}{}@{}", if n < 2 { String::new() } else { n.to_string() }, DOMAINS[((r >> 16) as usize) % DOMAINS.len()]),
            Kind::Phone => format!("+1 555 01{:02}", ((r >> 24) as usize + n) % 100),
            Kind::Iban => format!("DE00 {:04} {:04} {:04} {:04} {:02}", (r >> 8) % 10_000, (r >> 24) % 10_000, (r >> 40) % 10_000, (r >> 48) % 10_000, n % 100),
            Kind::Card => CARDS[n % CARDS.len()].to_string(),
            Kind::Ip => {
                let blocks = ["192.0.2", "198.51.100", "203.0.113"];
                format!("{}.{}", blocks[n % blocks.len()], 1 + ((r >> 32) as usize + n * 7) % 253)
            }
            Kind::User => if n == 0 { first.to_string() } else { format!("{first}{n}") },
            Kind::File => {
                let ext = original.rsplit_once('.').map(|(_, e)| e).filter(|e| e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric())).unwrap_or("bin");
                format!("attachment-{}.{ext}", n + 1)
            }
            Kind::Key => {
                // Keep a recognisable prefix so the trajectory still reads as "a key"; the body
                // says it is a stand-in, and the scan knows the word.
                let prefix = ["sk-", "rk-", "pk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "AIza", "eyJ"]
                    .into_iter()
                    .find(|p| original.starts_with(p))
                    .unwrap_or("key-");
                format!("{prefix}stand1n{hex}{:06x}", (r >> 24) & 0xff_ffff)
            }
            Kind::Token => format!("stand1n{hex}{:06x}", (r >> 24) & 0xff_ffff),
        }
    }

    fn surrogate(&mut self, kind: Kind, value: &str) -> String {
        if let Some(s) = self.map.get(&(kind, value.to_string())) {
            return s.clone();
        }
        let n = self.used.get(&kind).map_or(0, Vec::len);
        let s = self.make(kind, n, value);
        self.map.insert((kind, value.to_string()), s.clone());
        self.used.entry(kind).or_default().push(s.clone());
        s
    }

    /// Replace every finding in `text`; returns what was applied, in text order.
    pub fn redact(&mut self, text: &mut String) -> Vec<Applied> {
        let findings = scan(text);
        let mut applied = Vec::new();
        if findings.is_empty() && self.exact.is_empty() {
            return applied;
        }
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for f in &findings {
            out.push_str(&text[at..f.start]);
            let surrogate = self.surrogate(f.kind, &f.value);
            out.push_str(&surrogate);
            applied.push(Applied { kind: f.kind, surrogate });
            at = f.end;
        }
        out.push_str(&text[at..]);
        for (_, value, surrogate) in &self.exact {
            if !value.is_empty() && out.contains(value.as_str()) {
                out = out.replace(value.as_str(), surrogate);
            }
        }
        *text = out;
        applied
    }

    /// Also replace this exact phrase wherever it occurs, by `replacement`, or by a neutral
    /// `redacted-<6 hex>` when none is given: the person's own addition, listed as `CUSTOM`.
    pub fn redact_phrase(&mut self, phrase: &str, replacement: Option<&str>) -> String {
        let phrase = phrase.trim();
        if let Some((_, _, s)) = self.exact.iter().find(|(l, v, _)| l == "CUSTOM" && v == phrase) {
            return s.clone();
        }
        let n = self.exact.iter().filter(|(l, _, _)| l == "CUSTOM").count();
        let surrogate = match replacement.map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) => r.to_string(),
            None => format!("redacted-{:06x}", mix(self.seed, Kind::File, 1000 + n) & 0xff_ffff),
        };
        self.exact.push(("CUSTOM".into(), phrase.to_string(), surrogate.clone()));
        surrogate
    }

    /// Also replace this exact value wherever it occurs, by a surrogate of `kind` (an
    /// attachment's name becomes `attachment-1.pdf`, say). Returns the surrogate.
    pub fn redact_exact(&mut self, kind: Kind, value: &str) -> String {
        let value = value.trim();
        let label = kind.as_str().to_string();
        if let Some((_, _, s)) = self.exact.iter().find(|(l, v, _)| *l == label && v == value) {
            return s.clone();
        }
        let n = self.exact.iter().filter(|(l, _, _)| *l == label).count();
        let surrogate = self.make(kind, n, value);
        self.exact.push((label, value.to_string(), surrogate.clone()));
        surrogate
    }

    /// The surrogates in use, by kind, as a bundle's `redactions` lists them: what a reader may
    /// take for real and shouldn't. Never the originals.
    pub fn surrogates(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut out: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        for (kind, list) in &self.used {
            out.entry(kind.as_str().to_string()).or_default().extend(list.iter().cloned());
        }
        for (label, _, s) in &self.exact {
            let e = out.entry(label.clone()).or_default();
            if !e.contains(s) {
                e.push(s.clone());
            }
        }
        out
    }
}

/// 16 random bytes without a dependency on a random crate: a v4 UUID's bytes.
fn uuid_like_random() -> [u8; 16] {
    use sha2::{Digest, Sha256};
    // The best entropy available without `rand`: the time, the process, a stack address and a
    // counter, hashed. Good enough for a surrogate seed, which only needs to differ per bundle.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut h = Sha256::new();
    h.update(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    let local = 0u8;
    h.update((&local as *const u8 as usize).to_le_bytes());
    h.update(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_le_bytes());
    h.finalize()[..16].try_into().unwrap()
}

/// Where a secret still is in a bundle's text, after redaction (a client's placeholders can be
/// undone by hand, and an old client may not scan). Each is a short description of the place.
pub fn secrets_in(bundle: &crate::Bundle) -> Vec<String> {
    let mut out = Vec::new();
    // The stand-ins the bundle declares for keys and tokens look like keys and tokens; that is
    // the point of them, and they are not secrets.
    let declared: HashSet<&str> = bundle
        .redactions
        .iter()
        .filter(|(k, _)| k.as_str() == "KEY" || k.as_str() == "TOKEN")
        .flat_map(|(_, v)| v.iter().map(String::as_str))
        .collect();
    let is_secret = |f: &Finding| f.kind.is_secret() && !declared.contains(f.value.as_str());
    for t in &bundle.turns {
        let mut check = |what: String, s: &str| {
            for f in scan(s) {
                if is_secret(&f) {
                    out.push(format!("{what}: a {} at byte {}", f.kind.as_str().to_lowercase(), f.start));
                }
            }
        };
        check(format!("turn {} prompt", t.seq), &t.prompt);
        check(format!("turn {} system", t.seq), &t.system);
        for (i, m) in t.messages.iter().enumerate() {
            for b in &m.content {
                for s in crate::validate::block_texts(b) {
                    check(format!("turn {} message {}", t.seq, i + 1), s);
                }
            }
        }
        for c in &t.calls {
            if let Some(r) = &c.reasoning {
                check(format!("turn {} call {} reasoning", t.seq, c.seq), r);
            }
        }
    }
    if let Some(n) = bundle.outcome.as_ref().and_then(|o| o.notes.as_deref()) {
        for f in scan(n) {
            if is_secret(&f) {
                out.push(format!("outcome notes: a {} at byte {}", f.kind.as_str().to_lowercase(), f.start));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, String)> {
        scan(text).into_iter().map(|f| (f.kind, f.value)).collect()
    }

    #[test]
    fn keys_tokens_and_bearer_headers_are_secrets() {
        assert_eq!(kinds("use sk-abcdefghijklmnopqrstuvwxyz123456 please"), vec![(Kind::Key, "sk-abcdefghijklmnopqrstuvwxyz123456".into())]);
        assert_eq!(kinds("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij12")[0].0, Kind::Key);
        assert_eq!(kinds("Authorization: Bearer abc.def-ghi_jkl.mno123456")[0], (Kind::Key, "abc.def-ghi_jkl.mno123456".into()));
        assert_eq!(kinds("export OPENAI_API_KEY=\"QxZ123456789abcdef\"")[0], (Kind::Key, "QxZ123456789abcdef".into()));
        assert_eq!(kinds("https://x.example/?q=1&token=abcdefghijk&b=2")[0], (Kind::Token, "abcdefghijk".into()));
        assert!(kinds("AKIAABCDEFGHIJKLMNOP").iter().any(|k| k.0 == Kind::Key));
        assert!(kinds("-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----").iter().any(|k| k.0 == Kind::Key));
        // Not a key: a placeholder, a masked value, a short word after "token".
        assert_eq!(kinds("api_key=sk-stand1n0a1b2c3d4e5f token: ********** secret: no"), vec![]);
    }

    #[test]
    fn personal_data_with_a_shape_is_found() {
        assert_eq!(kinds("write to ana.lopez@oatkeg.co today"), vec![(Kind::Email, "ana.lopez@oatkeg.co".into())]);
        assert_eq!(kinds("call +49 30 1234567 or (415) 867-5309")[0].0, Kind::Phone);
        assert_eq!(kinds("call +49 30 1234567 or (415) 867-5309").len(), 2);
        assert_eq!(kinds("IBAN DE89 3704 0044 0532 0130 00")[0].0, Kind::Iban);
        assert_eq!(kinds("card 4539 1488 0343 6467")[0].0, Kind::Card);
        assert_eq!(kinds("version 1234 5678 9012 3456 of the thing"), vec![]); // fails Luhn
        assert_eq!(kinds("server 93.184.216.34 and 127.0.0.1 and 192.168.1.5 and 203.0.113.9"), vec![(Kind::Ip, "93.184.216.34".into())]);
        assert_eq!(kinds("see /Users/hannes/code and /home/root/x and C:\\Users\\Maya\\Desktop"), vec![(Kind::User, "hannes".into()), (Kind::User, "Maya".into())]);
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        assert_eq!(kinds("Version 2.3.4 was released in 2024, with 10 000 downloads and a 3-2-1 backup rule"), vec![]);
        assert_eq!(kinds("the key is to start early; token budget is 4096"), vec![]);
    }

    #[test]
    fn the_redactor_gives_each_value_one_surrogate_across_strings() {
        let mut r = Redactor::with_seed(7);
        let mut a = "mail ana@oatkeg.co and bob@corp.example, from 93.184.216.34".to_string();
        let mut b = "ana@oatkeg.co again; see /Users/hannes and key sk-abcdefghijklmnopqrstuvwxyz".to_string();
        r.redact(&mut a);
        r.redact(&mut b);
        let s = r.surrogates();
        let emails = &s["EMAIL"];
        assert_eq!(emails.len(), 2);
        assert!(emails.iter().all(|e| e.ends_with("@example.com") || e.ends_with("@example.net") || e.ends_with("@example.org")), "{emails:?}");
        assert_ne!(emails[0], emails[1]);
        assert_eq!(a, format!("mail {} and {}, from {}", emails[0], emails[1], s["IP"][0]));
        assert!(b.starts_with(&format!("{} again; see /Users/{} and key sk-stand1n", emails[0], s["USER"][0])), "{b}");
        assert!(s["IP"][0].starts_with("192.0.2."));
        assert!(FIRST_NAMES.contains(&s["USER"][0].as_str()));
        // The same seed gives the same surrogates; another seed, others.
        let mut r2 = Redactor::with_seed(7);
        let mut a2 = "mail ana@oatkeg.co".to_string();
        r2.redact(&mut a2);
        assert_eq!(a2, format!("mail {}", emails[0]));
        let mut r3 = Redactor::with_seed(8);
        let mut a3 = "mail ana@oatkeg.co".to_string();
        r3.redact(&mut a3);
        assert_ne!(a3, a2);
        // Redacting the result again changes nothing: surrogates are in reserved ranges.
        let before = a.clone();
        Redactor::with_seed(9).redact(&mut a);
        assert_eq!(a, before);
    }

    #[test]
    fn surrogates_keep_the_shape_of_what_they_replace() {
        let mut r = Redactor::with_seed(1);
        let mut t = "call +49 30 1234567, pay DE89 3704 0044 0532 0130 00 or card 4539 1488 0343 6467, https://x.example/?token=abcdefghijk&x=1; ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij12".to_string();
        r.redact(&mut t);
        let s = r.surrogates();
        assert!(s["PHONE"][0].starts_with("+1 555 01"), "{t}");
        assert!(s["IBAN"][0].starts_with("DE00 "), "{t}");
        assert!(luhn_ok(&s["CARD"][0]));
        assert!(t.contains(&format!("token={}&x=1", s["TOKEN"][0])), "{t}");
        assert!(s["KEY"][0].starts_with("ghp_stand1n"), "{t}");
        assert!(scan(&t).iter().all(|f| !f.kind.is_secret()), "a stand-in reads as a secret: {t}");
    }

    #[test]
    fn phrases_and_file_names_the_person_adds_are_replaced_too() {
        let mut r = Redactor::with_seed(3);
        let co = r.redact_phrase("Oat Keg Co", None);
        assert!(co.starts_with("redacted-"), "{co}");
        assert_eq!(r.redact_phrase("Oat Keg Co", None), co);
        assert_eq!(r.redact_phrase("Portland", Some("Springfield")), "Springfield");
        assert_eq!(r.redact_exact(Kind::File, "Q3 invoice.PDF"), "attachment-1.PDF");
        assert_eq!(r.redact_exact(Kind::File, "photo.jpg"), "attachment-2.jpg");
        let mut s = "I work at Oat Keg Co, the Oat Keg Co in Portland; see Q3 invoice.PDF".to_string();
        r.redact(&mut s);
        assert_eq!(s, format!("I work at {co}, the {co} in Springfield; see attachment-1.PDF"));
        assert_eq!(r.surrogates()["CUSTOM"], vec![co, "Springfield".to_string()]);
        assert_eq!(r.surrogates()["FILE"], vec!["attachment-1.PDF", "attachment-2.jpg"]);
    }

    #[test]
    fn secrets_left_in_a_bundle_are_located() {
        let mut b = crate::example::bundle();
        assert_eq!(secrets_in(&b), Vec::<String>::new());
        b.turns[0].messages[2].content[0]["content"] = serde_json::json!("token=sk-abcdefghijklmnopqrstuvwxyz");
        assert_eq!(secrets_in(&b), vec!["turn 1 message 3: a key at byte 6"]);
        // A key-shaped stand-in the bundle declares is not a secret.
        b.turns[0].messages[2].content[0]["content"] = serde_json::json!("token=sk-stand1n0a1b2c3d4e5f");
        assert_eq!(secrets_in(&b), Vec::<String>::new());
        b.turns[0].messages[2].content[0]["content"] = serde_json::json!("token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij12");
        assert_eq!(secrets_in(&b).len(), 1);
        b.redactions.insert("KEY".into(), vec!["ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij12".into()]);
        assert_eq!(secrets_in(&b), Vec::<String>::new());
    }
}



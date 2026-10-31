//! The pattern scan: secrets and personal data that have a shape (keys, tokens, emails, phone
//! numbers, bank and card numbers, IP addresses, URL tokens, home directories), and the
//! [`Redactor`] that replaces them with typed placeholders, the same for the same value.
//!
//! What has no shape (a name, an employer, "the oat-milk startup in Portland") is for the
//! model pass and the person's own eyes; this scan does not promise to catch it.

use regex::Regex;
use std::collections::HashMap;
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
    !(octets[0] == 127 || octets[0] == 0 || octets[0] == 10 || (octets[0] == 192 && octets[1] == 168) || (octets[0] == 172 && (16..=31).contains(&octets[1])) || (octets[0] == 169 && octets[1] == 254) || octets == [255, 255, 255, 255])
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

/// A placeholder this crate wrote, which must not be scanned again.
fn is_placeholder(s: &str) -> bool {
    crate::validate::placeholder_regex().is_match(s) && s.starts_with('[') && s.ends_with(']')
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
            let keep = match p.kind {
                Kind::Ip => ip_is_public(value),
                Kind::Card => luhn_ok(value),
                Kind::User => !COMMON_USERS.contains(&value),
                Kind::Key | Kind::Token => !is_placeholder(value) && !value.chars().all(|c| c == '*' || c == 'x' || c == 'X' || c == '.'),
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

/// Replaces findings with `[KIND_n]`, numbering each distinct value once per kind across every
/// string it is given, so the same email is `[EMAIL_1]` wherever it occurs in a bundle.
#[derive(Debug, Default, Clone)]
pub struct Redactor {
    numbers: HashMap<(Kind, String), u32>,
    counts: HashMap<Kind, u32>,
    /// Values given by name (`redact_exact`): (placeholder kind, value, number).
    exact: Vec<(String, String, u32)>,
}

/// What one replacement did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub kind: Kind,
    pub placeholder: String,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    fn number(&mut self, kind: Kind, value: &str) -> u32 {
        if let Some(n) = self.numbers.get(&(kind, value.to_string())) {
            return *n;
        }
        let n = self.counts.entry(kind).or_default();
        *n += 1;
        let n = *n;
        self.numbers.insert((kind, value.to_string()), n);
        n
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
            let n = self.number(f.kind, &f.value);
            let placeholder = format!("[{}_{n}]", f.kind.as_str());
            out.push_str(&placeholder);
            applied.push(Applied { kind: f.kind, placeholder });
            at = f.end;
        }
        out.push_str(&text[at..]);
        for (label, value, n) in &self.exact {
            if !value.is_empty() && out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &format!("[{label}_{n}]"));
            }
        }
        *text = out;
        applied
    }

    /// Also replace this exact phrase wherever it occurs, as `[CUSTOM_n]`; the person's own
    /// addition. Returns the placeholder.
    pub fn redact_phrase(&mut self, phrase: &str) -> String {
        self.redact_exact(Kind::File, phrase, true)
    }

    /// Also replace this exact value wherever it occurs, as a placeholder of `kind` (an
    /// attachment's name as `[FILE_n]`, say); with `custom`, as `[CUSTOM_n]` instead.
    pub fn redact_exact(&mut self, kind: Kind, value: &str, custom: bool) -> String {
        let value = value.trim();
        let label = if custom { "CUSTOM".to_string() } else { kind.as_str().to_string() };
        if let Some((_, _, n)) = self.exact.iter().find(|(l, v, _)| *l == label && v == value) {
            return format!("[{label}_{n}]");
        }
        let n = self.exact.iter().filter(|(l, _, _)| *l == label).count() as u32 + 1;
        self.exact.push((label.clone(), value.to_string(), n));
        format!("[{label}_{n}]")
    }

    /// How many placeholders of each kind exist, as a bundle's `redactions` map wants them.
    pub fn counts(&self) -> std::collections::BTreeMap<String, u32> {
        let mut out: std::collections::BTreeMap<String, u32> =
            self.counts.iter().filter(|(_, n)| **n > 0).map(|(k, n)| (k.as_str().to_string(), *n)).collect();
        for (label, _, n) in &self.exact {
            let e = out.entry(label.clone()).or_default();
            *e = (*e).max(*n);
        }
        out
    }
}

/// Where a secret still is in a bundle's text, after redaction (a client's placeholders can be
/// undone by hand, and an old client may not scan). Each is a short description of the place.
pub fn secrets_in(bundle: &crate::Bundle) -> Vec<String> {
    let mut out = Vec::new();
    for t in &bundle.turns {
        let mut check = |what: String, s: &str| {
            for f in scan(s) {
                if f.kind.is_secret() {
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
            if f.kind.is_secret() {
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
        assert_eq!(kinds("api_key=[KEY_1] token: ********** secret: no"), vec![]);
    }

    #[test]
    fn personal_data_with_a_shape_is_found() {
        assert_eq!(kinds("write to ana.lopez@example.org today"), vec![(Kind::Email, "ana.lopez@example.org".into())]);
        assert_eq!(kinds("call +49 30 1234567 or (415) 555-0199")[0].0, Kind::Phone);
        assert_eq!(kinds("call +49 30 1234567 or (415) 555-0199").len(), 2);
        assert_eq!(kinds("IBAN DE89 3704 0044 0532 0130 00")[0].0, Kind::Iban);
        assert_eq!(kinds("card 4111 1111 1111 1111")[0].0, Kind::Card);
        assert_eq!(kinds("version 1234 5678 9012 3456 of the thing"), vec![]); // fails Luhn
        assert_eq!(kinds("server 203.0.113.9 and 127.0.0.1 and 192.168.1.5"), vec![(Kind::Ip, "203.0.113.9".into())]);
        assert_eq!(kinds("see /Users/hannes/code and /home/root/x and C:\\Users\\Maya\\Desktop"), vec![(Kind::User, "hannes".into()), (Kind::User, "Maya".into())]);
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        assert_eq!(kinds("Version 2.3.4 was released in 2024, with 10 000 downloads and a 3-2-1 backup rule"), vec![]);
        assert_eq!(kinds("the key is to start early; token budget is 4096"), vec![]);
    }

    #[test]
    fn the_redactor_numbers_each_value_once_across_strings() {
        let mut r = Redactor::new();
        let mut a = "mail ana@example.org and bob@example.org".to_string();
        let mut b = "ana@example.org again, from 203.0.113.9".to_string();
        r.redact(&mut a);
        r.redact(&mut b);
        assert_eq!(a, "mail [EMAIL_1] and [EMAIL_2]");
        assert_eq!(b, "[EMAIL_1] again, from [IP_1]");
        assert_eq!(r.counts().get("EMAIL"), Some(&2));
        assert_eq!(r.counts().get("IP"), Some(&1));
        // Redacting the result again changes nothing.
        r.redact(&mut a);
        assert_eq!(a, "mail [EMAIL_1] and [EMAIL_2]");
    }

    #[test]
    fn phrases_the_person_adds_are_replaced_too() {
        let mut r = Redactor::new();
        assert_eq!(r.redact_phrase("Oat Keg Co"), "[CUSTOM_1]");
        assert_eq!(r.redact_phrase("Oat Keg Co"), "[CUSTOM_1]");
        let mut s = "I work at Oat Keg Co, the Oat Keg Co in Portland".to_string();
        r.redact(&mut s);
        assert_eq!(s, "I work at [CUSTOM_1], the [CUSTOM_1] in Portland");
        assert_eq!(r.counts().get("CUSTOM"), Some(&1));
    }

    #[test]
    fn secrets_left_in_a_bundle_are_located() {
        let mut b = crate::example::bundle();
        assert_eq!(secrets_in(&b), Vec::<String>::new());
        b.turns[0].messages[2].content[0]["content"] = serde_json::json!("token=sk-abcdefghijklmnopqrstuvwxyz");
        assert_eq!(secrets_in(&b), vec!["turn 1 message 3: a key at byte 6"]);
    }
}


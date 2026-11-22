//! The user's time zone, and the date and time a run is told it is.
//!
//! A run may carry a time zone (`timezone` on `POST /v1/runs`, an IANA name such as
//! `America/Los_Angeles`). The clock in its system prompt is then the user's local one, so
//! "today", "tonight" and "tomorrow" mean what they mean to the user. cedar's own clock is UTC: in
//! Portland at 7:45 PM on a Saturday it is already Sunday in UTC, and a model told only the UTC
//! date answers "tomorrow" with Monday. Without a zone the note says the date is UTC's and that
//! the user's may differ, instead of passing it off as theirs.

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

/// `raw` as a time zone cedar works with: an IANA name (`Europe/Zurich`), in its canonical
/// spelling. `UTC` and `Etc/…` zones are accepted too.
pub fn normalize(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 64 {
        return Err(format!("timezone {raw:?} is not a time zone name such as \"Europe/Zurich\""));
    }
    raw.parse::<Tz>()
        .map(|tz| tz.name().to_string())
        .map_err(|_| format!("timezone {raw:?} is not a time zone name such as \"Europe/Zurich\""))
}

/// The zone named `name`, if it is one (a name saved before is, but a zone can leave the
/// database in a later `chrono-tz`).
fn zone(name: Option<&str>) -> Option<Tz> {
    name.and_then(|n| n.parse().ok())
}

/// The user's date at `now`: in their zone if it is known, else UTC's.
pub fn today(now: DateTime<Utc>, timezone: Option<&str>) -> NaiveDate {
    match zone(timezone) {
        Some(tz) => now.with_timezone(&tz).date_naive(),
        None => now.date_naive(),
    }
}

/// The opening line of cedar's note: the date and time at `now`, the user's if their zone is
/// known.
pub fn date_note(now: DateTime<Utc>, timezone: Option<&str>) -> String {
    const USE: &str = "Use this date, and this year, for anything recent: searches, date filters, \
        \"the last 24 hours\".";
    match zone(timezone) {
        Some(tz) => {
            let local = now.with_timezone(&tz);
            format!(
                "Today is {} and the user's local time is {} ({}, UTC{}); it is {} UTC. This is \
                 the user's date: \"today\", \"tonight\", \"tomorrow\" and weekdays are relative to \
                 it, not to UTC. {USE}",
                local.format("%A, %Y-%m-%d"),
                local.format("%H:%M"),
                tz.name(),
                local.format("%:z"),
                now.format("%H:%M"),
            )
        }
        None => format!(
            "Today is {} and the time is {} UTC. The user's time zone is not known, so their date \
             can be a day before or after this one: when \"today\", \"tonight\", \"tomorrow\" or a \
             weekday matters, work out their local date from where they are (a place they \
             mention, or where you know they live), or ask if you can't tell, and say \
             which date you took. {USE}",
            now.format("%A, %Y-%m-%d"),
            now.format("%H:%M"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn names_are_checked_and_made_canonical() {
        assert_eq!(normalize(" America/Los_Angeles ").unwrap(), "America/Los_Angeles");
        assert_eq!(normalize("UTC").unwrap(), "UTC");
        assert!(normalize("").is_err());
        assert!(normalize("PST8PDTX").is_err());
        assert!(normalize("Mars/Olympus_Mons").is_err());
    }

    #[test]
    fn the_date_is_the_users_not_utcs() {
        // Saturday 7:43 PM in Portland is Sunday 03:43 UTC.
        let now = at("2026-11-22T03:43:00Z");
        assert_eq!(today(now, Some("America/Los_Angeles")).to_string(), "2026-11-21");
        assert_eq!(today(now, None).to_string(), "2026-11-22");
        let note = date_note(now, Some("America/Los_Angeles"));
        assert!(note.starts_with("Today is Saturday, 2026-11-21 and the user's local time is 19:43 (America/Los_Angeles, UTC-08:00); it is 03:43 UTC."), "{note}");
    }

    #[test]
    fn without_a_zone_the_note_says_so() {
        let note = date_note(at("2026-11-22T03:43:00Z"), None);
        assert!(note.starts_with("Today is Sunday, 2026-11-22 and the time is 03:43 UTC."), "{note}");
        assert!(note.contains("time zone is not known"), "{note}");
        // A zone that is no longer known reads as none.
        assert_eq!(date_note(at("2026-11-22T03:43:00Z"), Some("Nowhere/Gone")), note);
    }
}

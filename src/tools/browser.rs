//! `browser`: drive a real web browser, for pages that need JavaScript or interaction.
//!
//! The browser runs in its own container for the run (see [`crate::browser`]); this tool checks
//! the model's request, passes it on, and turns the reply into text. Only the actions below are
//! passed on, and a URL to navigate to must be http(s) and resolve to public addresses, like
//! `fetch_url`'s. Pages the browser reaches by itself (links, redirects, scripts) are not checked
//! here: that is up to the network the browser container runs on.

use super::fetch::check_public_url;
use super::{append_window, Tool, ToolContext};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::time::Duration;

/// How long one call may take. Longer than the driver's own limits (30s to load a page plus a few
/// seconds to settle), so a timeout means the browser is stuck, not that a page is slow.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
/// Runs of at least this many look-alike elements are written as one line (see [`compact`]).
const MIN_RUN: usize = 4;

pub struct BrowserTool {
    allow_private: bool,
}

impl BrowserTool {
    /// `allow_private` lets `navigate` reach loopback and private addresses (`WEB_FETCH_ALLOW_PRIVATE`).
    pub fn new(allow_private: bool) -> Self {
        Self { allow_private }
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Drive a real web browser, for pages that need JavaScript or interaction: search forms, \
         \"show all\" buttons, dialogs, lists that load more as you scroll. The browser keeps its \
         page between calls. Every call returns the page as text, with interactive elements \
         numbered like [12] button \"Show all reviews\"; pass that number as `ref` to click or type. \
         The numbers change whenever the page does, so always use ones from the latest result. \
         When a dialog is open, only the dialog is shown. For plain pages, fetch_url is faster."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "action": { "type": "string",
                                "enum": ["navigate", "snapshot", "click", "type", "scroll", "back"],
                                "description": "navigate: load `url`. snapshot: describe the current page again. click: click element `ref`. type: replace the text in field `ref` with `text`. scroll: scroll the open dialog, or the page (or the area around element `ref`), to load more. back: go to the previous page." },
                    "url": { "type": "string", "description": "navigate: the http or https URL." },
                    "ref": { "type": "integer", "description": "click, type, scroll: an element number from the latest result." },
                    "text": { "type": "string", "description": "type: the text to enter." },
                    "submit": { "type": "boolean", "description": "type: press Enter afterwards (default false)." },
                    "direction": { "type": "string", "enum": ["down", "up"], "description": "scroll: default down." },
                    "start": { "type": "integer", "minimum": 0,
                               "description": "snapshot: character offset to continue a page that was cut off." }
                },
                "required": ["action"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let session = ctx
            .browser
            .as_ref()
            .ok_or_else(|| anyhow!("the browser is not available"))?;
        let (request, start) = request(input)?;
        if request["action"] == "navigate" {
            let url = request["url"].as_str().unwrap_or_default();
            check_public_url(url, self.allow_private).await?;
        }
        let reply = session.call(&request, CALL_TIMEOUT).await?;
        Ok(format_reply(&reply, start, ctx.max_output))
    }
}

/// The request for the driver, holding only what `input`'s action takes, and the `start` offset.
fn request(input: &Value) -> Result<(Value, usize)> {
    let action = input["action"]
        .as_str()
        .ok_or_else(|| anyhow!("missing action"))?;
    let mut req = Map::new();
    req.insert("action".into(), action.into());
    let element = |req: &mut Map<String, Value>| -> Result<()> {
        let r = input["ref"].as_u64().ok_or_else(|| {
            anyhow!("{action} needs `ref`, an element number from the latest result")
        })?;
        req.insert("ref".into(), r.into());
        Ok(())
    };
    match action {
        "navigate" => {
            let url = input["url"]
                .as_str()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .ok_or_else(|| anyhow!("navigate needs `url`"))?;
            req.insert("url".into(), url.into());
        }
        "click" => element(&mut req)?,
        "type" => {
            element(&mut req)?;
            let text = input["text"]
                .as_str()
                .ok_or_else(|| anyhow!("type needs `text`"))?;
            req.insert("text".into(), text.into());
            req.insert(
                "submit".into(),
                input["submit"].as_bool().unwrap_or(false).into(),
            );
        }
        "scroll" => {
            if !input["ref"].is_null() {
                element(&mut req)?;
            }
            let direction = match &input["direction"] {
                Value::Null => "down",
                v => v
                    .as_str()
                    .filter(|d| matches!(*d, "down" | "up"))
                    .ok_or_else(|| anyhow!("direction must be \"down\" or \"up\""))?,
            };
            req.insert("direction".into(), direction.into());
        }
        "snapshot" | "back" => {}
        other => {
            bail!("unknown action {other:?}; use navigate, snapshot, click, type, scroll, or back")
        }
    }
    let start = match &input["start"] {
        Value::Null => 0,
        v => v
            .as_u64()
            .ok_or_else(|| anyhow!("start must be a non-negative integer"))? as usize,
    };
    if start > 0 && action != "snapshot" {
        // Anything else would act on the page again just to read further.
        bail!("start only goes with action \"snapshot\"");
    }
    Ok((Value::Object(req), start))
}

/// The tool result: the page's URL, title, and (unless it is a success) HTTP status, then its
/// text from `start`, cut to `max_bytes`.
fn format_reply(reply: &Value, start: usize, max_bytes: usize) -> String {
    let mut out = format!("URL: {}\n", reply["url"].as_str().unwrap_or_default());
    if let Some(title) = reply["title"].as_str().filter(|t| !t.is_empty()) {
        out.push_str(&format!("Title: {title}\n"));
    }
    if let Some(status) = reply["status"].as_u64().filter(|s| !(200..300).contains(s)) {
        out.push_str(&format!("HTTP status: {status}\n"));
    }
    out.push('\n');
    let text = compact(reply["text"].as_str().unwrap_or_default());
    if text.is_empty() {
        out.push_str("[the page has no text]");
        return out;
    }
    append_window(&mut out, &text, start, max_bytes, |next| {
        format!("call browser with action \"snapshot\" and start={next}")
    });
    out
}

/// One `[12] button "Name" (disabled)` line of a snapshot.
struct RefLine<'a> {
    num: u64,
    role: &'a str,
    name: &'a str,
    /// Everything after the name, like ` (disabled)` or ` -> /path`.
    rest: &'a str,
}

fn parse_ref_line(line: &str) -> Option<RefLine<'_>> {
    let body = line.strip_prefix('[')?;
    let (num, body) = body.split_once("] ")?;
    let num = num.parse().ok()?;
    let (role, body) = body.split_once(" \"")?;
    let (name, rest) = body.rsplit_once('"')?;
    Some(RefLine {
        num,
        role,
        name,
        rest,
    })
}

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// What stays the same across a run of look-alike elements: the name with its numbers and
/// weekdays blanked out, as in a calendar's "13, Tuesday, October 2026. Available".
fn shape(line: &RefLine) -> String {
    let mut name = line.name.to_string();
    for day in WEEKDAYS {
        name = name.replace(day, "~");
    }
    let mut out = String::with_capacity(name.len());
    let mut in_number = false;
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !in_number {
                out.push('#');
            }
            in_number = true;
        } else {
            out.push(c);
            in_number = false;
        }
    }
    format!("{}\u{0}{out}\u{0}{}", line.role, line.rest)
}

/// The first number in a name, which tells look-alike elements apart ("13" in "13, Tuesday, ...").
fn label(name: &str) -> Option<&str> {
    let start = name.find(|c: char| c.is_ascii_digit())?;
    let len = name[start..]
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(name.len() - start);
    Some(&name[start..start + len])
}

/// Shorten runs of look-alike elements with consecutive numbers, such as a calendar's day buttons:
/// the first is kept as it is, the others are listed by number and label only, e.g.
/// `(likewise, only the number differs: [44] 14, [45] 15, ...)`.
fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        let mut end = i + 1;
        if let Some(first) = parse_ref_line(lines[i]).filter(|l| label(l.name).is_some()) {
            let key = shape(&first);
            let mut labels = vec![label(first.name).unwrap_or_default()];
            while let Some(next) = lines.get(end).and_then(|l| parse_ref_line(l)) {
                let next_label = label(next.name).unwrap_or_default();
                if next.num != first.num + (end - i) as u64
                    || shape(&next) != key
                    || labels.contains(&next_label)
                {
                    break;
                }
                labels.push(next_label);
                end += 1;
            }
            if end - i >= MIN_RUN {
                out.push_str(lines[i]);
                out.push_str("\n(likewise, only the number differs: ");
                let others: Vec<String> = (1..end - i)
                    .map(|k| format!("[{}] {}", first.num + k as u64, labels[k]))
                    .collect();
                out.push_str(&others.join(", "));
                out.push_str(")\n");
                i = end;
                continue;
            }
            end = i + 1;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i = end;
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_only_what_their_action_takes() {
        let (req, start) = request(&json!({
            "action": "navigate", "url": " https://example.com/ ", "ref": 3, "text": "x"
        }))
        .unwrap();
        assert_eq!(
            req,
            json!({ "action": "navigate", "url": "https://example.com/" })
        );
        assert_eq!(start, 0);

        let (req, _) =
            request(&json!({ "action": "type", "ref": 3, "text": "Hood River" })).unwrap();
        assert_eq!(
            req,
            json!({ "action": "type", "ref": 3, "text": "Hood River", "submit": false })
        );
        let (req, _) = request(&json!({ "action": "scroll" })).unwrap();
        assert_eq!(req, json!({ "action": "scroll", "direction": "down" }));
        let (req, _) =
            request(&json!({ "action": "scroll", "ref": 7, "direction": "up" })).unwrap();
        assert_eq!(
            req,
            json!({ "action": "scroll", "ref": 7, "direction": "up" })
        );
        let (req, start) = request(&json!({ "action": "snapshot", "start": 500 })).unwrap();
        assert_eq!((req, start), (json!({ "action": "snapshot" }), 500));
    }

    #[test]
    fn bad_requests_are_explained() {
        for (input, expected) in [
            (json!({}), "missing action"),
            (json!({ "action": "evaluate" }), "unknown action"),
            (json!({ "action": "navigate" }), "needs `url`"),
            (json!({ "action": "click" }), "needs `ref`"),
            (json!({ "action": "click", "ref": "12" }), "needs `ref`"),
            (json!({ "action": "click", "ref": -1 }), "needs `ref`"),
            (json!({ "action": "type", "ref": 1 }), "needs `text`"),
            (
                json!({ "action": "scroll", "direction": "left" }),
                "direction",
            ),
            (
                json!({ "action": "click", "ref": 1, "start": 10 }),
                "start only",
            ),
            (
                json!({ "action": "snapshot", "start": -5 }),
                "start must be",
            ),
        ] {
            let err = request(&input).unwrap_err().to_string();
            assert!(err.contains(expected), "{input}: {err}");
        }
    }

    #[tokio::test]
    async fn navigation_is_limited_to_public_http_urls() {
        for url in [
            "file:///etc/passwd",
            "http://127.0.0.1:8080/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://localhost/",
        ] {
            assert!(check_public_url(url, false).await.is_err(), "{url}");
        }
        assert!(check_public_url("http://localhost/", true).await.is_ok());
    }

    #[test]
    fn replies_show_where_the_page_is_and_cut_long_text() {
        let reply = json!({
            "url": "https://x.test/a", "title": "A page", "status": 200,
            "text": "# Hello\n[1] button \"Go\"", "visible_chars": 20,
        });
        assert_eq!(
            format_reply(&reply, 0, 10_000),
            "URL: https://x.test/a\nTitle: A page\n\n# Hello\n[1] button \"Go\""
        );

        let blocked = json!({ "url": "https://x.test/", "title": "", "status": 403, "text": "Access denied" });
        assert_eq!(
            format_reply(&blocked, 0, 10_000),
            "URL: https://x.test/\nHTTP status: 403\n\nAccess denied"
        );

        let long = json!({ "url": "u", "title": "t", "status": null, "text": "x".repeat(1000) });
        let out = format_reply(&long, 0, 200);
        assert!(out.len() < 300, "{}", out.len());
        assert!(
            out.ends_with("call browser with action \"snapshot\" and start=183 to continue]"),
            "{out}"
        );
        let rest = format_reply(&long, 183, 10_000);
        assert_eq!(rest.matches('x').count(), 1000 - 183);

        let empty = json!({ "url": "about:blank", "title": "", "status": null, "text": "" });
        assert!(format_reply(&empty, 0, 100).ends_with("[the page has no text]"));
    }

    #[test]
    fn calendar_days_are_written_once() {
        let mut text = String::from("### October 2026\n");
        for (k, day) in (13..=17).enumerate() {
            let weekday = WEEKDAYS[(k + 1) % 7];
            text.push_str(&format!(
                "[{}] button \"{day}, {weekday}, October 2026. Available. Select as check-in date.\"\n",
                43 + k
            ));
        }
        text.push_str("[48] button \"18, Sunday, October 2026. Unavailable\" (disabled)\n");
        text.push_str("[49] button \"Clear dates\"");
        assert_eq!(
            compact(&text),
            "### October 2026\n\
             [43] button \"13, Tuesday, October 2026. Available. Select as check-in date.\"\n\
             (likewise, only the number differs: [44] 14, [45] 15, [46] 16, [47] 17)\n\
             [48] button \"18, Sunday, October 2026. Unavailable\" (disabled)\n\
             [49] button \"Clear dates\""
        );
    }

    #[test]
    fn only_long_runs_of_numbered_look_alikes_are_shortened() {
        // Too short, different shapes, gaps in the numbering, repeated labels, no numbers at all.
        let unchanged = [
            "[1] button \"Bedroom 1\"\n[2] button \"Bedroom 2\"\n[3] button \"Bedroom 3\"",
            "[1] button \"Comfort 6\"\n[2] button \"Amenities 4\"\n[3] button \"Location 5\"\n[4] button \"Quiet 2\"",
            "[1] link \"Page 1\"\n[2] link \"Page 2\"\n[4] link \"Page 3\"\n[5] link \"Page 4\"",
            "[1] button \"Day 1\"\n[2] button \"Day 1\"\n[3] button \"Day 1\"\n[4] button \"Day 1\"",
            "[1] button \"Show more\"\n[2] button \"Show more\"\n[3] button \"Show more\"\n[4] button \"Show more\"",
            "[1] button \"Day 1\" (disabled)\n[2] button \"Day 2\"\n[3] button \"Day 3\"\n[4] button \"Day 4\" (disabled)",
        ];
        for text in unchanged {
            assert_eq!(compact(text), text);
        }
        assert_eq!(
            compact("intro\n[7] link \"Page 1\" -> /p/1\n[8] link \"Page 2\" -> /p/1\n[9] link \"Page 3\" -> /p/1\n[10] link \"Page 4\" -> /p/1\nend"),
            "intro\n[7] link \"Page 1\" -> /p/1\n(likewise, only the number differs: [8] 2, [9] 3, [10] 4)\nend"
        );
    }
}

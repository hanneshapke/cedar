use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

/// The tool's name. Clients draw its result as a card the user can send as feedback.
pub const NAME: &str = "report_limitation";

const MAX_SUMMARY_CHARS: usize = 300;
const MAX_NEEDED_CHARS: usize = 200;

/// Why something couldn't be done, as the card and the feedback list group it.
pub const CATEGORIES: &[&str] = &["missing_tool", "not_connected", "blocked", "limit", "other"];

/// What the model reads back.
const NOTE: &str = "The user is shown this as a card with a button to send it as feedback. \
    Nothing is recorded unless they press it. Tell them plainly what you couldn't do and what \
    you did instead; don't say feedback was sent.";

/// Notes something the run could not do ("I can't book the table: there is no tool for it").
///
/// The tool records nothing. It checks the report, and the chat front end draws the result as a
/// card with a button; pressing it saves the report on that server for whoever runs it. A page
/// or an email the run read can therefore make the model report something, but never send
/// anything anywhere. The result is `{"limitation": {"summary", "category", "needed"?},
/// "note"}`.
pub struct ReportLimitation;

#[async_trait]
impl Tool for ReportLimitation {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        "Report something the user asked for that you could not do with what you have here: no \
         tool for it, an account that isn't connected, a site or host you were kept from, or a \
         limit you ran into. Use it once you have tried what you can, at most once per gap, and \
         not for things you simply don't know or for a request you declined. Give a one-sentence \
         summary without names, addresses or other personal details (\"Could not book a \
         restaurant table: no booking tool\"), a category, and, if you know, what would make it \
         possible. The user sees it as a card and decides whether to send it as feedback."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "summary": { "type": "string",
                                 "description": "One sentence: what could not be done and why, with no personal details." },
                    "category": { "type": "string", "enum": CATEGORIES,
                                  "description": "missing_tool: no tool does this. not_connected: a service or account isn't set up. blocked: a site, host or policy refused. limit: time, turns, size or context ran out. other: anything else." },
                    "needed": { "type": "string",
                                "description": "What would make it possible, e.g. \"a calendar connection\" or \"access to example.com\"." }
                },
                "required": ["summary", "category"] })
    }

    async fn call(&self, _ctx: &ToolContext, input: &Value) -> Result<String> {
        let limitation = parse_input(input)?;
        Ok(json!({ "limitation": limitation, "note": NOTE }).to_string())
    }
}

/// One line of text, trimmed and with its whitespace collapsed.
fn line(value: &Value, what: &str, max: usize) -> Result<Option<String>> {
    let Some(s) = value.as_str() else {
        if value.is_null() {
            return Ok(None);
        }
        bail!("give {what} as text");
    };
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        return Ok(None);
    }
    if s.chars().count() > max {
        bail!("{what} is longer than {max} characters; say it in one sentence");
    }
    Ok(Some(s))
}

/// The checked report.
fn parse_input(input: &Value) -> Result<Value> {
    let summary = line(&input["summary"], "summary", MAX_SUMMARY_CHARS)?
        .ok_or_else(|| anyhow!("give summary: one sentence on what could not be done"))?;
    let category = match input["category"].as_str().map(str::trim) {
        Some(c) if CATEGORIES.contains(&c) => c,
        Some(c) => bail!("category must be one of {}, not {c:?}", CATEGORIES.join(", ")),
        None => "other",
    };
    let mut out = json!({ "summary": summary, "category": category });
    if let Some(needed) = line(&input["needed"], "needed", MAX_NEEDED_CHARS)? {
        out["needed"] = json!(needed);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_is_checked_and_tidied() {
        let ok = parse_input(&json!({ "summary": "  Could not book\n a table: no booking tool ",
                                      "category": "missing_tool", "needed": " a booking tool " }))
            .unwrap();
        assert_eq!(
            ok,
            json!({ "summary": "Could not book a table: no booking tool", "category": "missing_tool",
                    "needed": "a booking tool" })
        );
        let bare = parse_input(&json!({ "summary": "Ran out of turns" })).unwrap();
        assert_eq!(bare, json!({ "summary": "Ran out of turns", "category": "other" }));

        let err = |input: Value| parse_input(&input).unwrap_err().to_string();
        assert!(err(json!({ "category": "limit" })).contains("give summary"));
        assert!(err(json!({ "summary": " ", "category": "limit" })).contains("give summary"));
        assert!(err(json!({ "summary": "x", "category": "nope" })).contains("category must be"));
        assert!(err(json!({ "summary": "x".repeat(301), "category": "limit" })).contains("longer than 300"));
        assert!(err(json!({ "summary": "x", "category": "limit", "needed": 3 })).contains("needed as text"));
    }
}

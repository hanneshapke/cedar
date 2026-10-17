//! The plan a run commits to before it starts: one tool-free model call that breaks the prompt
//! into a few steps. An interactive run waits for the user to agree to them (edited, reordered,
//! added to or cut); the agreed steps then go into the system prompt for the rest of the run.

use crate::compact::trim_messages;
use crate::models::PlanMode;
use serde::Serialize;
use serde_json::{json, Value};

/// The system prompt for the planning call.
pub const SYSTEM: &str = "You plan work for an AI agent before it starts. The agent has a \
    sandbox for code and files, and these tools: {tools}. You write the plan; you do not carry \
    out the task.";

/// The request that closes the conversation for the planning call.
pub const INSTRUCTION: &str = "Break the latest request into 3 to 7 concrete steps the agent \
    will work through, in order. Each step is one short imperative line (at most 12 words) \
    saying what will be done, e.g. \"Find 2025 market-share figures\". Reply with JSON only: \
    {\"steps\": [\"...\", \"...\"]}";

/// Prompts with fewer words than this get no plan in `auto` mode: they are questions, not tasks.
pub const AUTO_MIN_WORDS: usize = 15;
/// What a client may send back.
pub const MAX_STEPS: usize = 12;
pub const MAX_STEP_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub id: String,
    pub title: String,
}

/// Whether a run in `mode` plans before answering `prompt`.
pub fn wanted(mode: PlanMode, prompt: &str) -> bool {
    match mode {
        PlanMode::Off => false,
        PlanMode::On => true,
        PlanMode::Auto => prompt.split_whitespace().count() >= AUTO_MIN_WORDS,
    }
}

/// The system prompt for the planning call, naming the run's tools.
pub fn system(tool_names: &[&str]) -> String {
    SYSTEM.replace("{tools}", &tool_names.join(", "))
}

/// The planning call's messages: the run's history (trimmed as for compaction) and the prompt
/// with the instruction.
pub fn messages(history: &[Value], prompt: &str) -> Vec<Value> {
    let mut messages = trim_messages(history.to_vec(), 500, 20_000);
    messages.push(json!({ "role": "user", "content": format!("{prompt}\n\n---\n{INSTRUCTION}") }));
    messages
}

/// The steps in the model's reply: `{"steps": [...]}` or a bare array, possibly in a code fence,
/// of strings or `{title}` objects; failing that, a numbered list. `None` for fewer than two.
pub fn parse_reply(text: &str) -> Option<Vec<Step>> {
    let json_part = |open: char, close: char| {
        let start = text.find(open)?;
        let end = text.rfind(close)?;
        (end > start)
            .then(|| serde_json::from_str::<Value>(&text[start..=end]).ok())
            .flatten()
    };
    let titles: Vec<String> = match json_part('{', '}')
        .map(|v| v["steps"].clone())
        .filter(Value::is_array)
        .or_else(|| json_part('[', ']'))
    {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|s| match s {
                Value::String(t) => Some(t.clone()),
                other => other["title"].as_str().map(String::from),
            })
            .collect(),
        _ => text
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                let (num, rest) = l.split_once(['.', ')'])?;
                (!num.is_empty() && num.chars().all(|c| c.is_ascii_digit()))
                    .then(|| rest.trim().to_string())
            })
            .collect(),
    };
    let steps: Vec<Step> = titles
        .into_iter()
        .map(|t| t.trim().trim_matches('"').trim().to_string())
        .filter(|t| !t.is_empty())
        .take(7)
        .enumerate()
        .map(|(i, title)| Step {
            id: (i + 1).to_string(),
            title: title.chars().take(MAX_STEP_CHARS).collect(),
        })
        .collect();
    (steps.len() >= 2).then_some(steps)
}

/// The steps a client sent back (`{"steps": [{id, title}, ...]}`; plain strings are fine too).
pub fn parse_answer(body: &Value) -> Result<Vec<Step>, String> {
    let items = body["steps"]
        .as_array()
        .ok_or("steps must be an array of {id, title}")?;
    if items.is_empty() || items.len() > MAX_STEPS {
        return Err(format!("give between 1 and {MAX_STEPS} steps"));
    }
    let mut steps = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let (id, title) = match item {
            Value::String(t) => (None, t.as_str()),
            Value::Object(_) => (
                item["id"]
                    .as_str()
                    .map(String::from)
                    .or_else(|| item["id"].as_u64().map(|n| n.to_string())),
                item["title"].as_str().unwrap_or_default(),
            ),
            _ => return Err(format!("step {}: expected {{id, title}}", i + 1)),
        };
        let title = title.trim();
        if title.is_empty() {
            return Err(format!("step {}: title must not be empty", i + 1));
        }
        if title.chars().count() > MAX_STEP_CHARS || title.chars().any(char::is_control) {
            return Err(format!(
                "step {}: title must be one line of at most {MAX_STEP_CHARS} characters",
                i + 1
            ));
        }
        let id = id
            .filter(|id| !id.is_empty() && id.len() <= 40)
            .unwrap_or_else(|| (i + 1).to_string());
        steps.push(Step {
            id,
            title: title.to_string(),
        });
    }
    Ok(steps)
}

/// What the system prompt gets for the rest of the run.
pub fn note(steps: &[Step]) -> String {
    let list: Vec<String> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s.title))
        .collect();
    format!(
        "Before you started, this plan was agreed with the user. Work through it in order; if a \
         step turns out to be impossible or unnecessary, say so briefly and go on.\n{}",
        list.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_plans_only_for_longer_prompts() {
        assert!(!wanted(PlanMode::Auto, "what's 2 + 2?"));
        assert!(wanted(
            PlanMode::Auto,
            "compare the three largest cloud providers by 2025 market share and write a short \
             report with a chart and sources"
        ));
        assert!(wanted(PlanMode::On, "hi"));
        assert!(!wanted(PlanMode::Off, &"word ".repeat(50)));
    }

    #[test]
    fn replies_are_parsed_leniently() {
        let titles =
            |t: &str| parse_reply(t).map(|s| s.into_iter().map(|s| s.title).collect::<Vec<_>>());
        assert_eq!(
            titles(r#"{"steps": ["Find data", "Draw chart"]}"#).unwrap(),
            ["Find data", "Draw chart"]
        );
        assert_eq!(
            titles("```json\n{\"steps\": [{\"title\": \"A\"}, {\"title\": \"B\"}]}\n```").unwrap(),
            ["A", "B"]
        );
        assert_eq!(titles(r#"["A", "B", "C"]"#).unwrap(), ["A", "B", "C"]);
        assert_eq!(
            titles("Plan:\n1. Search\n2) Read\n3. Write").unwrap(),
            ["Search", "Read", "Write"]
        );
        assert_eq!(
            titles(&format!("[{}]", ["\"x\""; 9].join(",")))
                .unwrap()
                .len(),
            7
        );
        assert!(titles(r#"{"steps": ["only one"]}"#).is_none());
        assert!(titles("I can't plan that.").is_none());
        let ids: Vec<String> = parse_reply(r#"["A","B"]"#)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["1", "2"]);
    }

    #[test]
    fn answers_are_validated() {
        let steps = parse_answer(
            &json!({ "steps": [{ "id": "1", "title": " A " }, "B", { "id": 7, "title": "C" }] }),
        )
        .unwrap();
        assert_eq!(
            steps,
            [
                Step {
                    id: "1".into(),
                    title: "A".into()
                },
                Step {
                    id: "2".into(),
                    title: "B".into()
                },
                Step {
                    id: "7".into(),
                    title: "C".into()
                },
            ]
        );
        assert!(parse_answer(&json!({ "steps": [] })).is_err());
        assert!(parse_answer(&json!({ "steps": [{ "title": "" }] })).is_err());
        assert!(parse_answer(&json!({ "steps": ["a\nb"] })).is_err());
        assert!(parse_answer(&json!({})).is_err());
    }

    #[test]
    fn the_note_lists_the_steps_in_order() {
        let note = note(&[
            Step {
                id: "9".into(),
                title: "A".into(),
            },
            Step {
                id: "2".into(),
                title: "B".into(),
            },
        ]);
        assert!(note.ends_with("\n1. A\n2. B"));
    }
}

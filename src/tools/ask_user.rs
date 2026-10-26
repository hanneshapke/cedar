use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Map, Value};

/// The tool's name; the agent loop ends a run after a successful call (see [`crate::agent`]).
pub const NAME: &str = "ask_user";

const MAX_QUESTIONS: usize = 4;
const MAX_OPTIONS: usize = 8;
const MAX_QUESTION_CHARS: usize = 500;
const MAX_HEADER_CHARS: usize = 40;
const MAX_LABEL_CHARS: usize = 100;
const MAX_DESCRIPTION_CHARS: usize = 200;

/// What the model reads back: the questions are out, and the answer comes as the next message.
const NOTE: &str = "The questions are shown to the user with a form to answer them. This run \
    ends here; the user's answer arrives as the next message. Do not answer the questions yourself.";

/// Asks the user one or more clarifying questions, drawn by the chat front end as a form from this
/// tool's result.
///
/// Each question offers choices as radio buttons (one answer) or checkboxes (`multi_select`), and
/// the form always adds a free-form answer. The tool itself only checks and tidies the questions;
/// a successful call ends the run (with `stop_reason: ask_user`) so the user can reply, and the
/// reply starts the next run of the conversation. The result is `{"questions": [{"question",
/// "header"?, "multi_select", "options": [{"label", "description"?}]}], "note"}`.
pub struct AskUser;

#[async_trait]
impl Tool for AskUser {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        "Ask the user clarifying questions when their request is ambiguous and the answer would \
         change what you do. Each question can offer up to 8 choices, shown as radio buttons, or as \
         checkboxes with multi_select when several may apply; the user can always answer in their \
         own words instead, so don't add an \"Other\" choice. Ask up to 4 questions in one call. \
         The run ends after this call, and the answers come back as the user's next message. Don't \
         ask about what you can find out yourself or reasonably assume, and don't use it to show a \
         plan or to ask whether to go ahead with one: ask about the choice itself."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "questions": {
                        "type": "array", "minItems": 1, "maxItems": MAX_QUESTIONS,
                        "description": "The questions, in the order they are shown.",
                        "items": { "type": "object",
                                   "properties": {
                                       "question": { "type": "string",
                                                     "description": "The full question, ending with a question mark." },
                                       "header": { "type": "string",
                                                   "description": "A short label for the question, e.g. \"Format\" (at most 40 characters)." },
                                       "options": {
                                           "type": "array", "maxItems": MAX_OPTIONS,
                                           "description": "The likely answers, 2 to 8 of them; leave out for an open question.",
                                           "items": { "type": "object",
                                                      "properties": {
                                                          "label": { "type": "string", "description": "The choice, in a few words." },
                                                          "description": { "type": "string", "description": "What choosing it means." }
                                                      },
                                                      "required": ["label"] }
                                       },
                                       "multi_select": { "type": "boolean",
                                                         "description": "Let the user pick several options (checkboxes) instead of one (radio buttons). Default false." }
                                   },
                                   "required": ["question"] }
                    }
                },
                "required": ["questions"] })
    }

    async fn call(&self, _ctx: &ToolContext, input: &Value) -> Result<String> {
        let questions = parse_input(input)?;
        Ok(json!({ "questions": questions, "note": NOTE }).to_string())
    }
}

/// The questions of a successful call's result as plain text, for the run's final text: clients
/// that don't draw the form still see what was asked.
pub fn as_text(output: &str) -> Option<String> {
    let result: Value = serde_json::from_str(output).ok()?;
    let questions = result["questions"].as_array()?;
    let mut lines = Vec::new();
    for (i, q) in questions.iter().enumerate() {
        let question = q["question"].as_str()?;
        lines.push(if questions.len() > 1 {
            format!("{}. {question}", i + 1)
        } else {
            question.to_string()
        });
        for option in q["options"].as_array().into_iter().flatten() {
            let label = option["label"].as_str().unwrap_or_default();
            lines.push(match option["description"].as_str() {
                Some(d) => format!("   - {label}: {d}"),
                None => format!("   - {label}"),
            });
        }
    }
    Some(lines.join("\n"))
}

/// `value` as trimmed text cut to `max` characters, or `None` if it is missing or blank.
fn text(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(max).collect())
}

fn parse_input(input: &Value) -> Result<Vec<Value>> {
    let list = input["questions"]
        .as_array()
        .filter(|q| !q.is_empty())
        .ok_or_else(|| anyhow!("give questions as a list with at least one question"))?;
    if list.len() > MAX_QUESTIONS {
        bail!(
            "ask at most {MAX_QUESTIONS} questions at once, got {}",
            list.len()
        );
    }
    list.iter()
        .enumerate()
        .map(|(i, q)| parse_question(q).with_context(|| format!("question {}", i + 1)))
        .collect()
}

fn parse_question(q: &Value) -> Result<Value> {
    if !q.is_object() {
        bail!("must be an object with a question");
    }
    let question =
        text(&q["question"], MAX_QUESTION_CHARS).ok_or_else(|| anyhow!("the question is empty"))?;
    let multi_select = match &q["multi_select"] {
        Value::Null => false,
        Value::Bool(b) => *b,
        _ => bail!("multi_select must be true or false"),
    };
    let mut options: Vec<Value> = Vec::new();
    let raw = match &q["options"] {
        Value::Null => &[][..],
        Value::Array(a) => a.as_slice(),
        _ => bail!("options must be a list"),
    };
    for (i, option) in raw.iter().enumerate() {
        // A bare string is taken as the label: models write lists of choices that way.
        let (label, description) = match option {
            Value::String(_) => (text(option, MAX_LABEL_CHARS), None),
            Value::Object(_) => (
                text(&option["label"], MAX_LABEL_CHARS),
                text(&option["description"], MAX_DESCRIPTION_CHARS),
            ),
            _ => bail!("option {} must be an object with a label", i + 1),
        };
        let label = label.ok_or_else(|| anyhow!("option {} has no label", i + 1))?;
        // The form always offers a free-form answer, so an "Other" choice would only repeat it.
        if is_other(&label) {
            continue;
        }
        if options.iter().any(|o| {
            o["label"]
                .as_str()
                .is_some_and(|l| l.eq_ignore_ascii_case(&label))
        }) {
            bail!("option {:?} is listed twice", label);
        }
        let mut o = Map::new();
        o.insert("label".into(), json!(label));
        if let Some(d) = description {
            o.insert("description".into(), json!(d));
        }
        options.push(Value::Object(o));
    }
    if options.len() == 1 {
        bail!("give at least 2 options, or none for an open question");
    }
    if options.len() > MAX_OPTIONS {
        bail!("give at most {MAX_OPTIONS} options, got {}", options.len());
    }
    let mut out = Map::new();
    out.insert("question".into(), json!(question));
    if let Some(header) = text(&q["header"], MAX_HEADER_CHARS) {
        out.insert("header".into(), json!(header));
    }
    out.insert(
        "multi_select".into(),
        json!(multi_select && !options.is_empty()),
    );
    out.insert("options".into(), Value::Array(options));
    Ok(Value::Object(out))
}

fn is_other(label: &str) -> bool {
    let l = label
        .trim_end_matches(['.', '…', ':'])
        .trim()
        .to_lowercase();
    matches!(
        l.as_str(),
        "other" | "something else" | "other (please specify)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_are_tidied() {
        let input = json!({ "questions": [
            { "question": "  Which format?  ", "header": " Format ",
              "options": [{ "label": "PDF", "description": "A printable file" }, "Markdown", { "label": "Other" }] },
            { "question": "Which sections?", "multi_select": true,
              "options": [{ "label": "Intro" }, { "label": "Results", "description": " " }] },
            { "question": "Anything else?", "multi_select": true },
        ] });
        assert_eq!(
            parse_input(&input).unwrap(),
            [
                json!({ "question": "Which format?", "header": "Format", "multi_select": false,
                        "options": [{ "label": "PDF", "description": "A printable file" }, { "label": "Markdown" }] }),
                json!({ "question": "Which sections?", "multi_select": true,
                        "options": [{ "label": "Intro" }, { "label": "Results" }] }),
                // Nothing to tick, so it is an open question whatever multi_select said.
                json!({ "question": "Anything else?", "multi_select": false, "options": [] }),
            ]
        );
    }

    #[test]
    fn long_text_is_cut() {
        let input = json!({ "questions": [{ "question": "q".repeat(900), "header": "h".repeat(90),
                                            "options": ["a".repeat(300), "b"] }] });
        let q = &parse_input(&input).unwrap()[0];
        assert_eq!(q["question"].as_str().unwrap().chars().count(), 500);
        assert_eq!(q["header"].as_str().unwrap().chars().count(), 40);
        assert_eq!(
            q["options"][0]["label"].as_str().unwrap().chars().count(),
            100
        );
    }

    #[test]
    fn bad_input_is_rejected_with_the_question_number() {
        let err = |input: Value| format!("{:#}", parse_input(&input).unwrap_err());
        assert!(err(json!({})).contains("at least one question"));
        assert!(err(json!({ "questions": [] })).contains("at least one question"));
        let five: Vec<Value> = (0..5).map(|_| json!({ "question": "?" })).collect();
        assert!(err(json!({ "questions": five })).contains("at most 4 questions at once, got 5"));
        assert_eq!(
            err(json!({ "questions": [{ "question": "a?" }, { "question": " " }] })),
            "question 2: the question is empty"
        );
        assert_eq!(
            err(json!({ "questions": ["a?"] })),
            "question 1: must be an object with a question"
        );
        assert!(
            err(json!({ "questions": [{ "question": "a?", "options": ["x"] }] }))
                .contains("at least 2 options")
        );
        assert!(
            err(json!({ "questions": [{ "question": "a?", "options": ["x", "Other"] }] }))
                .contains("at least 2 options")
        );
        assert!(
            err(json!({ "questions": [{ "question": "a?", "options": ["x", "X"] }] }))
                .contains("\"X\" is listed twice")
        );
        assert!(
            err(json!({ "questions": [{ "question": "a?", "options": ["x", {}] }] }))
                .contains("option 2 has no label")
        );
        assert!(
            err(json!({ "questions": [{ "question": "a?", "multi_select": "yes" }] }))
                .contains("multi_select must be true or false")
        );
        let nine: Vec<String> = (0..9).map(|i| format!("o{i}")).collect();
        assert!(
            err(json!({ "questions": [{ "question": "a?", "options": nine }] }))
                .contains("at most 8 options, got 9")
        );
    }

    #[test]
    fn the_result_reads_as_text() {
        let one = json!({ "questions": [{ "question": "Which?", "multi_select": false,
                                          "options": [{ "label": "A", "description": "first" }, { "label": "B" }] }] });
        assert_eq!(
            as_text(&one.to_string()).unwrap(),
            "Which?\n   - A: first\n   - B"
        );
        let two = json!({ "questions": [{ "question": "One?", "options": [] }, { "question": "Two?", "options": [] }] });
        assert_eq!(as_text(&two.to_string()).unwrap(), "1. One?\n2. Two?");
        assert_eq!(as_text("not json"), None);
    }
}

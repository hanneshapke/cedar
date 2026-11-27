//! Google Tasks through the REST API: the user's task lists, the tasks on one, and adding a task.
//! Like the other Google tools they run in the cedar process: the OAuth tokens stay here, and the
//! sandbox has no network.
//!
//! Reading, and `create_task`; nothing here completes, changes, or deletes a task.
//!
//! A task's due date is a day, not a time: Google keeps only the date, so the tools take and give
//! dates.

use super::google::{page_size, required_str, select, str_arg, without_empty, GoogleApi};
use super::{truncate_output, Tool, ToolContext};
use crate::config::GoogleApiConfig;
use crate::mcp::GoogleOAuth;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::NaiveDate;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// The tools implemented here, in the order they are offered.
const TOOLS: &[&str] = &["list_task_lists", "list_tasks", "create_task"];
/// The list Google gives every account, which the tools use unless told otherwise.
const DEFAULT_LIST: &str = "@default";

/// The Tasks tools `cfg` allows, sharing one API client.
pub fn tools(cfg: &GoogleApiConfig, auth: Arc<GoogleOAuth>) -> Result<Vec<Box<dyn Tool>>> {
    let api = Arc::new(GoogleApi::new(&cfg.url, auth, "Tasks")?);
    Ok(select(&cfg.prefix, &cfg.tools, TOOLS)
        .into_iter()
        .map(|op| {
            Box::new(TasksTool {
                name: format!("{}_{op}", cfg.prefix),
                op,
                api: api.clone(),
            }) as Box<dyn Tool>
        })
        .collect())
}

/// One Tasks operation; `op` is one of [`TOOLS`].
pub struct TasksTool {
    name: String,
    op: &'static str,
    api: Arc<GoogleApi>,
}

#[async_trait]
impl Tool for TasksTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn pollable(&self) -> bool {
        matches!(self.op, "list_task_lists" | "list_tasks")
    }

    fn description(&self) -> &str {
        match self.op {
            "list_task_lists" => {
                "List the user's Google Tasks lists with their IDs. Most people have one; the \
                 other tools use the default list unless given a taskListId."
            }
            "list_tasks" => {
                "List the tasks on one of the user's Google Tasks lists (their to-dos): title, \
                 notes, due date and whether it is done. Open tasks only unless showCompleted \
                 is true. A subtask names its parent."
            }
            "create_task" => {
                "Add a task to one of the user's Google Tasks lists, with optional notes and a \
                 due date. It is created once per call, so don't repeat a call that succeeded."
            }
            _ => unreachable!("not a Tasks tool: {}", self.op),
        }
    }

    fn schema(&self) -> Value {
        let list = json!({ "type": "string",
            "description": "The task list's ID, from list_task_lists (default: the user's default list)." });
        let date = |what: &str| {
            json!({ "type": "string", "description": format!("{what}, as a date (e.g. 2026-04-30).") })
        };
        let page_token = json!({ "type": "string",
            "description": "The nextPageToken of a previous call, to get the next page." });
        match self.op {
            "list_task_lists" => json!({ "type": "object", "properties": {
                "pageToken": page_token,
            } }),
            "list_tasks" => json!({ "type": "object", "properties": {
                "taskListId": list,
                "showCompleted": { "type": "boolean", "description": "Include tasks that are done (default false)." },
                "dueAfter": date("Only tasks due on or after this day"),
                "dueBefore": date("Only tasks due on or before this day"),
                "pageSize": { "type": "integer", "minimum": 1, "maximum": 100,
                    "description": "How many to return (default 50, at most 100)." },
                "pageToken": page_token,
            } }),
            "create_task" => json!({ "type": "object", "properties": {
                "title": { "type": "string", "description": "What is to be done." },
                "notes": { "type": "string", "description": "Details for the task." },
                "due": date("The day it is due; tasks have no time of day"),
                "taskListId": list,
            }, "required": ["title"] }),
            _ => unreachable!("not a Tasks tool: {}", self.op),
        }
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let t = ctx.timeout;
        let out = match self.op {
            "list_task_lists" => self.list_task_lists(input, t).await?,
            "list_tasks" => self.list_tasks(input, t).await?,
            "create_task" => {
                let list = str_arg(input, "taskListId").unwrap_or(DEFAULT_LIST);
                let task = self
                    .api
                    .post(&["lists", list, "tasks"], &new_task(input)?, t)
                    .await?;
                json!({ "task": render_task(&task), "note": "The task is on the list." })
            }
            _ => unreachable!("not a Tasks tool: {}", self.op),
        };
        Ok(truncate_output(
            &serde_json::to_string(&without_empty(out))?,
            ctx.max_output,
        ))
    }
}

impl TasksTool {
    async fn list_task_lists(&self, input: &Value, t: Duration) -> Result<Value> {
        let mut query = vec![("maxResults", "100".to_string())];
        if let Some(token) = str_arg(input, "pageToken") {
            query.push(("pageToken", token.to_string()));
        }
        let reply = self.api.get(&["users", "@me", "lists"], &query, t).await?;
        let lists: Vec<Value> = items(&reply)
            .map(|l| json!({ "id": l["id"], "title": l["title"], "updated": l["updated"] }))
            .collect();
        Ok(json!({ "taskLists": lists, "nextPageToken": reply["nextPageToken"] }))
    }

    async fn list_tasks(&self, input: &Value, t: Duration) -> Result<Value> {
        let list = str_arg(input, "taskListId").unwrap_or(DEFAULT_LIST);
        let done = input["showCompleted"].as_bool() == Some(true);
        let mut query = vec![
            ("maxResults", page_size(input, 50, 100).to_string()),
            ("showCompleted", done.to_string()),
            // Tasks ticked off in Google's own apps are hidden, not just completed.
            ("showHidden", done.to_string()),
        ];
        if let Some(day) = str_arg(input, "dueAfter") {
            query.push(("dueMin", due_time(parse_day(day, "dueAfter")?)));
        }
        if let Some(day) = str_arg(input, "dueBefore") {
            // dueMax is the first moment not included: the day after.
            let next = parse_day(day, "dueBefore")?
                .succ_opt()
                .ok_or_else(|| anyhow!("dueBefore is out of range"))?;
            query.push(("dueMax", due_time(next)));
        }
        if let Some(token) = str_arg(input, "pageToken") {
            query.push(("pageToken", token.to_string()));
        }
        let reply = self.api.get(&["lists", list, "tasks"], &query, t).await?;
        let tasks: Vec<Value> = items(&reply).map(render_task).collect();
        Ok(json!({ "taskListId": list, "tasks": tasks, "nextPageToken": reply["nextPageToken"] }))
    }
}

fn items(v: &Value) -> impl Iterator<Item = &Value> {
    v["items"].as_array().into_iter().flatten()
}

/// The body of the `tasks.insert` call that create_task's `input` asks for.
fn new_task(input: &Value) -> Result<Value> {
    let mut body = json!({ "title": required_str(input, "title")? });
    if let Some(notes) = str_arg(input, "notes") {
        body["notes"] = json!(notes);
    }
    if let Some(day) = str_arg(input, "due") {
        body["due"] = json!(due_time(parse_day(day, "due")?));
    }
    Ok(body)
}

/// A day given as a date, or as a time whose date is taken (Tasks has no time of day).
fn parse_day(s: &str, key: &str) -> Result<NaiveDate> {
    s.get(..10)
        .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
        .ok_or_else(|| anyhow!("{key}: invalid date {s:?}: use e.g. 2026-04-30"))
}

/// A day as Google Tasks writes one: midnight UTC, of which it keeps the date.
fn due_time(day: NaiveDate) -> String {
    format!("{day}T00:00:00.000Z")
}

/// A task as the model sees it.
fn render_task(t: &Value) -> Value {
    json!({
        "id": t["id"],
        "title": t["title"],
        "notes": t["notes"],
        "due": t["due"].as_str().and_then(|d| d.get(..10)),
        "done": (t["status"] == "completed").then_some(true),
        "completed": t["completed"],
        "parentTaskId": t["parent"],
        "link": t["webViewLink"],
        "updated": t["updated"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_task_has_a_day_not_a_time() {
        assert_eq!(
            new_task(&json!({ "title": " Renew passport ", "notes": "Form DS-82", "due": "2026-12-01" })).unwrap(),
            json!({ "title": "Renew passport", "notes": "Form DS-82", "due": "2026-12-01T00:00:00.000Z" })
        );
        // A time is taken by its date.
        assert_eq!(
            new_task(&json!({ "title": "x", "due": "2026-12-01T17:30:00-08:00" })).unwrap()["due"],
            "2026-12-01T00:00:00.000Z"
        );
        assert_eq!(new_task(&json!({ "title": "x" })).unwrap(), json!({ "title": "x" }));
        assert!(new_task(&json!({ "notes": "x" })).unwrap_err().to_string().contains("title is required"));
        let err = new_task(&json!({ "title": "x", "due": "friday" })).unwrap_err().to_string();
        assert!(err.contains("due: invalid date"), "{err}");
    }

    #[test]
    fn tasks_are_summarized() {
        let task = without_empty(render_task(&json!({
            "id": "t1", "title": "Call Ana", "status": "needsAction", "notes": "",
            "due": "2026-12-01T00:00:00.000Z", "parent": "t0",
            "webViewLink": "https://tasks.google.com/task/t1",
        })));
        assert_eq!(
            task,
            json!({ "id": "t1", "title": "Call Ana", "due": "2026-12-01", "parentTaskId": "t0",
                "link": "https://tasks.google.com/task/t1" })
        );
        let done = render_task(&json!({ "id": "t2", "status": "completed", "completed": "2026-11-20T10:00:00.000Z" }));
        assert_eq!(done["done"], true);
    }
}

//! The agent loop: call the model, run the tools it asks for, feed the results back, and stop
//! when the model stops asking for tools.

use crate::browser::BrowserSession;
use crate::events::Emitter;
use crate::llm::{LlmClient, MessageRequest};
use crate::models::{event, Run};
use crate::sandbox::{Sandbox, SandboxFatal};
use crate::store::Store;
use crate::tools::{ToolContext, ToolSet, ToolView};
use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// What every run shares.
pub struct AgentEnv {
    pub llm: Arc<dyn LlmClient>,
    /// The current tools. Swapped when an admin changes an integration; a run keeps the set it
    /// started with.
    pub tools: Arc<ArcSwap<ToolSet>>,
    pub store: Store,
    pub max_tokens: u32,
    pub tool_timeout: Duration,
    pub tool_output_max_bytes: usize,
    /// The sandbox's Docker network mode, for the note on what the sandbox can reach.
    pub sandbox_network: String,
}

#[derive(Debug)]
pub struct Completion {
    /// The text of the model's final turn.
    pub final_text: String,
    pub stop_reason: Option<String>,
}

#[derive(Debug)]
pub enum AgentError {
    Cancelled,
    Failed(String),
}

impl From<anyhow::Error> for AgentError {
    fn from(e: anyhow::Error) -> Self {
        AgentError::Failed(format!("{e:#}"))
    }
}

/// Run `run` to completion. Persists the conversation and emits events as it goes; the caller owns
/// the run's status, the sandbox's and browser's lifetimes, and the terminal event.
pub async fn run_agent(
    env: &AgentEnv,
    run: &Run,
    sandbox: Arc<dyn Sandbox>,
    browser: Option<Arc<BrowserSession>>,
    events: &Emitter,
    cancel: &CancellationToken,
) -> Result<Completion, AgentError> {
    let ctx = ToolContext {
        sandbox,
        browser,
        timeout: env.tool_timeout,
        max_output: env.tool_output_max_bytes,
    };
    let tool_set = env.tools.load_full();
    // Picks up MCP servers that weren't reachable (or not tried) when earlier runs started.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        _ = tool_set.connect_remote(env.tool_timeout) => {}
    }
    let tools = tool_set.select(run.tools.as_deref());
    let tool_defs = tools.definitions();
    let system = with_note(&run.system, &tools.environment_note(&env.sandbox_network));
    // A continuation starts with a copy of its parent's conversation (made when it was created).
    let mut messages: Vec<Value> = env
        .store
        .list_messages(run.id)
        .await?
        .into_iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    let workspace_lost = !messages.is_empty() && !run.workspace_reused;
    let opening = opening_message(&messages, &run.prompt, workspace_lost);
    push_message(env, run, &mut messages, "user", opening).await?;

    for turn in 1..=run.max_turns {
        let req = MessageRequest {
            model: &run.model,
            max_tokens: env.max_tokens,
            system: &system,
            tools: &tool_defs,
            messages: &messages,
        };
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = env.llm.create_message(&req) => r?,
        };

        env.store.record_turn(run.id, turn, resp.usage).await?;
        events
            .emit(
                event::USAGE,
                json!({ "turn": turn, "input_tokens": resp.usage.input_tokens, "output_tokens": resp.usage.output_tokens }),
            )
            .await?;
        push_message(
            env,
            run,
            &mut messages,
            "assistant",
            Value::Array(resp.content.clone()),
        )
        .await?;

        let mut final_text = String::new();
        let mut tool_calls = Vec::new();
        for block in &resp.content {
            match block["type"].as_str() {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or_default();
                    final_text.push_str(text);
                    if !text.is_empty() {
                        events
                            .emit(event::ASSISTANT_TEXT, json!({ "turn": turn, "text": text }))
                            .await?;
                    }
                }
                Some("tool_use") => tool_calls.push(block),
                _ => {}
            }
        }

        match resp.stop_reason.as_deref() {
            Some("tool_use") if !tool_calls.is_empty() => {
                let mut results = Vec::with_capacity(tool_calls.len());
                for block in tool_calls {
                    results.push(execute_tool_call(&tools, &ctx, events, cancel, block).await?);
                }
                push_message(env, run, &mut messages, "user", Value::Array(results)).await?;
            }
            other => {
                return Ok(Completion {
                    final_text,
                    stop_reason: other.map(String::from),
                })
            }
        }
    }
    Err(AgentError::Failed(format!(
        "run did not finish within {} turns",
        run.max_turns
    )))
}

/// The run's system prompt followed by the environment note. The note is added per request, not
/// stored, so it follows the server's current configuration.
fn with_note(system: &str, note: &str) -> String {
    match (system.trim().is_empty(), note.is_empty()) {
        (_, true) => system.to_string(),
        (true, false) => note.to_string(),
        (false, false) => format!("{system}\n\n{note}"),
    }
}

const INTERRUPTED: &str = "The run ended before this tool call finished, so it has no result.";
const WORKSPACE_LOST: &str =
    "(Note: the sandbox workspace from earlier in this conversation has expired. \
    Files created before this message no longer exist.)";

/// The user message that starts a run: the prompt, after anything needed to make the inherited
/// history valid. If the parent stopped between asking for tools and getting their results, those
/// `tool_use` blocks still need matching `tool_result` blocks before the conversation can go on.
pub fn opening_message(history: &[Value], prompt: &str, workspace_lost: bool) -> Value {
    let text = if workspace_lost {
        format!("{WORKSPACE_LOST}\n\n{prompt}")
    } else {
        prompt.to_string()
    };
    let unanswered: Vec<&Value> = match history.last() {
        Some(last) if last["role"] == "assistant" => last["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b["type"] == "tool_use")
                    .map(|b| &b["id"])
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if unanswered.is_empty() {
        return Value::String(text);
    }
    let mut blocks: Vec<Value> = unanswered
        .into_iter()
        .map(|id| json!({ "type": "tool_result", "tool_use_id": id, "content": INTERRUPTED, "is_error": true }))
        .collect();
    blocks.push(json!({ "type": "text", "text": text }));
    Value::Array(blocks)
}

/// Execute one `tool_use` block and return the matching `tool_result` block.
async fn execute_tool_call(
    tools: &ToolView<'_>,
    ctx: &ToolContext,
    events: &Emitter,
    cancel: &CancellationToken,
    block: &Value,
) -> Result<Value, AgentError> {
    let id = block["id"].as_str().unwrap_or_default();
    let name = block["name"].as_str().unwrap_or_default();
    let input = &block["input"];
    events
        .emit(
            event::TOOL_CALL,
            json!({ "id": id, "name": name, "input": input }),
        )
        .await?;

    let started = Instant::now();
    let call = async {
        // A tool left out of the run's selection is as unknown as a made-up one.
        match tools.get(name) {
            Some(tool) => tool.call(ctx, input).await,
            None => Err(anyhow!("unknown tool: {name}")),
        }
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        r = call => r,
    };

    let (output, is_error) = match result {
        Ok(out) => (out, false),
        Err(e) if e.downcast_ref::<SandboxFatal>().is_some() => {
            return Err(AgentError::Failed(e.to_string()))
        }
        // Errors go back to the model so it can recover.
        Err(e) => (format!("{e:#}"), true),
    };
    events
        .emit(
            event::TOOL_RESULT,
            json!({ "id": id, "output": output, "is_error": is_error, "duration_ms": started.elapsed().as_millis() as u64 }),
        )
        .await?;

    let mut block = json!({ "type": "tool_result", "tool_use_id": id, "content": output });
    if is_error {
        block["is_error"] = Value::Bool(true);
    }
    Ok(block)
}

/// Append to the in-memory history sent to the API and to the stored copy.
async fn push_message(
    env: &AgentEnv,
    run: &Run,
    history: &mut Vec<Value>,
    role: &str,
    content: Value,
) -> Result<()> {
    env.store.append_message(run.id, role, &content).await?;
    history.push(json!({ "role": role, "content": content }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_note_follows_the_system_prompt() {
        assert_eq!(with_note("be brief", "offline"), "be brief\n\noffline");
        assert_eq!(with_note("", "offline"), "offline");
        assert_eq!(with_note("be brief", ""), "be brief");
    }

    #[test]
    fn a_fresh_run_opens_with_the_plain_prompt() {
        assert_eq!(opening_message(&[], "hi", false), json!("hi"));
    }

    #[test]
    fn a_lost_workspace_is_mentioned() {
        let history = [
            json!({ "role": "user", "content": "a" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "b" }] }),
        ];
        let opening = opening_message(&history, "next", true);
        let text = opening.as_str().unwrap();
        assert!(text.starts_with(WORKSPACE_LOST) && text.ends_with("next"));
    }

    #[test]
    fn unanswered_tool_calls_get_error_results_first() {
        let history = [
            json!({ "role": "user", "content": "go" }),
            json!({ "role": "assistant", "content": [
                { "type": "text", "text": "running" },
                { "type": "tool_use", "id": "t1", "name": "bash", "input": {} },
                { "type": "tool_use", "id": "t2", "name": "bash", "input": {} },
            ] }),
        ];
        let opening = opening_message(&history, "next", false);
        let blocks = opening.as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["tool_use_id"], "t1");
        assert_eq!(blocks[1]["tool_use_id"], "t2");
        assert_eq!(blocks[1]["is_error"], true);
        assert_eq!(blocks[2], json!({ "type": "text", "text": "next" }));
    }
}

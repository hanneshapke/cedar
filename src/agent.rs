//! The agent loop: call the model, run the tools it asks for, feed the results back, and stop
//! when the model stops asking for tools.

use crate::events::Emitter;
use crate::llm::{LlmClient, MessageRequest};
use crate::models::{event, Run};
use crate::sandbox::{Sandbox, SandboxFatal};
use crate::store::Store;
use crate::tools::{ToolContext, ToolSet};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// What every run shares.
pub struct AgentEnv {
    pub llm: Arc<dyn LlmClient>,
    pub tools: Arc<ToolSet>,
    pub store: Store,
    pub max_tokens: u32,
    pub tool_timeout: Duration,
    pub tool_output_max_bytes: usize,
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
/// the run's status, the sandbox's lifetime, and the terminal event.
pub async fn run_agent(
    env: &AgentEnv,
    run: &Run,
    sandbox: Arc<dyn Sandbox>,
    events: &Emitter,
    cancel: &CancellationToken,
) -> Result<Completion, AgentError> {
    let ctx = ToolContext { sandbox, timeout: env.tool_timeout, max_output: env.tool_output_max_bytes };
    let tool_defs = env.tools.definitions();
    let mut messages: Vec<Value> = Vec::new();
    push_message(env, run, &mut messages, "user", Value::String(run.prompt.clone())).await?;

    for turn in 1..=run.max_turns {
        let req = MessageRequest {
            model: &run.model,
            max_tokens: env.max_tokens,
            system: &run.system,
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
        push_message(env, run, &mut messages, "assistant", Value::Array(resp.content.clone())).await?;

        let mut final_text = String::new();
        let mut tool_calls = Vec::new();
        for block in &resp.content {
            match block["type"].as_str() {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or_default();
                    final_text.push_str(text);
                    if !text.is_empty() {
                        events.emit(event::ASSISTANT_TEXT, json!({ "turn": turn, "text": text })).await?;
                    }
                }
                Some("tool_use") => tool_calls.push(block),
                // Server tools (web search) are executed by Anthropic; just report them. Their
                // result blocks are already in `content` and go back unmodified with the history.
                Some("server_tool_use") => {
                    events
                        .emit(event::WEB_SEARCH, json!({ "turn": turn, "name": block["name"], "input": block["input"] }))
                        .await?;
                }
                _ => {}
            }
        }

        match resp.stop_reason.as_deref() {
            Some("tool_use") if !tool_calls.is_empty() => {
                let mut results = Vec::with_capacity(tool_calls.len());
                for block in tool_calls {
                    results.push(execute_tool_call(env, &ctx, events, cancel, block).await?);
                }
                push_message(env, run, &mut messages, "user", Value::Array(results)).await?;
            }
            // The server paused a long-running server-tool loop; resend as-is.
            Some("pause_turn") => {}
            other => return Ok(Completion { final_text, stop_reason: other.map(String::from) }),
        }
    }
    Err(AgentError::Failed(format!("run did not finish within {} turns", run.max_turns)))
}

/// Execute one `tool_use` block and return the matching `tool_result` block.
async fn execute_tool_call(
    env: &AgentEnv,
    ctx: &ToolContext,
    events: &Emitter,
    cancel: &CancellationToken,
    block: &Value,
) -> Result<Value, AgentError> {
    let id = block["id"].as_str().unwrap_or_default();
    let name = block["name"].as_str().unwrap_or_default();
    let input = &block["input"];
    events.emit(event::TOOL_CALL, json!({ "id": id, "name": name, "input": input })).await?;

    let started = Instant::now();
    let call = async {
        match env.tools.get(name) {
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
        Err(e) if e.downcast_ref::<SandboxFatal>().is_some() => return Err(AgentError::Failed(e.to_string())),
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
async fn push_message(env: &AgentEnv, run: &Run, history: &mut Vec<Value>, role: &str, content: Value) -> Result<()> {
    env.store.append_message(run.id, role, &content).await?;
    history.push(json!({ "role": role, "content": content }));
    Ok(())
}

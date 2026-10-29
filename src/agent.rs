//! The agent loop: call the model, run the tools it asks for, feed the results back, and stop
//! when the model stops asking for tools.

use crate::approvals::{self, Gate, Rules};
use crate::attachments;
use crate::browser::BrowserSession;
use crate::config::SandboxConfig;
use crate::events::Emitter;
use crate::integrations::Integrations;
use crate::llm::{LlmRouter, MessageRequest};
use crate::models::{event, PlanMode, Run, Usage, DEFAULT_MAX_TOKENS};
use crate::plan;
use crate::policy::Endpoint;
use crate::sandbox::{Reach, Runtime, Sandbox, SandboxFatal};
use crate::sources::{Sources, CITE_NOTE};
use crate::store::Store;
use crate::tools::{question_text, ToolContext, ToolModel, ToolSet, ToolView, ASK_USER};
use crate::waits::{Answer, Waits};
use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio_util::sync::CancellationToken;

/// What every run shares.
pub struct AgentEnv {
    /// The clients of the registered models; a run uses the one for its `model_id`.
    pub llm: Arc<LlmRouter>,
    /// The current tools. Swapped when an admin changes an integration; a run keeps the set it
    /// started with.
    pub tools: Arc<ArcSwap<ToolSet>>,
    pub store: Store,
    pub tool_timeout: Duration,
    pub tool_output_max_bytes: usize,
    /// How sandboxes are set up: what they may reach depends on it and on the runtime (see
    /// [`Reach`]), for the note on what the sandbox can reach and for which calls need approval.
    pub sandbox: SandboxConfig,
    /// Which calls wait for approval in interactive runs.
    pub approvals: Rules,
    /// How long a run waits for an answer to its plan or an approval.
    pub approval_timeout: Duration,
    /// Where runs wait for those answers.
    pub waits: Arc<Waits>,
    /// The model registry, for whether a run's model takes images.
    pub integrations: Arc<Integrations>,
    /// How many pages of a run's attached PDFs, in all, a model that takes images is shown.
    pub pdf_preview_pages: usize,
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

/// Run `run` to completion, or until the model asks the user something with `ask_user` (the run then
/// completes with `stop_reason: ask_user` and the questions as its final text). Persists the
/// conversation and emits events as it goes; the caller owns the run's status, the sandbox's and
/// browser's lifetimes, and the terminal event.
pub async fn run_agent(
    env: &AgentEnv,
    run: &Run,
    sandbox: Arc<dyn Sandbox>,
    browser: Option<Arc<BrowserSession>>,
    events: &Emitter,
    cancel: &CancellationToken,
) -> Result<Completion, AgentError> {
    let ctx = ToolContext {
        sandbox: sandbox.clone(),
        browser,
        timeout: env.tool_timeout,
        max_output: env.tool_output_max_bytes,
        model: env.llm.client_for(&run.model_id).map(|client| {
            Arc::new(ToolModel::new(
                client,
                run.model.clone(),
                max_tokens(env, run),
                run.reasoning.clone(),
            ))
        }),
        locale: run.locale.clone(),
    };
    // Connections the key's owner always allows, which the sandbox's base policy doesn't. Only an
    // OpenShell policy can let them through one by one.
    let allowed: Vec<Endpoint> = if sandbox.runtime() == Runtime::OpenShell {
        env.store
            .grants(&run.owner, approvals::Rule::Network.as_str())
            .await?
            .iter()
            .filter_map(|g| Endpoint::from_grant(g))
            .collect()
    } else {
        Vec::new()
    };
    if !allowed.is_empty() {
        if let Err(e) = sandbox.allow(&allowed).await {
            tracing::warn!(run_id = %run.id, error = %format!("{e:#}"), "could not apply the connections this key always allows");
        }
    }
    let tool_set = env.tools.load_full();
    // Picks up MCP servers that weren't reachable (or not tried) when earlier runs started.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        _ = tool_set.connect_remote(env.tool_timeout) => {}
    }
    let tools = tool_set.select(run.tools.as_deref());
    let mut tool_defs = tools.definitions();
    let mut note = tools.environment_note(&Reach::of(sandbox.runtime(), &env.sandbox));
    // In `tool` mode the model decides whether a request needs a plan.
    let offers_plan = run.plan == PlanMode::Tool;
    if offers_plan {
        tool_defs.push(plan::tool_definition());
        note.push(' ');
        note.push_str(plan::TOOL_NOTE);
    }
    let mut planned = false;
    if ["web_search", "fetch_url", "download_file"]
        .iter()
        .any(|t| tools.get(t).is_some())
    {
        note.push(' ');
        note.push_str(CITE_NOTE);
    }
    if let Some(directive) = crate::locale::directive(&run.locale) {
        note.push(' ');
        note.push_str(&directive);
    }
    let mut system = with_note(&run.system, &note);
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
    let attachments = env.store.list_attachments(run.id).await?;
    let blocks = if attachments.is_empty() {
        vec![]
    } else {
        // Looked up now, so an admin's change since the run was created applies.
        let images = env
            .integrations
            .models()
            .iter()
            .any(|m| m.id == run.model_id && m.multimodal);
        let prepare = attachments::prepare(
            sandbox.as_ref(),
            &attachments,
            images,
            env.pdf_preview_pages,
            env.tool_timeout,
        );
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            blocks = prepare => blocks?,
        }
    };
    let opening = with_attachments(opening, blocks);

    if plan::wanted(run.plan, &run.prompt) {
        let names: Vec<&str> = tool_defs.iter().filter_map(|d| d["name"].as_str()).collect();
        if let Some(steps) = make_plan(env, run, &names, &messages, events, cancel).await? {
            system = with_note(&system, &plan::note(&steps));
        }
    }
    push_message(env, run, &mut messages, "user", opening).await?;
    let mut sources = Sources::default();

    for turn in 1..=run.max_turns {
        let req = MessageRequest {
            model: &run.model,
            // Looked up each turn, so an admin's change applies to the next call.
            max_tokens: max_tokens(env, run),
            reasoning_effort: run.reasoning.as_deref(),
            system: &system,
            tools: &tool_defs,
            messages: &messages,
        };
        // Looked up each turn, so a changed address applies to the next call.
        let llm = env.llm.client_for(&run.model_id).ok_or_else(|| {
            AgentError::Failed(format!("model {:?} is no longer registered", run.model_id))
        })?;
        let started = Instant::now();
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = llm.create_message(&req) => r?,
        };
        if let Some(text) = &resp.reasoning {
            events
                .emit(
                    event::ASSISTANT_REASONING,
                    json!({ "turn": turn, "text": text, "duration_ms": started.elapsed().as_millis() as u64, "done": true }),
                )
                .await?;
        }

        env.store.record_turn(run.id, turn, resp.usage).await?;
        events
            .emit(event::USAGE, usage_event(env, run, turn, resp.usage))
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
                let mut asked = Vec::new();
                for block in tool_calls {
                    if offers_plan && block["name"] == plan::TOOL {
                        let result =
                            propose_plan(env, run, &mut planned, events, cancel, block).await?;
                        results.push(result);
                        continue;
                    }
                    let result =
                        execute_tool_call(env, run, &tools, &ctx, &mut sources, events, cancel, block)
                            .await?;
                    // A tool that asked the model something (search_products, plan_trip) counts toward the turn.
                    if let Some(model) = &ctx.model {
                        let used = model.take_usage();
                        if used != Usage::default() {
                            env.store.record_turn(run.id, turn, used).await?;
                            let mut data = usage_event(env, run, turn, used);
                            data["purpose"] = json!("tool");
                            data["tool"] = block["name"].clone();
                            events.emit(event::USAGE, data).await?;
                        }
                    }
                    if block["name"] == ASK_USER && result["is_error"] != true {
                        asked.extend(result["content"].as_str().and_then(question_text));
                    }
                    results.push(result);
                }
                push_message(env, run, &mut messages, "user", Value::Array(results)).await?;
                // The model asked the user something: stop here, and the answer starts the next run.
                if !asked.is_empty() {
                    let mut parts = vec![final_text];
                    parts.extend(asked);
                    parts.retain(|p| !p.trim().is_empty());
                    return Ok(Completion {
                        final_text: parts.join("\n\n"),
                        stop_reason: Some(ASK_USER.to_string()),
                    });
                }
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

/// The opening message with the run's attachments (image blocks and the note on the saved files,
/// see [`attachments::prepare`]) placed before its text: any tool results first, then the files,
/// then the prompt. Without blocks it is unchanged.
pub fn with_attachments(opening: Value, blocks: Vec<Value>) -> Value {
    if blocks.is_empty() {
        return opening;
    }
    let mut parts = match opening {
        Value::String(text) => vec![json!({ "type": "text", "text": text })],
        Value::Array(parts) => parts,
        other => vec![other],
    };
    let text = parts.pop();
    parts.extend(blocks);
    parts.extend(text);
    Value::Array(parts)
}

/// The most tokens one reply of the run's model may take: the registered entry's limit, or the
/// default when the entry is gone.
fn max_tokens(env: &AgentEnv, run: &Run) -> u32 {
    env.integrations
        .models()
        .iter()
        .find(|m| m.id == run.model_id)
        .map_or(DEFAULT_MAX_TOKENS, |m| m.max_tokens)
}

/// The `usage` event for one model call, with what it cost when the model has a price.
fn usage_event(env: &AgentEnv, run: &Run, turn: u32, usage: Usage) -> Value {
    let mut data = json!({ "turn": turn, "input_tokens": usage.input_tokens, "output_tokens": usage.output_tokens });
    let price = env
        .integrations
        .models()
        .iter()
        .find(|m| m.id == run.model_id)
        .and_then(|m| m.price);
    if let Some(price) = price {
        data["cost_usd"] = json!(price.cost_usd(usage));
    }
    data
}

/// Ask the model for a plan and, in an interactive run, wait for the user to agree to it.
/// Returns the steps to follow, or `None` when the model gave no usable plan; a failed planning
/// call is logged and the run goes on without one.
async fn make_plan(
    env: &AgentEnv,
    run: &Run,
    tool_names: &[&str],
    history: &[Value],
    events: &Emitter,
    cancel: &CancellationToken,
) -> Result<Option<Vec<plan::Step>>, AgentError> {
    let Some(llm) = env.llm.client_for(&run.model_id) else {
        return Ok(None);
    };
    let system = plan::system(tool_names);
    let messages = plan::messages(history, &run.prompt);
    let req = MessageRequest {
        model: &run.model,
        max_tokens: max_tokens(env, run),
        reasoning_effort: run.reasoning.as_deref(),
        system: &system,
        tools: &[],
        messages: &messages,
    };
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        r = llm.create_message(&req) => r,
    };
    let resp = match resp {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(run_id = %run.id, error = %format!("{e:#}"), "planning call failed; going on without a plan");
            return Ok(None);
        }
    };
    env.store.record_turn(run.id, 0, resp.usage).await?;
    let mut usage = usage_event(env, run, 0, resp.usage);
    usage["purpose"] = json!("plan");
    events.emit(event::USAGE, usage).await?;
    let text: String = resp
        .content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let Some(steps) = plan::parse_reply(&text) else {
        tracing::info!(run_id = %run.id, "the planning call gave no usable plan");
        return Ok(None);
    };
    let (steps, _) = settle_plan(env, run, steps, None, events, cancel).await?;
    Ok(Some(steps))
}

/// Show `steps` as a `plan` event and, in an interactive run, wait for the user to agree to them.
/// Returns the steps to follow and who settled them (`user` or `timeout`; `None` when nobody was
/// asked). `origin` marks a plan the model proposed with [`plan::TOOL`].
async fn settle_plan(
    env: &AgentEnv,
    run: &Run,
    steps: Vec<plan::Step>,
    origin: Option<&str>,
    events: &Emitter,
    cancel: &CancellationToken,
) -> Result<(Vec<plan::Step>, Option<&'static str>), AgentError> {
    let shown = |awaiting: bool| {
        let mut data = json!({ "steps": steps, "editable": awaiting, "awaiting": awaiting });
        if let Some(origin) = origin {
            data["origin"] = json!(origin);
        }
        data
    };
    if !run.interactive {
        events.emit(event::PLAN, shown(false)).await?;
        return Ok((steps, None));
    }
    let rx = env.waits.register(run.id, "plan");
    events.emit(event::PLAN, shown(true)).await?;
    match env.waits.wait(run.id, "plan", rx, env.approval_timeout, cancel).await {
        Answer::Cancelled => Err(AgentError::Cancelled),
        Answer::TimedOut => {
            events
                .emit(event::PLAN_UPDATED, json!({ "steps": steps, "by": "timeout" }))
                .await?;
            Ok((steps, Some("timeout")))
        }
        Answer::Given(answer) => {
            // The API checked the steps before handing them over.
            let agreed = plan::parse_answer(&answer).unwrap_or(steps);
            events
                .emit(event::PLAN_UPDATED, json!({ "steps": agreed, "by": "user" }))
                .await?;
            Ok((agreed, Some("user")))
        }
    }
}

/// Handle a [`plan::TOOL`] call: show the plan (and wait for it in an interactive run) and return
/// the `tool_result` block, with the plan to follow or why there is none.
async fn propose_plan(
    env: &AgentEnv,
    run: &Run,
    planned: &mut bool,
    events: &Emitter,
    cancel: &CancellationToken,
    block: &Value,
) -> Result<Value, AgentError> {
    let id = block["id"].as_str().unwrap_or_default();
    let outcome = if *planned {
        Err(plan::TOOL_AGAIN.to_string())
    } else {
        plan::parse_tool_input(&block["input"])
    };
    let content = match outcome {
        Ok(steps) => {
            *planned = true;
            let (steps, by) = settle_plan(env, run, steps, Some("tool"), events, cancel).await?;
            plan::tool_result(&steps, by)
        }
        Err(message) => {
            return Ok(json!({ "type": "tool_result", "tool_use_id": id, "content": message, "is_error": true }))
        }
    };
    Ok(json!({ "type": "tool_result", "tool_use_id": id, "content": content }))
}

/// What the model is told when a call it asked for was not approved.
const DECLINED: &str = "The user declined this call, so it did not run. Don't try it again     another way; go on without it, or say what you would need.";
const NOT_APPROVED: &str = "No one approved this call in time, so it did not run. Go on without     it, or say what you would need.";

/// In an interactive run, wait for the user's OK if the call needs one (and isn't always
/// allowed already). `Ok(None)`: go ahead; `Ok(Some(message))`: don't, and tell the model this.
#[allow(clippy::too_many_arguments)]
async fn approve(
    env: &AgentEnv,
    tools: &ToolView<'_>,
    reach: &Reach,
    run: &Run,
    events: &Emitter,
    cancel: &CancellationToken,
    call_id: &str,
    name: &str,
    input: &Value,
) -> Result<Option<&'static str>, AgentError> {
    if !run.interactive {
        return Ok(None);
    }
    let personal = tools
        .get(name)
        .and_then(|t| t.personal())
        .and_then(|t| approvals::check_personal(&env.approvals, t));
    let Some(gate) = approvals::check(&env.approvals, name, input, reach).or(personal) else {
        return Ok(None);
    };
    let rule = gate.rule.as_str();
    if env.store.granted(&run.owner, rule, &gate.grants).await? {
        return Ok(None);
    }
    match ask(env, run, events, cancel, call_id, name, input, &gate).await? {
        Decision::Allow => Ok(None),
        Decision::Always => {
            env.store.grant(&run.owner, rule, &gate.grants).await?;
            Ok(None)
        }
        Decision::Timeout => Ok(Some(NOT_APPROVED)),
        Decision::Deny => Ok(Some(DECLINED)),
    }
}

/// What the user answered to an approval card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Allow,
    Always,
    Deny,
    Timeout,
}

/// Show `gate` as an `approval.required` event and wait for the user's answer.
#[allow(clippy::too_many_arguments)]
async fn ask(
    env: &AgentEnv,
    run: &Run,
    events: &Emitter,
    cancel: &CancellationToken,
    call_id: &str,
    name: &str,
    input: &Value,
    gate: &Gate,
) -> Result<Decision, AgentError> {
    let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let key = format!("approval:{id}");
    let rx = env.waits.register(run.id, &key);
    events
        .emit(
            event::APPROVAL_REQUIRED,
            json!({ "id": id, "call_id": call_id, "tool": name, "input": input, "rule": gate.rule.as_str(),
                    "reason": gate.reason, "scope": gate.scope }),
        )
        .await?;
    let decision = match env.waits.wait(run.id, &key, rx, env.approval_timeout, cancel).await {
        Answer::Cancelled => return Err(AgentError::Cancelled),
        Answer::TimedOut => "timeout".to_string(),
        Answer::Given(v) => v["decision"].as_str().unwrap_or("deny").to_string(),
    };
    let by = if decision == "timeout" { "timeout" } else { "user" };
    events
        .emit(event::APPROVAL_RESOLVED, json!({ "id": id, "decision": decision, "by": by }))
        .await?;
    Ok(match decision.as_str() {
        "allow" => Decision::Allow,
        "allow_always" => Decision::Always,
        "timeout" => Decision::Timeout,
        _ => Decision::Deny,
    })
}

/// The calls that run the model's own code in the sandbox, and so the ones whose connections the
/// sandbox's policy may refuse.
const RUNS_CODE: &[&str] = &["bash", "run_python"];

/// After a call that ran code in the sandbox: the connections and requests the sandbox's policy
/// refused meanwhile, as `sandbox.denied` events. In an interactive run, the user is asked about
/// each refused connection (the `network` rule), and what they allow is let through from then on.
/// Returns what the model is told about it, or `None` if nothing was refused.
#[allow(clippy::too_many_arguments)]
async fn blocked_connections(
    env: &AgentEnv,
    run: &Run,
    ctx: &ToolContext,
    events: &Emitter,
    cancel: &CancellationToken,
    call_id: &str,
    name: &str,
    since: SystemTime,
) -> Result<Option<String>, AgentError> {
    let denials = match ctx.sandbox.denials(since).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(run_id = %run.id, error = %format!("{e:#}"), "could not read the sandbox's refused connections");
            return Ok(None);
        }
    };
    if denials.is_empty() {
        return Ok(None);
    }
    let mut lines = Vec::new();
    let mut endpoints: Vec<Endpoint> = Vec::new();
    for d in &denials {
        events
            .emit(
                event::SANDBOX_DENIED,
                json!({ "call_id": call_id, "program": d.binary, "host": d.host, "port": d.port,
                        "what": d.what, "reason": d.reason }),
            )
            .await?;
        let program = d
            .binary
            .as_deref()
            .map(|b| b.rsplit('/').next().unwrap_or(b));
        lines.push(match (program, d.what.as_str()) {
            (Some(p), "connect") => format!("- {p} → {}:{}", d.host, d.port),
            (None, "connect") => format!("- a connection to {}:{}", d.host, d.port),
            (_, what) => format!("- {what} on {}:{}", d.host, d.port),
        });
        if let (Some(binary), "connect") = (&d.binary, d.what.as_str()) {
            let e = Endpoint {
                binary: binary.clone(),
                host: d.host.clone(),
                port: d.port,
            };
            if e.check().is_ok() && !endpoints.contains(&e) {
                endpoints.push(e);
            }
        }
    }
    let mut note = format!(
        "[the sandbox's network policy blocked]\n{}",
        lines.join("\n")
    );
    let mut allowed = Vec::new();
    let mut declined = Vec::new();
    for endpoint in endpoints {
        let shown = format!("{} → {}:{}", endpoint.binary, endpoint.host, endpoint.port);
        let rule = approvals::Rule::Network.as_str();
        let grant = vec![endpoint.grant()];
        let decision = if env.store.granted(&run.owner, rule, &grant).await? {
            Decision::Always
        } else {
            let Some(gate) = run
                .interactive
                .then(|| approvals::check_connection(&env.approvals, &endpoint))
                .flatten()
            else {
                continue;
            };
            let input =
                json!({ "program": endpoint.binary, "host": endpoint.host, "port": endpoint.port });
            ask(env, run, events, cancel, call_id, name, &input, &gate).await?
        };
        match decision {
            Decision::Allow | Decision::Always => {
                if decision == Decision::Always {
                    env.store.grant(&run.owner, rule, &grant).await?;
                }
                match ctx.sandbox.allow(std::slice::from_ref(&endpoint)).await {
                    Ok(()) => allowed.push(shown),
                    Err(e) => {
                        tracing::warn!(run_id = %run.id, error = %format!("{e:#}"), "could not allow a connection");
                        declined.push(shown);
                    }
                }
            }
            Decision::Deny | Decision::Timeout => declined.push(shown),
        }
    }
    if !allowed.is_empty() {
        note.push_str(&format!(
            "\nThe user allowed {}; run the command again.",
            allowed.join(", ")
        ));
    }
    if !declined.is_empty() {
        note.push_str(&format!(
            "\nThe user did not allow {}; don't try to reach it another way.",
            declined.join(", ")
        ));
    }
    if allowed.is_empty() && declined.is_empty() {
        note.push_str(
            "\nThe sandbox's policy doesn't allow this, and nobody can be asked during this run; \
             go on without it, or say what you would need.",
        );
    }
    Ok(Some(note))
}

/// Execute one `tool_use` block and return the matching `tool_result` block.
#[allow(clippy::too_many_arguments)]
async fn execute_tool_call(
    env: &AgentEnv,
    run: &Run,
    tools: &ToolView<'_>,
    ctx: &ToolContext,
    sources: &mut Sources,
    events: &Emitter,
    cancel: &CancellationToken,
    block: &Value,
) -> Result<Value, AgentError> {
    let id = block["id"].as_str().unwrap_or_default();
    let name = block["name"].as_str().unwrap_or_default();
    let input = &block["input"];
    // A call to a tool the run doesn't have fails below without running, so it needs no OK.
    let refused = if tools.get(name).is_some() {
        let reach = Reach::of(ctx.sandbox.runtime(), &env.sandbox);
        approve(env, tools, &reach, run, events, cancel, id, name, input).await?
    } else {
        None
    };
    events
        .emit(
            event::TOOL_CALL,
            json!({ "id": id, "name": name, "input": input }),
        )
        .await?;

    let started = Instant::now();
    let since = SystemTime::now();
    let call = async {
        if let Some(message) = refused {
            return Err(anyhow!(message));
        }
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

    let (mut output, is_error) = match result {
        Ok(out) => (out, false),
        Err(e) if e.downcast_ref::<SandboxFatal>().is_some() => {
            return Err(AgentError::Failed(e.to_string()))
        }
        // Errors go back to the model so it can recover.
        Err(e) => (format!("{e:#}"), true),
    };
    if refused.is_none() && RUNS_CODE.contains(&name) && tools.get(name).is_some() {
        if let Some(note) =
            blocked_connections(env, run, ctx, events, cancel, id, name, since).await?
        {
            output.push_str("\n\n");
            output.push_str(&note);
        }
    }
    let mut data = json!({ "id": id, "output": output, "is_error": is_error, "duration_ms": started.elapsed().as_millis() as u64 });
    // The model sees the numbers it cites by; the event keeps the output as the tool wrote it
    // and lists the numbered sources beside it.
    let mut for_model = output;
    if !is_error {
        if let Some(numbered) = sources.number_result(name, input, &for_model) {
            data["sources"] = json!(numbered.sources);
            for_model = numbered.output;
        }
    }
    events.emit(event::TOOL_RESULT, data).await?;

    let mut block = json!({ "type": "tool_result", "tool_use_id": id, "content": for_model });
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
    fn attachments_sit_between_tool_results_and_the_prompt() {
        let image = json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGk=" }, "title": "a.png" });
        let pdf = json!({ "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "aGk=" }, "title": "b.pdf" });

        let from_text = with_attachments(json!("look"), vec![image.clone(), pdf.clone()]);
        assert_eq!(
            from_text,
            json!([image, pdf, { "type": "text", "text": "look" }])
        );

        let result =
            json!({ "type": "tool_result", "tool_use_id": "t1", "content": "x", "is_error": true });
        let from_blocks = with_attachments(
            json!([result, { "type": "text", "text": "look" }]),
            vec![image.clone()],
        );
        assert_eq!(
            from_blocks,
            json!([result, image, { "type": "text", "text": "look" }])
        );
    }

    #[test]
    fn no_attachments_leaves_the_opening_alone() {
        assert_eq!(with_attachments(json!("hi"), vec![]), json!("hi"));
        let blocks = json!([{ "type": "text", "text": "hi" }]);
        assert_eq!(with_attachments(blocks.clone(), vec![]), blocks);
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

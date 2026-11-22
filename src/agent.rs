//! The agent loop: call the model, run the tools it asks for, feed the results back, and stop
//! when the model stops asking for tools.

use crate::approvals::{self, Gate, Rules};
use crate::attachments;
use crate::browser::BrowserSession;
use crate::config::SandboxConfig;
use crate::context::{self, Budget};
use crate::events::Emitter;
use crate::integrations::Integrations;
use crate::llm::{LlmClient, LlmResponse, LlmRouter, MessageRequest};
use crate::actions::{Actions, ActionsContext};
use crate::memory::{self, Memories, MemoryContext};
use crate::records::{self, Records, RecordsContext};
use crate::models::{event, PlanMode, Run, Usage, DEFAULT_MAX_TOKENS};
use crate::plan;
use crate::routing;
use crate::policy::Endpoint;
use crate::sandbox::{Reach, Runtime, Sandbox, SandboxFatal};
use crate::sources::{Sources, CITE_NOTE};
use crate::store::Store;
use crate::tools::{
    question_text, Handover, ToolContext, ToolModel, ToolView, ACCOUNT_GROUPS, ASK_USER,
    RECORDS_SAVE, REMEMBER, SAVED_ACTIONS,
};
use crate::trace;
use crate::waits::{Answer, Waits};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio_util::sync::CancellationToken;

/// What every run shares.
pub struct AgentEnv {
    /// The clients of the registered models; a run uses the one for its `model_id`.
    pub llm: Arc<LlmRouter>,
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
    /// After how many plain "allow" answers for the same thing an approval offers "always
    /// allow"; 0 offers it from the first time.
    pub always_allow_after: u32,
    /// Where runs wait for those answers.
    pub waits: Arc<Waits>,
    /// The model registry, for whether a run's model takes images.
    pub integrations: Arc<Integrations>,
    /// How many pages of a run's attached PDFs, in all, a model that takes images is shown.
    pub pdf_preview_pages: usize,
    /// What owners remember across conversations; `None` when memory is turned off.
    pub memories: Option<Memories>,
    /// Whether an interactive run ends with a call that notes the facts about the user its
    /// conversation stated (see [`note_memory`]).
    pub memory_extract: bool,
    /// Whether that call also notes what the run did (an episode) and how a task went well (a
    /// procedure); then every run with tools makes it, unattended ones too.
    pub memory_experience: bool,
    /// After how many finished runs of an owner the manager consolidates their memory; 0 never.
    pub memory_consolidate_after: u32,
    /// What owners keep as records across conversations; `None` when records are turned off.
    pub records: Option<Records>,
    /// What owners showed the agent how to do; `None` when actions are turned off.
    pub actions: Option<Actions>,
    /// `RUN_TIMEOUT_SECS`: how long a run may take (time spent waiting for the user not counted)
    /// while no admin has saved `run_timeout_secs`; the model is told when little of the limit is
    /// left (see [`time_note`]).
    pub run_timeout: Duration,
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
    created: &Run,
    sandbox: Arc<dyn Sandbox>,
    browser: Option<Arc<BrowserSession>>,
    events: &Emitter,
    cancel: &CancellationToken,
) -> Result<Completion, AgentError> {
    // A continuation starts with a copy of its parent's conversation (made when it was created).
    let mut messages: Vec<Value> = env
        .store
        .list_messages(created.id)
        .await?
        .into_iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    let attachments = env.store.list_attachments(created.id).await?;
    // A run that left the choice of model to cedar moves between the smallest and the biggest
    // registered one (see [`routing`]); `run` is the run on the model it is using, and
    // `on_small` whether it may still move up.
    let (mut run, big) = match route(env, created, &messages, &attachments).await? {
        Some((run, big)) => (run, Some(big)),
        None => (created.clone(), None),
    };
    let mut on_small = big.as_ref().is_some_and(|big| big.id != run.model_id);
    let run = &mut run;
    // The model a tool asks (search_products, plan_trip): such calls are for the biggest.
    let tool_model = big.as_ref().map_or_else(|| run.clone(), |big| on_model(run, big));
    let ctx = ToolContext {
        sandbox: sandbox.clone(),
        browser,
        timeout: env.tool_timeout,
        max_output: env.tool_output_max_bytes,
        model: env.llm.client_for(&tool_model.model_id).map(|client| {
            Arc::new(ToolModel::new(
                client,
                tool_model.model.clone(),
                max_tokens(env, &tool_model),
                tool_model.reasoning.clone(),
            ))
        }),
        locale: run.locale.clone(),
        timezone: run.timezone.clone(),
        memory: env.memories.as_ref().filter(|_| !run.private).map(|memories| MemoryContext {
            memories: memories.clone(),
            owner: run.owner.clone(),
            run_id: run.id,
            interactive: run.interactive,
        }),
        records: env.records.as_ref().filter(|_| !run.private).map(|records| RecordsContext {
            records: records.clone(),
            owner: run.owner.clone(),
            run_id: run.id,
            interactive: run.interactive,
        }),
        actions: env.actions.as_ref().map(|actions| ActionsContext {
            actions: actions.clone(),
            owner: run.owner.clone(),
        }),
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
    // The owner's tools: a user's act for the Google account they connected themselves.
    let tool_set = env.integrations.tools_for(&run.owner).await;
    // Picks up MCP servers that weren't reachable (or not tried) when earlier runs started.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        _ = tool_set.connect_remote(env.tool_timeout) => {}
    }
    // How much of the model's window the run's requests may take (see [`context`]).
    let mut budget = Budget::new(
        model_entry(env, run).and_then(|m| m.context_window),
        max_tokens(env, run),
    );
    // With a small window the definitions of every tool would fill much of it: a run that may
    // use all of them gets the ones its request seems to need.
    let selects_all = run.tools.as_ref().is_none_or(|t| t.iter().any(|g| g == "*"));
    let narrowed: Option<Vec<String>> = (budget.is_small() && selects_all).then(|| {
        tool_set.narrow_selection(&format!("{}\n{}", run.prompt, history_text(&messages)))
    });
    if let Some(allow) = &narrowed {
        tracing::info!(run_id = %run.id, window = ?budget.window(), tools = ?allow, "small context window: offering only the tools the request seems to need");
    }
    // A private run keeps nothing about the user: it gets no memory or records tools, and with
    // them goes everything below that reads or writes either.
    let tools = tool_set
        .select(narrowed.as_deref().or(created.tools.as_deref()))
        .without(if run.private { crate::tools::KEEPING_GROUPS } else { &[] });
    let mut tool_defs = tools.definitions();
    let mut note = tools.environment_note(&Reach::of(sandbox.runtime(), &env.sandbox));
    // How this run hands over to its user: what waits for their OK, what is held for them.
    note.push(' ');
    note.push_str(&tools.handover_note(Handover {
        interactive: run.interactive,
        approvals: !env.approvals.is_empty(),
        hold_writes: run.hold_writes,
    }));
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
    if let Some(window) = budget.window().filter(|_| budget.is_small()) {
        note.push(' ');
        note.push_str(&context::small_window_note(window));
        if narrowed.is_some() {
            note.push_str(
                " To fit the window, only the tools this request seems to need are offered; if \
                 the task needs another kind of tool, say so in your answer.",
            );
        }
    }
    // Which record sets the run keeps for the user, read per request like memory. It goes
    // before the memory note, which must stay the last thing in the prompt (see
    // [`memory::note_span`]).
    let memory_on = env.memories.is_some() && tools.get(REMEMBER).is_some();
    let records_on = env.records.is_some() && tools.get(RECORDS_SAVE).is_some();
    if let (Some(records), true) = (&env.records, records_on) {
        let sets = records.list(&run.owner).await?;
        let columns = records.columns_by_set(&run.owner, &sets).await?;
        note.push_str("\n\n");
        note.push_str(&records::note(&sets, &columns, run.interactive, memory_on));
    }
    // What the owner showed the agent how to do, by name; the steps are read with the tool.
    if let (Some(actions), true) = (&env.actions, tools.get(SAVED_ACTIONS).is_some()) {
        let saved = actions.list(&run.owner).await?;
        if !saved.is_empty() {
            note.push_str("\n\n");
            note.push_str(&crate::actions::note(&saved, tools.get("browser").is_some()));
        }
    }
    // Read per request, so a memory deleted in Settings is gone from the very next run. The
    // `memory` group switches reading and writing together. The profile and pinned
    // instructions are always shown; the rest is what the request calls for (see
    // [`memory::recall`]), and what was shown goes out as a `memory.recalled` event.
    if let (Some(memories), true) = (&env.memories, memory_on) {
        let query = memory::recall_query(&messages, &run.prompt);
        // Beside the profile, which sums the owner up, a whole listing of what is remembered
        // about them only costs tokens in an unattended run or a small window: there only what
        // the request calls for is shown.
        let whole = run.interactive && !budget.is_small();
        let recall = memories.recall(&run.owner, &query, whole).await?;
        memories.recalled(&run.owner, &recall.ids()).await?;
        if recall.profile.is_some() || !recall.shown.is_empty() {
            events.emit(event::MEMORY_RECALLED, recall.summary()).await?;
        }
        note.push_str("\n\n");
        note.push_str(&memory::note(&recall, run.interactive, records_on));
    }
    // At the head of cedar's note, where the model can't miss it: so it needs no call to learn
    // the date, and searches for this year's news rather than the year its training ended. The
    // client's own prompt stays first, so the start of every request stays the same.
    // It is the user's clock when the run knows their time zone: by UTC's alone, "tomorrow"
    // asked on a Saturday evening in Portland is Monday (see [`crate::timezone`]).
    let date = crate::timezone::date_note(Utc::now(), run.timezone.as_deref());
    let mut system = with_note(&run.system, &with_note(&date, &note));
    let workspace_lost = !messages.is_empty() && !run.workspace_reused;
    let opening = opening_message(&messages, &run.prompt, workspace_lost);
    // CSV and JSON attachments go into record sets named after them before the model sees
    // anything, when the run asks for it (see [`records::import`]); the note says what became
    // of each, and a `records.updated` event what was imported.
    let mut imported: Vec<Option<String>> = vec![None; attachments.len()];
    if run.import_attachments && records_on {
        let records = env.records.as_ref().expect("records are on");
        let mut done = Vec::new();
        for (i, a) in attachments.iter().enumerate().filter(|(_, a)| a.is_table()) {
            imported[i] = Some(match records.import_file(&run.owner, run.id, a).await? {
                Ok(r) => {
                    done.push(json!({ "file": a.name, "set": r.set, "added": r.added, "updated": r.updated, "key": r.key }));
                    format!(
                        "its {} rows were imported into the record set {} ({} added, {} updated; key {}); query it with records_lookup",
                        r.rows,
                        r.set,
                        r.added,
                        r.updated,
                        if r.key.is_empty() { "none".to_string() } else { r.key.join(", ") }
                    )
                }
                Err(why) => format!("not imported into records ({why}); read it with read_file or run_python"),
            });
        }
        if !done.is_empty() {
            events.emit(event::RECORDS_UPDATED, json!({ "imported": done })).await?;
        }
    }
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
            &imported,
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
    // A private run keeps no trace (see [`call_model`]).
    if !run.private {
        env.store
            .record_sent_prompt(run.id, &system, &tool_defs)
            .await?;
    }
    let mut sources = Sources::default();

    // The system prompt and tools are the same in every request; only the messages grow.
    let fixed_bytes = system.len() + serde_json::to_string(&tool_defs).map_or(0, |t| t.len());
    let mut breaker = Breaker::default();
    let mut cuts = context::Cuts::default();
    let run_started = Instant::now();
    // The limit the manager enforces for this run (see [`crate::manager::run_time_limit`]).
    let time_limit = crate::manager::run_time_limit(&env.store, env.run_timeout).await;
    for turn in 1..=run.max_turns {
        // A run that is still working after its first turns is a complex task.
        if let (true, Some(big)) = (on_small && turn > routing::SMALL_TURNS, &big) {
            move_to(env, run, big, &mut budget, "the task takes many turns").await?;
            on_small = false;
        }
        let resp = 'ask: loop {
        // Looked up each turn, so an admin's change applies to the next call.
        budget.set_max_reply(max_tokens(env, run));
        let llm = env.llm.client_for(&run.model_id).ok_or_else(|| {
            AgentError::Failed(format!("model {:?} is no longer registered", run.model_id))
        })?;
        // A reply cut off at its limit before it said or asked anything gets one more try with
        // less of the conversation; a second one ends the run as failed.
        let mut tight = false;
        let resp = loop {
            // What was cut before is cut the same way again, so the start of the request stays
            // the same text and the server can reuse its cache for it.
            let applied = (!cuts.is_empty()).then(|| cuts.apply(&messages));
            let current: &[Value] = applied.as_deref().unwrap_or(&messages);
            let fitted = match budget.message_bytes(fixed_bytes, tight) {
                Some(max) if context::size(current) > max => {
                    // Down to well below the limit, so the next turns fit without new cuts.
                    let fitted = context::fit(&messages, max * context::REFIT_PERCENT / 100, &cuts);
                    let before = budget.estimate(fixed_bytes + context::size(current));
                    let after = budget.estimate(fixed_bytes + context::size(&fitted.messages));
                    let window = budget.window().unwrap_or_default();
                    events
                        .emit(
                            event::CONTEXT_TRIMMED,
                            context::trimmed_event(turn, before, after, &fitted, window),
                        )
                        .await?;
                    cuts = fitted.cuts.clone();
                    Some(fitted)
                }
                _ => None,
            };
            let sent: &[Value] = fitted.as_ref().map_or(current, |f| &f.messages);
            let sent_bytes = fixed_bytes + context::size(sent);
            let req = MessageRequest {
                model: &run.model,
                max_tokens: budget.reply_tokens(budget.estimate(sent_bytes)),
                reasoning_effort: run.reasoning.as_deref(),
                system: &system,
                tools: &tool_defs,
                messages: sent,
            };
            let started = Instant::now();
            let resp = call_model(env, run, llm.as_ref(), &req, trace::AGENT, turn, cancel).await??;
            // The smallest model reached for a complex tool: the turn is asked again of the
            // biggest (below), and nothing of this reply is shown.
            let dropped = on_small && calls_complex_tool(&resp);
            if let Some(text) = resp.reasoning.as_ref().filter(|_| !dropped) {
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
            budget.calibrate(sent_bytes, resp.usage);
            let learned = budget.learn_window(req.max_tokens, resp.stop_reason.as_deref(), resp.usage);
            if dropped || !cut_off_empty(&resp) {
                break resp;
            }
            // Less conversation only helps when the window, not the reply limit, cut it short:
            // the reply stopped before the tokens it asked for, or asked for less than its limit
            // because the window had no more room.
            let window_bound = learned
                || resp.usage.output_tokens < u64::from(req.max_tokens)
                || req.max_tokens < budget.max_reply();
            if !tight && window_bound {
                tracing::warn!(run_id = %run.id, turn, input_tokens = resp.usage.input_tokens, window = ?budget.window(), "the reply was cut off by the context window before it answered; trying again with less of the conversation");
                tight = true;
                continue;
            }
            return Err(AgentError::Failed(cut_off_reason(&budget, req.max_tokens, resp.usage, window_bound)));
        };
        match &big {
            Some(big) if on_small && calls_complex_tool(&resp) => {
                move_to(env, run, big, &mut budget, "a complex tool call").await?;
                on_small = false;
            }
            _ => break 'ask resp,
        }
        };
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
                // The calls share what is left of the window, so their results fit beside the
                // conversation instead of being cut down before the model reads them.
                let used = budget.estimate(fixed_bytes + context::size(&messages));
                let turn_ctx = ToolContext {
                    max_output: budget.tool_output_bytes(used, tool_calls.len(), env.tool_output_max_bytes),
                    ..ctx.clone()
                };
                let ctx = &turn_ctx;
                let mut results = Vec::with_capacity(tool_calls.len());
                let mut asked = Vec::new();
                // Plan proposals are answered where they stand; the calls between them run in
                // groups (see [`execute_tool_calls`]).
                let mut pending: Vec<&Value> = Vec::new();
                let mut calls = tool_calls.into_iter().peekable();
                while let Some(block) = calls.next() {
                    let proposal = offers_plan && block["name"] == plan::TOOL;
                    if !proposal {
                        pending.push(block);
                    }
                    if proposal || calls.peek().is_none() {
                        let done = execute_tool_calls(env, run, &tools, ctx, &mut sources, &mut breaker, events, cancel, &pending).await?;
                        // A tool that asked the model something (search_products, plan_trip)
                        // counts toward the turn.
                        if let Some(model) = &ctx.model {
                            let used = model.take_usage();
                            if used != Usage::default() {
                                env.store.record_turn(run.id, turn, used).await?;
                                let mut data = usage_event(env, run, turn, used);
                                data["purpose"] = json!("tool");
                                let names: Vec<&str> = pending.iter().filter_map(|b| b["name"].as_str()).collect();
                                data["tool"] = json!(names.join(", "));
                                events.emit(event::USAGE, data).await?;
                            }
                        }
                        for (block, result) in pending.drain(..).zip(done) {
                            if block["name"] == ASK_USER && result["is_error"] != true {
                                asked.extend(result["content"].as_str().and_then(question_text));
                            }
                            results.push(result);
                        }
                    }
                    if proposal {
                        results.push(propose_plan(env, run, &mut planned, events, cancel, block).await?);
                    }
                }
                let elapsed = run_started.elapsed().saturating_sub(env.waits.paused(run.id));
                if let Some(note) = time_note(elapsed, time_limit, run.max_turns - turn) {
                    results.push(json!({ "type": "text", "text": note }));
                }
                push_message(env, run, &mut messages, "user", Value::Array(results)).await?;
                // The model asked the user something: stop here, and the answer starts the next run.
                if !asked.is_empty() {
                    let mut parts = vec![final_text];
                    parts.extend(asked);
                    parts.retain(|p| !p.trim().is_empty());
                    let done = Completion {
                        final_text: parts.join("\n\n"),
                        stop_reason: Some(ASK_USER.to_string()),
                    };
                    return finish(env, run, memory_on, &messages, events, cancel, done).await;
                }
            }
            other => {
                let done = Completion {
                    final_text,
                    stop_reason: other.map(String::from),
                };
                return finish(env, run, memory_on, &messages, events, cancel, done).await;
            }
        }
    }
    Err(AgentError::Failed(format!(
        "run did not finish within {} turns",
        run.max_turns
    )))
}

/// The run on the model it starts with and the biggest model, for a run that left the choice of
/// model to cedar (see [`routing`]); `None` for a run that named its model. Looked up now, so
/// the registry as it is counts, not as it was when the run was created. It starts on the
/// smallest model unless it plans first; a conversation with images stays among the models that
/// take them.
async fn route(
    env: &AgentEnv,
    run: &Run,
    messages: &[Value],
    attachments: &[crate::models::Attachment],
) -> Result<Option<(Run, crate::models::ModelEntry)>, AgentError> {
    if !run.model_auto {
        return Ok(None);
    }
    let shown = |m: &Value| {
        m["content"].as_array().is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "image"))
    };
    let images = attachments.iter().any(|a| a.is_image()) || messages.iter().any(shown);
    let Some(tiers) = routing::Tiers::pick(&env.integrations.models(), images) else {
        return Ok(None);
    };
    // A request to do something out in the world (buy, book, cancel) starts with the biggest
    // model, like one that plans: see [`routing::asks_to_act`].
    let first = if plan::wanted(run.plan, &run.prompt) || routing::asks_to_act(&run.prompt) {
        &tiers.big
    } else {
        &tiers.small
    };
    let start = on_model(run, first);
    if start.model_id != run.model_id || start.reasoning != run.reasoning {
        env.store
            .set_run_model(run.id, &start.model_id, &start.model, start.reasoning.as_deref())
            .await?;
    }
    Ok(Some((start, tiers.big)))
}

/// `run` as it is on `entry`: that model, at its default reasoning level.
fn on_model(run: &Run, entry: &crate::models::ModelEntry) -> Run {
    Run {
        model_id: entry.id.clone(),
        model: entry.model.clone(),
        reasoning: entry.default_reasoning.clone(),
        ..run.clone()
    }
}

/// Take the run on with the biggest model (see [`routing`]): its requests, its limits and its
/// window from here on, and what `GET /v1/runs/{id}` reports as the run's model.
async fn move_to(
    env: &AgentEnv,
    run: &mut Run,
    big: &crate::models::ModelEntry,
    budget: &mut Budget,
    why: &str,
) -> Result<(), AgentError> {
    tracing::info!(run_id = %run.id, from = %run.model_id, to = %big.id, why, "going on with the biggest model");
    *run = on_model(run, big);
    *budget = Budget::new(big.context_window, big.max_tokens);
    env.store
        .set_run_model(run.id, &run.model_id, &run.model, run.reasoning.as_deref())
        .await?;
    Ok(())
}

/// Whether a reply asks for a tool that is for the biggest model (see [`routing::complex_tool`]).
fn calls_complex_tool(resp: &LlmResponse) -> bool {
    resp.content
        .iter()
        .filter(|b| b["type"] == "tool_use")
        .filter_map(|b| b["name"].as_str())
        .any(routing::complex_tool)
}

/// The run's completion, after what it learned has been noted (see [`note_memory`]). The answer
/// is already out, so being cancelled while noting only skips it.
async fn finish(
    env: &AgentEnv,
    run: &Run,
    memory_on: bool,
    messages: &[Value],
    events: &Emitter,
    cancel: &CancellationToken,
    done: Completion,
) -> Result<Completion, AgentError> {
    if memory_on {
        match note_memory(env, run, messages, &done, events, cancel).await {
            Ok(()) | Err(AgentError::Cancelled) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

/// Note what a run learned, with one tool-free model call over its conversation (see
/// [`memory::extract`]): the facts the user stated (interactive runs, with `MEMORY_EXTRACT`),
/// an episode and, when the work went well, a procedure (`MEMORY_EXPERIENCE`). An episode row is
/// written even without the call. What was saved goes out as a `memory.updated` event. A failed
/// call is logged and the run completes without.
async fn note_memory(
    env: &AgentEnv,
    run: &Run,
    messages: &[Value],
    done: &Completion,
    events: &Emitter,
    cancel: &CancellationToken,
) -> Result<(), AgentError> {
    let Some(memories) = &env.memories else {
        return Ok(());
    };
    let transcript = memory::extract::transcript(messages, &run.prompt);
    let asked = done.stop_reason.as_deref() == Some(ASK_USER);
    // A short label for the episode, and what the model writing the notes is told.
    let (outcome, told) = if asked {
        ("waiting for the user's answer", "waiting for the user's answer")
    } else if done.final_text.trim().is_empty() {
        (
            "no answer",
            "without an answer: the assistant's last reply was empty, so what it set out to \
             produce may not exist; don't describe the work as done",
        )
    } else if done.stop_reason.as_deref() == Some("max_tokens") {
        ("cut off", "cut off: the assistant's final answer was cut short at its token limit")
    } else {
        ("completed", "completed")
    };
    // A procedure is only learned from a run that did what it set out to do.
    let succeeded = outcome == "completed";
    let facts = env.memory_extract && run.interactive;
    let experience = env.memory_experience;
    let worked = !transcript.tools.is_empty();
    // Data the user handed over goes into records from the same call, when the run may save
    // records (see [`memory::extract::admit_rows`]).
    let records = env.records.as_ref().filter(|_| {
        facts && run.tools.as_ref().is_none_or(|t| t.iter().any(|g| g == "records" || g == RECORDS_SAVE || g == "*"))
    });
    let sets: Option<Vec<(String, Vec<String>)>> = match records {
        Some(r) => {
            let list = r.list(&run.owner).await?;
            let columns = r.columns_by_set(&run.owner, &list).await?;
            Some(list.iter().map(|s| (s.name.clone(), columns.get(&s.name).cloned().unwrap_or_default())).collect())
        }
        None => None,
    };
    let call = facts || (experience && worked);
    let mut noted = memory::extract::Noted::default();
    let remembered = memories.active(&run.owner).await?;
    if call && !transcript.text.trim().is_empty() {
        if let Some(llm) = env.llm.client_for(&run.model_id) {
            let (system, call_messages) =
                memory::extract::call(
                &remembered,
                sets.as_deref(),
                &transcript,
                facts,
                experience,
                told,
                crate::timezone::today(Utc::now(), run.timezone.as_deref()),
            );
            let reasoning = quiet_reasoning(env, run);
            let req = MessageRequest {
                model: &run.model,
                max_tokens: max_tokens(env, run).min(QUIET_MAX_TOKENS),
                reasoning_effort: reasoning.as_deref(),
                system: &system,
                tools: &[],
                messages: &call_messages,
            };
            match call_model(env, run, llm.as_ref(), &req, trace::MEMORY, 0, cancel).await? {
                Ok(resp) => {
                    env.store.record_turn(run.id, 0, resp.usage).await?;
                    let mut usage = usage_event(env, run, 0, resp.usage);
                    usage["purpose"] = json!("memory");
                    events.emit(event::USAGE, usage).await?;
                    let text: String = resp
                        .content
                        .iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect();
                    noted = memory::extract::parse(&text);
                }
                Err(e) => {
                    tracing::warn!(run_id = %run.id, error = %format!("{e:#}"), "the memory call failed; nothing noted from this conversation");
                }
            }
        }
    }
    let actor = memory::Actor::Run(run.id);
    let mut saved: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    if facts {
        for c in &noted.facts {
            match memory::extract::admit(c, &transcript.user_text, &remembered, run.id, &noted.topics) {
                Ok(new) => match memories.remember(&run.owner, new, actor).await? {
                    Ok((_, memory::Saved::Unchanged)) => {}
                    Ok((m, status)) => saved.push(json!({ "status": status, "memory": m })),
                    Err(refused) => skipped.push(json!({ "text": c.text, "reason": refused.to_string() })),
                },
                Err(skip) => skipped.push(json!({ "text": skip.text, "reason": skip.reason })),
            }
        }
    }
    let mut saved_records: Vec<Value> = Vec::new();
    if let (Some(records), Some(sets)) = (records, &sets) {
        // The user's own words and the files they attached are what a row may be taken from.
        let mut source = transcript.user_text.clone();
        for a in env.store.list_attachments(run.id).await? {
            if let Some(text) = records::import::text_of(&a) {
                source.push('\n');
                source.extend(text.chars().take(records::import::MAX_TEXT_CHARS));
            }
        }
        for note in &noted.records {
            let (rows, rejected) = memory::extract::admit_rows(note, &source);
            skipped.extend(rejected.into_iter().map(|s| json!({ "text": s.text, "reason": s.reason })));
            if rows.is_empty() {
                continue;
            }
            let exists = sets.iter().any(|(name, _)| *name == note.set);
            let description = note.description.clone().unwrap_or_else(|| "Data the user handed over".to_string());
            let opts = records::SaveOptions {
                description: (!exists).then_some(description.as_str()),
                key: (!exists).then_some(note.key.as_slice()),
                health_data: None,
                run_id: Some(run.id),
            };
            match records.save(&run.owner, &note.set, &rows, opts).await? {
                Ok(done) => saved_records.push(json!({ "set": done.set.name, "added": done.added, "updated": done.updated })),
                Err(refused) => skipped.push(json!({ "text": format!("{}: {} rows", note.set, rows.len()), "reason": refused.to_string() })),
            }
        }
    }
    let mut episode = None;
    if experience {
        let row = memory::extract::episode(run.id, &run.prompt, noted.episode.as_ref(), outcome, &transcript, &noted.topics);
        if let Ok((m, _)) = memories.remember(&run.owner, row, actor).await? {
            episode = Some(m);
        }
        if let Some(p) = noted.procedure.as_ref().filter(|_| worked && succeeded) {
            if let Ok((m, status @ (memory::Saved::Added | memory::Saved::Superseded))) =
                memories.remember(&run.owner, memory::extract::procedure(run.id, p, &noted.topics), actor).await?
            {
                saved.push(json!({ "status": status, "memory": m }));
            }
        }
    }
    if saved.is_empty() && skipped.is_empty() && episode.is_none() && saved_records.is_empty() {
        return Ok(());
    }
    events
        .emit(
            event::MEMORY_UPDATED,
            json!({ "saved": saved, "skipped": skipped, "episode": episode, "records": saved_records }),
        )
        .await?;
    Ok(())
}

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

/// Call the model and record the call for the run's trace, unless the run is private. The outer
/// error is a cancellation or a failure to record it; the inner one is the call's own.
async fn call_model(
    env: &AgentEnv,
    run: &Run,
    llm: &dyn LlmClient,
    req: &MessageRequest<'_>,
    purpose: &str,
    turn: u32,
    cancel: &CancellationToken,
) -> Result<Result<LlmResponse>, AgentError> {
    let started_at = Utc::now();
    let started = Instant::now();
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        r = llm.create_message(req) => r,
    };
    if run.private {
        return Ok(resp);
    }
    let call = trace::NewCall {
        turn,
        purpose,
        request: trace::request(req, purpose),
        response: resp.as_ref().ok().map(trace::response),
        error: resp.as_ref().err().map(|e| format!("{e:#}")),
        started_at,
        duration_ms: started.elapsed().as_millis() as u64,
    };
    env.store.record_llm_call(run.id, &call).await?;
    Ok(resp)
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

/// The run's registered model, `None` when the entry is gone.
fn model_entry(env: &AgentEnv, run: &Run) -> Option<crate::models::ModelEntry> {
    env.integrations
        .models()
        .iter()
        .find(|m| m.id == run.model_id)
        .cloned()
}

/// The reasoning level for the run's bookkeeping calls (noting what it learned), which need no
/// deliberation: the lowest the model offers, else the run's own.
fn quiet_reasoning(env: &AgentEnv, run: &Run) -> Option<String> {
    let levels = model_entry(env, run).map(|m| m.reasoning_levels).unwrap_or_default();
    ["none", "minimal", "low"]
        .iter()
        .find(|l| levels.iter().any(|v| v == *l))
        .map(|l| l.to_string())
        .or_else(|| run.reasoning.clone())
}

/// The most tokens a bookkeeping call's reply gets: its answer is a short JSON object.
const QUIET_MAX_TOKENS: u32 = 4_096;

/// Whether a reply stopped at its token limit before it said or asked anything: what a
/// conversation that fills the model's window gets.
fn cut_off_empty(resp: &LlmResponse) -> bool {
    resp.stop_reason.as_deref() == Some("max_tokens")
        && resp.content.iter().all(|b| match b["type"].as_str() {
            Some("text") => b["text"].as_str().unwrap_or_default().trim().is_empty(),
            Some("tool_use") => false,
            _ => true,
        })
}

/// Why a run ended after replies that were cut off before they answered.
fn cut_off_reason(budget: &Budget, requested: u32, usage: Usage, window_bound: bool) -> String {
    match (window_bound, budget.window()) {
        (true, Some(window)) => format!(
            "the model's reply was cut off before it answered: the conversation ({} tokens) left \
             too little of the model's context window ({window} tokens) for a reply, even after \
             older tool results were cut down",
            usage.input_tokens
        ),
        _ => format!(
            "the model used up its reply limit of {requested} tokens (its reasoning included) \
             without answering; raise the model's max_tokens or choose a lower reasoning level"
        ),
    }
}

/// The text of earlier user messages and the names of the tools called before, which show what
/// a continuation may need.
fn history_text(messages: &[Value]) -> String {
    let mut out = String::new();
    for m in messages {
        match &m["content"] {
            Value::String(text) if m["role"] == "user" => {
                out.push_str(text);
                out.push('\n');
            }
            Value::Array(blocks) => {
                for b in blocks {
                    if b["type"] == "tool_use" {
                        out.push_str(b["name"].as_str().unwrap_or_default());
                        out.push('\n');
                    } else if b["type"] == "text" && m["role"] == "user" {
                        out.push_str(b["text"].as_str().unwrap_or_default());
                        out.push('\n');
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The `usage` event for one model call, with what it cost when the model has a price.
fn usage_event(env: &AgentEnv, run: &Run, turn: u32, usage: Usage) -> Value {
    let mut data = json!({ "turn": turn, "model_id": run.model_id, "input_tokens": usage.input_tokens, "output_tokens": usage.output_tokens });
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
    let resp = match call_model(env, run, llm.as_ref(), &req, trace::PLAN, 0, cancel).await? {
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

/// What the model reads back for a call that was held (see [`hold`]).
const HELD_NOTE: &str = "Not done yet: this call is held in the user's outbox and is only made \
    if they approve it there. Don't make it again and don't wait for it. Go on with the rest, \
    and tell the user that it is waiting for their approval.";

/// At most this many calls of one run are held; more are refused, so a run that loops can't
/// fill its owner's outbox.
const MAX_HELD_PER_RUN: i64 = 20;

/// What becomes of a call before it runs.
enum Verdict {
    /// Make it.
    Go,
    /// Don't, and tell the model this.
    Refused(String),
    /// Don't: it waits in the owner's outbox. The model is told this, as the call's result.
    Held(String),
}

/// Decide whether a call runs now. In an interactive run, wait for the user's OK if the call
/// needs one (and isn't always allowed already); one that sends or changes something in the
/// user's accounts and got no answer in time is held for them rather than dropped. In a run
/// nobody answers, such a call is held when the run says so (`hold_writes`), and everything
/// else goes ahead.
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
) -> Result<Verdict, AgentError> {
    // By the tool's group in this run's set, so a personal tool whose name merely looks like an
    // account's is never held: it needs a sandbox to run.
    let holdable = ACCOUNT_GROUPS.contains(&tools.group(name)) && crate::tools::holdable(name);
    if !run.interactive {
        if run.hold_writes && holdable {
            return hold(env, run, events, call_id, name, input, "unattended").await;
        }
        return Ok(Verdict::Go);
    }
    let personal = tools
        .get(name)
        .and_then(|t| t.personal())
        .and_then(|t| approvals::check_personal(&env.approvals, t));
    let Some(gate) = approvals::check(&env.approvals, name, input, reach).or(personal) else {
        return Ok(Verdict::Go);
    };
    let rule = gate.rule.as_str();
    if env.store.granted(&run.owner, rule, &gate.grants).await? {
        return Ok(Verdict::Go);
    }
    match ask(env, run, events, cancel, call_id, name, input, &gate).await? {
        Decision::Allow => Ok(Verdict::Go),
        Decision::Always => {
            env.store.grant(&run.owner, rule, &gate.grants).await?;
            Ok(Verdict::Go)
        }
        Decision::Timeout if holdable => hold(env, run, events, call_id, name, input, "timeout").await,
        Decision::Timeout => Ok(Verdict::Refused(NOT_APPROVED.into())),
        Decision::Deny => Ok(Verdict::Refused(DECLINED.into())),
    }
}

/// Keep a call in its owner's outbox instead of making it, and say so with a `call.held` event.
/// The call is stored as the model wrote it; approving it later makes exactly that call (see
/// `POST /v1/held/{id}`).
async fn hold(
    env: &AgentEnv,
    run: &Run,
    events: &Emitter,
    call_id: &str,
    name: &str,
    input: &Value,
    why: &str,
) -> Result<Verdict, AgentError> {
    if env.store.count_held(run.id).await? >= MAX_HELD_PER_RUN {
        return Ok(Verdict::Refused(format!(
            "{MAX_HELD_PER_RUN} calls of this run already wait for the user's approval, so this \
             one was not kept. Stop here and tell the user what is waiting."
        )));
    }
    let reason = approvals::describe(name, input);
    let held = env
        .store
        .hold_call(run.id, &run.owner, call_id, name, input, &reason, why)
        .await?;
    // The same call made twice is kept once; only the first is announced.
    if held.call_id == call_id {
        events
            .emit(
                event::CALL_HELD,
                json!({ "id": held.id, "call_id": call_id, "tool": name, "input": input,
                        "reason": reason, "why": why }),
            )
            .await?;
    }
    Ok(Verdict::Held(
        json!({ "held": { "id": held.id, "tool": name, "reason": reason, "why": why }, "note": HELD_NOTE })
            .to_string(),
    ))
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
    let rule = gate.rule.as_str();
    // "Always allow" is earned: it is offered once the owner plainly allowed the same thing
    // `always_allow_after` times, counted since they last declined it. Until then the event
    // carries no `scope`, which is how clients know not to offer it, and `always` says how far
    // along it is.
    let after = env.always_allow_after;
    let allowed = match &gate.scope {
        Some(_) if after > 0 => env.store.allow_count(&run.owner, rule, &gate.grants).await?,
        _ => 0,
    };
    let offered = gate.scope.is_some() && allowed >= after;
    let mut data = json!({ "id": id, "call_id": call_id, "tool": name, "input": input, "rule": rule,
                           "reason": gate.reason, "scope": gate.scope.as_ref().filter(|_| offered) });
    if gate.scope.is_some() && after > 0 {
        data["always"] = json!({ "scope": gate.scope, "allowed": allowed, "after": after, "offered": offered });
    }
    let rx = env.waits.register(run.id, &key);
    events.emit(event::APPROVAL_REQUIRED, data).await?;
    let decision = match env.waits.wait(run.id, &key, rx, env.approval_timeout, cancel).await {
        Answer::Cancelled => return Err(AgentError::Cancelled),
        Answer::TimedOut => "timeout",
        Answer::Given(v) => match v["decision"].as_str() {
            Some("allow") => "allow",
            // An "always" that wasn't offered counts as this once.
            Some("allow_always") if offered => "allow_always",
            Some("allow_always") => "allow",
            _ => "deny",
        },
    };
    let by = if decision == "timeout" { "timeout" } else { "user" };
    events
        .emit(event::APPROVAL_RESOLVED, json!({ "id": id, "decision": decision, "by": by }))
        .await?;
    Ok(match decision {
        "allow" => {
            env.store.count_allow(&run.owner, rule, &gate.grants).await?;
            Decision::Allow
        }
        "allow_always" => {
            env.store.reset_allow_counts(&run.owner, rule, &gate.grants).await?;
            Decision::Always
        }
        "timeout" => Decision::Timeout,
        _ => {
            env.store.reset_allow_counts(&run.owner, rule, &gate.grants).await?;
            Decision::Deny
        }
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

/// From this share of the run's time on, or with this few turns left, each turn's results end
/// with a note on what is left (see [`time_note`]).
const TIME_NOTE_SHARE: f64 = 0.6;
const TIME_NOTE_TURNS: u32 = 3;

/// The note that ends a turn's results once little of the run's time or turns is left, so the
/// model writes its result from what it has rather than being stopped with nothing.
pub fn time_note(elapsed: Duration, limit: Duration, turns_left: u32) -> Option<String> {
    let late = !limit.is_zero() && elapsed.as_secs_f64() >= limit.as_secs_f64() * TIME_NOTE_SHARE;
    if !late && turns_left > TIME_NOTE_TURNS {
        return None;
    }
    let left = limit.saturating_sub(elapsed);
    let minutes = left.as_secs().div_ceil(60).max(1);
    Some(format!(
        "[Time check: about {minutes} minute{} and {turns_left} turn{} are left before this run is \
         stopped, and a stopped run delivers nothing. Stop gathering: produce the result now \
         (write the files, give the answer) from what you have, and say what is missing.]",
        if minutes == 1 { "" } else { "s" },
        if turns_left == 1 { "" } else { "s" },
    ))
}

/// How many temporary failures in a row (a rate limit, a server that is unavailable, a
/// timeout) take a tool out of a run: calling it again only spends time.
const BREAKER_FAILURES: u32 = 2;

/// Which tools of a run keep failing for reasons a retry in the same run won't fix (see
/// [`BREAKER_FAILURES`]). A tool that answered resets its count.
#[derive(Default)]
pub struct Breaker {
    failures: std::collections::HashMap<String, u32>,
}

impl Breaker {
    /// Why a call to `name` is not made, when it has failed too often.
    pub fn refusal(&self, name: &str) -> Option<String> {
        let n = *self.failures.get(name)?;
        (n >= BREAKER_FAILURES).then(|| {
            format!(
                "not called: {name} failed {n} times in a row with temporary errors (rate \
                 limited, unavailable or timing out), so it is left out for the rest of this run. \
                 Don't call it again: use another source, or carry on without it and say in your \
                 answer what is missing."
            )
        })
    }

    /// Count a call's outcome.
    pub fn record(&mut self, name: &str, is_error: bool, output: &str) {
        if is_error && is_temporary(output) {
            *self.failures.entry(name.to_string()).or_default() += 1;
        } else {
            self.failures.remove(name);
        }
    }
}

/// Whether a tool's error looks temporary: rate limits, servers that are down, timeouts.
pub fn is_temporary(error: &str) -> bool {
    let lower = error.to_lowercase();
    [
        "429", "502", "503", "504", "too many requests", "rate limit", "rate-limit",
        "service unavailable", "bad gateway", "gateway timeout", "timed out", "timeout",
        "couldn't reach", "could not reach", "connection refused", "connection reset",
        // Windows words a refused or dropped connection differently (os errors 10060, 10061).
        "actively refused", "forcibly closed", "did not properly respond", "os error 10061",
        "os error 10060", "os error 10054",
    ]
    .iter()
    .any(|m| lower.contains(m))
}

/// Whether a call must run on its own, in order: anything that works in the sandbox (files,
/// code), the browser (one page), questions and plans, memory and records, downloads, and
/// account tools that change something. Other calls of a turn (searches, pages, lookups) run at
/// the same time.
fn runs_alone(tools: &ToolView<'_>, name: &str) -> bool {
    let group = tools.group(name);
    matches!(group, "core" | "browser" | "ask_user" | "memory" | "records")
        || name == "download_file"
        || name == plan::TOOL
        || crate::tools::writes(name)
}

/// A tool call that was approved (or refused) and announced, ready to run.
struct Prepared<'b> {
    block: &'b Value,
    id: String,
    name: String,
    /// Why it isn't run: refused by the user or a rule, or taken out by the [`Breaker`].
    refused: Option<String>,
    /// The call was held instead of made: what the model reads as its result.
    held: Option<String>,
    since: SystemTime,
}

/// Approve (in an interactive run) and announce a call.
#[allow(clippy::too_many_arguments)]
async fn prepare_call<'b>(
    env: &AgentEnv,
    run: &Run,
    tools: &ToolView<'_>,
    ctx: &ToolContext,
    breaker: &Breaker,
    events: &Emitter,
    cancel: &CancellationToken,
    block: &'b Value,
) -> Result<Prepared<'b>, AgentError> {
    let id = block["id"].as_str().unwrap_or_default().to_string();
    let name = block["name"].as_str().unwrap_or_default().to_string();
    let input = &block["input"];
    let mut refused = breaker.refusal(&name);
    let mut held = None;
    // A call to a tool the run doesn't have fails without running, so it needs no OK.
    if refused.is_none() && tools.get(&name).is_some() {
        let reach = Reach::of(ctx.sandbox.runtime(), &env.sandbox);
        match approve(env, tools, &reach, run, events, cancel, &id, &name, input).await? {
            Verdict::Go => {}
            Verdict::Refused(why) => refused = Some(why),
            Verdict::Held(output) => held = Some(output),
        }
    }
    events
        .emit(
            event::TOOL_CALL,
            json!({ "id": id, "name": name, "input": input }),
        )
        .await?;
    Ok(Prepared { block, id, name, refused, held, since: SystemTime::now() })
}

/// Run a prepared call; the result and how long it took.
async fn run_call(
    tools: &ToolView<'_>,
    ctx: &ToolContext,
    cancel: &CancellationToken,
    call: &Prepared<'_>,
) -> Result<(Result<String>, Duration), AgentError> {
    let started = Instant::now();
    let work = async {
        if let Some(message) = &call.refused {
            return Err(anyhow!(message.clone()));
        }
        if let Some(output) = &call.held {
            return Ok(output.clone());
        }
        // A tool left out of the run's selection is as unknown as a made-up one.
        match tools.get(&call.name) {
            Some(tool) => tool.call(ctx, &call.block["input"]).await,
            None => Err(anyhow!("unknown tool: {}", call.name)),
        }
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        r = work => r,
    };
    Ok((result, started.elapsed()))
}

/// Turn a call's result into the `tool_result` block the model sees: refused connections noted,
/// sources numbered, the `tool.result` event sent, the [`Breaker`] told.
#[allow(clippy::too_many_arguments)]
async fn finish_call(
    env: &AgentEnv,
    run: &Run,
    tools: &ToolView<'_>,
    ctx: &ToolContext,
    sources: &mut Sources,
    breaker: &mut Breaker,
    events: &Emitter,
    cancel: &CancellationToken,
    call: &Prepared<'_>,
    result: Result<String>,
    took: Duration,
) -> Result<Value, AgentError> {
    let (id, name, input) = (call.id.as_str(), call.name.as_str(), &call.block["input"]);
    let (mut output, is_error) = match result {
        Ok(out) => (out, false),
        Err(e) if e.downcast_ref::<SandboxFatal>().is_some() => {
            return Err(AgentError::Failed(e.to_string()))
        }
        // Errors go back to the model so it can recover.
        Err(e) => (format!("{e:#}"), true),
    };
    if call.refused.is_none() && call.held.is_none() {
        breaker.record(name, is_error, &output);
    }
    if call.refused.is_none() && RUNS_CODE.contains(&name) && tools.get(name).is_some() {
        if let Some(note) =
            blocked_connections(env, run, ctx, events, cancel, id, name, call.since).await?
        {
            output.push_str("\n\n");
            output.push_str(&note);
        }
    }
    let mut data = json!({ "id": id, "output": output, "is_error": is_error, "duration_ms": took.as_millis() as u64 });
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

/// Run a turn's calls (except plan proposals, which the caller handles): those that may run at
/// the same time (see [`runs_alone`]) are run together, each of the others on its own, in the
/// order the model gave. Results come back in that order.
#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls(
    env: &AgentEnv,
    run: &Run,
    tools: &ToolView<'_>,
    ctx: &ToolContext,
    sources: &mut Sources,
    breaker: &mut Breaker,
    events: &Emitter,
    cancel: &CancellationToken,
    blocks: &[&Value],
) -> Result<Vec<Value>, AgentError> {
    let mut results = Vec::with_capacity(blocks.len());
    let mut i = 0;
    while i < blocks.len() {
        let alone = |b: &Value| runs_alone(tools, b["name"].as_str().unwrap_or_default());
        let mut j = i + 1;
        if !alone(blocks[i]) {
            while j < blocks.len() && !alone(blocks[j]) {
                j += 1;
            }
        }
        let mut batch = Vec::with_capacity(j - i);
        for block in &blocks[i..j] {
            batch.push(prepare_call(env, run, tools, ctx, breaker, events, cancel, block).await?);
        }
        let ran = futures_util::future::join_all(batch.iter().map(|c| run_call(tools, ctx, cancel, c))).await;
        for (call, outcome) in batch.iter().zip(ran) {
            let (result, took) = outcome?;
            results.push(
                finish_call(env, run, tools, ctx, sources, breaker, events, cancel, call, result, took)
                    .await?,
            );
        }
        i = j;
    }
    Ok(results)
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
    fn a_tool_that_keeps_failing_for_temporary_reasons_is_left_out() {
        let mut b = Breaker::default();
        let limited = "arxiv_search failed (exit status 1): arXiv's API is rate limiting or unavailable (HTTP 429)";
        b.record("arxiv_search", true, limited);
        assert!(b.refusal("arxiv_search").is_none(), "one failure is not enough");
        b.record("arxiv_search", true, "couldn't reach arXiv's API (The read operation timed out)");
        let refusal = b.refusal("arxiv_search").unwrap();
        assert!(refusal.contains("Don't call it again"), "{refusal}");
        // Other tools are not affected, and an answer resets the count.
        assert!(b.refusal("fetch_url").is_none());
        b.record("arxiv_search", false, "3 papers");
        assert!(b.refusal("arxiv_search").is_none());
        // A mistake in the call is not temporary: the model can fix it.
        b.record("fetch_url", true, "missing url");
        b.record("fetch_url", true, "missing url");
        assert!(b.refusal("fetch_url").is_none());
        assert!(is_temporary("arXiv answered 503 (Service Unavailable)"));
        assert!(is_temporary(
            "tcp connect error: No connection could be made because the target machine actively \
             refused it. (os error 10061)"
        ));
        assert!(is_temporary("tcp connect error: Connection refused (os error 111)"));
        assert!(!is_temporary("unknown tool: frobnicate"));
    }

    #[test]
    fn little_time_left_is_said() {
        let limit = Duration::from_secs(600);
        assert_eq!(time_note(Duration::from_secs(100), limit, 20), None);
        let late = time_note(Duration::from_secs(400), limit, 20).unwrap();
        assert!(late.contains("about 4 minutes and 20 turns"), "{late}");
        let few = time_note(Duration::from_secs(10), limit, 1).unwrap();
        assert!(few.contains("1 turn are") || few.contains("1 turn "), "{few}");
        let over = time_note(Duration::from_secs(700), limit, 5).unwrap();
        assert!(over.contains("about 1 minute and"), "{over}");
    }

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

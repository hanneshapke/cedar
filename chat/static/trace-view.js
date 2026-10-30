'use strict';

// Expert mode: the trace of a response, shared by every UI. Turned on per browser under Settings →
// Advanced settings → Expert mode (localStorage, key `cedar-chat-expert`), it shows a Trace button on each message
// you sent, which opens a dialog with everything the agent did for the response, from cedar's
// `GET /v1/runs/{id}/trace` (through `/api/turns/{id}/trace`):
//
//   - Timeline: each call to the model (what it was sent, what it answered, its reasoning, tokens
//     and time) and, between them, each tool call with its input, output and time, plus the plan,
//     approvals and blocked connections as they happened
//   - Conversation: the messages as the model sees them (content blocks)
//   - Prompt & tools: the system prompt and the tool definitions the model was sent
//   - Events: the run's event log
//   - JSON: the whole trace, to copy or download
//
// A UI adds the button with cedarTrace.button(turnId); it is hidden by CSS (trace-view.css) unless
// <html data-expert> is set, so switching the mode shows or hides the buttons already drawn.

const cedarTrace = (() => {
  const KEY = 'cedar-chat-expert';
  /** Events shown in the timeline beside the model and tool calls. */
  const TIMELINE_EVENTS = new Set([
    'run.started', 'plan', 'plan.updated', 'approval.required', 'approval.resolved', 'sandbox.denied',
    'run.completed', 'run.failed', 'run.cancelled',
  ]);

  let dialog = null;
  let current = null; // { turnId, trace, tab }

  // -- the mode ----------------------------------------------------------------------------------

  function enabled() {
    try { return localStorage.getItem(KEY) === '1'; } catch { return false; }
  }

  function set(on) {
    try {
      if (on) localStorage.setItem(KEY, '1');
      else localStorage.removeItem(KEY);
    } catch { /* then it only lasts for this visit */ }
    apply(on);
  }

  function apply(on = enabled()) {
    document.documentElement.toggleAttribute('data-expert', on);
  }

  // -- helpers -----------------------------------------------------------------------------------

  function el(tag, className, text) {
    const e = document.createElement(tag);
    if (className) e.className = className;
    if (text !== undefined && text !== null) e.textContent = text;
    return e;
  }

  function btn(label, onClick, className = 'cedar-trace-btn') {
    const b = el('button', className, label);
    b.type = 'button';
    b.addEventListener('click', onClick);
    return b;
  }

  async function api(path) {
    const headers = {};
    try {
      const token = localStorage.getItem('cedar-chat-token');
      if (token) headers.Authorization = `Bearer ${token}`;
    } catch { /* no saved token: the server may not need one */ }
    const res = await fetch(`/api${path}`, { headers });
    const body = await res.json().catch(() => null);
    if (!res.ok) throw new Error(body?.error?.message || tr('trace.requestFailed', { status: String(res.status) }));
    return body;
  }

  const num = (n) => Number(n || 0).toLocaleString(cedarI18n.locale);

  function duration(ms) {
    if (typeof ms !== 'number') return '';
    if (ms < 1000) return `${ms} ms`;
    return `${(ms / 1000).toLocaleString(cedarI18n.locale, { maximumFractionDigits: 1 })} s`;
  }

  function size(bytes) {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }

  /** "+1.2 s" from the start of the run. */
  function offset(at, start) {
    if (!Number.isFinite(at) || !Number.isFinite(start)) return '';
    return `+${duration(Math.max(0, at - start))}`;
  }

  function json(value) {
    return JSON.stringify(value, null, 2);
  }

  function pre(text, className = '') {
    return el('pre', `cedar-trace-pre ${className}`.trim(), text);
  }

  /** A titled fold; `open` unfolds it. Its body is built when first opened. */
  function fold(summary, build, { open = false, className = '' } = {}) {
    const d = el('details', `cedar-trace-fold ${className}`.trim());
    const s = el('summary');
    if (typeof summary === 'string') s.textContent = summary;
    else s.append(...[summary].flat());
    d.append(s);
    let built = false;
    const fill = () => {
      if (built || !d.open) return;
      built = true;
      const body = el('div', 'cedar-trace-fold-body');
      body.append(...[build()].flat().filter(Boolean));
      d.append(body);
    };
    d.addEventListener('toggle', fill);
    if (open) { d.open = true; fill(); }
    return d;
  }

  function chip(text, kind = '') {
    return el('span', `cedar-trace-chip ${kind}`.trim(), text);
  }

  async function copy(text, button) {
    try {
      await navigator.clipboard.writeText(text);
      const was = button.textContent;
      button.textContent = tr('trace.copied');
      setTimeout(() => { button.textContent = was; }, 1500);
    } catch { /* the clipboard may be unavailable (no HTTPS); nothing else to do */ }
  }

  function copyButton(label, text) {
    const b = btn(label, () => copy(typeof text === 'function' ? text() : text, b), 'cedar-trace-btn small');
    return b;
  }

  // -- content blocks ----------------------------------------------------------------------------

  /** One content block of a message or a response. */
  function block(b) {
    if (typeof b === 'string') return pre(b);
    const wrap = el('div', `cedar-trace-block ${b?.type || ''}`.trim());
    switch (b?.type) {
      case 'text':
        wrap.append(el('div', 'cedar-trace-label', 'text'), pre(b.text || ''));
        break;
      case 'tool_use':
        wrap.append(el('div', 'cedar-trace-label', `tool_use · ${b.name} · ${b.id}`), pre(json(b.input ?? {}), 'code'));
        break;
      case 'tool_result': {
        const label = el('div', 'cedar-trace-label', `tool_result · ${b.tool_use_id}`);
        if (b.is_error) label.append(' ', chip('is_error', 'error'));
        const content = typeof b.content === 'string' ? b.content : json(b.content);
        wrap.append(label, pre(content));
        break;
      }
      case 'image':
      case 'document': {
        const src = b.source || {};
        const what = [b.type, src.media_type, b.title, typeof src.bytes === 'number' ? size(src.bytes) : null].filter(Boolean).join(' · ');
        wrap.append(el('div', 'cedar-trace-label', what));
        break;
      }
      default:
        wrap.append(el('div', 'cedar-trace-label', b?.type || 'block'), pre(json(b), 'code'));
    }
    return wrap;
  }

  function blocks(content) {
    if (typeof content === 'string') return [block({ type: 'text', text: content })];
    if (Array.isArray(content)) return content.map(block);
    return [pre(json(content), 'code')];
  }

  function message(m, { fresh = false } = {}) {
    const wrap = el('div', `cedar-trace-msg ${m.role}${fresh ? ' fresh' : ''}`);
    const head = el('div', 'cedar-trace-msg-head');
    head.append(chip(m.role, m.role));
    if (m.seq !== undefined) head.append(el('span', 'cedar-trace-dim', `#${m.seq}`));
    if (fresh) head.append(chip(tr('trace.new'), 'accent'));
    wrap.append(head, ...blocks(m.content));
    return wrap;
  }

  // -- the timeline ------------------------------------------------------------------------------

  /** The request a call was sent, rebuilt from the trace: an agent call refers to the run's prompt,
   *  tools and the first `message_count` messages; a planning call carries its own. */
  function fullRequest(trace, call) {
    const r = call.request || {};
    const own = call.purpose !== 'agent';
    const messages = own ? r.messages : (trace.messages || []).slice(0, r.message_count).map(({ role, content }) => ({ role, content }));
    return {
      model: r.model,
      max_tokens: r.max_tokens,
      reasoning_effort: r.reasoning_effort ?? null,
      system: own ? r.system : trace.system,
      tools: own ? r.tools : trace.tools,
      messages,
    };
  }

  function params(rows) {
    const dl = el('dl', 'cedar-trace-params');
    for (const [k, v] of rows) {
      if (v === undefined || v === null || v === '') continue;
      dl.append(el('dt', '', k), el('dd', '', String(v)));
    }
    return dl;
  }

  function callCard(trace, call, prevCount, start) {
    const req = call.request || {};
    const resp = call.response;
    const usage = resp?.usage;
    const failed = Boolean(call.error);
    const head = [
      chip(call.purpose === 'plan' ? tr('trace.planning') : tr('trace.modelCall'), failed ? 'error' : 'accent'),
      el('span', 'cedar-trace-title', call.purpose === 'plan' ? tr('trace.planCall') : tr('trace.turn', { n: String(call.turn) })),
      el('span', 'cedar-trace-dim', offset(Date.parse(call.started_at), start)),
      el('span', 'cedar-trace-grow'),
    ];
    if (usage) head.push(el('span', 'cedar-trace-dim', tr('trace.tokens', { input: num(usage.input_tokens), output: num(usage.output_tokens) })));
    if (resp?.stop_reason) head.push(chip(resp.stop_reason));
    if (failed) head.push(chip(tr('trace.failed'), 'error'));
    head.push(el('span', 'cedar-trace-time', duration(call.duration_ms)));

    return fold(head, () => {
      const out = [];
      // Request
      const reqSec = el('section', 'cedar-trace-sec');
      const reqHead = el('div', 'cedar-trace-sec-head');
      reqHead.append(el('h4', '', tr('trace.request')), copyButton(tr('trace.copyRequest'), () => json(fullRequest(trace, call))));
      reqSec.append(reqHead);
      const count = req.message_count ?? 0;
      reqSec.append(params([
        ['model', req.model],
        ['max_tokens', req.max_tokens],
        ['reasoning_effort', req.reasoning_effort],
        ['messages', count],
        ['tools', call.purpose === 'agent' ? (trace.tools || []).length : (req.tools || []).length],
        ['started_at', new Date(call.started_at).toLocaleString(cedarI18n.locale)],
      ]));
      if (call.purpose === 'agent') {
        const all = (trace.messages || []).slice(0, count);
        // The first call's new message is the prompt; what came before it is the earlier conversation.
        const from = prevCount ? Math.min(prevCount, count) : Math.max(0, count - 1);
        const fresh = all.slice(from);
        reqSec.append(el('p', 'cedar-trace-note', tr('trace.sentNote', { count, fresh: String(fresh.length) })));
        reqSec.append(...fresh.map((m) => message(m, { fresh: true })));
        if (from > 0) {
          reqSec.append(fold(tr('trace.earlier', { count: from }), () => all.slice(0, from).map((m) => message(m))));
        }
        if (trace.system) reqSec.append(fold(tr('trace.systemPrompt'), () => pre(trace.system)));
      } else {
        if (req.system) reqSec.append(fold(tr('trace.systemPrompt'), () => pre(req.system)));
        reqSec.append(...(req.messages || []).map((m) => message(m)));
      }
      out.push(reqSec);

      // Response
      const resSec = el('section', 'cedar-trace-sec');
      const resHead = el('div', 'cedar-trace-sec-head');
      resHead.append(el('h4', '', tr('trace.response')));
      if (resp) resHead.append(copyButton(tr('trace.copyResponse'), () => json(resp)));
      resSec.append(resHead);
      if (failed) resSec.append(pre(call.error, 'error'));
      if (resp) {
        resSec.append(params([
          ['stop_reason', resp.stop_reason],
          ['input_tokens', usage ? num(usage.input_tokens) : null],
          ['output_tokens', usage ? num(usage.output_tokens) : null],
          ['duration', duration(call.duration_ms)],
        ]));
        if (resp.reasoning) resSec.append(fold(tr('trace.reasoning'), () => pre(resp.reasoning), { className: 'reasoning' }));
        resSec.append(...blocks(resp.content || []));
      }
      out.push(resSec);
      return out;
    }, { open: failed, className: `call${failed ? ' failed' : ''}` });
  }

  function toolCard(item, start) {
    const { call, result } = item;
    const failed = result?.is_error;
    const head = [
      chip(tr('trace.tool'), failed ? 'error' : 'tool'),
      el('span', 'cedar-trace-title mono', call.name),
      el('span', 'cedar-trace-dim', offset(item.at, start)),
      el('span', 'cedar-trace-grow'),
    ];
    if (!result) head.push(chip(tr('trace.noResult'), 'warn'));
    else if (failed) head.push(chip(tr('trace.error'), 'error'));
    if (result?.sources?.length) head.push(chip(tr('trace.sources', { count: result.sources.length })));
    head.push(el('span', 'cedar-trace-time', result ? duration(result.duration_ms) : ''));
    return fold(head, () => {
      const out = [];
      const input = el('section', 'cedar-trace-sec');
      const ih = el('div', 'cedar-trace-sec-head');
      ih.append(el('h4', '', tr('trace.input')), copyButton(tr('trace.copy'), () => json(call.input ?? {})));
      input.append(ih, el('div', 'cedar-trace-dim mono', `id ${call.id}`), pre(json(call.input ?? {}), 'code'));
      out.push(input);
      if (result) {
        const output = el('section', 'cedar-trace-sec');
        const oh = el('div', 'cedar-trace-sec-head');
        oh.append(el('h4', '', tr('trace.output')), copyButton(tr('trace.copy'), () => result.output ?? ''));
        output.append(oh, pre(result.output ?? '', failed ? 'error' : ''));
        if (result.sources?.length) output.append(fold(tr('trace.sources', { count: result.sources.length }), () => pre(json(result.sources), 'code')));
        out.push(output);
      }
      return out;
    }, { className: `tool${failed ? ' failed' : ''}` });
  }

  function eventRow(event, start) {
    const row = el('div', `cedar-trace-event ${event.type.replace(/\./g, '-')}`);
    row.append(chip(event.type, /failed|denied/.test(event.type) ? 'error' : event.type.startsWith('approval') ? 'warn' : ''));
    row.append(el('span', 'cedar-trace-dim', offset(Date.parse(event.created_at), start)));
    const summary = eventSummary(event);
    if (summary) row.append(el('span', 'cedar-trace-event-text', summary));
    return fold(row, () => pre(json(event.data), 'code'), { className: 'event' });
  }

  function eventSummary(event) {
    const d = event.data || {};
    switch (event.type) {
      case 'run.started': return [d.model, d.reasoning, d.sandbox?.runtime, d.workspace_reused ? 'workspace reused' : null].filter(Boolean).join(' · ');
      case 'plan': case 'plan.updated': return (d.steps || []).map((s) => s.title).join(' → ');
      case 'approval.required': return [d.tool, d.reason].filter(Boolean).join(' · ');
      case 'approval.resolved': return [d.decision, d.by].filter(Boolean).join(' · ');
      case 'sandbox.denied': return `${d.program || ''} → ${d.host}:${d.port}`;
      case 'run.completed': return d.stop_reason || '';
      case 'run.failed': return d.error || '';
      case 'run.cancelled': return d.reason || '';
      default: return '';
    }
  }

  /** Model calls, tool calls and the notable events, in the order they happened. */
  function timelineItems(trace) {
    const items = [];
    (trace.calls || []).forEach((call, i) => items.push({ kind: 'call', at: Date.parse(call.started_at), order: 0, i, call }));
    const results = new Map();
    for (const e of trace.events || []) if (e.type === 'tool.result') results.set(e.data?.id, e.data);
    for (const e of trace.events || []) {
      const at = Date.parse(e.created_at);
      if (e.type === 'tool.call') items.push({ kind: 'tool', at, order: 1, i: e.seq, call: e.data || {}, result: results.get(e.data?.id) });
      else if (TIMELINE_EVENTS.has(e.type)) items.push({ kind: 'event', at, order: 1, i: e.seq, event: e });
    }
    // A model call starts before the events it leads to, which are written in their own order.
    items.sort((a, b) => (a.at - b.at) || (a.order - b.order) || (a.i - b.i));
    return items;
  }

  function startOf(trace) {
    const times = [
      ...(trace.calls || []).map((c) => Date.parse(c.started_at)),
      ...(trace.events || []).map((e) => Date.parse(e.created_at)),
    ].filter(Number.isFinite);
    return times.length ? Math.min(...times) : NaN;
  }

  function renderTimeline(trace) {
    const out = [];
    if (!(trace.calls || []).length) out.push(el('p', 'cedar-trace-note', tr('trace.noCalls')));
    const start = startOf(trace);
    let prevCount = 0;
    for (const item of timelineItems(trace)) {
      if (item.kind === 'call') {
        out.push(callCard(trace, item.call, prevCount, start));
        // The next agent call is shown what it was sent beyond this one's request and reply.
        if (item.call.purpose === 'agent') prevCount = (item.call.request?.message_count || 0) + (item.call.response ? 1 : 0);
      } else if (item.kind === 'tool') {
        out.push(toolCard(item, start));
      } else {
        out.push(eventRow(item.event, start));
      }
    }
    return out;
  }

  // -- the other tabs ----------------------------------------------------------------------------

  function renderConversation(trace) {
    const msgs = trace.messages || [];
    if (!msgs.length) return [el('p', 'cedar-trace-note', tr('trace.noMessages'))];
    return msgs.map((m) => message(m));
  }

  function renderPrompt(trace) {
    const out = [];
    const sys = el('section', 'cedar-trace-sec');
    const sh = el('div', 'cedar-trace-sec-head');
    sh.append(el('h4', '', tr('trace.systemPrompt')));
    if (trace.system) sh.append(copyButton(tr('trace.copy'), trace.system));
    sys.append(sh, trace.system ? pre(trace.system) : el('p', 'cedar-trace-note', tr('trace.noPrompt')));
    out.push(sys);
    const tools = trace.tools || [];
    const ts = el('section', 'cedar-trace-sec');
    const th = el('div', 'cedar-trace-sec-head');
    th.append(el('h4', '', tr('trace.toolDefs', { count: tools.length })));
    if (tools.length) th.append(copyButton(tr('trace.copy'), () => json(tools)));
    ts.append(th);
    for (const t of tools) {
      const head = [el('span', 'cedar-trace-title mono', t.name), el('span', 'cedar-trace-dim cedar-trace-ellipsis', (t.description || '').split('\n')[0])];
      ts.append(fold(head, () => [t.description ? pre(t.description) : null, pre(json(t.input_schema ?? t), 'code')]));
    }
    out.push(ts);
    return out;
  }

  function renderEvents(trace) {
    const events = trace.events || [];
    if (!events.length) return [el('p', 'cedar-trace-note', tr('trace.noEvents'))];
    const start = startOf(trace);
    return events.map((e) => {
      const row = el('div', 'cedar-trace-event');
      row.append(el('span', 'cedar-trace-dim mono', `#${e.seq}`), chip(e.type), el('span', 'cedar-trace-dim', offset(Date.parse(e.created_at), start)));
      return fold(row, () => pre(json(e.data), 'code'), { className: 'event' });
    });
  }

  function renderJson(trace) {
    return [pre(json(trace), 'code raw')];
  }

  const TABS = [
    { id: 'timeline', label: () => tr('trace.tab.timeline'), render: renderTimeline },
    { id: 'conversation', label: () => tr('trace.tab.conversation'), render: renderConversation },
    { id: 'prompt', label: () => tr('trace.tab.prompt'), render: renderPrompt },
    { id: 'events', label: () => tr('trace.tab.events'), render: renderEvents },
    { id: 'json', label: () => 'JSON', render: renderJson },
  ];

  // -- the dialog --------------------------------------------------------------------------------

  function ensureDialog() {
    if (dialog) return dialog;
    dialog = el('dialog', 'cedar-trace');
    dialog.setAttribute('aria-labelledby', 'cedar-trace-title');
    dialog.innerHTML = `
      <div class="cedar-trace-head">
        <div class="cedar-trace-heading">
          <h3 id="cedar-trace-title"></h3>
          <div class="cedar-trace-summary"></div>
        </div>
        <div class="cedar-trace-actions"></div>
      </div>
      <div class="cedar-trace-tabs" role="tablist"></div>
      <div class="cedar-trace-body" role="tabpanel"></div>`;
    dialog.addEventListener('click', (e) => { if (e.target === dialog) dialog.close(); });
    document.body.append(dialog);
    return dialog;
  }

  function summaryChips(trace) {
    const run = trace.run || {};
    const calls = trace.calls || [];
    const tools = (trace.events || []).filter((e) => e.type === 'tool.call').length;
    const modelMs = calls.reduce((n, c) => n + (c.duration_ms || 0), 0);
    const toolMs = (trace.events || []).filter((e) => e.type === 'tool.result').reduce((n, e) => n + (e.data?.duration_ms || 0), 0);
    const parts = [
      chip(run.status || '', run.status === 'failed' ? 'error' : run.status === 'completed' ? 'ok' : 'warn'),
      chip(run.model || ''),
      run.reasoning ? chip(`reasoning: ${run.reasoning}`) : null,
      chip(tr('trace.summary.calls', { count: calls.length, time: duration(modelMs) })),
      chip(tr('trace.summary.tools', { count: tools, time: duration(toolMs) })),
      run.usage ? chip(tr('trace.tokens', { input: num(run.usage.input_tokens), output: num(run.usage.output_tokens) })) : null,
      el('span', 'cedar-trace-dim mono', run.id || ''),
    ];
    return parts.filter(Boolean);
  }

  function render() {
    if (!dialog || !current) return;
    const { trace, error, loading } = current;
    dialog.querySelector('#cedar-trace-title').textContent = tr('trace.title');
    const summary = dialog.querySelector('.cedar-trace-summary');
    summary.textContent = '';
    if (trace) summary.append(...summaryChips(trace));

    const actions = dialog.querySelector('.cedar-trace-actions');
    actions.textContent = '';
    const refresh = btn(tr('trace.refresh'), () => load(current.turnId));
    refresh.disabled = loading;
    actions.append(refresh);
    if (trace) {
      actions.append(copyButton(tr('trace.copyAll'), () => json(trace)));
      actions.append(btn(tr('trace.download'), () => download(trace)));
    }
    const close = btn('×', () => dialog.close(), 'cedar-trace-btn close');
    close.setAttribute('aria-label', tr('trace.close'));
    close.title = tr('trace.close');
    actions.append(close);

    const tabs = dialog.querySelector('.cedar-trace-tabs');
    tabs.textContent = '';
    for (const tab of TABS) {
      const b = btn(tab.label(), () => { current.tab = tab.id; render(); }, 'cedar-trace-tab');
      b.setAttribute('role', 'tab');
      b.setAttribute('aria-selected', String(current.tab === tab.id));
      tabs.append(b);
    }

    const body = dialog.querySelector('.cedar-trace-body');
    body.textContent = '';
    if (loading && !trace) { body.append(el('p', 'cedar-trace-note', tr('trace.loading'))); return; }
    if (error) body.append(pre(error, 'error'));
    if (!trace) return;
    const active = ['queued', 'running'].includes(trace.run?.status);
    if (active) body.append(el('p', 'cedar-trace-note warn', tr('trace.live')));
    const tab = TABS.find((t) => t.id === current.tab) || TABS[0];
    body.append(...tab.render(trace));
  }

  function download(trace) {
    const blob = new Blob([json(trace)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = el('a');
    a.href = url;
    a.download = `cedar-trace-${trace.run?.id || 'run'}.json`;
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  async function load(turnId) {
    current = { ...(current?.turnId === turnId ? current : { tab: current?.tab || 'timeline' }), turnId, loading: true, error: null };
    render();
    try {
      current.trace = await api(`/turns/${encodeURIComponent(turnId)}/trace`);
    } catch (err) {
      current.error = err.message;
    } finally {
      current.loading = false;
      render();
    }
  }

  /** Open the trace of a turn's response. */
  function open(turnId) {
    ensureDialog();
    if (current?.turnId !== turnId) current = { turnId, tab: current?.tab || 'timeline' };
    if (!dialog.open) dialog.showModal();
    load(turnId);
  }

  /** The Trace button for a turn, shown only in expert mode. */
  function button(turnId) {
    const b = el('button', 'trace-btn');
    b.type = 'button';
    b.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h10M4 12h16M4 18h7"/><circle cx="17" cy="6" r="2"/><circle cx="14" cy="18" r="2"/></svg>';
    b.append(el('span', '', tr('trace.button')));
    b.title = tr('trace.buttonTitle');
    b.addEventListener('click', (e) => { e.stopPropagation(); open(turnId); });
    return b;
  }

  // -- Settings → Advanced settings → Expert mode ------------------------------------------------

  /** The Expert mode block that settings.js puts at the top of the Advanced settings page. It needs
   *  no admin token, since it only changes this browser. */
  function settingsBlock() {
    const block = el('div', 'cedar-set-block');
    const head = el('div', 'cedar-set-block-head');
    head.append(el('span', 'settings-eyebrow', tr('trace.settings.title')));
    const row = el('div', 'cedar-set-integration');
    const main = el('div', 'cedar-set-integration-main');
    main.append(el('strong', '', tr('trace.settings.switch')), el('small', '', tr('trace.settings.switchDesc')));
    const side = el('div', 'cedar-set-integration-side');
    const sw = el('button', 'cedar-set-switch');
    sw.type = 'button';
    sw.setAttribute('role', 'switch');
    sw.setAttribute('aria-checked', String(enabled()));
    sw.setAttribute('aria-label', tr('trace.settings.switch'));
    sw.addEventListener('click', () => {
      set(!enabled());
      sw.setAttribute('aria-checked', String(enabled()));
      if (typeof cedarSettingsShell !== 'undefined') cedarSettingsShell.refresh('advanced');
    });
    side.append(sw);
    row.append(main, side);
    block.append(head, row);
    return block;
  }

  apply();

  return { enabled, set, button, open, settingsBlock };
})();
